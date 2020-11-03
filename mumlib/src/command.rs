use crate::state::{Channel, Server};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Command {
    ChannelJoin {
        channel_identifier: String,
    },
    ChannelList,
    ConfigReload,
    InputVolumeSet(f32),
    OutputVolumeSet(f32),
    UserVolumeSet(String, f32),
    ServerConnect {
        host: String,
        port: u16,
        username: String,
        accept_invalid_cert: bool,
    },
    ServerDisconnect,
    Status,
    DeafenSelf,
    MuteSelf,
    MuteOther(String),
    ServerStatus {
        host: String,
        port: u16,
    },
}

#[derive(Debug, Deserialize, Serialize)]
pub enum CommandResponse {
    ChannelList {
        channels: Channel,
    },
    ServerConnect {
        welcome_message: Option<String>,
    },
    Status {
        server_state: Server,
    },
    ServerStatus {
        version: u32,
        users: u32,
        max_users: u32,
        bandwidth: u32,
    },
}
