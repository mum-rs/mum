use serde::{Deserialize, Serialize};
use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Error {
    Disconnected,
    AlreadyConnected,
    ChannelIdentifierError(String, ChannelIdentifierError),
    InvalidServerAddr(String, u16),
    InvalidUsername(String),
    InvalidServerPassword,
    Unimplemented,
    NotConnectedToChannel,
    ServerCertReject,
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Disconnected => write!(f, "Not connected to a server"),
            Error::AlreadyConnected => write!(f, "Already connected to a server"),
            Error::ChannelIdentifierError(id, kind) => write!(f, "{}: {}", kind, id),
            Error::InvalidServerAddr(addr, port) => {
                write!(f, "Invalid server address: {}: {}", addr, port)
            }
            Error::InvalidUsername(username) => write!(f, "Invalid username: {}", username),
            Error::InvalidServerPassword => write!(f, "Invalid server password"),
            Error::Unimplemented => write!(f, "Unimplemented"),
            Error::NotConnectedToChannel => write!(f, "Not connected to a channel"),
            Error::ServerCertReject => write!(f, "Invalid server certificate"),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum ChannelIdentifierError {
    Invalid,
    Ambiguous,
}

impl fmt::Display for ChannelIdentifierError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChannelIdentifierError::Invalid => write!(f, "Invalid channel identifier"),
            ChannelIdentifierError::Ambiguous => write!(f, "Ambiguous channel identifier"),
        }
    }
}

impl std::error::Error for ChannelIdentifierError {}

#[derive(Debug)]
pub enum ConfigError {
    InvalidConfig,
    TomlError(toml_edit::TomlError),
    TomlErrorSer(toml_edit::ser::Error),
    TomlErrorDe(toml_edit::de::Error),

    WontCreateFile,
    IOError(std::io::Error),
}

impl std::error::Error for ConfigError {}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::InvalidConfig => write!(f, "Invalid configuration"),
            ConfigError::TomlError(e) => write!(f, "Invalid TOML: {}", e),
            ConfigError::TomlErrorSer(e) => write!(f, "Invalid TOML when serializing: {}", e),
            ConfigError::TomlErrorDe(e) => write!(f, "Invalid TOML when deserializing: {}", e),
            ConfigError::WontCreateFile => {
                write!(f, "File does not exist but caller didn't allow creation")
            }
            ConfigError::IOError(e) => write!(f, "IO error: {}", e),
        }
    }
}

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        ConfigError::IOError(e)
    }
}

impl From<toml_edit::ser::Error> for ConfigError {
    fn from(e: toml_edit::ser::Error) -> Self {
        ConfigError::TomlErrorSer(e)
    }
}

impl From<toml_edit::de::Error> for ConfigError {
    fn from(e: toml_edit::de::Error) -> Self {
        ConfigError::TomlErrorDe(e)
    }
}

impl From<toml_edit::TomlError> for ConfigError {
    fn from(e: toml_edit::TomlError) -> Self {
        ConfigError::TomlError(e)
    }
}
