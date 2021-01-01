pub mod tcp;
pub mod udp;

use std::net::SocketAddr;

#[derive(Debug, Eq, PartialEq, Clone, Copy, Hash)]
pub enum VoiceStreamType {
    TCP,
    UDP
}

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
