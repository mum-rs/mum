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
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Disconnected=> write!(f, "Not connected to a server"),
            Error::AlreadyConnected=> write!(f, "Already connected to a server"),
            Error::ChannelIdentifierError(id, kind) => write!(f, "{}: {}", kind, id),
            Error::InvalidServerAddr(addr, port) => {
                write!(f, "Invalid server address: {}: {}", addr, port)
            }
            Error::InvalidUsername(username) => write!(f, "Invalid username: {}", username),
            Error::InvalidServerPassword => write!(f, "Invalid server password")
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
