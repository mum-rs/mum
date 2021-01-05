pub mod channel;
pub mod server;
pub mod user;

use crate::audio::{Audio, NotificationEvents};
use crate::network::{ConnectionInfo, VoiceStreamType};
use crate::network::tcp::{TcpEvent, TcpEventData};
use crate::notify;
use crate::state::server::Server;

use log::*;
use mumble_protocol::control::msgs;
use mumble_protocol::control::ControlPacket;
use mumble_protocol::ping::PongPacket;
use mumble_protocol::voice::Serverbound;
use mumlib::command::{Command, CommandResponse};
use mumlib::config::Config;
use mumlib::error::{ChannelIdentifierError, Error};
use crate::state::user::UserDiff;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatePhase {
    Disconnected,
    Connecting,
    Connected(VoiceStreamType),
}

pub struct State {
    config: Config,
    server: Option<Server>,
    audio: Audio,

    phase_watcher: (watch::Sender<StatePhase>, watch::Receiver<StatePhase>),
}

impl State {
    pub fn new() -> Self {
        let config = mumlib::config::read_default_cfg();
        let audio = Audio::new(
            config.audio.input_volume.unwrap_or(1.0),
            config.audio.output_volume.unwrap_or(1.0),
        );
        let mut state = Self {
            config,
            server: None,
            audio,
            phase_watcher: watch::channel(StatePhase::Disconnected),
        };
        state.reload_config();
        state
    }

    pub fn handle_command(
        &mut self,
        command: Command,
        packet_sender: &mut mpsc::UnboundedSender<ControlPacket<Serverbound>>,
        connection_info_sender: &mut watch::Sender<Option<ConnectionInfo>>,
    ) -> ExecutionContext {
        match command {
            Command::ChannelJoin { channel_identifier } => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected(_)) {
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
                packet_sender.send(msg.into()).unwrap();
                now!(Ok(None))
            }
            Command::ChannelList => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected(_)) {
                    return now!(Err(Error::DisconnectedError));
                }
                let list = channel::into_channel(
                    self.server.as_ref().unwrap().channels(),
                    self.server.as_ref().unwrap().users(),
                );
                now!(Ok(Some(CommandResponse::ChannelList { channels: list })))
            }
            Command::ConfigReload => {
                self.reload_config();
                now!(Ok(None))
            }
            Command::DeafenSelf(toggle) => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected(_)) {
                    return now!(Err(Error::DisconnectedError));
                }

                let server = self.server().unwrap();
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

                let mut new_deaf = None;
                if let Some((mute, deafen)) = action {
                    if server.deafened() != deafen {
                        self.audio.play_effect(if deafen {
                            NotificationEvents::Deafen
                        } else {
                            NotificationEvents::Undeafen
                        });
                    } else if server.muted() != mute {
                        self.audio.play_effect(if mute {
                            NotificationEvents::Mute
                        } else {
                            NotificationEvents::Unmute
                        });
                    }
                    let mut msg = msgs::UserState::new();
                    if server.muted() != mute {
                        msg.set_self_mute(mute);
                    } else if !mute && !deafen && server.deafened() {
                        msg.set_self_mute(false);
                    }
                    if server.deafened() != deafen {
                        msg.set_self_deaf(deafen);
                        new_deaf = Some(deafen);
                    }
                    let server = self.server_mut().unwrap();
                    server.set_muted(mute);
                    server.set_deafened(deafen);
                    packet_sender.send(msg.into()).unwrap();
                }

                now!(Ok(new_deaf.map(|b| CommandResponse::DeafenStatus { is_deafened: b })))
            }
            Command::InputVolumeSet(volume) => {
                self.audio.set_input_volume(volume);
                now!(Ok(None))
            }
            Command::MuteOther(string, toggle) => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected(_)) {
                    return now!(Err(Error::DisconnectedError));
                }

                let id = self
                    .server_mut()
                    .unwrap()
                    .users_mut()
                    .iter_mut()
                    .find(|(_, user)| user.name() == string);

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
            Command::MuteSelf(toggle) => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected(_)) {
                    return now!(Err(Error::DisconnectedError));
                }

                let server = self.server().unwrap();
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

                let mut new_mute = None;
                if let Some((mute, deafen)) = action {
                    if server.deafened() != deafen {
                        self.audio.play_effect(if deafen {
                            NotificationEvents::Deafen
                        } else {
                            NotificationEvents::Undeafen
                        });
                    } else if server.muted() != mute {
                        self.audio.play_effect(if mute {
                            NotificationEvents::Mute
                        } else {
                            NotificationEvents::Unmute
                        });
                    }
                    let mut msg = msgs::UserState::new();
                    if server.muted() != mute {
                        msg.set_self_mute(mute);
                        new_mute = Some(mute)
                    } else if !mute && !deafen && server.deafened() {
                        msg.set_self_mute(false);
                        new_mute = Some(false)
                    }
                    if server.deafened() != deafen {
                        msg.set_self_deaf(deafen);
                    }
                    let server = self.server_mut().unwrap();
                    server.set_muted(mute);
                    server.set_deafened(deafen);
                    packet_sender.send(msg.into()).unwrap();
                }

                now!(Ok(new_mute.map(|b| CommandResponse::MuteStatus { is_muted: b })))
            }
            Command::OutputVolumeSet(volume) => {
                self.audio.set_output_volume(volume);
                now!(Ok(None))
            }
            Command::Ping => {
                now!(Ok(Some(CommandResponse::Pong)))
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
                    .send(StatePhase::Connecting)
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
                connection_info_sender
                    .send(Some(ConnectionInfo::new(
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
            Command::ServerDisconnect => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected(_)) {
                    return now!(Err(Error::DisconnectedError));
                }

                self.server = None;
                self.audio.clear_clients();

                self.phase_watcher
                    .0
                    .send(StatePhase::Disconnected)
                    .unwrap();
                self.audio.play_effect(NotificationEvents::ServerDisconnect);
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
            Command::Status => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected(_)) {
                    return now!(Err(Error::DisconnectedError));
                }
                let state = self.server.as_ref().unwrap().into();
                now!(Ok(Some(CommandResponse::Status {
                    server_state: state, //guaranteed not to panic because if we are connected, server is guaranteed to be Some
                })))
            }
            Command::UserVolumeSet(string, volume) => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected(_)) {
                    return now!(Err(Error::DisconnectedError));
                }
                let user_id = match self
                    .server()
                    .unwrap()
                    .users()
                    .iter()
                    .find(|e| e.1.name() == string)
                    .map(|e| *e.0)
                {
                    None => return now!(Err(Error::InvalidUsernameError(string))),
                    Some(v) => v,
                };

                self.audio.set_user_volume(user_id, volume);
                now!(Ok(None))
            }
        }
    }

    pub fn parse_user_state(&mut self, msg: msgs::UserState) {
        if !msg.has_session() {
            warn!("Can't parse user state without session");
            return;
        }
        let session = msg.get_session();
        // check if this is initial state
        if !self.server().unwrap().users().contains_key(&session) {
            self.create_user(msg);
        } else {
            self.update_user(msg);
        }
    }

    fn create_user(&mut self, msg: msgs::UserState) {
        if !msg.has_name() {
            warn!("Missing name in initial user state");
            return;
        }

        let session = msg.get_session();

        if msg.get_name() == self.server().unwrap().username().unwrap() {
            // this is us
            *self.server_mut().unwrap().session_id_mut() = Some(session);
        } else {
            // this is someone else
            self.audio_mut().add_client(session);

            // send notification only if we've passed the connecting phase
            if matches!(*self.phase_receiver().borrow(), StatePhase::Connected(_)) {
                let channel_id = msg.get_channel_id();

                if channel_id
                    == self.get_users_channel(self.server().unwrap().session_id().unwrap())
                {
                    if let Some(channel) = self.server().unwrap().channels().get(&channel_id) {
                        notify::send(format!(
                            "{} connected and joined {}",
                            &msg.get_name(),
                            channel.name()
                        ));
                    }

                    self.audio.play_effect(NotificationEvents::UserConnected);
                }
            }
        }

        self.server_mut()
            .unwrap()
            .users_mut()
            .insert(session, user::User::new(msg));
    }

    fn update_user(&mut self, msg: msgs::UserState) {
        let session = msg.get_session();

        let from_channel = self.get_users_channel(session);

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

        if Some(session) != self.server().unwrap().session_id() {
            //send notification if the user moved to or from any channel
            if let Some(to_channel) = diff.channel_id {
                let this_channel =
                    self.get_users_channel(self.server().unwrap().session_id().unwrap());
                if from_channel == this_channel || to_channel == this_channel {
                    if let Some(channel) = self.server().unwrap().channels().get(&to_channel) {
                        notify::send(format!(
                            "{} moved to channel {}",
                            user.name(),
                            channel.name()
                        ));
                    } else {
                        warn!("{} moved to invalid channel {}", user.name(), to_channel);
                    }
                    self.audio.play_effect(if from_channel == this_channel {
                        NotificationEvents::UserJoinedChannel
                    } else {
                        NotificationEvents::UserLeftChannel
                    });
                }
            }

            //send notification if a user muted/unmuted
            if mute != None || deaf != None {
                let mut s = user.name().to_string();
                if let Some(mute) = mute {
                    s += if mute { " muted" } else { " unmuted" };
                }
                if mute.is_some() && deaf.is_some() {
                    s += " and";
                }
                if let Some(deaf) = deaf {
                    s += if deaf { " deafened" } else { " undeafened" };
                }
                s += " themselves";
                notify::send(s);
            }
        }
    }

    pub fn remove_client(&mut self, msg: msgs::UserRemove) {
        if !msg.has_session() {
            warn!("Tried to remove user state without session");
            return;
        }

        let this_channel = self.get_users_channel(self.server().unwrap().session_id().unwrap());
        let other_channel = self.get_users_channel(msg.get_session());
        if this_channel == other_channel {
            self.audio.play_effect(NotificationEvents::UserDisconnected);
            if let Some(user) = self.server().unwrap().users().get(&msg.get_session()) {
                notify::send(format!("{} disconnected", &user.name()));
            }
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
        if let Some(sound_effects) = &self.config.audio.sound_effects {
            self.audio.load_sound_effects(sound_effects);
        }
    }

    pub fn broadcast_phase(&self, phase: StatePhase) {
        self.phase_watcher
            .0
            .send(phase)
            .unwrap();
    }

    pub fn initialized(&self) {
        self.broadcast_phase(StatePhase::Connected(VoiceStreamType::TCP));
        self.audio.play_effect(NotificationEvents::ServerConnect);
    }

    pub fn audio(&self) -> &Audio {
        &self.audio
    }
    pub fn audio_mut(&mut self) -> &mut Audio {
        &mut self.audio
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
    fn get_users_channel(&self, user_id: u32) -> u32 {
        self.server()
            .unwrap()
            .users()
            .iter()
            .find(|e| *e.0 == user_id)
            .unwrap()
            .1
            .channel()
    }
}
