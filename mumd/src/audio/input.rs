use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{InputCallbackInfo, Sample, SampleFormat, SampleRate, StreamConfig};
use log::*;
use tokio::sync::watch;

use crate::audio::SAMPLE_RATE;
use crate::error::{AudioError, AudioStream};
use crate::state::StatePhase;

pub fn callback<T: Sample>(
    mut input_sender: futures_channel::mpsc::Sender<f32>,
    input_volume_receiver: watch::Receiver<f32>,
    phase_watcher: watch::Receiver<StatePhase>,
) -> impl FnMut(&[T], &InputCallbackInfo) + Send + 'static {
    move |data: &[T], _info: &InputCallbackInfo| {
        if !matches!(&*phase_watcher.borrow(), StatePhase::Connected(_)) {
            return;
        }
        let input_volume = *input_volume_receiver.borrow();
        for sample in data.iter().map(|e| e.to_f32()).map(|e| e * input_volume) {
            if let Err(_e) = input_sender.try_send(sample) {
                warn!("Error sending audio: {}", _e);
            }
        }
    }
}

pub trait AudioInputDevice {
    fn play(&self) -> Result<(), AudioError>;
    fn pause(&self) -> Result<(), AudioError>;
    fn set_volume(&self, volume: f32);
    fn sample_receiver(&mut self) -> futures_channel::mpsc::Receiver<f32>;
    fn num_channels(&self) -> usize;
}

pub struct DefaultAudioInputDevice {
    stream: cpal::Stream,
    sample_receiver: Option<futures_channel::mpsc::Receiver<f32>>,
    volume_sender: watch::Sender<f32>,
    channels: u16,
}

impl DefaultAudioInputDevice {
    pub fn new(
        input_volume: f32,
        phase_watcher: watch::Receiver<StatePhase>,
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

        let input_stream = match input_supported_sample_format {
            SampleFormat::F32 => input_device.build_input_stream(
                &input_config,
                callback::<f32>(sample_sender, input_volume_receiver, phase_watcher),
                err_fn,
            ),
            SampleFormat::I16 => input_device.build_input_stream(
                &input_config,
                callback::<i16>(sample_sender, input_volume_receiver, phase_watcher),
                err_fn,
            ),
            SampleFormat::U16 => input_device.build_input_stream(
                &input_config,
                callback::<u16>(sample_sender, input_volume_receiver, phase_watcher),
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
    fn sample_receiver(&mut self) -> futures_channel::mpsc::Receiver<f32> {
        let ret = self.sample_receiver.take();
        ret.unwrap()
    }
    fn num_channels(&self) -> usize {
        self.channels as usize
    }
}
