use mumble_protocol::{Serverbound, control::ControlPacket};
use mumlib::error::ConfigError;
use std::fmt;
use tokio::sync::mpsc;
use serde::{Deserialize, Serialize};

pub type ServerSendError = mpsc::error::SendError<ControlPacket<Serverbound>>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum CommandError {
    NotConnected,
    Disconnected,
    ChannelIdentifierError,

    UnableToConnect(ConnectionError),

    GenericError, //TODO remove
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandError::NotConnected => write!(f, "Command Not Connected"),
            CommandError::Disconnected => write!(f, "Command Disconnected"),
            CommandError::ChannelIdentifierError => write!(
                f, "Channel Identifier not found"
            ),
            CommandError::UnableToConnect(x)=> write!(f, "Unable to connect: {}", x),
            CommandError::GenericError => write!(f, "Command Generic Error"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum ConnectionError {
    GenericError, //TODO remove
}

impl fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConnectionError::GenericError => write!(f, "Command Generic Error"),
        }
    }
}

#[derive(Debug)]
pub enum TcpError {
    TlsConnectorBuilderError(native_tls::Error),
    TlsConnectError(native_tls::Error),
    SendError(ServerSendError),

    IOError(std::io::Error),
    GenericError, //TODO remove
}

impl fmt::Display for TcpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TcpError::TlsConnectorBuilderError(e)
                => write!(f, "Error building TLS connector: {}", e),
            TcpError::TlsConnectError(e)
                => write!(f, "TLS error when connecting: {}", e),
            TcpError::SendError(e) => write!(f, "Couldn't send packet: {}", e),
            TcpError::IOError(e) => write!(f, "IO error: {}", e),
            TcpError::GenericError => write!(f, "Tcp Generic Error"),
        }
    }
}

impl From<std::io::Error> for TcpError {
    fn from(e: std::io::Error) -> Self {
        TcpError::IOError(e)
    }
}

impl From<ServerSendError> for TcpError {
    fn from(e: ServerSendError) -> Self {
        TcpError::SendError(e)
    }
}

pub enum UdpError {
    DisconnectBeforeCryptSetup,

    IOError(std::io::Error),
    GenericError, //TODO remove
}

impl From<std::io::Error> for UdpError {
    fn from(e: std::io::Error) -> Self {
        UdpError::IOError(e)
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
    OutputPauseError(cpal::PauseStreamError),
    InputPlayError(cpal::PlayStreamError),
    InputPauseError(cpal::PauseStreamError),
    InputDeviceClosed,
    GenericError, //TODO remove
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::NoDevice(s) => write!(f, "No {} device", s),
            AudioError::NoConfigs(s, e) => write!(f, "No {} configs: {}", s, e),
            AudioError::NoSupportedConfig(s) => write!(f, "No supported {} config found", s),
            AudioError::InvalidStream(s, e) => write!(f, "Invalid {} stream: {}", s, e),
            AudioError::OutputPlayError(e) => write!(f, "Playback error: {}", e),
            AudioError::OutputPauseError(e) => write!(f, "Playback error: {}", e),
            AudioError::InputPlayError(e) => write!(f, "Recording error: {}", e),
            AudioError::InputPauseError(e) => write!(f, "Recording error: {}", e),
            AudioError::InputDeviceClosed => write!(f, "Input Device Closed"),
            AudioError::GenericError => write!(f, "Audio Generic Error"),
        }
    }
}

pub enum StateError {
    AudioError(AudioError),
    ConfigError(ConfigError),

    GenericError, //TODO remove
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
            StateError::GenericError => write!(f, "Generic error"),
        }
    }
}
