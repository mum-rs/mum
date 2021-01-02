use cpal::{InputCallbackInfo, Sample};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use log::*;
use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};
use futures_util::task::Waker;

pub fn callback<T: Sample>(
    mut input_sender: futures::channel::mpsc::Sender<f32>,
    input_volume_receiver: watch::Receiver<f32>,
) -> impl FnMut(&[T], &InputCallbackInfo) + Send + 'static {
    move |data: &[T], _info: &InputCallbackInfo| {
        let input_volume = *input_volume_receiver.borrow();
        for sample in data
            .iter()
            .map(|e| e.to_f32())
            .map(|e| e * input_volume) {
            if let Err(_e) = input_sender.try_send(sample) {
                // warn!("Error sending audio: {}", e)
            }
        }
    }
}

struct AudioStream<T> {
    data: Arc<Mutex<(VecDeque<T>, Option<Waker>)>>,
}

impl<T> AudioStream<T> {
    fn new() -> Self {
        Self {
            data: Arc::new(Mutex::new((VecDeque::new(), None)))
        }
    }
    
    fn insert_sample(&self, sample: T) {
        let mut data = self.data.lock().unwrap();
        data.0.push_back(sample);
        if let Some(waker) = data.1.take() {
            waker.wake();
        }
    }
}

impl<T> Stream for AudioStream<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let s = self.get_mut();
        let mut data = s.data.lock().unwrap();
        if data.0.len() > 0 {
            Poll::Ready(data.0.pop_front())
        } else {
            data.1 = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}