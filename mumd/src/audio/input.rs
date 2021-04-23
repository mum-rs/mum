use cpal::{InputCallbackInfo, Sample, SampleFormat, SampleRate, StreamConfig};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::{watch, RwLock, mpsc};
use tokio::select;
use mumble_protocol::control::ControlPacket;
use mumble_protocol::voice::{VoicePacket, VoicePacketPayload};
use log::*;
use std::sync::Arc;

use crate::audio::{SAMPLE_RATE, OPUS_ENCODE_FRAME_SIZE};
use crate::error::{AudioError, AudioStream};
use crate::state::State;

pub fn callback<T: Sample>(
    input_sender: mpsc::Sender<f32>,
    mute_state: watch::Receiver<bool>,
    conn_state: watch::Receiver<bool>,
) -> impl FnMut(&[T], &InputCallbackInfo) + Send + 'static {
    move |data: &[T], _info: &InputCallbackInfo| {
        //ignore input is muted or not connected
        if !*conn_state.borrow() || *mute_state.borrow() {
            return;
        }
        for sample in data.iter().map(|e| e.to_f32()) {
            if let Err(_e) = input_sender.try_send(sample) {
                warn!("Error sending audio: {}", _e);
            }
        }
    }
}

pub trait AudioInputDevice {
    fn play(&self) -> Result<(), AudioError>;
    fn pause(&self) -> Result<(), AudioError>;
    fn num_channels(&self) -> u16;
}

pub struct DefaultAudioInputDevice {
    stream: cpal::Stream,
    channels: u16,
}

impl DefaultAudioInputDevice {
    pub async fn new(
        sample_sender: mpsc::Sender<f32>,
        mute_state: watch::Receiver<bool>,
        conn_state: watch::Receiver<bool>,
    ) -> Result<Self, AudioError> {
        let host = cpal::default_host();

        let input_device = host
            .default_input_device()
            .ok_or(AudioError::NoDevice(AudioStream::Input))?;
        let input_supported_config = input_device
            .supported_input_configs()
            .map_err(|e| AudioError::NoConfigs(AudioStream::Input, e))?
            .find_map(|c| {
                if c.min_sample_rate() <= SampleRate(SAMPLE_RATE) && c.max_sample_rate() >= SampleRate(SAMPLE_RATE) {
                    Some(c)
                } else {
                    None
                }
            })
            .ok_or(AudioError::NoSupportedConfig(AudioStream::Input))?
            .with_sample_rate(SampleRate(SAMPLE_RATE));
        let input_supported_sample_format = input_supported_config.sample_format();
        let input_config: StreamConfig = input_supported_config.into();

        let err_fn = |err| error!("An error occurred on the output audio stream: {}", err);

        let input_stream = match input_supported_sample_format {
            SampleFormat::F32 => input_device.build_input_stream(
                &input_config,
                callback::<f32>(sample_sender, mute_state, conn_state),
                err_fn,
            ),
            SampleFormat::I16 => input_device.build_input_stream(
                &input_config,
                callback::<i16>(sample_sender, mute_state, conn_state),
                err_fn,
            ),
            SampleFormat::U16 => input_device.build_input_stream(
                &input_config,
                callback::<u16>(sample_sender, mute_state, conn_state),
                err_fn,
            ),
        }
        .map_err(|e| AudioError::InvalidStream(AudioStream::Input, e))?;

        let res = Self {
            stream: input_stream,
            channels: input_config.channels,
        };
        Ok(res)
    }
}

impl AudioInputDevice for DefaultAudioInputDevice {
    fn play(&self) -> Result<(), AudioError> {
        self.stream.play().map_err(|e| AudioError::InputPlayError(e))
    }
    fn pause(&self) -> Result<(), AudioError> {
        self.stream.pause().map_err(|e| AudioError::InputPauseError(e))
    }
    fn num_channels(&self) -> u16 {
        self.channels
    }
}

pub async fn handle(state: Arc<RwLock<State>>) -> Result<(), AudioError> {
    let state_lock = state.read().await;
    let mut server = state_lock.server_recv();
    let muted = state_lock.muted_recv();
    let mut connected = state_lock.connected_recv();
    drop(state_lock);

    loop {
        // wait not in idle state
        if server.borrow().is_none() {
            server.changed().await.unwrap();
            continue;
        }

        //wait the server connection, only after that open audio input
        if !*connected.borrow() {
            connected.changed().await.unwrap();
            continue;
        }

        //channel used by AudioInputDevice (cpal) => AudioInput (tokio task)
        //TODO find a better size
        let (sample_sender, sample_receiver) = mpsc::channel(10_000);

        //open the input device
        let device = DefaultAudioInputDevice::new(
            sample_sender,
            muted.clone(),
            connected.clone(),
        ).await?;
        device.play()?;

        select! {
            _ = tokio::spawn(check_changed_server(Arc::clone(&state))) => {},
            _ = tokio::spawn(
                produce(
                    Arc::clone(&state),
                    sample_receiver,
                    device.num_channels(),
                )
            ) => {},
        };

        debug!("Audio Input closed");
    }
}

//audio input only stop if into idle state or disconnected.
//it stops if disconnected, otherwise we could fill the audio_stream channel
//because there is no one consuming that
async fn check_changed_server(state: Arc<RwLock<State>>) {
    let state_lock = state.read().await;
    let mut server = state_lock.server_recv();
    let mut connected = state_lock.connected_recv();
    drop(state_lock);

    loop {
        select!(
            _ = server.changed() => {
                if server.borrow().is_none() {
                    //no server to connect, in idle state
                    return;
                }
            },
            _ = connected.changed() => {
                if !*connected.borrow() {
                    //disconnected, stop producing audio
                    return;
                }
            },
        );
    }
}

async fn produce(
    state: Arc<RwLock<State>>,
    mut sample_receiver: mpsc::Receiver<f32>,
    channels: u16,
) -> Result<(), AudioError> {
    let mut buffer = Vec::with_capacity(OPUS_ENCODE_FRAME_SIZE as usize);
    let mut alltime_high = 0f32;
    const MUTE_PERCENT: f32 = 0.1;
    let mut noise_samples_ago = 0usize;
    let mut packet_number = 0u64;

    let state_lock = state.read().await;
    let link_udp = state_lock.link_udp_recv();
    let tcp = state_lock.get_tcp_sink_sender();
    let udp = state_lock.get_udp_sink_sender();
    let input_volume = state_lock.get_input_volume_state_recv();
    drop(state_lock);

    //delay of 1s
    let silence_delay = ((SAMPLE_RATE as usize) * (channels as usize)) as usize;
    let mut encoder = opus::Encoder::new(
        SAMPLE_RATE as u32,
        match channels {
            1 => opus::Channels::Mono,
            2 => opus::Channels::Stereo,
            x => unimplemented!("Only 1 or 2 channels supported, got {})", x),
        },
        opus::Application::Voip,
    ).unwrap();

    loop {
        let read = sample_receiver.recv().await.ok_or_else(||
            AudioError::InputDeviceClosed
        )?;
        //this code "replaces" the noise box
        //TODO do better then that
        if read > alltime_high {
            alltime_high = read;
            noise_samples_ago = 0;
        } else if read > alltime_high * MUTE_PERCENT {
            noise_samples_ago = 0;
        } else {
            noise_samples_ago = noise_samples_ago.saturating_add(1);
        }

        //only encode audio, if noice is detected
        if noise_samples_ago < silence_delay {
            let read = read * *input_volume.borrow();
            buffer.push(read);
            if buffer.len() == OPUS_ENCODE_FRAME_SIZE as usize {
                let encoded = encoder.encode_vec_float(
                    &buffer,
                    OPUS_ENCODE_FRAME_SIZE as usize,
                ).unwrap();

                let packet = VoicePacket::Audio {
                    _dst: std::marker::PhantomData,
                    target: 0,      // normal speech
                    session_id: (), // unused for server-bound packets
                    seq_num: packet_number,
                    payload: VoicePacketPayload::Opus(encoded.into(), false),
                    position_info: None,
                };
                packet_number = packet_number.wrapping_add(1);

                {
                    if *link_udp.borrow() {
                        udp.send(packet).unwrap();
                    } else {
                        tcp.send(ControlPacket::UDPTunnel(Box::new(packet))).unwrap();
                    }
                }

                buffer.clear();
            }
        }
    }
}
