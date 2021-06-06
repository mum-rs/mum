use crate::state::{Channel, Server};

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    UserConnected(String, Option<String>),
    UserDisconnected(String, Option<String>),
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Event::UserConnected(user, channel) => {
                write!(f, "{} connected to {}", user, channel.as_deref().unwrap_or("unknown channel"))
            }
            Event::UserDisconnected(user, channel) => {
                write!(f, "{} disconnected from {}", user, channel.as_deref().unwrap_or("unknown channel"))
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Command {
    ChannelJoin {
        channel_identifier: String,
    },
    ChannelList,
    ConfigReload,
    DeafenSelf(Option<bool>),
    Events {
        block: bool
    },
    InputVolumeSet(f32),
    MuteOther(String, Option<bool>),
    MuteSelf(Option<bool>),
    OutputVolumeSet(f32),
    PastMessages {
        block: bool,
    },
    Ping,
    SendMessage {
        message: String,
        targets: Vec<MessageTarget>,
    },
    ServerConnect {
        host: String,
        port: u16,
        username: String,
        password: Option<String>,
        accept_invalid_cert: bool,
    },
    ServerDisconnect,
    ServerStatus {
        host: String,
        port: u16,
    },
    Status,
    UserVolumeSet(String, f32),
}

#[derive(Debug, Deserialize, Serialize)]
pub enum CommandResponse {
    ChannelList {
        channels: Channel,
    },
    DeafenStatus {
        is_deafened: bool,
    },
    Event {
        event: Event,
    },
    MuteStatus {
        is_muted: bool,
    },
    PastMessage {
        message: (String, String),
    },
    Pong,
    ServerConnect {
        welcome_message: Option<String>,
    },
    ServerStatus {
        version: u32,
        users: u32,
        max_users: u32,
        bandwidth: u32,
    },
    Status {
        server_state: Server,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum MessageTarget {
    Channel { recursive: bool, name: String },
    User { name: String },
}
