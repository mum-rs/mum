pub mod input;
pub mod output;

use crate::audio::output::SaturatingAdd;
use crate::network::VoiceStreamType;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig};
use dasp_frame::Frame;
use dasp_interpolate::linear::Linear;
use dasp_sample::{SignedSample, ToSample, Sample};
use dasp_signal::{self as signal, Signal};
use futures::Stream;
use futures::stream::StreamExt;
use futures::task::{Context, Poll};
use log::*;
use mumble_protocol::Serverbound;
use mumble_protocol::voice::{VoicePacketPayload, VoicePacket};
use mumlib::config::SoundEffect;
use opus::Channels;
use std::{
    borrow::Cow,
    collections::{hash_map::Entry, HashMap, VecDeque},
    convert::TryFrom,
    fmt::Debug,
    fs::File,
    future::Future,
    io::Read,
    pin::Pin,
    sync::{Arc, Mutex},
};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use tokio::sync::{watch};

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

pub struct Audio {
    output_config: StreamConfig,
    _output_stream: cpal::Stream,
    _input_stream: cpal::Stream,

    input_channel_receiver: Arc<tokio::sync::Mutex<Box<dyn Stream<Item = VoicePacket<Serverbound>> + Unpin>>>,
    input_volume_sender: watch::Sender<f32>,

    output_volume_sender: watch::Sender<f32>,

    user_volumes: Arc<Mutex<HashMap<u32, (f32, bool)>>>,

    client_streams: Arc<Mutex<HashMap<(VoiceStreamType, u32), output::ClientStream>>>,

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
        .unwrap();

        let (sample_sender, sample_receiver) = futures::channel::mpsc::channel(1_000_000);

        let (input_volume_sender, input_volume_receiver) = watch::channel::<f32>(input_volume);

        let input_stream = match input_supported_sample_format {
            SampleFormat::F32 => input_device.build_input_stream(
                &input_config,
                input::callback::<f32>(
                    sample_sender,
                    input_volume_receiver,
                ),
                err_fn,
            ),
            SampleFormat::I16 => input_device.build_input_stream(
                &input_config,
                input::callback::<i16>(
                    sample_sender,
                    input_volume_receiver,
                ),
                err_fn,
            ),
            SampleFormat::U16 => input_device.build_input_stream(
                &input_config,
                input::callback::<u16>(
                    sample_sender,
                    input_volume_receiver,
                ),
                err_fn,
            ),
        }
        .unwrap();

        let opus_stream = OpusEncoder::new(
            4,
            input_config.sample_rate.0,
            input_config.channels as usize,
            StreamingSignalExt::into_interleaved_samples(
                StreamingNoiseGate::new(
                    from_interleaved_samples_stream::<_, f32>(sample_receiver), //TODO group frames correctly
                    0.09,
                    10_000))).enumerate().map(|(i, e)| VoicePacket::Audio {
                        _dst: std::marker::PhantomData,
                        target: 0,      // normal speech
                        session_id: (), // unused for server-bound packets
                        seq_num: i as u64,
                        payload: VoicePacketPayload::Opus(e.into(), false),
                        position_info: None,
                    });

        output_stream.play().unwrap();

        let mut res = Self {
            output_config,
            _output_stream: output_stream,
            _input_stream: input_stream,
            input_volume_sender,
            input_channel_receiver: Arc::new(tokio::sync::Mutex::new(Box::new(opus_stream))),
            client_streams,
            sounds: HashMap::new(),
            output_volume_sender,
            user_volumes,
            play_sounds,
        };
        res.load_sound_effects(&[]);
        res
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
                    _ => unimplemented!() // TODO handle gracefully (this might not even happen)
                };
                let mut signal = signal::from_interleaved_samples_iter::<_, [f32; 2]>(iter);
                let interp = Linear::new(Signal::next(&mut signal), Signal::next(&mut signal));
                let samples = signal
                    .from_hz_to_hz(interp, spec.sample_rate as f64, SAMPLE_RATE as f64)
                    .until_exhausted()
                    // if the source audio is stereo and is being played as mono, discard the right audio
                    .flat_map(
                        |e| if self.output_config.channels == 1 {
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
                    .decode_packet(payload, self.output_config.channels as usize);
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
                        self.output_config.sample_rate.0,
                        self.output_config.channels,
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

    pub fn input_receiver(&self) -> Arc<tokio::sync::Mutex<Box<dyn Stream<Item = VoicePacket<Serverbound>> + Unpin>>> {
        Arc::clone(&self.input_channel_receiver)
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

struct StreamingNoiseGate<S: StreamingSignal> {
    open: usize,
    signal: S,
    activate_threshold: <<S::Frame as Frame>::Sample as Sample>::Float,
    deactivation_delay: usize,
}

impl<S: StreamingSignal> StreamingNoiseGate<S> {
    pub fn new(
        signal: S,
        activate_threshold: <<S::Frame as Frame>::Sample as Sample>::Float,
        deactivation_delay: usize,
    ) -> StreamingNoiseGate<S> {
        Self {
            open: 0,
            signal,
            activate_threshold,
            deactivation_delay
        }
    }
}

impl<S> StreamingSignal for StreamingNoiseGate<S>
    where
        S: StreamingSignal + Unpin,
        <<<S as StreamingSignal>::Frame as Frame>::Sample as Sample>::Float: Unpin,
        <S as StreamingSignal>::Frame: Unpin {
    type Frame = S::Frame;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Frame> {
        let s = self.get_mut();

        let frame = match S::poll_next(Pin::new(&mut s.signal), cx) {
            Poll::Ready(v) => v,
            Poll::Pending => return Poll::Pending,
        };

        match s.open {
            0 => {
                if frame.to_float_frame().channels().any(|e| abs(e) >= s.activate_threshold) {
                    s.open = s.deactivation_delay;
                }
            }
            _ => {
                if frame.to_float_frame().channels().any(|e| abs(e) >= s.activate_threshold) {
                    s.open = s.deactivation_delay;
                } else {
                    s.open -= 1;
                }
            }
        }

        if s.open != 0 {
            Poll::Ready(frame)
        } else {
            Poll::Ready(<S::Frame as Frame>::EQUILIBRIUM)
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

    fn is_exhausted(&self) -> bool {
        false
    }
}

impl<S> StreamingSignal for S
    where
        S: Signal + Unpin {
    type Frame = S::Frame;

    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Frame> {
        Poll::Ready(self.get_mut().next())
    }
}

trait StreamingSignalExt: StreamingSignal {
    fn next(&mut self) -> Next<'_, Self> {
        Next {
            stream: self
        }
    }

    fn into_interleaved_samples(self) -> IntoInterleavedSamples<Self>
        where
            Self: Sized {
        IntoInterleavedSamples { signal: self, current_frame: None }
    }
}

impl<S> StreamingSignalExt for S
    where S: StreamingSignal {}

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

struct IntoInterleavedSamples<S: StreamingSignal> {
    signal: S,
    current_frame: Option<<S::Frame as Frame>::Channels>,
}

impl<S> Stream for IntoInterleavedSamples<S>
    where
        S: StreamingSignal + Unpin,
        <<S as StreamingSignal>::Frame as Frame>::Channels: Unpin {
    type Item = <S::Frame as Frame>::Sample;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let s = self.get_mut();
        loop {
            if s.current_frame.is_some() {
                if let Some(channel) = s.current_frame.as_mut().unwrap().next() {
                    return Poll::Ready(Some(channel));
                }
            }
            match S::poll_next(Pin::new(&mut s.signal), cx) {
                Poll::Ready(val) => {
                    s.current_frame = Some(val.channels());
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

struct FromStream<S> {
    stream: S,
    underlying_exhausted: bool,
}

impl<S> StreamingSignal for FromStream<S>
    where
        S: Stream + Unpin,
        S::Item: Frame + Unpin {
    type Frame = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Frame> {
        let s = self.get_mut();
        if s.underlying_exhausted {
            return Poll::Ready(<Self::Frame as Frame>::EQUILIBRIUM);
        }
        match S::poll_next(Pin::new(&mut s.stream), cx) {
            Poll::Ready(Some(val)) => {
                Poll::Ready(val)
            }
            Poll::Ready(None) => {
                s.underlying_exhausted = true;
                return Poll::Ready(<Self::Frame as Frame>::EQUILIBRIUM);
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_exhausted(&self) -> bool {
        self.underlying_exhausted
    }
}


struct FromInterleavedSamplesStream<S, F>
    where
        F: Frame {
    stream: S,
    next_buf: Vec<F::Sample>,
    underlying_exhausted: bool,
}

fn from_interleaved_samples_stream<S, F>(stream: S) -> FromInterleavedSamplesStream<S, F>
    where
        S: Stream + Unpin,
        S::Item: Sample,
        F: Frame<Sample = S::Item> {
    FromInterleavedSamplesStream {
        stream,
        next_buf: Vec::new(),
        underlying_exhausted: false,
    }
}

impl<S, F> StreamingSignal for FromInterleavedSamplesStream<S, F>
    where
        S: Stream + Unpin,
        S::Item: Sample + Unpin,
        F: Frame<Sample = S::Item> + Unpin {
    type Frame = F;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Frame> {
        let s = self.get_mut();
        if s.underlying_exhausted {
            return Poll::Ready(F::EQUILIBRIUM);
        }
        while s.next_buf.len() < F::CHANNELS {
            match S::poll_next(Pin::new(&mut s.stream), cx) {
                Poll::Ready(Some(v)) => {
                    s.next_buf.push(v);
                }
                Poll::Ready(None) => {
                    s.underlying_exhausted = true;
                    return Poll::Ready(F::EQUILIBRIUM);
                }
                Poll::Pending => return Poll::Pending,
            }
        }

        let mut data = s.next_buf.iter().cloned();
        let n = F::from_samples(&mut data).unwrap();
        s.next_buf.clear();
        Poll::Ready(n)
    }

    fn is_exhausted(&self) -> bool {
        self.underlying_exhausted
    }
}

struct OpusEncoder<S> {
    encoder: opus::Encoder,
    frame_size: u32,
    sample_rate: u32,
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
        loop {
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
            if s.input_buffer.iter().any(|&e| e != 0.0) {
                break;
            }
            s.input_buffer.clear();
        }

        let encoded = s.encoder.encode_vec_float(&s.input_buffer, opus_frame_size).unwrap();
        s.input_buffer.clear();
        Poll::Ready(Some(encoded))
    }
}
