pub mod channel;
pub mod server;
pub mod user;

use crate::audio::{AudioInput, AudioOutput, NotificationEvents};
use crate::error::StateError;
use crate::network::{ConnectionInfo, VoiceStreamType};
use crate::network::tcp::{TcpEvent, TcpEventData};
use crate::notifications;
use crate::state::server::Server;

use log::*;
use mumble_protocol::control::msgs;
use mumble_protocol::control::ControlPacket;
use mumble_protocol::ping::PongPacket;
use mumble_protocol::voice::Serverbound;
use mumlib::command::{Command, CommandResponse, MessageTarget};
use mumlib::config::Config;
use mumlib::Error;
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
        Box<dyn FnOnce(Option<PongPacket>) -> mumlib::error::Result<Option<CommandResponse>> + Send>,
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
    audio_input: AudioInput,
    audio_output: AudioOutput,
    message_buffer: Vec<(String, u32)>,

    phase_watcher: (watch::Sender<StatePhase>, watch::Receiver<StatePhase>),
}

impl State {
    pub fn new() -> Result<Self, StateError> {
        let config = mumlib::config::read_default_cfg()?;
        let phase_watcher = watch::channel(StatePhase::Disconnected);
        let audio_input = AudioInput::new(
            config.audio.input_volume.unwrap_or(1.0),
            phase_watcher.1.clone(),
        ).map_err(|e| StateError::AudioError(e))?;
        let audio_output = AudioOutput::new(
            config.audio.output_volume.unwrap_or(1.0),
        ).map_err(|e| StateError::AudioError(e))?;
        let mut state = Self {
            config,
            server: None,
            audio_input,
            audio_output,
            message_buffer: Vec::new(),
            phase_watcher,
        };
        state.reload_config();
        Ok(state)
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
                    return now!(Err(Error::Disconnected));
                }

                let id = match self.server().unwrap().channel_name(&channel_identifier) {
                    Ok((id, _)) => id,
                    Err(e) => return now!(Err(Error::ChannelIdentifierError(channel_identifier, e))),
                };

                let mut msg = msgs::UserState::new();
                msg.set_session(self.server.as_ref().unwrap().session_id().unwrap());
                msg.set_channel_id(id);
                packet_sender.send(msg.into()).unwrap();
                now!(Ok(None))
            }
            Command::ChannelList => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected(_)) {
                    return now!(Err(Error::Disconnected));
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
                    return now!(Err(Error::Disconnected));
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
                        self.audio_output.play_effect(if deafen {
                            NotificationEvents::Deafen
                        } else {
                            NotificationEvents::Undeafen
                        });
                    } else if server.muted() != mute {
                        self.audio_output.play_effect(if mute {
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
                self.audio_input.set_volume(volume);
                now!(Ok(None))
            }
            Command::MuteOther(string, toggle) => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected(_)) {
                    return now!(Err(Error::Disconnected));
                }

                let id = self
                    .server_mut()
                    .unwrap()
                    .users_mut()
                    .iter_mut()
                    .find(|(_, user)| user.name() == string);

                let (id, user) = match id {
                    Some(id) => (*id.0, id.1),
                    None => return now!(Err(Error::InvalidUsername(string))),
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
                    self.audio_output.set_mute(id, action);
                }

                return now!(Ok(None));
            }
            Command::MuteSelf(toggle) => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected(_)) {
                    return now!(Err(Error::Disconnected));
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
                        self.audio_output.play_effect(if deafen {
                            NotificationEvents::Deafen
                        } else {
                            NotificationEvents::Undeafen
                        });
                    } else if server.muted() != mute {
                        self.audio_output.play_effect(if mute {
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
                self.audio_output.set_volume(volume);
                now!(Ok(None))
            }
            Command::Ping => {
                now!(Ok(Some(CommandResponse::Pong)))
            }
            Command::ServerConnect {
                host,
                port,
                username,
                password,
                accept_invalid_cert,
            } => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Disconnected) {
                    return now!(Err(Error::AlreadyConnected));
                }
                let mut server = Server::new();
                *server.username_mut() = Some(username);
                *server.password_mut() = password;
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
                        return now!(Err(Error::InvalidServerAddr(host, port)));
                    }
                };
                connection_info_sender
                    .send(Some(ConnectionInfo::new(
                        socket_addr,
                        host,
                        accept_invalid_cert,
                    )))
                    .unwrap();
                at!(TcpEvent::Connected, |res| {
                    //runs the closure when the client is connected
                    if let TcpEventData::Connected(res) = res {
                        res.map(|msg| {
                            Some(CommandResponse::ServerConnect {
                                welcome_message: if msg.has_welcome_text() {
                                    Some(msg.get_welcome_text().to_string())
                                } else {
                                    None
                                },
                            })
                        })
                    } else {
                        unreachable!("callback should be provided with a TcpEventData::Connected");
                    }
                })
            }
            Command::ServerDisconnect => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected(_)) {
                    return now!(Err(Error::Disconnected));
                }

                self.server = None;

                self.phase_watcher
                    .0
                    .send(StatePhase::Disconnected)
                    .unwrap();
                self.audio_output.play_effect(NotificationEvents::ServerDisconnect);
                now!(Ok(None))
            }
            Command::ServerStatus { host, port } => ExecutionContext::Ping(
                Box::new(move || {
                    match (host.as_str(), port)
                        .to_socket_addrs()
                        .map(|mut e| e.next())
                    {
                        Ok(Some(v)) => Ok(v),
                        _ => Err(Error::InvalidServerAddr(host, port)),
                    }
                }),
                Box::new(move |pong| {
                    Ok(pong.map(|pong| CommandResponse::ServerStatus {
                        version: pong.version,
                        users: pong.users,
                        max_users: pong.max_users,
                        bandwidth: pong.bandwidth,
                    }))
                }),
            ),
            Command::Status => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected(_)) {
                    return now!(Err(Error::Disconnected));
                }
                let state = self.server.as_ref().unwrap().into();
                now!(Ok(Some(CommandResponse::Status {
                    server_state: state, //guaranteed not to panic because if we are connected, server is guaranteed to be Some
                })))
            }
            Command::UserVolumeSet(string, volume) => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected(_)) {
                    return now!(Err(Error::Disconnected));
                }
                let user_id = match self
                    .server()
                    .unwrap()
                    .users()
                    .iter()
                    .find(|e| e.1.name() == string)
                    .map(|e| *e.0)
                {
                    None => return now!(Err(Error::InvalidUsername(string))),
                    Some(v) => v,
                };

                self.audio_output.set_user_volume(user_id, volume);
                now!(Ok(None))
            }
            Command::PastMessages { block } => {
                let server = match self.server.as_ref() {
                    Some(s) => s,
                    None => return now!(Err(Error::Disconnected)),
                };
                let messages = std::mem::take(&mut self.message_buffer).into_iter()
                    .map(|(msg, user)| (msg, server.users().get(&user).unwrap().name().to_string())).collect();
                
                now!(Ok(Some(CommandResponse::PastMessages { messages })))
            }
            Command::SendMessage { message, targets } => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected(_)) {
                    return now!(Err(Error::Disconnected));
                }
                
                let mut msg = msgs::TextMessage::new();

                msg.set_message(message);

                for target in targets {
                    match target {
                        MessageTarget::Channel { recursive, name } => {
                            let channel_id = self
                                .server()
                                .unwrap()
                                .channel_name(&name);
                            
                            let channel_id = match channel_id {
                                Ok(id) => id,
                                Err(e) => return now!(Err(Error::ChannelIdentifierError(name, e))),
                            }.0;
                            
                            if recursive {
                                msg.mut_tree_id()
                            } else {
                                msg.mut_channel_id()
                            }.push(channel_id);
                        }
                        MessageTarget::User { name } => {
                            let id = self
                                .server()
                                .unwrap()
                                .users()
                                .iter()
                                .find(|(_, user)| user.name() == &name)
                                .map(|(e, _)| *e);

                            let id = match id {
                                Some(id) => id,
                                None => return now!(Err(Error::InvalidUsername(name))),
                            };

                            msg.mut_session().push(id);
                        }
                    }
                }

                packet_sender.send(msg.into()).unwrap();

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
            // send notification only if we've passed the connecting phase
            if matches!(*self.phase_receiver().borrow(), StatePhase::Connected(_)) {
                let channel_id = msg.get_channel_id();

                if channel_id
                    == self.get_users_channel(self.server().unwrap().session_id().unwrap())
                {
                    if let Some(channel) = self.server().unwrap().channels().get(&channel_id) {
                        notifications::send(format!(
                            "{} connected and joined {}",
                            &msg.get_name(),
                            channel.name()
                        ));
                    }

                    self.audio_output.play_effect(NotificationEvents::UserConnected);
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
                        notifications::send(format!(
                            "{} moved to channel {}",
                            user.name(),
                            channel.name()
                        ));
                    } else {
                        warn!("{} moved to invalid channel {}", user.name(), to_channel);
                    }
                    self.audio_output.play_effect(if from_channel == this_channel {
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
                notifications::send(s);
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
            self.audio_output.play_effect(NotificationEvents::UserDisconnected);
            if let Some(user) = self.server().unwrap().users().get(&msg.get_session()) {
                notifications::send(format!("{} disconnected", &user.name()));
            }
        }

        self.server_mut()
            .unwrap()
            .users_mut()
            .remove(&msg.get_session());
        info!("User {} disconnected", msg.get_session());
    }

    pub fn reload_config(&mut self) {
        match mumlib::config::read_default_cfg() {
            Ok(config) => {
                self.config = config;
            }
            Err(e) => error!("Couldn't read config: {}", e),
        }
        if let Some(input_volume) = self.config.audio.input_volume {
            self.audio_input.set_volume(input_volume);
        }
        if let Some(output_volume) = self.config.audio.output_volume {
            self.audio_output.set_volume(output_volume);
        }
        if let Some(sound_effects) = &self.config.audio.sound_effects {
            self.audio_output.load_sound_effects(sound_effects);
        }
    }
    
    pub fn register_message(&mut self, msg: (String, u32)) {
        self.message_buffer.push(msg);
    }

    pub fn broadcast_phase(&self, phase: StatePhase) {
        self.phase_watcher
            .0
            .send(phase)
            .unwrap();
    }

    pub fn initialized(&self) {
        self.broadcast_phase(StatePhase::Connected(VoiceStreamType::TCP));
        self.audio_output.play_effect(NotificationEvents::ServerConnect);
    }

    pub fn audio_input(&self) -> &AudioInput {
        &self.audio_input
    }
    pub fn audio_output(&self) -> &AudioOutput {
        &self.audio_output
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
    pub fn password(&self) -> Option<&str> {
        self.server.as_ref().map(|e| e.password()).flatten()
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
