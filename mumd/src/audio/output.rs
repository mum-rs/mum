use crate::audio::{SAMPLE_RATE, SaturatingAdd};
use crate::error::{AudioError, AudioStream};
use crate::state::State;

use log::*;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig, OutputCallbackInfo, Sample};
use mumlib::config::SoundEffect;
use mumble_protocol::voice::VoicePacketPayload;
use dasp_signal::{self as signal, Signal};
use dasp_interpolate::linear::Linear;
use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::ops::AddAssign;
use std::convert::TryFrom;
use std::sync::Arc;
use tokio::sync::{watch, RwLock, Mutex};
use std::fs::File;
use std::io::Read;
use tokio::select;
use strum_macros::EnumIter;
use strum::IntoEnumIterator;
use futures_util::future::poll_fn;

pub fn curry_callback<T: Sample + AddAssign + SaturatingAdd + std::fmt::Display>(
    mut receiver: futures_channel::mpsc::Receiver<f32>,
    deaf: watch::Receiver<bool>,
) -> impl FnMut(&mut [T], &OutputCallbackInfo) + Send + 'static {
    move |data: &mut [T], _info: &OutputCallbackInfo| {
        if *deaf.borrow() {
            for sample in data.iter_mut() {
                *sample = Sample::from(&0.0);
            }
        } else {
            for sample in data.iter_mut() {
                *sample = match receiver.try_next() {
                    Ok(Some(sample_received)) => Sample::from(&sample_received),
                    //no data available
                    Err(_) => Sample::from(&0.0),
                    //never happen, close the device before closing the channel
                    Ok(None) => panic!("AudioOutput Channel Closed"),
                };
            }
        }
    }
}

pub trait AudioOutputDevice {
    fn play(&self) -> Result<(), AudioError>;
    fn pause(&self) -> Result<(), AudioError>;
    fn num_channels(&self) -> usize;
}

pub struct DefaultAudioOutputDevice {
    config: StreamConfig,
    stream: cpal::Stream,
}

impl DefaultAudioOutputDevice {
    pub async fn new(
        sample_receiver: futures_channel::mpsc::Receiver<f32>,
        deaf: watch::Receiver<bool>,
    ) -> Result<Self, AudioError> {
        let host = cpal::default_host();
        let output_device = host
            .default_output_device()
            .ok_or(AudioError::NoDevice(AudioStream::Output))?;
        let output_supported_config = output_device
            .supported_output_configs()
            .map_err(|e| AudioError::NoConfigs(AudioStream::Output, e))?
            .find_map(|c| {
                if c.min_sample_rate() <= SampleRate(SAMPLE_RATE) && c.max_sample_rate() >= SampleRate(SAMPLE_RATE) {
                    Some(c)
                } else {
                    None
                }
            })
            .ok_or(AudioError::NoSupportedConfig(AudioStream::Output))?
            .with_sample_rate(SampleRate(SAMPLE_RATE));
        let output_supported_sample_format = output_supported_config.sample_format();
        let output_config: StreamConfig = output_supported_config.into();

        let err_fn = |err| error!("An error occurred on the output audio stream: {}", err);

        let output_stream = match output_supported_sample_format {
            SampleFormat::F32 => output_device.build_output_stream(
                &output_config,
                curry_callback::<f32>(sample_receiver, deaf),
                err_fn,
            ),
            SampleFormat::I16 => output_device.build_output_stream(
                &output_config,
                curry_callback::<i16>(sample_receiver, deaf),
                err_fn,
            ),
            SampleFormat::U16 => output_device.build_output_stream(
                &output_config,
                curry_callback::<u16>(sample_receiver, deaf),
                err_fn,
            ),
        }
        .map_err(|e| AudioError::InvalidStream(AudioStream::Output, e))?;

        Ok(Self {
            config: output_config,
            stream: output_stream,
        })
    }
}

#[derive(Debug, Eq, PartialEq, Clone, Copy, Hash, EnumIter)]
pub enum NotificationEvents {
    ServerConnect,
    ServerDisconnect,
    UserConnected,
    UserDisconnected,
    UserJoinedChannel,
    UserLeftChannel,
    Mute,
    Unmute,
    Deafen,
    Undeafen,
}

impl TryFrom<&str> for NotificationEvents {
    type Error = ();
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s {
            "server_connect" => Ok(NotificationEvents::ServerConnect),
            "server_disconnect" => Ok(NotificationEvents::ServerDisconnect),
            "user_connected" => Ok(NotificationEvents::UserConnected),
            "user_disconnected" => Ok(NotificationEvents::UserDisconnected),
            "user_joined_channel" => Ok(NotificationEvents::UserJoinedChannel),
            "user_left_channel" => Ok(NotificationEvents::UserLeftChannel),
            "mute" => Ok(NotificationEvents::Mute),
            "unmute" => Ok(NotificationEvents::Unmute),
            "deafen" => Ok(NotificationEvents::Deafen),
            "undeafen" => Ok(NotificationEvents::Undeafen),
            _ => {
                warn!("Unknown notification event '{}' in config", s);
                Err(())
            }
        }
    }
}

impl AudioOutputDevice for DefaultAudioOutputDevice {
    fn play(&self) -> Result<(), AudioError> {
        self.stream.play().map_err(|e| AudioError::OutputPlayError(e))
    }

    fn pause(&self) -> Result<(), AudioError> {
        self.stream.pause().map_err(|e| AudioError::OutputPauseError(e))
    }

    fn num_channels(&self) -> usize {
        self.config.channels as usize
    }
}

#[derive(Debug, Clone)]
pub enum AudioOutputMessage {
    VoicePacket {
        //source: VoiceStreamType,
        //target: u8,
        user_id: u32,
        seq_num: u64,
        data: VoicePacketPayload
        // position_info,
    },
    LoadSoundEffects (Vec<SoundEffect>),
    Effects (NotificationEvents),
}

pub async fn handle(state: Arc<RwLock<State>>) -> Result<(), AudioError> {
    //TODO find better size
    let deaf = state.read().await.deaf_recv();

    let effects = Arc::new(Mutex::new(VecDeque::new()));

    loop {
        //don't need to wait for connection, we need to output audio if needded
        //even if not connected, we could want to play a notification

        //open audio output device
        let (sample_sender, sample_receiver) = futures_channel::mpsc::channel(10_000);
        let device = DefaultAudioOutputDevice::new(
            sample_receiver,
            deaf.clone(),
        ).await?;
        let channels = match device.num_channels() {
            1 => opus::Channels::Mono,
            2 => opus::Channels::Stereo,
            x => unimplemented!("Only 1 or 2 channels supported, got {})", x),
        };
        device.play()?;

        select! {
            //new message: receive and process the message
            _ = tokio::spawn(receive_message(
                    Arc::clone(&state),
                    channels,
                    Arc::clone(&effects),
                )
            ) => {},
            //send is available: send the next sample to device channel
            _ = tokio::spawn(produce_sample(
                    Arc::clone(&state),
                    sample_sender,
                    Arc::clone(&effects),
                )
            ) => {},
            //never exit, only on error
        };
        //TODO handle errors from select
    }
}

async fn produce_sample(
    state: Arc<RwLock<State>>,
    mut sample_sender: futures_channel::mpsc::Sender<f32>,
    effects: Arc<Mutex<VecDeque<f32>>>,
) -> Result<(), AudioError> {
    let volume = state.read().await.get_output_volume_state_recv();
    loop {
        //wait the send channel is available
        poll_fn(|cx| sample_sender.poll_ready(cx)).await
            .map_err(|_| AudioError::GenericError)?;

        //produce a sample
        let mut sample = effects.lock().await.pop_front().unwrap_or(Sample::from(&0.0));
        let state = state.read().await;
        let users = state.users.read().await;
        for user in users.values() {
            let mut user = user.write().await;
            if let Some(new_sample) = user.buffer.pop_front() {
                sample = sample.saturating_add(new_sample * user.volume);
            }
        }
        let sample = sample * *volume.borrow();
        //send the sample produced
        sample_sender.start_send(sample).map_err(|_| AudioError::GenericError)?;
    }
}


async fn receive_message(
    state: Arc<RwLock<State>>,
    channels: opus::Channels,
    effects: Arc<Mutex<VecDeque<f32>>>,
) -> Result<(), AudioError> {
    let mut sounds = load_sound_effects(&[])?;

    let state_lock = state.read().await;
    let mut audio_sink = state_lock.get_audio_sink_receiver().await.unwrap();
    let deaf = state_lock.deaf_recv();
    drop(state_lock);

    let error = loop {
        let message = audio_sink.recv().await;
        match message {
            None => break AudioError::GenericError,
            Some(AudioOutputMessage::VoicePacket{user_id, seq_num, data}) => {
                if *deaf.borrow() {
                    //TODO ignore voice could fUp the decoder???
                    //if deaf, ignore the message
                    continue;
                }
                let state = state.read().await;
                let users = state.users.read().await;
                let mut user_lock = match users.get(&user_id) {
                    None => break AudioError::GenericError,
                    Some(entry) => {
                        { //user read lock
                            let user_lock = entry.read().await;
                            if let Some(last_frame) = user_lock.last_frame {
                                //TODO check overflow in last_frame/seq_num?
                                //if a new frame is generated each millisecond
                                //this could overflow in around 500 million year
                                if seq_num <= last_frame {
                                    //this frame was already received, ignore it
                                    continue;
                                }
                            }
                        } //user read lock
                        let mut user_lock = entry.write().await;
                        if let None = user_lock.last_frame {
                            user_lock.last_frame = Some(0);
                        }
                        if let None = user_lock.decoder {
                            let decoder = opus::Decoder::new(
                                SAMPLE_RATE,
                                channels,
                            );
                            match decoder {
                                Ok(decoder) => user_lock.decoder = Some(Mutex::new(decoder)),
                                Err(_) => break AudioError::GenericError,
                            }
                        }
                        user_lock
                    }
                };
                match data {
                    VoicePacketPayload::Opus(bytes, _eot) => {
                        let mut out: Vec<f32> = vec![0.0; 720 * (channels as usize) * 4]; //720 is because that is the max size of packet we can get that we want to decode
                        //safe because decoder is created automatically
                        let parsed = user_lock.decoder.as_ref().unwrap().lock().await
                            .decode_float(&bytes, &mut out, false);
                        match parsed {
                            Err(_) => break AudioError::GenericError,
                            Ok(parsed) => {
                                out.truncate(parsed);
                                user_lock.buffer.extend(&out);
                            }
                        }
                    }
                    _ => {
                        unimplemented!("Payload type not supported");
                    }
                }
            },
            Some(AudioOutputMessage::LoadSoundEffects(effect)) => {
                match load_sound_effects(&effect) {
                    Ok(sound_effects) => sounds = sound_effects,
                    Err(_) => break AudioError::GenericError,
                }
            },
            Some(AudioOutputMessage::Effects(effect)) => {
                if *deaf.borrow() {
                    //if deaf, ignore
                    continue;
                }
                match sounds.get(&effect) {
                    Some((samples_channels, samples)) => {
                        match (samples_channels, channels as u16) {
                            //same number of channels, just forward
                            (1, 1) | (2, 2) => effects.lock().await.extend(samples),
                            //effect is mono, device is sterio, duplicate mono
                            (1, 2) => {
                                effects.lock().await.extend(
                                    samples
                                    .iter()
                                    .flat_map(|e| vec![*e, *e])
                                )
                            },
                            //effect is sterio, device is mono, ignore one channel
                            (2, 1) => effects.lock().await.extend(samples.iter().step_by(2)),
                            _ => unimplemented!("Only mono and stereo sound is supported. See #80.")
                        }
                    },
                    None => {},
                }
            },
        }
    };
    state.read().await.set_audio_sink_receiver(audio_sink).await;
    Err(error)
}

fn load_sound_effects(
    sound_effects: &[SoundEffect],
) -> Result<HashMap<NotificationEvents, (u16, Vec<f32>)>, AudioError> {
    let overrides: HashMap<_, _> = sound_effects
        .iter()
        .filter_map(|sound_effect| {
            let (event, file) = (&sound_effect.event, &sound_effect.file);
            if let Ok(event) = NotificationEvents::try_from(event.as_str()) {
                Some((event, file))
            } else {
                None
            }
        })
        .collect();

    NotificationEvents::iter()
        .map(|event| {
            let bytes = overrides.get(&event)
                .map(|file| get_sfx(file))
                .unwrap_or_else(get_default_sfx);
            let reader = hound::WavReader::new(bytes.as_ref())
                .map_err(|_| AudioError::GenericError)?;
            let spec = reader.spec();
            let samples = match spec.sample_format {
                hound::SampleFormat::Float => reader
                    .into_samples::<f32>()
                    .map(|e| e)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| AudioError::GenericError)?,
                hound::SampleFormat::Int => reader
                    .into_samples::<i16>()
                    .map(|e| e.map(|e| cpal::Sample::to_f32(&e)))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_: hound::Error| AudioError::GenericError)?,
            };
            let channels = spec.channels;
            if spec.sample_rate != SAMPLE_RATE {
                let iter: Box<dyn Iterator<Item = f32>> = match channels {
                    1 => Box::new(samples.into_iter().flat_map(|e| vec![e, e])),
                    2 => Box::new(samples.into_iter()),
                    _ => unimplemented!("Only mono and stereo sound is supported. See #80.")
                };
                let mut signal = signal::from_interleaved_samples_iter::<_, [f32; 2]>(iter);
                let interp = Linear::new(Signal::next(&mut signal), Signal::next(&mut signal));
                let samples = signal
                    .from_hz_to_hz(interp, spec.sample_rate as f64, SAMPLE_RATE as f64)
                    .until_exhausted()
                    .flat_map(
                        |e| if channels == 1 {
                            vec![e[0]]
                        } else {
                            e.to_vec()
                        }
                    )
                    .collect::<Vec<f32>>();
                Ok((event, (channels, samples)))
            } else {
                Ok((event, (channels, samples)))
            }
        })
        .collect::<Result<HashMap<_, _>, _>>()
}

// moo
fn get_sfx(file: &str) -> Cow<'static, [u8]> {
    let mut buf: Vec<u8> = Vec::new();
    if let Ok(mut file) = File::open(file) {
        file.read_to_end(&mut buf).unwrap();
        Cow::from(buf)
    } else {
        warn!("File not found: '{}'", file);
        get_default_sfx()
    }
}

fn get_default_sfx() -> Cow<'static, [u8]> {
    Cow::from(include_bytes!("../fallback_sfx.wav").as_ref())
}
