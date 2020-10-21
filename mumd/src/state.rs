pub mod server;
pub mod channel;
pub mod user;

use crate::audio::Audio;
use crate::network::ConnectionInfo;
use crate::state::server::Server;

use log::*;
use mumble_protocol::control::msgs;
use mumble_protocol::control::ControlPacket;
use mumble_protocol::voice::Serverbound;
use mumlib::command::{Command, CommandResponse};
use mumlib::config::Config;
use mumlib::error::{ChannelIdentifierError, Error};
use std::net::ToSocketAddrs;
use tokio::sync::{mpsc, watch};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StatePhase {
    Disconnected,
    Connecting,
    Connected,
}

pub struct State {
    config: Option<Config>,
    server: Option<Server>,
    audio: Audio,

    packet_sender: mpsc::UnboundedSender<ControlPacket<Serverbound>>,
    connection_info_sender: watch::Sender<Option<ConnectionInfo>>,

    phase_watcher: (watch::Sender<StatePhase>, watch::Receiver<StatePhase>),
}

impl State {
    pub fn new(
        packet_sender: mpsc::UnboundedSender<ControlPacket<Serverbound>>,
        connection_info_sender: watch::Sender<Option<ConnectionInfo>>,
    ) -> Self {
        let audio = Audio::new();
        let mut state = Self {
            config: mumlib::config::read_default_cfg(),
            server: None,
            audio,
            packet_sender,
            connection_info_sender,
            phase_watcher: watch::channel(StatePhase::Disconnected),
        };
        state.reload_config();
        state
    }

    //TODO? move bool inside Result
    pub async fn handle_command(
        &mut self,
        command: Command,
    ) -> (bool, mumlib::error::Result<Option<CommandResponse>>) {
        match command {
            Command::ChannelJoin { channel_identifier } => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected) {
                    return (false, Err(Error::DisconnectedError));
                }

                let channels = self.server()
                    .unwrap()
                    .channels();

                let matches = channels.iter()
                    .map(|e| (e.0, e.1.path(channels)))
                    .filter(|e| e.1.ends_with(&channel_identifier))
                    .collect::<Vec<_>>();
                let id = match matches.len() {
                    0 => {
                        let soft_matches = channels.iter()
                            .map(|e| (e.0, e.1.path(channels).to_lowercase()))
                            .filter(|e| e.1.ends_with(&channel_identifier.to_lowercase()))
                            .collect::<Vec<_>>();
                        match soft_matches.len() {
                            0 => return (false, Err(Error::ChannelIdentifierError(channel_identifier, ChannelIdentifierError::Invalid))),
                            1 => *soft_matches.get(0).unwrap().0,
                            _ => return (false, Err(Error::ChannelIdentifierError(channel_identifier, ChannelIdentifierError::Invalid))),
                        }
                    },
                    1 => *matches.get(0).unwrap().0,
                    _ => return (false, Err(Error::ChannelIdentifierError(channel_identifier, ChannelIdentifierError::Ambiguous))),
                };

                let mut msg = msgs::UserState::new();
                msg.set_session(self.server.as_ref().unwrap().session_id().unwrap());
                msg.set_channel_id(id);
                self.packet_sender.send(msg.into()).unwrap();
                (false, Ok(None))
            }
            Command::ChannelList => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected) {
                    return (false, Err(Error::DisconnectedError));
                }
                (
                    false,
                    Ok(Some(CommandResponse::ChannelList {
                        channels: channel::into_channel(
                            self.server.as_ref().unwrap().channels(),
                            self.server.as_ref().unwrap().users(),
                        ),
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
                let mut server = Server::new();
                *server.username_mut() = Some(username);
                *server.host_mut() = Some(format!("{}:{}", host, port));
                self.server = Some(server);
                self.phase_watcher
                    .0
                    .broadcast(StatePhase::Connecting)
                    .unwrap();

                let socket_addr = match (host.as_ref(), port)
                    .to_socket_addrs()
                    .map(|mut e| e.next())
                {
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
                        server_state: self.server.as_ref().unwrap().into(), //guaranteed not to panic because if we are connected, server is guaranteed to be Some
                    })),
                )
            }
            Command::ServerDisconnect => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected) {
                    return (false, Err(Error::DisconnectedError));
                }

                self.server = None;
                self.audio.clear_clients();

                self.phase_watcher
                    .0
                    .broadcast(StatePhase::Disconnected)
                    .unwrap();
                (false, Ok(None))
            }
            Command::InputVolumeSet(volume) => {
                self.audio.set_input_volume(volume);
                (false, Ok(None))
            }
            Command::ConfigReload => {
                self.reload_config();
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
        } else if msg.get_name() == self.server.as_ref().unwrap().username().unwrap() {
            match self.server.as_ref().unwrap().session_id() {
                None => {
                    debug!("Found our session id: {}", msg.get_session());
                    *self.server_mut().unwrap().session_id_mut() = Some(msg.get_session());
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

    pub fn reload_config(&mut self) {
        if let Some(config) = mumlib::config::read_default_cfg() {
            self.config = Some(config);
            let config = &self.config.as_ref().unwrap();
            if let Some(audio_config) = &config.audio {
                if let Some(input_volume) = audio_config.input_volume {
                    self.audio.set_input_volume(input_volume);
                }
            }
        } else {
            warn!("config file not found");
        }
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
    pub fn server(&self) -> Option<&Server> {
        self.server.as_ref()
    }
    pub fn server_mut(&mut self) -> Option<&mut Server> {
        self.server.as_mut()
    }
    pub fn username(&self) -> Option<&str> {
        self.server.as_ref().map(|e| e.username()).flatten()
    }
}

