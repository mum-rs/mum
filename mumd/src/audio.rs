pub mod input;
pub mod output;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, Stream, StreamConfig};
use log::*;
use mumble_protocol::voice::VoicePacketPayload;
use opus::Channels;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};

pub struct Audio {
    output_config: StreamConfig,
    _output_stream: Stream,
    _input_stream: Stream,

    input_channel_receiver: Option<mpsc::Receiver<VoicePacketPayload>>,
    input_volume_sender: watch::Sender<f32>,

    client_streams: Arc<Mutex<HashMap<u32, output::ClientStream>>>,
}

impl Audio {
    pub fn new() -> Self {
        let sample_rate = SampleRate(48000);

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

        let client_streams = Arc::new(Mutex::new(HashMap::new()));
        let output_stream = match output_supported_sample_format {
            SampleFormat::F32 => output_device.build_output_stream(
                &output_config,
                output::curry_callback::<f32>(Arc::clone(&client_streams)),
                err_fn,
            ),
            SampleFormat::I16 => output_device.build_output_stream(
                &output_config,
                output::curry_callback::<i16>(Arc::clone(&client_streams)),
                err_fn,
            ),
            SampleFormat::U16 => output_device.build_output_stream(
                &output_config,
                output::curry_callback::<u16>(Arc::clone(&client_streams)),
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

        let (input_volume_sender, input_volume_receiver) = watch::channel::<f32>(1.0);

        let input_stream = match input_supported_sample_format {
            SampleFormat::F32 => input_device.build_input_stream(
                &input_config,
                input::callback::<f32>(
                    input_encoder,
                    input_sender,
                    input_config.sample_rate.0,
                    input_volume_receiver.clone(),
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
                    input_volume_receiver.clone(),
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
                    input_volume_receiver.clone(),
                    4, // 10 ms
                ),
                err_fn,
            ),
        }
        .unwrap();

        output_stream.play().unwrap();

        Self {
            output_config,
            _output_stream: output_stream,
            _input_stream: input_stream,
            input_volume_sender,
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
        self.input_volume_sender.broadcast(input_volume).unwrap();
    }
}
