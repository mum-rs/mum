//! All things audio.
//!
//! Audio is handled mostly as signals from [dasp_signal]. Input/output is handled by [cpal].

pub mod input;
pub mod output;
pub mod transformers;

use crate::audio::input::{AudioInputDevice, DefaultAudioInputDevice};
use crate::audio::output::{AudioOutputDevice, ClientStream, DefaultAudioOutputDevice};
use crate::error::AudioError;
use crate::network::VoiceStreamType;
use crate::state::StatePhase;

use dasp_interpolate::linear::Linear;
use dasp_signal::{self as signal, Signal};
use futures_util::stream::Stream;
use futures_util::StreamExt;
use log::*;
use mumble_protocol::voice::{VoicePacket, VoicePacketPayload};
use mumble_protocol::Serverbound;
use mumlib::config::SoundEffect;
use std::borrow::Cow;
use std::collections::{hash_map::Entry, HashMap};
use std::convert::TryFrom;
use std::fmt::Debug;
use std::fs::File;
use std::io::Read;
use std::sync::{Arc, Mutex};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use tokio::sync::watch;

/// The sample rate used internally.
const SAMPLE_RATE: u32 = 48000;

/// All types of notifications that can be shown. Each notification can be bound to its own audio
/// file.
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

/// Input audio state. Input audio is picket up from an [AudioInputDevice] (e.g.
/// a microphone) and sent over the network.
pub struct AudioInput {
    device: DefaultAudioInputDevice,

    /// Outgoing voice packets that should be sent over the network.
    channel_receiver:
        Arc<tokio::sync::Mutex<Box<dyn Stream<Item = VoicePacket<Serverbound>> + Unpin>>>,
}

impl AudioInput {
    pub fn new(
        input_volume: f32,
        phase_watcher: watch::Receiver<StatePhase>,
    ) -> Result<Self, AudioError> {
        let mut default = DefaultAudioInputDevice::new(input_volume, phase_watcher, 4)?;

        let opus_stream = default
            .sample_receiver()
            .unwrap()
            .enumerate()
            .map(|(i, e)| VoicePacket::Audio {
                _dst: std::marker::PhantomData,
                target: 0,      // normal speech
                session_id: (), // unused for server-bound packets
                seq_num: i as u64,
                payload: VoicePacketPayload::Opus(e.into(), false),
                position_info: None,
            });

        default.play()?;

        let res = Self {
            device: default,
            channel_receiver: Arc::new(tokio::sync::Mutex::new(Box::new(opus_stream))),
        };
        Ok(res)
    }

    pub fn receiver(
        &self,
    ) -> Arc<tokio::sync::Mutex<Box<dyn Stream<Item = VoicePacket<Serverbound>> + Unpin>>> {
        Arc::clone(&self.channel_receiver)
    }

    pub fn set_volume(&self, input_volume: f32) {
        self.device.set_volume(input_volume);
    }
}

/// Audio output state. The audio is received from each client over the network,
/// decoded, merged and finally played to an [AudioOutputDevice] (e.g. speaker,
/// headphones, ...).
pub struct AudioOutput {
    device: DefaultAudioOutputDevice,
    /// The volume and mute-status of a user ID.
    user_volumes: Arc<Mutex<HashMap<u32, (f32, bool)>>>,

    /// The client stream per user ID. A separate stream is kept for UDP and TCP.
    ///
    /// Shared with [DefaultAudioOutputDevice].
    client_streams: Arc<Mutex<ClientStream>>,

    /// Which sound effect should be played on an event.
    sounds: HashMap<NotificationEvents, Vec<f32>>,
}

impl AudioOutput {
    pub fn new(output_volume: f32) -> Result<Self, AudioError> {
        let user_volumes = Arc::new(std::sync::Mutex::new(HashMap::new()));

        let default = DefaultAudioOutputDevice::new(output_volume, Arc::clone(&user_volumes))?;
        default.play()?;

        let client_streams = default.client_streams();

        let mut res = Self {
            device: default,
            sounds: HashMap::new(),
            client_streams,
            user_volumes,
        };
        res.load_sound_effects(&[]);
        Ok(res)
    }

    /// Loads sound effects, getting unspecified effects from [get_default_sfx].
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

        // This makes sure that every [NotificationEvent] is present in [self.sounds].
        self.sounds = NotificationEvents::iter()
            .map(|event| {
                let bytes = overrides
                    .get(&event)
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
                    _ => unimplemented!("Only mono and stereo sound is supported. See #80."),
                };
                let mut signal = signal::from_interleaved_samples_iter::<_, [f32; 2]>(iter);
                let interp = Linear::new(Signal::next(&mut signal), Signal::next(&mut signal));
                let samples = signal
                    .from_hz_to_hz(interp, spec.sample_rate as f64, SAMPLE_RATE as f64)
                    .until_exhausted()
                    // if the source audio is stereo and is being played as mono, discard the right audio
                    .flat_map(|e| {
                        if self.device.num_channels() == 1 {
                            vec![e[0]]
                        } else {
                            e.to_vec()
                        }
                    })
                    .collect::<Vec<f32>>();
                (event, samples)
            })
            .collect();
    }

    /// Decodes a voice packet.
    pub fn decode_packet_payload(
        &self,
        stream_type: VoiceStreamType,
        session_id: u32,
        payload: VoicePacketPayload,
    ) {
        self.client_streams
            .lock()
            .unwrap()
            .decode_packet((stream_type, session_id), payload);
    }

    /// Sets the volume of the output device.
    pub fn set_volume(&self, output_volume: f32) {
        self.device.set_volume(output_volume);
    }

    /// Sets the incoming volume of a user.
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

    /// Mutes another user.
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

    /// Queues a sound effect.
    pub fn play_effect(&self, effect: NotificationEvents) {
        let samples = self.sounds.get(&effect).unwrap();
        self.client_streams.lock().unwrap().add_sound_effect(samples);
    }
}

/// Reads a sound effect from disk.
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

/// Gets the default sound effect.
fn get_default_sfx() -> Cow<'static, [u8]> {
    Cow::from(include_bytes!("fallback_sfx.wav").as_ref())
}
