use cpal::{InputCallbackInfo, Sample};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
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