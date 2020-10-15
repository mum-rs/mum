use serde::{Serialize, Deserialize};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Serialize, Deserialize)]
pub enum Error {
    DisconnectedError,
    AlreadyConnectedError,
    InvalidChannelIdError,
    InvalidServerAddrError,
}