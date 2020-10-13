pub enum Command {
    ChannelJoin {
        channel_id: u32,
    },
    ChannelList,
    ServerConnect {
        host: String,
        port: u16,
        username: String,
        accept_invalid_cert: bool, //TODO ask when connecting
    },
    ServerDisconnect,
    Status,
}
