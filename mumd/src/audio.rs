pub mod input;
pub mod output;
pub mod sound_effects;
pub mod transformers;

use crate::error::AudioError;
use crate::network::VoiceStreamType;
use crate::state::StatePhase;

use futures_util::stream::Stream;
use futures_util::StreamExt;
use mumble_protocol::voice::{VoicePacket, VoicePacketPayload};
use mumble_protocol::Serverbound;
use mumlib::config::SoundEffect;
use std::collections::{hash_map::Entry, HashMap};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

use self::input::{AudioInputDevice, DefaultAudioInputDevice};
use self::output::{AudioOutputDevice, ClientStream, DefaultAudioOutputDevice};
use self::sound_effects::NotificationEvents;

const SAMPLE_RATE: u32 = 48000;

pub struct AudioInput {
    device: DefaultAudioInputDevice,

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

pub struct AudioOutput {
    device: DefaultAudioOutputDevice,
    user_volumes: Arc<Mutex<HashMap<u32, (f32, bool)>>>,

    client_streams: Arc<Mutex<ClientStream>>,

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

    pub fn load_sound_effects(&mut self, overrides: &[SoundEffect]) {
        self.sounds = sound_effects::load_sound_effects(overrides, self.device.num_channels());
    }

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

    pub fn set_volume(&self, output_volume: f32) {
        self.device.set_volume(output_volume);
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
        self.client_streams.lock().unwrap().extend(None, samples);
    }
}
