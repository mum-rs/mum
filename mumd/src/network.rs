pub mod tcp;
pub mod udp;

use std::net::SocketAddr;

#[derive(Clone, Debug)]
pub struct ConnectionInfo {
    socket_addr: SocketAddr,
    hostname: String,
    accept_invalid_cert: bool,
}

impl ConnectionInfo {
    pub fn new(socket_addr: SocketAddr, hostname: String, accept_invalid_cert: bool) -> Self {
        Self {
            socket_addr,
            hostname,
            accept_invalid_cert,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum VoiceStreamType {
    TCP,
    UDP,
}
