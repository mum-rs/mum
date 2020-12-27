use crate::audio::VoiceStreamType;

use cpal::{OutputCallbackInfo, Sample};
use mumble_protocol::voice::VoicePacketPayload;
use opus::Channels;
use std::collections::{HashMap, VecDeque};
use std::ops::AddAssign;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

pub struct ClientStream {
    buffer: VecDeque<f32>, //TODO ring buffer?
    opus_decoder: opus::Decoder,
}

impl ClientStream {
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            buffer: VecDeque::new(),
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

    pub fn decode_packet(&mut self, payload: VoicePacketPayload, channels: usize) {
        match payload {
            VoicePacketPayload::Opus(bytes, _eot) => {
                let mut out: Vec<f32> = vec![0.0; 720 * channels * 4]; //720 is because that is the max size of packet we can get that we want to decode
                let parsed = self
                    .opus_decoder
                    .decode_float(&bytes, &mut out, false)
                    .expect("Error decoding");
                out.truncate(parsed);
                self.buffer.extend(out);
            }
            _ => {
                unimplemented!("Payload type not supported");
            }
        }
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
    effect_sound: Arc<Mutex<VecDeque<f32>>>,
    user_bufs: Arc<Mutex<HashMap<(VoiceStreamType, u32), ClientStream>>>,
    output_volume_receiver: watch::Receiver<f32>,
    user_volumes: Arc<Mutex<HashMap<u32, (f32, bool)>>>,
) -> impl FnMut(&mut [T], &OutputCallbackInfo) + Send + 'static {
    move |data: &mut [T], _info: &OutputCallbackInfo| {
        for sample in data.iter_mut() {
            *sample = Sample::from(&0.0);
        }

        let volume = *output_volume_receiver.borrow();

        let mut effects_sound = effect_sound.lock().unwrap();
        let mut user_bufs = user_bufs.lock().unwrap();
        //TODO in 1.49 we can do this:
        // for ((voice_stream, id), client_stream) in &mut *user_bufs {
        for (id, client_stream) in &mut *user_bufs {
            let (user_volume, muted) = user_volumes
                .lock()
                .unwrap()
                .get(&id.1) // session id
                .cloned()
                .unwrap_or((1.0, false));
            for sample in data.iter_mut() {
                let s = client_stream.buffer.pop_front().unwrap_or(0.0) * volume * user_volume;
                if !muted {
                    *sample = sample.saturating_add(Sample::from(&s));
                }
            }
        }

        for sample in data.iter_mut() {
            *sample = sample.saturating_add(Sample::from(
                &(effects_sound.pop_front().unwrap_or(0.0) * volume),
            ));
        }
    }
}
