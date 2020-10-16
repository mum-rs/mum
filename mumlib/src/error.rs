use serde::{Serialize, Deserialize};
use std::fmt::Display;
use serde::export::Formatter;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Serialize, Deserialize)]
pub enum Error {
    DisconnectedError,
    AlreadyConnectedError,
    InvalidChannelIdentifierError(String),
    InvalidServerAddrError(String, u16),
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::DisconnectedError => write!(f, "Not connected to a server"),
            Error::AlreadyConnectedError => write!(f, "Already connected to a server"),
            Error::InvalidChannelIdentifierError(id) => write!(f, "Couldn't find channel {}", id),
            Error::InvalidServerAddrError(addr, port) => write!(f, "Invalid server address: {}:{}", addr, port),
        }
    }
}