use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{InputCallbackInfo, Sample, SampleFormat, SampleRate, StreamConfig};
use log::*;
use tokio::sync::watch;

use crate::audio::SAMPLE_RATE;
use crate::audio::transformers::{NoiseGate, Transformer};
use crate::error::{AudioError, AudioStream};
use crate::state::StatePhase;

pub fn callback<T: Sample>(
    mut input_sender: futures_channel::mpsc::Sender<Vec<u8>>,
    mut transformers: Vec<Box<dyn Transformer + Send + 'static>>,
    frame_size: u32,
    sample_rate: u32,
    channels: u16,
    input_volume_receiver: watch::Receiver<f32>,
    phase_watcher: watch::Receiver<StatePhase>,
) -> impl FnMut(&[T], &InputCallbackInfo) + Send + 'static {
    let buffer_size = (sample_rate * frame_size / 400) as usize;
    let mut opus_encoder = opus::Encoder::new(
            sample_rate,
            match channels {
                1 => opus::Channels::Mono,
                2 => opus::Channels::Stereo,
                _ => unimplemented!("Only 1 or 2 channels supported, got {}", channels),
            },
            opus::Application::Voip,
        )
        .unwrap();
    let mut buffer = Vec::with_capacity(buffer_size);

    move |data: &[T], _info: &InputCallbackInfo| {
        if !matches!(&*phase_watcher.borrow(), StatePhase::Connected(_)) {
            return;
        }
        let input_volume = *input_volume_receiver.borrow();
        let mut data = data.iter().map(|e| e.to_f32()).map(|e| e * input_volume);

        while buffer.len() + data.len() > buffer_size {
            buffer.extend(data.by_ref().take(buffer_size - buffer.len()));
            let encoded = transformers
                .iter_mut()
                .try_fold(&mut buffer[..], |acc, e| e.transform(acc))
                .map(|buf| opus_encoder.encode_vec_float(&*buf, buffer_size).unwrap());

            if let Some(encoded) = encoded {
                if let Err(e) = input_sender.try_send(encoded) {
                    warn!("Error sending audio: {}", e);
                }
            }
            buffer.clear();
        }
        buffer.extend(data);
    }
}

pub trait AudioInputDevice {
    fn play(&self) -> Result<(), AudioError>;
    fn pause(&self) -> Result<(), AudioError>;
    fn set_volume(&self, volume: f32);
    fn sample_receiver(&mut self) -> Option<futures_channel::mpsc::Receiver<Vec<u8>>>;
    fn num_channels(&self) -> usize;
}

pub struct DefaultAudioInputDevice {
    stream: cpal::Stream,
    sample_receiver: Option<futures_channel::mpsc::Receiver<Vec<u8>>>,
    volume_sender: watch::Sender<f32>,
    channels: u16,
}

impl DefaultAudioInputDevice {
    pub fn new(
        input_volume: f32,
        phase_watcher: watch::Receiver<StatePhase>,
        frame_size: u32, // blocks of 2.5 ms
    ) -> Result<Self, AudioError> {
        let sample_rate = SampleRate(SAMPLE_RATE);

        let host = cpal::default_host();

        let input_device = host
            .default_input_device()
            .ok_or(AudioError::NoDevice(AudioStream::Input))?;
        let input_supported_config = input_device
            .supported_input_configs()
            .map_err(|e| AudioError::NoConfigs(AudioStream::Input, e))?
            .find_map(|c| {
                if c.min_sample_rate() <= sample_rate && c.max_sample_rate() >= sample_rate {
                    Some(c)
                } else {
                    None
                }
            })
            .ok_or(AudioError::NoSupportedConfig(AudioStream::Input))?
            .with_sample_rate(sample_rate);
        let input_supported_sample_format = input_supported_config.sample_format();
        let input_config: StreamConfig = input_supported_config.into();

        let err_fn = |err| error!("An error occurred on the output audio stream: {}", err);

        let (sample_sender, sample_receiver) = futures_channel::mpsc::channel(1_000_000);

        let (volume_sender, input_volume_receiver) = watch::channel::<f32>(input_volume);

        let transformers = vec![Box::new(NoiseGate::new(200)) as Box<dyn Transformer + Send + 'static>];

        let input_stream = match input_supported_sample_format {
            SampleFormat::F32 => input_device.build_input_stream(
                &input_config,
                callback::<f32>(
                    sample_sender, 
                    transformers, 
                    frame_size, 
                    SAMPLE_RATE, 
                    input_config.channels,
                    input_volume_receiver, 
                    phase_watcher
                ),
                err_fn,
            ),
            SampleFormat::I16 => input_device.build_input_stream(
                &input_config,
                callback::<i16>(
                    sample_sender, 
                    transformers, 
                    frame_size, 
                    SAMPLE_RATE, 
                    input_config.channels,
                    input_volume_receiver, 
                    phase_watcher
                ),
                err_fn,
            ),
            SampleFormat::U16 => input_device.build_input_stream(
                &input_config,
                callback::<u16>(
                    sample_sender, 
                    transformers, 
                    frame_size, 
                    SAMPLE_RATE, 
                    input_config.channels,
                    input_volume_receiver, 
                    phase_watcher
                ),
                err_fn,
            ),
        }
        .map_err(|e| AudioError::InvalidStream(AudioStream::Input, e))?;

        let res = Self {
            stream: input_stream,
            sample_receiver: Some(sample_receiver),
            volume_sender,
            channels: input_config.channels,
        };
        Ok(res)
    }
}

impl AudioInputDevice for DefaultAudioInputDevice {
    fn play(&self) -> Result<(), AudioError> {
        self.stream
            .play()
            .map_err(|e| AudioError::InputPlayError(e))
    }
    fn pause(&self) -> Result<(), AudioError> {
        self.stream
            .pause()
            .map_err(|e| AudioError::InputPauseError(e))
    }
    fn set_volume(&self, volume: f32) {
        self.volume_sender.send(volume).unwrap();
    }
    fn sample_receiver(&mut self) -> Option<futures_channel::mpsc::Receiver<Vec<u8>>> {
        self.sample_receiver.take()
    }
    fn num_channels(&self) -> usize {
        self.channels as usize
    }
}
