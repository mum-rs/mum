use crate::audio::Audio;
use crate::network::ConnectionInfo;

use log::*;
use mumble_protocol::control::msgs;
use mumble_protocol::control::ControlPacket;
use mumble_protocol::voice::Serverbound;
use mumlib::command::{Command, CommandResponse};
use mumlib::state::Server;
use std::net::ToSocketAddrs;
use tokio::sync::{mpsc, watch};
use mumlib::error::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StatePhase {
    Disconnected,
    Connecting,
    Connected,
}

pub struct State {
    server: Option<Server>,
    audio: Audio,

    packet_sender: mpsc::UnboundedSender<ControlPacket<Serverbound>>,
    connection_info_sender: watch::Sender<Option<ConnectionInfo>>,

    phase_watcher: (watch::Sender<StatePhase>, watch::Receiver<StatePhase>),

    username: Option<String>,
    session_id: Option<u32>,
}

impl State {
    pub fn new(
        packet_sender: mpsc::UnboundedSender<ControlPacket<Serverbound>>,
        connection_info_sender: watch::Sender<Option<ConnectionInfo>>,
    ) -> Self {
        Self {
            server: None,
            audio: Audio::new(),
            packet_sender,
            connection_info_sender,
            phase_watcher: watch::channel(StatePhase::Disconnected),
            username: None,
            session_id: None,
        }
    }

    //TODO? move bool inside Result
    pub async fn handle_command(
        &mut self,
        command: Command,
    ) -> (bool, mumlib::error::Result<Option<CommandResponse>>) {
        match command {
            Command::ChannelJoin { channel_id } => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected) {
                    return (false, Err(Error::DisconnectedError));
                }
                if let Some(server) = &self.server {
                    if !server.channels().contains_key(&channel_id) {
                        return (false, Err(Error::InvalidChannelIdError(channel_id)));
                    }
                }
                let mut msg = msgs::UserState::new();
                msg.set_session(self.session_id.unwrap());
                msg.set_channel_id(channel_id);
                self.packet_sender.send(msg.into()).unwrap();
                (false, Ok(None))
            }
            Command::ChannelList => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected) {
                    return (false, Err(Error::DisconnectedError));
                }
                (false, Ok(Some(CommandResponse::ChannelList {
                        channels: self.server.as_ref().unwrap().channels().clone(),
                    })),
                )
            }
            Command::ServerConnect {
                host,
                port,
                username,
                accept_invalid_cert,
            } => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Disconnected) {
                    return (false, Err(Error::AlreadyConnectedError));
                }
                self.server = Some(Server::new());
                self.username = Some(username);
                self.phase_watcher
                    .0
                    .broadcast(StatePhase::Connecting)
                    .unwrap();

                let socket_addr = match (host.as_ref(), port).to_socket_addrs().map(|mut e| e.next()) {
                    Ok(Some(v)) => v,
                    _ => {
                        warn!("Error parsing server addr");
                        return (false, Err(Error::InvalidServerAddrError(host, port)));
                    }
                };
                self.connection_info_sender
                    .broadcast(Some(ConnectionInfo::new(
                        socket_addr,
                        host,
                        accept_invalid_cert,
                    )))
                    .unwrap();
                (true, Ok(None))
            }
            Command::Status => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected) {
                    return (false, Err(Error::DisconnectedError));
                }
                (
                    false,
                    Ok(Some(CommandResponse::Status {
                        username: self.username.clone(),
                        server_state: self.server.clone().unwrap(), //guaranteed not to panic because if we are connected, server is guaranteed to be Some
                    })),
                )
            }
            Command::ServerDisconnect => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected) {
                    return (false, Err(Error::DisconnectedError));
                }

                self.session_id = None;
                self.username = None;
                self.server = None;
                self.audio.clear_clients();

                self.phase_watcher
                    .0
                    .broadcast(StatePhase::Disconnected)
                    .unwrap();
                (false, Ok(None))
            }
        }
    }

    pub fn parse_initial_user_state(&mut self, msg: msgs::UserState) {
        if !msg.has_session() {
            warn!("Can't parse user state without session");
            return;
        }
        if !msg.has_name() {
            warn!("Missing name in initial user state");
        } else if msg.get_name() == self.username.as_ref().unwrap() {
            match self.session_id {
                None => {
                    debug!("Found our session id: {}", msg.get_session());
                    self.session_id = Some(msg.get_session());
                }
                Some(session) => {
                    if session != msg.get_session() {
                        error!(
                            "Got two different session IDs ({} and {}) for ourselves",
                            session,
                            msg.get_session()
                        );
                    } else {
                        debug!("Got our session ID twice");
                    }
                }
            }
        }
        self.server.as_mut().unwrap().parse_user_state(msg);
    }

    pub fn initialized(&self) {
        self.phase_watcher
            .0
            .broadcast(StatePhase::Connected)
            .unwrap();
    }

    pub fn audio(&self) -> &Audio {
        &self.audio
    }
    pub fn audio_mut(&mut self) -> &mut Audio {
        &mut self.audio
    }
    pub fn packet_sender(&self) -> mpsc::UnboundedSender<ControlPacket<Serverbound>> {
        self.packet_sender.clone()
    }
    pub fn phase_receiver(&self) -> watch::Receiver<StatePhase> {
        self.phase_watcher.1.clone()
    }
    pub fn server_mut(&mut self) -> Option<&mut Server> {
        self.server.as_mut()
    }
    pub fn username(&self) -> Option<&String> {
        self.username.as_ref()
    }
}
