use cpal::traits::DeviceTrait;
use cpal::traits::HostTrait;
use cpal::{OutputCallbackInfo, Sample, SampleFormat, SampleRate, Stream, StreamConfig};
use mumble_protocol::voice::VoicePacketPayload;
use opus::Channels;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::collections::hash_map::Entry;
use std::ops::AddAssign;
use std::sync::Arc;
use std::sync::Mutex;

struct ClientStream {
    buffer: VecDeque<f32>, //TODO ring buffer?
    opus_decoder: opus::Decoder,
}

pub struct Audio {
    pub output_config: StreamConfig,
    pub output_stream: Stream,

    client_streams: Arc<Mutex<HashMap<u32, ClientStream>>>,
}

impl Audio {
    pub fn new() -> Self {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .expect("default output device not found");
        let mut supported_configs_range = device
            .supported_output_configs()
            .expect("error querying configs");
        let supported_config = supported_configs_range
            .next()
            .expect("no supported config??")
            .with_sample_rate(SampleRate(48000));
        let supported_sample_format = supported_config.sample_format();
        let config: StreamConfig = supported_config.into();

        let err_fn = |err| eprintln!("an error occurred on the output audio stream: {}", err);

        let client_streams = Arc::new(Mutex::new(HashMap::new()));
        let output_client_streams = Arc::clone(&client_streams);

        let stream = match supported_sample_format {
            SampleFormat::F32 => {
                device.build_output_stream(&config, curry_callback::<f32>(output_client_streams), err_fn)
            }
            SampleFormat::I16 => {
                device.build_output_stream(&config, curry_callback::<i16>(output_client_streams), err_fn)
            }
            SampleFormat::U16 => {
                device.build_output_stream(&config, curry_callback::<u16>(output_client_streams), err_fn)
            }
        }
        .unwrap();

        Self {
            output_config: config,
            output_stream: stream,
            client_streams,
        }
    }

    pub fn decode_packet(&self, session_id: u32, payload: VoicePacketPayload) {
        match self.client_streams.lock().unwrap().entry(session_id) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().decode_packet(payload, self.output_config.channels as usize);
            }
            Entry::Vacant(_) => {
                eprintln!("cannot find session id {}", session_id);
            }
        }
    }

    pub fn add_client(&self, session_id: u32) {
        match self.client_streams.lock().unwrap().entry(session_id) {
            Entry::Occupied(_) => {
                eprintln!("session id {} already exists", session_id);
            }
            Entry::Vacant(entry) => {
                entry.insert(ClientStream::new(
                    self.output_config.sample_rate.0,
                    self.output_config.channels
                ));
            }
        }
    }
}

impl ClientStream {
    fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            buffer: VecDeque::new(),
            opus_decoder: opus::Decoder::new(
                sample_rate,
                match channels {
                    1 => Channels::Mono,
                    2 => Channels::Stereo,
                    _ => unimplemented!(
                        "only 1 or 2 supported, got {})",
                        channels
                    ),
                },
            ).unwrap(),
        }
    }

    fn decode_packet(&mut self, payload: VoicePacketPayload, channels: usize) {
        match payload {
            VoicePacketPayload::Opus(bytes, _eot) => {
                let mut out: Vec<f32> =
                    vec![0.0; bytes.len() * channels * 4];
                self.opus_decoder
                    .decode_float(&bytes[..], &mut out, false)
                    .expect("error decoding");
                self.buffer.extend(out);
            }
            _ => {
                unimplemented!("payload type not supported");
            }
        }
    }
}

trait SaturatingAdd {
    fn saturating_add(self, rhs: Self) -> Self;
}

impl SaturatingAdd for f32 {
    fn saturating_add(self, rhs: Self) -> Self {
        match self + rhs {
            a if a < -1.0 => -1.0,
            a if a >  1.0 =>  1.0,
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

fn curry_callback<T: Sample + AddAssign + SaturatingAdd>(
    buf: Arc<Mutex<HashMap<u32, ClientStream>>>,
) -> impl FnMut(&mut [T], &OutputCallbackInfo) + Send + 'static {
    move |data: &mut [T], _info: &OutputCallbackInfo| {
        for sample in data.iter_mut() {
            *sample = Sample::from(&0.0);
        }

        let mut lock = buf.lock().unwrap();
        for client_stream in lock.values_mut() {
            for sample in data.iter_mut() {
                *sample = sample.saturating_add(Sample::from(&client_stream.buffer.pop_front().unwrap_or(0.0)));
            }
        }
    }
}
