use crate::network::VoiceStreamType;
use crate::audio::SAMPLE_RATE;
use crate::error::{AudioError, AudioStream};

use log::*;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig, OutputCallbackInfo, Sample};
use mumble_protocol::voice::VoicePacketPayload;
use opus::Channels;
use std::collections::{HashMap, VecDeque};
use std::ops::AddAssign;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

type ClientStreamKey = Option<(VoiceStreamType, u32)>;
type ConsumerInput<'a> = std::collections::hash_map::IterMut<'a, ClientStreamKey, VecDeque<f32>>;

pub struct ClientStream {
    buffer: HashMap<ClientStreamKey, VecDeque<f32>>, //TODO ring buffer?
    opus_decoder: opus::Decoder,
}

impl ClientStream {
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        let mut buffer = HashMap::new();
        //None is the system audio effects
        buffer.insert(None, VecDeque::new());
        Self {
            buffer,
            opus_decoder: opus::Decoder::new(
                sample_rate,
                match channels {
                    1 => Channels::Mono,
                    2 => Channels::Stereo,
                    _ => unimplemented!("Only 1 or 2 channels supported, got {}", channels),
                },
            )
            .unwrap(),
        }
    }

    pub fn decode_packet(&mut self, client: ClientStreamKey, payload: VoicePacketPayload, channels: usize) {
        match payload {
            VoicePacketPayload::Opus(bytes, _eot) => {
                let mut out: Vec<f32> = vec![0.0; 720 * channels * 4]; //720 is because that is the max size of packet we can get that we want to decode
                let parsed = self
                    .opus_decoder
                    .decode_float(&bytes, &mut out, false)
                    .expect("Error decoding");
                out.truncate(parsed);
                self.extend(client, &out);
            }
            _ => {
                unimplemented!("Payload type not supported");
            }
        }
    }

    pub fn consume<'a, F>(&mut self, consumer: F)
        where
            F: FnOnce(ConsumerInput) {
        let iter = self.buffer.iter_mut();
        consumer(iter);
        //remove empty Vec
        self.buffer.retain(|_, v| v.is_empty());
    }

    pub fn extend(&mut self, key: ClientStreamKey, values: &[f32]) {
        let entry = self.buffer.entry(key).or_insert(VecDeque::new());
        entry.extend(values.iter().copied());
    }
}

pub trait SaturatingAdd {
    fn saturating_add(self, rhs: Self) -> Self;
}

impl SaturatingAdd for f32 {
    fn saturating_add(self, rhs: Self) -> Self {
        match self + rhs {
            a if a < -1.0 => -1.0,
            a if a > 1.0 => 1.0,
            a => a,
        }
    }
}

impl SaturatingAdd for i16 {
    fn saturating_add(self, rhs: Self) -> Self {
        i16::saturating_add(self, rhs)
    }
}

impl SaturatingAdd for u16 {
    fn saturating_add(self, rhs: Self) -> Self {
        u16::saturating_add(self, rhs)
    }
}

pub trait AudioOutputDevice {
    fn play(&self) -> Result<(), AudioError>;
    fn pause(&self) -> Result<(), AudioError>;
    fn set_volume(&self, volume: f32);
    fn get_num_channels(&self) -> usize;
    fn get_client_streams(&self) -> Arc<Mutex<ClientStream>>;
}

pub struct DefaultAudioOutputDevice {
    config: StreamConfig,
    _stream: cpal::Stream,
    client_streams: Arc<Mutex<ClientStream>>,
    volume_sender: watch::Sender<f32>,
}

impl DefaultAudioOutputDevice {
    pub fn new(
        output_volume: f32,
        user_volumes: Arc<Mutex<HashMap<u32, (f32, bool)>>>,
    ) -> Result<Self, AudioError> {
        let sample_rate = SampleRate(SAMPLE_RATE);

        let host = cpal::default_host();
        let output_device = host
            .default_output_device()
            .ok_or(AudioError::NoDevice(AudioStream::Output))?;
        let output_supported_config = output_device
            .supported_output_configs()
            .map_err(|e| AudioError::NoConfigs(AudioStream::Output, e))?
            .find_map(|c| {
                if c.min_sample_rate() <= sample_rate && c.max_sample_rate() >= sample_rate {
                    Some(c)
                } else {
                    None
                }
            })
            .ok_or(AudioError::NoSupportedConfig(AudioStream::Output))?
            .with_sample_rate(sample_rate);
        let output_supported_sample_format = output_supported_config.sample_format();
        let output_config: StreamConfig = output_supported_config.into();
        let client_streams = Arc::new(std::sync::Mutex::new(ClientStream::new(sample_rate.0, output_config.channels)));

        let err_fn = |err| error!("An error occurred on the output audio stream: {}", err);

        let (output_volume_sender, output_volume_receiver) = watch::channel::<f32>(output_volume);

        let output_stream = match output_supported_sample_format {
            SampleFormat::F32 => output_device.build_output_stream(
                &output_config,
                curry_callback::<f32>(
                    Arc::clone(&client_streams),
                    output_volume_receiver,
                    user_volumes,
                ),
                err_fn,
            ),
            SampleFormat::I16 => output_device.build_output_stream(
                &output_config,
                curry_callback::<i16>(
                    Arc::clone(&client_streams),
                    output_volume_receiver,
                    user_volumes,
                ),
                err_fn,
            ),
            SampleFormat::U16 => output_device.build_output_stream(
                &output_config,
                curry_callback::<u16>(
                    Arc::clone(&client_streams),
                    output_volume_receiver,
                    user_volumes,
                ),
                err_fn,
            ),
        }
        .map_err(|e| AudioError::InvalidStream(AudioStream::Output, e))?;

        Ok(Self {
            config: output_config,
            _stream: output_stream,
            volume_sender: output_volume_sender,
            client_streams,
        })
    }
}

impl AudioOutputDevice for DefaultAudioOutputDevice {
    fn play(&self) -> Result<(), AudioError> {
        self._stream.play().map_err(|e| AudioError::OutputPlayError(e))
    }

    fn pause(&self) -> Result<(), AudioError> {
        self._stream.pause().map_err(|e| AudioError::OutputPauseError(e))
    }

    fn set_volume(&self, volume: f32) {
        self.volume_sender.send(volume).unwrap();
    }

    fn get_num_channels(&self) -> usize {
        self.config.channels as usize
    }

    fn get_client_streams(&self) -> Arc<Mutex<ClientStream>> {
        Arc::clone(&self.client_streams)
    }
}

pub fn curry_callback<T: Sample + AddAssign + SaturatingAdd + std::fmt::Display>(
    user_bufs: Arc<Mutex<ClientStream>>,
    output_volume_receiver: watch::Receiver<f32>,
    user_volumes: Arc<Mutex<HashMap<u32, (f32, bool)>>>,
) -> impl FnMut(&mut [T], &OutputCallbackInfo) + Send + 'static {
    move |data: &mut [T], _info: &OutputCallbackInfo| {
        for sample in data.iter_mut() {
            *sample = Sample::from(&0.0);
        }

        let volume = *output_volume_receiver.borrow();

        let mut user_bufs = user_bufs.lock().unwrap();
        let user_volumes = user_volumes.lock().unwrap();
        let mut saturating_add = |input: ConsumerInput| {
            for (k, v) in input {
                let (user_volume, muted) = match k {
                    Some((_, id)) => user_volumes.get(id).cloned().unwrap_or((1.0, false)),
                    None => (1.0, false),
                };
                for sample in data.iter_mut() {
                    if !muted {
                        *sample = sample.saturating_add(Sample::from(
                            &(v.pop_front().unwrap_or(0.0) * volume * user_volume),
                        ));
                    }
                }
            }
        };
        user_bufs.consume(&mut saturating_add);
    }
}
