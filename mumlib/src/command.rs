use crate::state::{Channel, Server};

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Command {
    ChannelJoin {
        channel_identifier: String,
    },
    ChannelList,
    ConfigReload,
    DeafenSelf(Option<bool>),
    InputVolumeSet(f32),
    MuteOther(String, Option<bool>),
    MuteSelf(Option<bool>),
    OutputVolumeSet(f32),
    Ping,
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
    PastMessages {
        block: bool,
    },
    SendMessage {
        message: String,
        targets: Vec<MessageTarget>,
    },
}

#[derive(Debug, Deserialize, Serialize)]
pub enum CommandResponse {
    ChannelList {
        channels: Channel,
    },
    DeafenStatus {
        is_deafened: bool,
    },
    MuteStatus {
        is_muted: bool,
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
    PastMessage {
        message: (NaiveDateTime, String, String),
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum MessageTarget {
    Channel { recursive: bool, name: String },
    User { name: String },
}
