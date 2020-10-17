use crate::state::{Channel, Server};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Command {
    ChannelJoin {
        channel_id: u32,
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
    ChannelList { channels: Channel },
    Status { server_state: Server },
}
