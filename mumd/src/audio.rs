use bytes::Bytes;
use cpal::traits::DeviceTrait;
use cpal::traits::HostTrait;
use cpal::{
    InputCallbackInfo, OutputCallbackInfo, Sample, SampleFormat, SampleRate, Stream, StreamConfig,
};
use mumble_protocol::voice::VoicePacketPayload;
use opus::Channels;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::ops::AddAssign;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::mpsc::{self, Receiver, Sender};

struct ClientStream {
    buffer: VecDeque<f32>, //TODO ring buffer?
    opus_decoder: opus::Decoder,
}

//TODO remove pub where possible
pub struct Audio {
    pub output_config: StreamConfig,
    pub output_stream: Stream,

    pub input_config: StreamConfig,
    pub input_stream: Stream,
    pub input_buffer: Arc<Mutex<VecDeque<f32>>>,
    input_channel_receiver: Option<Receiver<VoicePacketPayload>>,

    client_streams: Arc<Mutex<HashMap<u32, ClientStream>>>,
}

//TODO split into input/output
impl Audio {
    pub fn new() -> Self {
        let host = cpal::default_host();
        let output_device = host
            .default_output_device()
            .expect("default output device not found");
        let mut output_supported_configs_range = output_device
            .supported_output_configs()
            .expect("error querying output configs");
        let output_supported_config = output_supported_configs_range
            .next()
            .expect("no supported output config??")
            .with_sample_rate(SampleRate(48000));
        let output_supported_sample_format = output_supported_config.sample_format();
        let output_config: StreamConfig = output_supported_config.into();

        let input_device = host
            .default_input_device()
            .expect("default input device not found");
        let mut input_supported_configs_range = input_device
            .supported_input_configs()
            .expect("error querying input configs");
        let input_supported_config = input_supported_configs_range
            .next()
            .expect("no supported input config??")
            .with_sample_rate(SampleRate(48000));
        let input_supported_sample_format = input_supported_config.sample_format();
        let input_config: StreamConfig = input_supported_config.into();

        let err_fn = |err| eprintln!("an error occurred on the output audio stream: {}", err);

        let client_streams = Arc::new(Mutex::new(HashMap::new()));
        let output_stream = match output_supported_sample_format {
            SampleFormat::F32 => output_device.build_output_stream(
                &output_config,
                output_curry_callback::<f32>(Arc::clone(&client_streams)),
                err_fn,
            ),
            SampleFormat::I16 => output_device.build_output_stream(
                &output_config,
                output_curry_callback::<i16>(Arc::clone(&client_streams)),
                err_fn,
            ),
            SampleFormat::U16 => output_device.build_output_stream(
                &output_config,
                output_curry_callback::<u16>(Arc::clone(&client_streams)),
                err_fn,
            ),
        }
        .unwrap();

        let input_encoder = opus::Encoder::new(
            input_config.sample_rate.0,
            match input_config.channels {
                1 => Channels::Mono,
                2 => Channels::Stereo,
                _ => unimplemented!(
                    "only 1 or 2 channels supported, got {})",
                    input_config.channels
                ),
            },
            opus::Application::Voip,
        )
        .unwrap();
        let (input_sender, input_receiver) = mpsc::channel(100);

        let input_buffer = Arc::new(Mutex::new(VecDeque::new()));
        let input_stream = match input_supported_sample_format {
            SampleFormat::F32 => input_device.build_input_stream(
                &input_config,
                input_callback::<f32>(input_encoder,
                                      input_sender,
                                      input_config.sample_rate.0,
                                      10.0),
                err_fn,
            ),
            SampleFormat::I16 => input_device.build_input_stream(
                &input_config,
                input_callback::<i16>(input_encoder,
                                      input_sender,
                                      input_config.sample_rate.0,
                                      10.0),
                err_fn,
            ),
            SampleFormat::U16 => input_device.build_input_stream(
                &input_config,
                input_callback::<u16>(input_encoder,
                                      input_sender,
                                      input_config.sample_rate.0,
                                      10.0),
                err_fn,
            ),
        }
        .unwrap();

        Self {
            output_config,
            output_stream,
            input_config,
            input_stream,
            input_buffer,
            input_channel_receiver: Some(input_receiver),
            client_streams,
        }
    }

    pub fn decode_packet(&self, session_id: u32, payload: VoicePacketPayload) {
        match self.client_streams.lock().unwrap().entry(session_id) {
            Entry::Occupied(mut entry) => {
                entry
                    .get_mut()
                    .decode_packet(payload, self.output_config.channels as usize);
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
                    self.output_config.channels,
                ));
            }
        }
    }

    pub fn remove_client(&self, session_id: u32) {
        match self.client_streams.lock().unwrap().entry(session_id) {
            Entry::Occupied(entry) => {
                entry.remove();
            }
            Entry::Vacant(_) => {
                eprintln!(
                    "tried to remove session id {} that doesn't exist",
                    session_id
                );
            }
        }
    }

    pub fn take_receiver(&mut self) -> Option<Receiver<VoicePacketPayload>> {
        self.input_channel_receiver.take()
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
                    _ => unimplemented!("only 1 or 2 channels supported, got {}", channels),
                },
            )
            .unwrap(),
        }
    }

    fn decode_packet(&mut self, payload: VoicePacketPayload, channels: usize) {
        match payload {
            VoicePacketPayload::Opus(bytes, _eot) => {
                let mut out: Vec<f32> = vec![0.0; bytes.len() * channels * 4 + 1000];
                if bytes.len() != 120 {
                    println!("{}", bytes.len());
                }
                let parsed = self.opus_decoder
                    .decode_float(&bytes, &mut out, true)
                    .expect("error decoding"); //FIXME sometimes panics here
                out.truncate(parsed);
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

fn output_curry_callback<T: Sample + AddAssign + SaturatingAdd>(
    buf: Arc<Mutex<HashMap<u32, ClientStream>>>,
) -> impl FnMut(&mut [T], &OutputCallbackInfo) + Send + 'static {
    move |data: &mut [T], _info: &OutputCallbackInfo| {
        for sample in data.iter_mut() {
            *sample = Sample::from(&0.0);
        }

        let mut lock = buf.lock().unwrap();
        for client_stream in lock.values_mut() {
            for sample in data.iter_mut() {
                *sample = sample.saturating_add(Sample::from(
                    &client_stream.buffer.pop_front().unwrap_or(0.0),
                ));
            }
        }
    }
}

fn input_callback<T: Sample>(
    mut opus_encoder: opus::Encoder,
    mut input_sender: Sender<VoicePacketPayload>,
    sample_rate: u32,
    opus_frame_size_ms: f32,
) -> impl FnMut(&[T], &InputCallbackInfo) + Send + 'static {
    if ! (   opus_frame_size_ms ==  2.5
          || opus_frame_size_ms ==  5.0
          || opus_frame_size_ms == 10.0
          || opus_frame_size_ms == 20.0) {
        panic!("Unsupported opus frame size {}", opus_frame_size_ms);
    }
    let opus_frame_size = (opus_frame_size_ms * sample_rate as f32) as u32 / 1000;


    let buf = Arc::new(Mutex::new(VecDeque::new()));
    move |data: &[T], _info: &InputCallbackInfo| {
        let mut buf = buf.lock().unwrap();
        let out: Vec<f32> = data.iter().map(|e| e.to_f32()).collect();
        buf.extend(out);
        while buf.len() >= opus_frame_size as usize {
            let tail = buf.split_off(opus_frame_size as usize);
            let mut opus_buf: Vec<u8> = vec![0; opus_frame_size as usize];
            let result = opus_encoder
                .encode_float(&Vec::from(buf.clone()), &mut opus_buf)
                .unwrap();
            opus_buf.truncate(result);
            let bytes = Bytes::copy_from_slice(&opus_buf);
            input_sender
                .try_send(VoicePacketPayload::Opus(bytes, false))
                .unwrap(); //TODO handle full buffer / disconnect
            *buf = tail;
        }
    }
}
