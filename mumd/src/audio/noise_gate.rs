use dasp_frame::Frame;
use dasp_sample::{SignedSample, ToSample, Sample};
use dasp_signal::Signal;
use futures_util::stream::Stream;
use opus::Channels;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

type FloatSample<S> = <<S as Frame>::Sample as Sample>::Float;

pub struct StreamingNoiseGate<S: StreamingSignal> {
    open: usize,
    signal: S,
    deactivation_delay: usize,
    alltime_high: FloatSample<S::Frame>,
}

impl<S: StreamingSignal> StreamingNoiseGate<S> {
    pub fn new(
        signal: S,
        deactivation_delay: usize,
    ) -> StreamingNoiseGate<S> {
        Self {
            open: 0,
            signal,
            deactivation_delay,
            alltime_high: FloatSample::<S::Frame>::EQUILIBRIUM,
        }
    }
}

impl<S> StreamingSignal for StreamingNoiseGate<S>
    where
        S: StreamingSignal + Unpin,
        FloatSample<S::Frame>: Unpin,
        <S as StreamingSignal>::Frame: Unpin {
    type Frame = S::Frame;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Frame> {
        const MUTE_PERCENTAGE: f32 = 0.1;

        let s = self.get_mut();

        let frame = match S::poll_next(Pin::new(&mut s.signal), cx) {
            Poll::Ready(v) => v,
            Poll::Pending => return Poll::Pending,
        };

        if let Some(highest) = frame.to_float_frame().channels().find(|e| abs(e.clone()) > s.alltime_high) {
            s.alltime_high = highest;
        }

        match s.open {
            0 => {
                if frame.to_float_frame().channels().any(|e| abs(e) >= s.alltime_high.mul_amp(MUTE_PERCENTAGE.to_sample())) {
                    s.open = s.deactivation_delay;
                }
            }
            _ => {
                if frame.to_float_frame().channels().any(|e| abs(e) >= s.alltime_high.mul_amp(MUTE_PERCENTAGE.to_sample())) {
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

pub trait StreamingSignal {
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

pub trait StreamingSignalExt: StreamingSignal {
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

pub struct Next<'a, S: ?Sized> {
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

pub struct IntoInterleavedSamples<S: StreamingSignal> {
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


pub struct FromInterleavedSamplesStream<S, F>
    where
        F: Frame {
    stream: S,
    next_buf: Vec<F::Sample>,
    underlying_exhausted: bool,
}

pub fn from_interleaved_samples_stream<S, F>(stream: S) -> FromInterleavedSamplesStream<S, F>
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

pub struct OpusEncoder<S> {
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
    pub fn new(frame_size: u32, sample_rate: u32, channels: usize, stream: S) -> Self {
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
