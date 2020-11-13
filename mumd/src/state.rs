pub mod channel;
pub mod server;
pub mod user;

use crate::audio::Audio;
use crate::network::ConnectionInfo;
use crate::notify;
use crate::state::server::Server;

use crate::network::tcp::{TcpEvent, TcpEventData};
use log::*;
use mumble_protocol::control::msgs;
use mumble_protocol::control::ControlPacket;
use mumble_protocol::ping::PongPacket;
use mumble_protocol::voice::Serverbound;
use mumlib::command::{Command, CommandResponse};
use mumlib::config::Config;
use mumlib::error::{ChannelIdentifierError, Error};
use mumlib::state::UserDiff;
use std::net::{SocketAddr, ToSocketAddrs};
use tokio::sync::{mpsc, watch};

macro_rules! at {
    ($event:expr, $generator:expr) => {
        ExecutionContext::TcpEvent($event, Box::new($generator))
    };
}

macro_rules! now {
    ($data:expr) => {
        ExecutionContext::Now(Box::new(move || $data))
    };
}

//TODO give me a better name
pub enum ExecutionContext {
    TcpEvent(
        TcpEvent,
        Box<dyn FnOnce(TcpEventData) -> mumlib::error::Result<Option<CommandResponse>>>,
    ),
    Now(Box<dyn FnOnce() -> mumlib::error::Result<Option<CommandResponse>>>),
    Ping(
        Box<dyn FnOnce() -> mumlib::error::Result<SocketAddr>>,
        Box<dyn FnOnce(PongPacket) -> mumlib::error::Result<Option<CommandResponse>>>,
    ),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StatePhase {
    Disconnected,
    Connecting,
    Connected,
}

pub struct State {
    config: Config,
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
        let config = mumlib::config::read_default_cfg();
        let audio = Audio::new(
            config.audio.input_volume.unwrap_or(1.0),
            config.audio.output_volume.unwrap_or(1.0),
        );
        let mut state = Self {
            config,
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
    pub fn handle_command(&mut self, command: Command) -> ExecutionContext {
        match command {
            Command::ChannelJoin { channel_identifier } => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected) {
                    return now!(Err(Error::DisconnectedError));
                }

                let channels = self.server().unwrap().channels();

                let matches = channels
                    .iter()
                    .map(|e| (e.0, e.1.path(channels)))
                    .filter(|e| e.1.ends_with(&channel_identifier))
                    .collect::<Vec<_>>();
                let id = match matches.len() {
                    0 => {
                        let soft_matches = channels
                            .iter()
                            .map(|e| (e.0, e.1.path(channels).to_lowercase()))
                            .filter(|e| e.1.ends_with(&channel_identifier.to_lowercase()))
                            .collect::<Vec<_>>();
                        match soft_matches.len() {
                            0 => {
                                return now!(Err(Error::ChannelIdentifierError(
                                    channel_identifier,
                                    ChannelIdentifierError::Invalid
                                )))
                            }
                            1 => *soft_matches.get(0).unwrap().0,
                            _ => {
                                return now!(Err(Error::ChannelIdentifierError(
                                    channel_identifier,
                                    ChannelIdentifierError::Invalid
                                )))
                            }
                        }
                    }
                    1 => *matches.get(0).unwrap().0,
                    _ => {
                        return now!(Err(Error::ChannelIdentifierError(
                            channel_identifier,
                            ChannelIdentifierError::Ambiguous
                        )))
                    }
                };

                let mut msg = msgs::UserState::new();
                msg.set_session(self.server.as_ref().unwrap().session_id().unwrap());
                msg.set_channel_id(id);
                self.packet_sender.send(msg.into()).unwrap();
                now!(Ok(None))
            }
            Command::ChannelList => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected) {
                    return now!(Err(Error::DisconnectedError));
                }
                let list = channel::into_channel(
                    self.server.as_ref().unwrap().channels(),
                    self.server.as_ref().unwrap().users(),
                );
                now!(Ok(Some(CommandResponse::ChannelList { channels: list })))
            }
            Command::ServerConnect {
                host,
                port,
                username,
                accept_invalid_cert,
            } => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Disconnected) {
                    return now!(Err(Error::AlreadyConnectedError));
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
                        return now!(Err(Error::InvalidServerAddrError(host, port)));
                    }
                };
                self.connection_info_sender
                    .broadcast(Some(ConnectionInfo::new(
                        socket_addr,
                        host,
                        accept_invalid_cert,
                    )))
                    .unwrap();
                at!(TcpEvent::Connected, |e| {
                    //runs the closure when the client is connected
                    if let TcpEventData::Connected(msg) = e {
                        Ok(Some(CommandResponse::ServerConnect {
                            welcome_message: if msg.has_welcome_text() {
                                Some(msg.get_welcome_text().to_string())
                            } else {
                                None
                            },
                        }))
                    } else {
                        unreachable!("callback should be provided with a TcpEventData::Connected");
                    }
                })
            }
            Command::Status => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected) {
                    return now!(Err(Error::DisconnectedError));
                }
                let state = self.server.as_ref().unwrap().into();
                now!(Ok(Some(CommandResponse::Status {
                    server_state: state, //guaranteed not to panic because if we are connected, server is guaranteed to be Some
                })))
            }
            Command::ServerDisconnect => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected) {
                    return now!(Err(Error::DisconnectedError));
                }

                self.server = None;
                self.audio.clear_clients();

                self.phase_watcher
                    .0
                    .broadcast(StatePhase::Disconnected)
                    .unwrap();
                now!(Ok(None))
            }
            Command::ConfigReload => {
                self.reload_config();
                now!(Ok(None))
            }
            Command::InputVolumeSet(volume) => {
                self.audio.set_input_volume(volume);
                now!(Ok(None))
            }
            Command::OutputVolumeSet(volume) => {
                self.audio.set_output_volume(volume);
                now!(Ok(None))
            }
            Command::DeafenSelf(toggle) => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected) {
                    return now!(Err(Error::DisconnectedError));
                }

                let server = self.server_mut().unwrap();
                let action = match (toggle, server.muted(), server.deafened()) {
                    (Some(false), false, false) => None,
                    (Some(false), false, true) => Some((false, false)),
                    (Some(false), true, false) => None,
                    (Some(false), true, true) => Some((true, false)),
                    (Some(true), false, false) => Some((false, true)),
                    (Some(true), false, true) => None,
                    (Some(true), true, false) => Some((true, true)),
                    (Some(true), true, true) => None,
                    (None, false, false) => Some((false, true)),
                    (None, false, true) => Some((false, false)),
                    (None, true, false) => Some((true, true)),
                    (None, true, true) => Some((true, false)),
                };

                if let Some((mute, deafen)) = action {
                    let mut msg = msgs::UserState::new();
                    if server.muted() != mute {
                        msg.set_self_mute(mute);
                    } else if !mute && !deafen && server.deafened() {
                        msg.set_self_mute(false);
                    }
                    if server.deafened() != deafen {
                        msg.set_self_deaf(deafen);
                    }
                    server.set_muted(mute);
                    server.set_deafened(deafen);
                    self.packet_sender.send(msg.into()).unwrap();
                }

                now!(Ok(None))
            }
            Command::MuteSelf(toggle) => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected) {
                    return now!(Err(Error::DisconnectedError));
                }

                let server = self.server_mut().unwrap();
                let action = match (toggle, server.muted(), server.deafened()) {
                    (Some(false), false, false) => None,
                    (Some(false), false, true) => Some((false, false)),
                    (Some(false), true, false) => Some((false, false)),
                    (Some(false), true, true) => Some((false, false)),
                    (Some(true), false, false) => Some((true, false)),
                    (Some(true), false, true) => None,
                    (Some(true), true, false) => None,
                    (Some(true), true, true) => None,
                    (None, false, false) => Some((true, false)),
                    (None, false, true) => Some((false, false)),
                    (None, true, false) => Some((false, false)),
                    (None, true, true) => Some((false, false)),
                };

                if let Some((mute, deafen)) = action {
                    let mut msg = msgs::UserState::new();
                    if server.muted() != mute {
                        msg.set_self_mute(mute);
                    } else if !mute && !deafen && server.deafened() {
                        msg.set_self_mute(false);
                    }
                    if server.deafened() != deafen {
                        msg.set_self_deaf(deafen);
                    }
                    server.set_muted(mute);
                    server.set_deafened(deafen);
                    self.packet_sender.send(msg.into()).unwrap();
                }

                now!(Ok(None))
            }
            Command::MuteOther(string, toggle) => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected) {
                    return now!(Err(Error::DisconnectedError));
                }

                let id = self
                    .server_mut()
                    .unwrap()
                    .users_mut()
                    .iter_mut()
                    .find(|(_, user)| user.name() == &string);

                let (id, user) = match id {
                    Some(id) => (*id.0, id.1),
                    None => return now!(Err(Error::InvalidUsernameError(string))),
                };

                let action = match toggle {
                    Some(state) => {
                        if user.suppressed() != state {
                            Some(state)
                        } else {
                            None
                        }
                    }
                    None => Some(!user.suppressed()),
                };

                if let Some(action) = action {
                    user.set_suppressed(action);
                    self.audio.set_mute(id, action);
                }

                return now!(Ok(None));
            }
            Command::UserVolumeSet(string, volume) => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected) {
                    return now!(Err(Error::DisconnectedError));
                }
                let user_id = match self
                    .server()
                    .unwrap()
                    .users()
                    .iter()
                    .find(|e| e.1.name() == &string)
                    .map(|e| *e.0)
                {
                    None => return now!(Err(Error::InvalidUsernameError(string))),
                    Some(v) => v,
                };

                self.audio.set_user_volume(user_id, volume);
                now!(Ok(None))
            }
            Command::ServerStatus { host, port } => ExecutionContext::Ping(
                Box::new(move || {
                    match (host.as_str(), port)
                        .to_socket_addrs()
                        .map(|mut e| e.next())
                    {
                        Ok(Some(v)) => Ok(v),
                        _ => Err(mumlib::error::Error::InvalidServerAddrError(host, port)),
                    }
                }),
                Box::new(move |pong| {
                    Ok(Some(CommandResponse::ServerStatus {
                        version: pong.version,
                        users: pong.users,
                        max_users: pong.max_users,
                        bandwidth: pong.bandwidth,
                    }))
                }),
            ),
        }
    }

    pub fn parse_user_state(&mut self, msg: msgs::UserState) -> Option<UserDiff> {
        if !msg.has_session() {
            warn!("Can't parse user state without session");
            return None;
        }
        let session = msg.get_session();
        // check if this is initial state
        if !self.server().unwrap().users().contains_key(&session) {
            self.parse_initial_user_state(session, msg);
            None
        } else {
            Some(self.parse_updated_user_state(session, msg))
        }
    }

    fn parse_initial_user_state(&mut self, session: u32, msg: msgs::UserState) {
        if !msg.has_name() {
            warn!("Missing name in initial user state");
        } else if msg.get_name() == self.server().unwrap().username().unwrap() {
            // this is us
            *self.server_mut().unwrap().session_id_mut() = Some(session);
        } else {
            // this is someone else
            self.audio_mut().add_client(session);

            // send notification only if we've passed the connecting phase
            if *self.phase_receiver().borrow() == StatePhase::Connected {
                let channel_id = if msg.has_channel_id() {
                    msg.get_channel_id()
                } else {
                    0
                };
                if let Some(channel) = self.server().unwrap().channels().get(&channel_id) {
                    notify::send(format!(
                        "{} connected and joined {}",
                        &msg.get_name(),
                        channel.name()
                    ));
                }
            }
        }
        self.server_mut()
            .unwrap()
            .users_mut()
            .insert(session, user::User::new(msg));
    }

    fn parse_updated_user_state(&mut self, session: u32, msg: msgs::UserState) -> UserDiff {
        let user = self
            .server_mut()
            .unwrap()
            .users_mut()
            .get_mut(&session)
            .unwrap();

        let mute = if msg.has_self_mute() && user.self_mute() != msg.get_self_mute() {
            Some(msg.get_self_mute())
        } else {
            None
        };
        let deaf = if msg.has_self_deaf() && user.self_deaf() != msg.get_self_deaf() {
            Some(msg.get_self_deaf())
        } else {
            None
        };

        let diff = UserDiff::from(msg);
        user.apply_user_diff(&diff);
        let user = self.server().unwrap().users().get(&session).unwrap();

        //     send notification if the user moved to or from any channel
        //TODO our channel only
        if let Some(channel_id) = diff.channel_id {
            if let Some(channel) = self.server().unwrap().channels().get(&channel_id) {
                notify::send(format!(
                    "{} moved to channel {}",
                    &user.name(),
                    channel.name()
                ));
            } else {
                warn!("{} moved to invalid channel {}", &user.name(), channel_id);
            }
        }

        //     send notification if a user muted/unmuted
        //TODO our channel only
        let notify_desc = match (mute, deaf) {
            (Some(true), Some(true)) => Some(format!("{} muted and deafend themselves", &user.name())),
            (Some(false), Some(false)) => Some(format!("{} unmuted and undeafend themselves", &user.name())),
            (None, Some(true)) => Some(format!("{} deafend themselves", &user.name())),
            (None, Some(false)) => Some(format!("{} undeafend themselves", &user.name())),
            (Some(true), None) => Some(format!("{} muted themselves", &user.name())),
            (Some(false), None) => Some(format!("{} unmuted themselves", &user.name())),
            (Some(true), Some(false)) => Some(format!("{} muted and undeafened themselves", &user.name())),
            (Some(false), Some(true)) => Some(format!("{} unmuted and deafened themselves", &user.name())),
            (None, None) => None,
        };
        if let Some(notify_desc) = notify_desc {
            notify::send(notify_desc);
        }

        diff
    }

    pub fn remove_client(&mut self, msg: msgs::UserRemove) {
        if !msg.has_session() {
            warn!("Tried to remove user state without session");
            return;
        }
        if let Some(user) = self.server().unwrap().users().get(&msg.get_session()) {
            notify::send(format!("{} disconnected", &user.name()));
        }

        self.audio().remove_client(msg.get_session());
        self.server_mut()
            .unwrap()
            .users_mut()
            .remove(&msg.get_session());
        info!("User {} disconnected", msg.get_session());
    }

    pub fn reload_config(&mut self) {
        self.config = mumlib::config::read_default_cfg();
        if let Some(input_volume) = self.config.audio.input_volume {
            self.audio.set_input_volume(input_volume);
        }
        if let Some(output_volume) = self.config.audio.output_volume {
            self.audio.set_output_volume(output_volume);
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
