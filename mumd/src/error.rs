use mumble_protocol::{Serverbound, control::ControlPacket};
use mumlib::error::ConfigError;
use std::fmt;
use tokio::sync::mpsc;

pub enum TcpError {
    NoConnectionInfoReceived,
    TlsConnectorBuilderError(native_tls::Error),
    TlsConnectError(native_tls::Error),
    SendError(mpsc::error::SendError<ControlPacket<Serverbound>>),

    IOError(std::io::Error),
}

impl From<std::io::Error> for TcpError {
    fn from(e: std::io::Error) -> Self {
        TcpError::IOError(e)
    }
}

impl From<mpsc::error::SendError<ControlPacket<Serverbound>>> for TcpError {
    fn from(e: mpsc::error::SendError<ControlPacket<Serverbound>>) -> Self {
        TcpError::SendError(e)
    }
}

pub enum AudioStream {
    Input,
    Output,
}

impl fmt::Display for AudioStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioStream::Input => write!(f, "input"),
            AudioStream::Output => write!(f, "output"),
        }
    }
}

pub enum AudioError {
    NoDevice(AudioStream),
    NoConfigs(AudioStream, cpal::SupportedStreamConfigsError),
    NoSupportedConfig(AudioStream),
    InvalidStream(AudioStream, cpal::BuildStreamError),
    OutputPlayError(cpal::PlayStreamError),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::NoDevice(s) => write!(f, "No {} device", s),
            AudioError::NoConfigs(s, e) => write!(f, "No {} configs: {}", s, e),
            AudioError::NoSupportedConfig(s) => write!(f, "No supported {} config found", s),
            AudioError::InvalidStream(s, e) => write!(f, "Invalid {} stream: {}", s, e),
            AudioError::OutputPlayError(e) => write!(f, "Playback error: {}", e),
        }
    }
}

pub enum StateError {
    AudioError(AudioError),
    ConfigError(ConfigError),
}

impl From<AudioError> for StateError {
    fn from(e: AudioError) -> Self {
        StateError::AudioError(e)
    }
}

impl From<ConfigError> for StateError {
    fn from(e: ConfigError) -> Self {
        StateError::ConfigError(e)
    }
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateError::AudioError(e) => write!(f, "Audio error: {}", e),
            StateError::ConfigError(e) => write!(f, "Config error: {}", e),
        }
    }
}

