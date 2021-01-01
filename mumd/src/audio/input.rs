use bytes::Bytes;
use cpal::{InputCallbackInfo, Sample};
use mumble_protocol::voice::VoicePacketPayload;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};
use log::*;

pub fn callback<T: Sample>(
    mut opus_encoder: opus::Encoder,
    mut input_sender: mpsc::Sender<VoicePacketPayload>,
    sample_rate: u32,
    input_volume_receiver: watch::Receiver<f32>,
    opus_frame_size_blocks: u32, // blocks of 2.5ms
) -> impl FnMut(&[T], &InputCallbackInfo) + Send + 'static {
    if !(opus_frame_size_blocks == 1
        || opus_frame_size_blocks == 2
        || opus_frame_size_blocks == 4
        || opus_frame_size_blocks == 8)
    {
        panic!(
            "Unsupported amount of opus frame blocks {}",
            opus_frame_size_blocks
        );
    }
    let opus_frame_size = opus_frame_size_blocks * sample_rate / 400;

    let buf = Arc::new(Mutex::new(VecDeque::new()));
    move |data: &[T], _info: &InputCallbackInfo| {
        debug!("{:?}", _info);
        let mut buf = buf.lock().unwrap();
        let input_volume = *input_volume_receiver.borrow();
        let out: Vec<f32> = data
            .iter()
            .map(|e| e.to_f32())
            .map(|e| e * input_volume)
            .collect();
        buf.extend(out);
        while buf.len() >= opus_frame_size as usize {
            let tail = buf.split_off(opus_frame_size as usize);
            let mut opus_buf: Vec<u8> = vec![0; opus_frame_size as usize];
            let result = opus_encoder
                .encode_float(&Vec::from(buf.clone()), &mut opus_buf)
                .unwrap();
            opus_buf.truncate(result);
            let bytes = Bytes::copy_from_slice(&opus_buf);
            match input_sender.try_send(VoicePacketPayload::Opus(bytes, false)) {
                Ok(_) => {}
                Err(_e) => {
                    //warn!("Error sending audio packet: {:?}", e);
                }
            }
            *buf = tail;
        }
    }
}
