pub mod input;
pub mod output;
mod noise_gate;

use crate::audio::output::SaturatingAdd;
use crate::audio::noise_gate::{from_interleaved_samples_stream, OpusEncoder, StreamingNoiseGate, StreamingSignalExt};
use crate::error::{AudioError, AudioStream};
use crate::network::VoiceStreamType;
use crate::state::StatePhase;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig};
use dasp_interpolate::linear::Linear;
use dasp_signal::{self as signal, Signal};
use futures_util::stream::Stream;
use futures_util::StreamExt;
use log::*;
use mumble_protocol::Serverbound;
use mumble_protocol::voice::{VoicePacketPayload, VoicePacket};
use mumlib::config::SoundEffect;
use std::borrow::Cow;
use std::collections::{hash_map::Entry, HashMap, VecDeque};
use std::convert::TryFrom;
use std::fmt::Debug;
use std::fs::File;
use std::io::Read;
use std::sync::{Arc, Mutex};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use tokio::sync::watch;

const SAMPLE_RATE: u32 = 48000;

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

pub struct AudioInput {
    _stream: cpal::Stream,

    channel_receiver: Arc<tokio::sync::Mutex<Box<dyn Stream<Item = VoicePacket<Serverbound>> + Unpin>>>,
    volume_sender: watch::Sender<f32>,
}

impl AudioInput {
    pub fn new(input_volume: f32, phase_watcher: watch::Receiver<StatePhase>) -> Result<Self, AudioError> {
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

        let (input_volume_sender, input_volume_receiver) = watch::channel::<f32>(input_volume);

        let input_stream = match input_supported_sample_format {
            SampleFormat::F32 => input_device.build_input_stream(
                &input_config,
                input::callback::<f32>(
                    sample_sender,
                    input_volume_receiver,
                    phase_watcher,
                ),
                err_fn,
            ),
            SampleFormat::I16 => input_device.build_input_stream(
                &input_config,
                input::callback::<i16>(
                    sample_sender,
                    input_volume_receiver,
                    phase_watcher,
                ),
                err_fn,
            ),
            SampleFormat::U16 => input_device.build_input_stream(
                &input_config,
                input::callback::<u16>(
                    sample_sender,
                    input_volume_receiver,
                    phase_watcher,
                ),
                err_fn,
            ),
        }
        .map_err(|e| AudioError::InvalidStream(AudioStream::Input, e))?;

        let opus_stream = OpusEncoder::new(
                4,
                input_config.sample_rate.0,
                input_config.channels as usize,
                StreamingSignalExt::into_interleaved_samples(
                    StreamingNoiseGate::new(
                        from_interleaved_samples_stream::<_, f32>(sample_receiver), //TODO group frames correctly
                        10_000
                    )
                )
            ).enumerate()
            .map(|(i, e)| VoicePacket::Audio {
                _dst: std::marker::PhantomData,
                target: 0,      // normal speech
                session_id: (), // unused for server-bound packets
                seq_num: i as u64,
                payload: VoicePacketPayload::Opus(e.into(), false),
                position_info: None,
            }
        );

        input_stream.play().map_err(|e| AudioError::OutputPlayError(e))?;

        let res = Self {
            _stream: input_stream,
            volume_sender: input_volume_sender,
            channel_receiver: Arc::new(tokio::sync::Mutex::new(Box::new(opus_stream))),
        };
        Ok(res)
    }

    pub fn receiver(&self) -> Arc<tokio::sync::Mutex<Box<dyn Stream<Item = VoicePacket<Serverbound>> + Unpin>>> {
        Arc::clone(&self.channel_receiver)
    }

    pub fn set_volume(&self, input_volume: f32) {
        self.volume_sender.send(input_volume).unwrap();
    }
}

pub struct AudioOutput {
    config: StreamConfig,
    _stream: cpal::Stream,

    volume_sender: watch::Sender<f32>,

    user_volumes: Arc<Mutex<HashMap<u32, (f32, bool)>>>,

    client_streams: Arc<Mutex<HashMap<(VoiceStreamType, u32), output::ClientStream>>>,

    sounds: HashMap<NotificationEvents, Vec<f32>>,
    play_sounds: Arc<Mutex<VecDeque<f32>>>,
}

impl AudioOutput {
    pub fn new(output_volume: f32) -> Result<Self, AudioError> {
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

        let err_fn = |err| error!("An error occurred on the output audio stream: {}", err);

        let user_volumes = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let (output_volume_sender, output_volume_receiver) = watch::channel::<f32>(output_volume);
        let play_sounds = Arc::new(std::sync::Mutex::new(VecDeque::new()));

        let client_streams = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let output_stream = match output_supported_sample_format {
            SampleFormat::F32 => output_device.build_output_stream(
                &output_config,
                output::curry_callback::<f32>(
                    Arc::clone(&play_sounds),
                    Arc::clone(&client_streams),
                    output_volume_receiver,
                    Arc::clone(&user_volumes),
                ),
                err_fn,
            ),
            SampleFormat::I16 => output_device.build_output_stream(
                &output_config,
                output::curry_callback::<i16>(
                    Arc::clone(&play_sounds),
                    Arc::clone(&client_streams),
                    output_volume_receiver,
                    Arc::clone(&user_volumes),
                ),
                err_fn,
            ),
            SampleFormat::U16 => output_device.build_output_stream(
                &output_config,
                output::curry_callback::<u16>(
                    Arc::clone(&play_sounds),
                    Arc::clone(&client_streams),
                    output_volume_receiver,
                    Arc::clone(&user_volumes),
                ),
                err_fn,
            ),
        }
        .map_err(|e| AudioError::InvalidStream(AudioStream::Output, e))?;
        output_stream.play().map_err(|e| AudioError::OutputPlayError(e))?;

        let mut res = Self {
            config: output_config,
            _stream: output_stream,
            client_streams,
            sounds: HashMap::new(),
            volume_sender: output_volume_sender,
            user_volumes,
            play_sounds,
        };
        res.load_sound_effects(&[]);
        Ok(res)
    }

    pub fn load_sound_effects(&mut self, sound_effects: &[SoundEffect]) {
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

        self.sounds = NotificationEvents::iter()
            .map(|event| {
                let bytes = overrides.get(&event)
                    .map(|file| get_sfx(file))
                    .unwrap_or_else(get_default_sfx);
                let reader = hound::WavReader::new(bytes.as_ref()).unwrap();
                let spec = reader.spec();
                let samples = match spec.sample_format {
                    hound::SampleFormat::Float => reader
                        .into_samples::<f32>()
                        .map(|e| e.unwrap())
                        .collect::<Vec<_>>(),
                    hound::SampleFormat::Int => reader
                        .into_samples::<i16>()
                        .map(|e| cpal::Sample::to_f32(&e.unwrap()))
                        .collect::<Vec<_>>(),
                };
                let iter: Box<dyn Iterator<Item = f32>> = match spec.channels {
                    1 => Box::new(samples.into_iter().flat_map(|e| vec![e, e])),
                    2 => Box::new(samples.into_iter()),
                    _ => unimplemented!("Only mono and stereo sound is supported. See #80.")
                };
                let mut signal = signal::from_interleaved_samples_iter::<_, [f32; 2]>(iter);
                let interp = Linear::new(Signal::next(&mut signal), Signal::next(&mut signal));
                let samples = signal
                    .from_hz_to_hz(interp, spec.sample_rate as f64, SAMPLE_RATE as f64)
                    .until_exhausted()
                    // if the source audio is stereo and is being played as mono, discard the right audio
                    .flat_map(
                        |e| if self.config.channels == 1 {
                            vec![e[0]]
                        } else {
                            e.to_vec()
                        }
                    )
                    .collect::<Vec<f32>>();
                (event, samples)
            })
            .collect();
    }

    pub fn decode_packet_payload(&self, stream_type: VoiceStreamType, session_id: u32, payload: VoicePacketPayload) {
        match self.client_streams.lock().unwrap().entry((stream_type, session_id)) {
            Entry::Occupied(mut entry) => {
                entry
                    .get_mut()
                    .decode_packet(payload, self.config.channels as usize);
            }
            Entry::Vacant(_) => {
                warn!("Can't find session id {}", session_id);
            }
        }
    }

    pub fn add_client(&self, session_id: u32) {
        for stream_type in [VoiceStreamType::TCP, VoiceStreamType::UDP].iter() {
            match self.client_streams.lock().unwrap().entry((*stream_type, session_id)) {
                Entry::Occupied(_) => {
                    warn!("Session id {} already exists", session_id);
                }
                Entry::Vacant(entry) => {
                    entry.insert(output::ClientStream::new(
                        self.config.sample_rate.0,
                        self.config.channels,
                    ));
                }
            }
        }
    }

    pub fn remove_client(&self, session_id: u32) {
        for stream_type in [VoiceStreamType::TCP, VoiceStreamType::UDP].iter() {
            match self.client_streams.lock().unwrap().entry((*stream_type, session_id)) {
                Entry::Occupied(entry) => {
                    entry.remove();
                }
                Entry::Vacant(_) => {
                    warn!(
                        "Tried to remove session id {} that doesn't exist",
                        session_id
                    );
                }
            }
        }
    }

    pub fn clear_clients(&mut self) {
        self.client_streams.lock().unwrap().clear();
    }

    pub fn set_volume(&self, output_volume: f32) {
        self.volume_sender.send(output_volume).unwrap();
    }

    pub fn set_user_volume(&self, id: u32, volume: f32) {
        match self.user_volumes.lock().unwrap().entry(id) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().0 = volume;
            }
            Entry::Vacant(entry) => {
                entry.insert((volume, false));
            }
        }
    }

    pub fn set_mute(&self, id: u32, mute: bool) {
        match self.user_volumes.lock().unwrap().entry(id) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().1 = mute;
            }
            Entry::Vacant(entry) => {
                entry.insert((1.0, mute));
            }
        }
    }

    pub fn play_effect(&self, effect: NotificationEvents) {
        let samples = self.sounds.get(&effect).unwrap();

        let mut play_sounds = self.play_sounds.lock().unwrap();

        for (val, e) in play_sounds.iter_mut().zip(samples.iter()) {
            *val = val.saturating_add(*e);
        }

        let l = play_sounds.len();
        play_sounds.extend(samples.iter().skip(l));
    }
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
    Cow::from(include_bytes!("fallback_sfx.wav").as_ref())
}
