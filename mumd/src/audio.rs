pub mod input;
pub mod output;

use crate::audio::output::SaturatingAdd;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig};
use dasp_interpolate::linear::Linear;
use dasp_signal::{self as signal, Signal};
use log::*;
use mumble_protocol::voice::VoicePacketPayload;
use opus::Channels;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};
use dasp_frame::Frame;
use dasp_sample::{Sample, SignedSample, ToSample};
use dasp_ring_buffer::Fixed;
use futures::Stream;
use futures::task::{Context, Poll};
use std::pin::Pin;
use tokio_stream::StreamExt;
use std::convert::identity;
use std::future::Future;
use std::mem;

//TODO? move to mumlib
pub const EVENT_SOUNDS: &[(&'static [u8], NotificationEvents)] = &[
    (include_bytes!("resources/connect.wav"), NotificationEvents::ServerConnect),
    (
        include_bytes!("resources/disconnect.wav"),
        NotificationEvents::ServerDisconnect,
    ),
    (
        include_bytes!("resources/channel_join.wav"),
        NotificationEvents::UserConnected,
    ),
    (
        include_bytes!("resources/channel_leave.wav"),
        NotificationEvents::UserDisconnected,
    ),
    (
        include_bytes!("resources/channel_join.wav"),
        NotificationEvents::UserJoinedChannel,
    ),
    (
        include_bytes!("resources/channel_leave.wav"),
        NotificationEvents::UserLeftChannel,
    ),
    (include_bytes!("resources/mute.wav"), NotificationEvents::Mute),
    (include_bytes!("resources/unmute.wav"), NotificationEvents::Unmute),
    (include_bytes!("resources/deafen.wav"), NotificationEvents::Deafen),
    (include_bytes!("resources/undeafen.wav"), NotificationEvents::Undeafen),
];

const SAMPLE_RATE: u32 = 48000;

#[derive(Debug, Eq, PartialEq, Clone, Copy, Hash)]
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

pub struct Audio {
    output_config: StreamConfig,
    _output_stream: cpal::Stream,
    _input_stream: cpal::Stream,

    input_channel_receiver: Option<mpsc::Receiver<VoicePacketPayload>>,
    input_volume_sender: watch::Sender<f32>,

    output_volume_sender: watch::Sender<f32>,

    user_volumes: Arc<Mutex<HashMap<u32, (f32, bool)>>>,

    client_streams: Arc<Mutex<HashMap<u32, output::ClientStream>>>,

    sounds: HashMap<NotificationEvents, Vec<f32>>,
    play_sounds: Arc<Mutex<VecDeque<f32>>>,
}

impl Audio {
    pub fn new(input_volume: f32, output_volume: f32) -> Self {
        let sample_rate = SampleRate(SAMPLE_RATE);

        let host = cpal::default_host();
        let output_device = host
            .default_output_device()
            .expect("default output device not found");
        let output_supported_config = output_device
            .supported_output_configs()
            .expect("error querying output configs")
            .find_map(|c| {
                if c.min_sample_rate() <= sample_rate && c.max_sample_rate() >= sample_rate {
                    Some(c)
                } else {
                    None
                }
            })
            .unwrap()
            .with_sample_rate(sample_rate);
        let output_supported_sample_format = output_supported_config.sample_format();
        let output_config: StreamConfig = output_supported_config.into();

        let input_device = host
            .default_input_device()
            .expect("default input device not found");
        let input_supported_config = input_device
            .supported_input_configs()
            .expect("error querying output configs")
            .find_map(|c| {
                if c.min_sample_rate() <= sample_rate && c.max_sample_rate() >= sample_rate {
                    Some(c)
                } else {
                    None
                }
            })
            .unwrap()
            .with_sample_rate(sample_rate);
        let input_supported_sample_format = input_supported_config.sample_format();
        let input_config: StreamConfig = input_supported_config.into();

        let err_fn = |err| error!("An error occurred on the output audio stream: {}", err);

        let user_volumes = Arc::new(Mutex::new(HashMap::new()));
        let (output_volume_sender, output_volume_receiver) = watch::channel::<f32>(output_volume);
        let play_sounds = Arc::new(Mutex::new(VecDeque::new()));

        let client_streams = Arc::new(Mutex::new(HashMap::new()));
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
        .unwrap();

        let input_encoder = opus::Encoder::new(
            input_config.sample_rate.0,
            match input_config.channels {
                1 => Channels::Mono,
                2 => Channels::Stereo,
                _ => unimplemented!(
                    "Only 1 or 2 channels supported, got {})",
                    input_config.channels
                ),
            },
            opus::Application::Voip,
        )
        .unwrap();
        let (input_sender, input_receiver) = mpsc::channel(100);

        let (input_volume_sender, input_volume_receiver) = watch::channel::<f32>(input_volume);

        let input_stream = match input_supported_sample_format {
            SampleFormat::F32 => input_device.build_input_stream(
                &input_config,
                input::callback::<f32>(
                    input_encoder,
                    input_sender,
                    input_config.sample_rate.0,
                    input_volume_receiver,
                    4, // 10 ms
                ),
                err_fn,
            ),
            SampleFormat::I16 => input_device.build_input_stream(
                &input_config,
                input::callback::<i16>(
                    input_encoder,
                    input_sender,
                    input_config.sample_rate.0,
                    input_volume_receiver,
                    4, // 10 ms
                ),
                err_fn,
            ),
            SampleFormat::U16 => input_device.build_input_stream(
                &input_config,
                input::callback::<u16>(
                    input_encoder,
                    input_sender,
                    input_config.sample_rate.0,
                    input_volume_receiver,
                    4, // 10 ms
                ),
                err_fn,
            ),
        }
        .unwrap();

        output_stream.play().unwrap();

        let sounds = EVENT_SOUNDS
            .iter()
            .map(|(bytes, event)| {
                let reader = hound::WavReader::new(*bytes).unwrap();
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
                    _ => unimplemented!() // TODO handle gracefully (this might not even happen)
                };
                let mut signal = signal::from_interleaved_samples_iter::<_, [f32; 2]>(iter);
                let interp = Linear::new(signal.next(), signal.next());
                let samples = signal
                    .from_hz_to_hz(interp, spec.sample_rate as f64, SAMPLE_RATE as f64)
                    .until_exhausted()
                    // if the source audio is stereo and is being played as mono, discard the right audio
                    .flat_map(|e| if output_config.channels == 1 { vec![e[0]] } else { e.to_vec() })
                    .collect::<Vec<f32>>();
                (*event, samples)
            })
            .collect();

        Self {
            output_config,
            _output_stream: output_stream,
            _input_stream: input_stream,
            input_volume_sender,
            input_channel_receiver: Some(input_receiver),
            client_streams,
            sounds,
            output_volume_sender,
            user_volumes,
            play_sounds,
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
                warn!("Can't find session id {}", session_id);
            }
        }
    }

    pub fn add_client(&self, session_id: u32) {
        match self.client_streams.lock().unwrap().entry(session_id) {
            Entry::Occupied(_) => {
                warn!("Session id {} already exists", session_id);
            }
            Entry::Vacant(entry) => {
                entry.insert(output::ClientStream::new(
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
                warn!(
                    "Tried to remove session id {} that doesn't exist",
                    session_id
                );
            }
        }
    }

    pub fn take_receiver(&mut self) -> Option<mpsc::Receiver<VoicePacketPayload>> {
        self.input_channel_receiver.take()
    }

    pub fn clear_clients(&mut self) {
        self.client_streams.lock().unwrap().clear();
    }

    pub fn set_input_volume(&self, input_volume: f32) {
        self.input_volume_sender.send(input_volume).unwrap();
    }

    pub fn set_output_volume(&self, output_volume: f32) {
        self.output_volume_sender.send(output_volume).unwrap();
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

struct NoiseGate<S: Signal> {
    open: bool,
    signal: S,
    buffer: dasp_ring_buffer::Fixed<Vec<S::Frame>>,
    activate_threshold: <<S::Frame as Frame>::Sample as Sample>::Float,
    deactivate_threshold: <<S::Frame as Frame>::Sample as Sample>::Float,
}

impl<S: Signal> NoiseGate<S> {
    pub fn new(signal: S, activate_threshold: <<S::Frame as Frame>::Sample as Sample>::Float, deactivate_threshold: <<S::Frame as Frame>::Sample as Sample>::Float) -> NoiseGate<S> {
        Self {
            open: false,
            signal,
            buffer: Fixed::from(vec![<S::Frame as Frame>::EQUILIBRIUM; 4096]),
            activate_threshold,
            deactivate_threshold,
        }
    }
}

impl<S: Signal> Signal for NoiseGate<S> {
    type Frame = S::Frame;

    fn next(&mut self) -> Self::Frame {
        let frame = self.signal.next();
        self.buffer.push(frame);

        if self.open && self.buffer
            .iter()
            .all(|f| f.to_float_frame()
                .channels()
                .all(|s| abs(s - <<<S::Frame as Frame>::Sample as Sample>::Float as Sample>::EQUILIBRIUM) <= self.deactivate_threshold)) {
            self.open = false;
        } else if !self.open && self.buffer
            .iter()
            .any(|f| f.to_float_frame()
                .channels()
                .any(|s| abs(s - <<<S::Frame as Frame>::Sample as Sample>::Float as Sample>::EQUILIBRIUM) >= self.activate_threshold)) {
            self.open = true;
        }

        if self.open {
            frame
        } else {
            S::Frame::EQUILIBRIUM
        }
    }

    fn is_exhausted(&self) -> bool {
        self.signal.is_exhausted()
    }
}

fn abs<S: SignedSample>(sample: S) -> S {
    let zero = S::EQUILIBRIUM;
    if sample >= zero {
        sample
    } else {
        -sample
    }
}

trait StreamingSignal {
    type Frame: Frame;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Frame>;
}

trait StreamingSignalExt: StreamingSignal {
    fn next(&mut self) -> Next<'_, Self> {
        Next {
            stream: self
        }
    }
}

struct Next<'a, S: ?Sized> {
    stream: &'a mut S
}

impl<'a, S: StreamingSignal + Unpin> Future for Next<'a, S> {
    type Output = S::Frame;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match S::poll_next(Pin::new(self.stream), cx) {
            Poll::Ready(val) => {
                Poll::Ready(val)
            }
            Poll::Pending => Poll::Pending
        }
    }
}

struct FromStream<S: Stream> {
    stream: S,
    next: Option<S::Item>,
}

async fn from_stream<S>(mut stream: S) -> FromStream<S>
    where
        S: Stream + Unpin,
        S::Item: Frame {
    let next = stream.next().await;
    FromStream { stream, next }
}

impl<S> StreamingSignal for FromStream<S>
    where
        S: Stream + Unpin,
        S::Item: Frame + Unpin {
    type Frame = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Frame> {
        let s = self.get_mut();
        match s.next.take() {
            Some(v) => {
                match S::poll_next(Pin::new(&mut s.stream), cx) {
                    Poll::Ready(val) => {
                        s.next = val;
                        Poll::Ready(v)
                    }
                    Poll::Pending => Poll::Pending
                }
            }
            None => Poll::Ready(<Self::Frame as Frame>::EQUILIBRIUM)
        }
    }
}


struct FromInterleavedSamplesStream<S, F>
    where
        F: Frame {
    stream: S,
    next: Option<Vec<F::Sample>>,
}

async fn from_interleaved_samples_stream<S, F>(mut stream: S) -> FromInterleavedSamplesStream<S, F>
    where
        S: Stream + Unpin,
        S::Item: Sample,
        F: Frame<Sample = S::Item> {
    let mut data = Vec::with_capacity(F::CHANNELS);
    for _ in 0..F::CHANNELS {
        data.push(stream.next().await);
    }
    let data = data.into_iter().flat_map(identity).collect::<Vec<_>>();
    FromInterleavedSamplesStream { stream, next: if data.len() == F::CHANNELS { Some(data) } else { None } }
}

impl<S, F> StreamingSignal for FromInterleavedSamplesStream<S, F>
    where
        S: Stream + Unpin,
        S::Item: Sample + Unpin,
        F: Frame<Sample = S::Item> {
    type Frame = F;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Frame> {
        let s = self.get_mut();
        if s.next.is_some() {
            if s.next.as_ref().unwrap().len() == F::CHANNELS {
                let mut data_buf = mem::replace(&mut s.next, Some(Vec::new())).unwrap().into_iter();
                Poll::Ready(F::from_samples(&mut data_buf).unwrap())
            } else {
                match S::poll_next(Pin::new(&mut s.stream), cx) {
                    Poll::Ready(Some(v)) => {
                        s.next.as_mut().unwrap().push(v);
                        Poll::Pending
                    }
                    Poll::Ready(None) => {
                        s.next = None;
                        Poll::Ready(F::EQUILIBRIUM)
                    }
                    Poll::Pending => Poll::Pending,
                }
            }
        } else {
            Poll::Ready(F::EQUILIBRIUM)
        }
    }
}

struct OpusEncoder<S> {
    encoder: opus::Encoder,
    frame_size: u32,
    sample_rate: u32,
    channels: usize,
    stream: S,
    input_buffer: Vec<f32>,
    exhausted: bool,
}

impl<S, I> OpusEncoder<S>
    where
        S: Stream<Item = I>,
        I: ToSample<f32> {
    fn new(frame_size: u32, sample_rate: u32, channels: usize, stream: S) -> Self {
        let encoder = opus::Encoder::new(
            sample_rate,
            match channels {
                1 => Channels::Mono,
                2 => Channels::Stereo,
                _ => unimplemented!(
                    "Only 1 or 2 channels supported, got {})",
                    channels
                ),
            },
            opus::Application::Voip,
        ).unwrap();
        Self {
            encoder,
            frame_size,
            sample_rate,
            channels,
            stream,
            input_buffer: Vec::new(),
            exhausted: false,
        }
    }
}

impl<S, I> Stream for OpusEncoder<S>
    where
        S: Stream<Item = I> + Unpin,
        I: Sample + ToSample<f32> {
    type Item = Vec<u8>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let s = self.get_mut();
        if s.exhausted {
            return Poll::Ready(None);
        }
        let opus_frame_size = (s.frame_size * s.sample_rate / 400) as usize;
        while s.input_buffer.len() < opus_frame_size {
            match S::poll_next(Pin::new(&mut s.stream), cx) {
                Poll::Ready(Some(v)) => {
                    s.input_buffer.push(v.to_sample::<f32>());
                }
                Poll::Ready(None) => {
                    s.exhausted = true;
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }

        let encoded = s.encoder.encode_vec_float(&s.input_buffer, opus_frame_size).unwrap();
        s.input_buffer.clear();
        Poll::Ready(Some(encoded))
    }
}