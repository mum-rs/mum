//! Receives audio packets from the networking and plays them.

use crate::audio::SAMPLE_RATE;
use crate::error::{AudioError, AudioStream};
use crate::network::VoiceStreamType;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{OutputCallbackInfo, Sample, SampleFormat, SampleRate, StreamConfig};
use log::*;
use mumble_protocol::voice::VoicePacketPayload;
use std::collections::{HashMap, VecDeque};
use std::ops::AddAssign;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

type ClientStreamKey = (VoiceStreamType, u32);

/// Collected state for client opus decoders and sound effects.
pub struct ClientStream {
    buffer_clients: HashMap<ClientStreamKey, (VecDeque<f32>, opus::Decoder)>, //TODO ring buffer?
    buffer_effects: VecDeque<f32>,
    sample_rate: u32,
    channels: opus::Channels,
}

impl ClientStream {
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        let channels = match channels {
            1 => opus::Channels::Mono,
            2 => opus::Channels::Stereo,
            _ => unimplemented!("Only 1 or 2 channels supported, got {}", channels),
        };
        Self {
            buffer_clients: HashMap::new(),
            buffer_effects: VecDeque::new(),
            sample_rate,
            channels,
        }
    }

    fn get_client(&mut self, client: ClientStreamKey) -> &mut (VecDeque<f32>, opus::Decoder) {
        let sample_rate = self.sample_rate;
        let channels = self.channels;
        self.buffer_clients.entry(client).or_insert_with(|| {
            let opus_decoder = opus::Decoder::new(sample_rate, channels).unwrap();
            (VecDeque::new(), opus_decoder)
        })
    }

    /// Decodes a voice packet.
    pub fn decode_packet(&mut self, client: ClientStreamKey, payload: VoicePacketPayload) {
        match payload {
            VoicePacketPayload::Opus(bytes, _eot) => {
                let mut out: Vec<f32> = vec![0.0; 720 * (self.channels as usize) * 4]; //720 is because that is the max size of packet we can get that we want to decode
                let (buffer, decoder) = self.get_client(client);
                let parsed = decoder
                    .decode_float(&bytes, &mut out, false)
                    .expect("Error decoding");
                out.truncate(parsed);
                buffer.extend(&out);
            }
            _ => {
                unimplemented!("Payload type not supported");
            }
        }
    }

    /// Extends either a decoder queue or the buffer effect queue with some received values.
    pub fn extend(&mut self, client: Option<ClientStreamKey>, values: &[f32]) {
        let buffer = match client {
            Some(x) => &mut self.get_client(x).0,
            None => &mut self.buffer_effects,
        };
        buffer.extend(values.iter().copied());
    }
}

/// Adds two values in some saturating way.
/// 
/// Since we support [f32], [i16] and [u16] we need some way of adding two values
/// without peaking above/below the edge values. This trait ensures that we can
/// use all three primitive types as a generic parameter.
pub trait SaturatingAdd {
    /// Adds two values in some saturating way. See trait documentation.
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
    fn num_channels(&self) -> usize;
    fn client_streams(&self) -> Arc<Mutex<ClientStream>>;
}

pub struct DefaultAudioOutputDevice {
    config: StreamConfig,
    stream: cpal::Stream,
    /// The client stream per user ID. A separate stream is kept for UDP and TCP.
    ///
    /// Shared with [super::AudioOutput].
    client_streams: Arc<Mutex<ClientStream>>,
    /// Output volume configuration.
    volume_sender: watch::Sender<f32>,
}

impl DefaultAudioOutputDevice {
    /// Initializes the default audio output.
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
        let client_streams = Arc::new(std::sync::Mutex::new(ClientStream::new(
            sample_rate.0,
            output_config.channels,
        )));

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
            stream: output_stream,
            volume_sender: output_volume_sender,
            client_streams,
        })
    }
}

impl AudioOutputDevice for DefaultAudioOutputDevice {
    fn play(&self) -> Result<(), AudioError> {
        self.stream
            .play()
            .map_err(|e| AudioError::OutputPlayError(e))
    }

    fn pause(&self) -> Result<(), AudioError> {
        self.stream
            .pause()
            .map_err(|e| AudioError::OutputPauseError(e))
    }

    fn set_volume(&self, volume: f32) {
        self.volume_sender.send(volume).unwrap();
    }

    fn num_channels(&self) -> usize {
        self.config.channels as usize
    }

    fn client_streams(&self) -> Arc<Mutex<ClientStream>> {
        Arc::clone(&self.client_streams)
    }
}

/// Over-engineered way of handling multiple types of samples.
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
        for (k, v) in user_bufs.buffer_clients.iter_mut() {
            let (user_volume, muted) = user_volumes.get(&k.1).cloned().unwrap_or((1.0, false));
            for sample in data.iter_mut() {
                if !muted {
                    *sample = sample.saturating_add(Sample::from(
                        &(v.0.pop_front().unwrap_or(0.0) * volume * user_volume),
                    ));
                }
            }
        }
        for sample in data.iter_mut() {
            *sample = sample.saturating_add(Sample::from(
                &(user_bufs.buffer_effects.pop_front().unwrap_or(0.0) * volume),
            ));
        }
    }
}
