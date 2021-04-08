use crate::network::VoiceStreamType;
use log::*;

use cpal::{OutputCallbackInfo, Sample};
use mumble_protocol::voice::VoicePacketPayload;
use opus::Channels;
use std::collections::{HashMap, VecDeque, hash_map::Entry};
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
    }

    pub fn extend(&mut self, key: ClientStreamKey, values: &[f32]) {
        match self.buffer.entry(key) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().extend(values.iter().copied());
            }
            Entry::Vacant(_) => {
                match key {
                    None => warn!("Can't find session None"),
                    Some(key) => warn!("Can't find session id {}", key.1),
                }
            }
        }
    }

    pub fn add_client(&mut self, session_id: u32) {
        for stream_type in [VoiceStreamType::TCP, VoiceStreamType::UDP].iter() {
            match self.buffer.entry(Some((*stream_type, session_id))) {
                Entry::Occupied(_) => {
                    warn!("Session id {} already exists", session_id);
                }
                Entry::Vacant(entry) => {
                    entry.insert(VecDeque::new());
                }
            }
        }
    }

    pub fn remove_client(&mut self, session_id: u32) {
        for stream_type in [VoiceStreamType::TCP, VoiceStreamType::UDP].iter() {
            match self.buffer.entry(Some((*stream_type, session_id))) {
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
        self.buffer.retain(|k , _| k.is_none());
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
