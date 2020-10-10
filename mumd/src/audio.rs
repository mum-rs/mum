use cpal::traits::DeviceTrait;
use cpal::traits::HostTrait;
use cpal::{OutputCallbackInfo, Sample, SampleFormat, SampleRate, Stream, StreamConfig};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

pub struct Audio {
    pub output_buffer: Arc<Mutex<VecDeque<f32>>>, //TODO ring buffer?
    pub output_config: StreamConfig,
    pub output_stream: Stream,
}

impl Audio {
    pub fn new() -> Self {
        let output_buffer = Arc::new(Mutex::new(VecDeque::new()));

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

        let stream_audio_buf = Arc::clone(&output_buffer);
        let stream = match supported_sample_format {
            SampleFormat::F32 => {
                device.build_output_stream(&config, curry_callback::<f32>(stream_audio_buf), err_fn)
            }
            SampleFormat::I16 => {
                device.build_output_stream(&config, curry_callback::<i16>(stream_audio_buf), err_fn)
            }
            SampleFormat::U16 => {
                device.build_output_stream(&config, curry_callback::<u16>(stream_audio_buf), err_fn)
            }
        }
        .unwrap();

        Self {
            output_buffer,
            output_config: config,
            output_stream: stream,
        }
    }
}

fn curry_callback<T: Sample>(
    buf: Arc<Mutex<VecDeque<f32>>>,
) -> impl FnMut(&mut [T], &OutputCallbackInfo) + Send + 'static {
    move |data: &mut [T], _info: &OutputCallbackInfo| {
        let mut lock = buf.lock().unwrap();
        for sample in data.iter_mut() {
            *sample = Sample::from(&lock.pop_front().unwrap_or(0.0));
        }
    }
}
