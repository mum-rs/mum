use serde::export::Formatter;
use serde::{Deserialize, Serialize};
use std::fmt::Display;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Serialize, Deserialize)]
pub enum Error {
    DisconnectedError,
    AlreadyConnectedError,
    ChannelIdentifierError(String, ChannelIdentifierError),
    InvalidUserIdentifierError(String),
    InvalidServerAddrError(String, u16),
    InvalidUsernameError(String),
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::DisconnectedError => write!(f, "Not connected to a server"),
            Error::AlreadyConnectedError => write!(f, "Already connected to a server"),
            Error::ChannelIdentifierError(id, kind) => write!(f, "{}: {}", kind, id),
            Error::InvalidServerAddrError(addr, port) => write!(f, "Invalid server address: {}: {}", addr, port),
            Error::InvalidUserIdentifierError(name) => write!(f, "Invalid username: {}", name),
            Error::InvalidUsernameError(username) => write!(f, "Invalid username: {}", username),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ChannelIdentifierError {
    Invalid,
    Ambiguous,
}

impl Display for ChannelIdentifierError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelIdentifierError::Invalid => write!(f, "Invalid channel identifier"),
            ChannelIdentifierError::Ambiguous => write!(f, "Ambiguous channel identifier"),
        }
    }
}
