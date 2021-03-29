use serde::{Deserialize, Serialize};
use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Error {
    DisconnectedError,
    AlreadyConnectedError,
    ChannelIdentifierError(String, ChannelIdentifierError),
    InvalidServerAddrError(String, u16),
    InvalidUsernameError(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::DisconnectedError => write!(f, "Not connected to a server"),
            Error::AlreadyConnectedError => write!(f, "Already connected to a server"),
            Error::ChannelIdentifierError(id, kind) => write!(f, "{}: {}", kind, id),
            Error::InvalidServerAddrError(addr, port) => {
                write!(f, "Invalid server address: {}: {}", addr, port)
            }
            Error::InvalidUsernameError(username) => write!(f, "Invalid username: {}", username),
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
