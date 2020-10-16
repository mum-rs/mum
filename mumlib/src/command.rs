use crate::state::{Channel, Server};

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Command {
    ChannelJoin {
        channel_identifier: String,
    },
    ChannelList,
    ServerConnect {
        host: String,
        port: u16,
        username: String,
        accept_invalid_cert: bool,
    },
    ServerDisconnect,
    Status,
}

#[derive(Debug, Deserialize, Serialize)]
pub enum CommandResponse {
    ChannelList {
        channels: HashMap<u32, Channel>,
    },
    Status {
        username: Option<String>,
        server_state: Server,
    },
}
