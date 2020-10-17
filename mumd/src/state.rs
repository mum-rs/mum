use crate::audio::Audio;
use crate::network::ConnectionInfo;

use log::*;
use mumble_protocol::control::msgs;
use mumble_protocol::control::ControlPacket;
use mumble_protocol::voice::Serverbound;
use mumlib::command::{Command, CommandResponse};
use mumlib::error::{ChannelIdentifierError, Error};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::net::ToSocketAddrs;
use tokio::sync::{mpsc, watch};

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
        }
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
                msg.set_session(self.server.as_ref().unwrap().session_id.unwrap());
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
                        channels: into_channel(
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
                server.username = Some(username);
                server.host = Some(format!("{}:{}", host, port));
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
        }
    }

    pub fn parse_initial_user_state(&mut self, msg: msgs::UserState) {
        if !msg.has_session() {
            warn!("Can't parse user state without session");
            return;
        }
        if !msg.has_name() {
            warn!("Missing name in initial user state");
        } else if msg.get_name() == self.server.as_ref().unwrap().username.as_ref().unwrap() {
            match self.server.as_ref().unwrap().session_id {
                None => {
                    debug!("Found our session id: {}", msg.get_session());
                    self.server_mut().unwrap().session_id = Some(msg.get_session());
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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Server {
    channels: HashMap<u32, Channel>,
    users: HashMap<u32, User>,
    pub welcome_text: Option<String>,

    username: Option<String>,
    session_id: Option<u32>,

    host: Option<String>,
}

impl Server {
    pub fn new() -> Self {
        Self {
            channels: HashMap::new(),
            users: HashMap::new(),
            welcome_text: None,
            username: None,
            session_id: None,
            host: None,
        }
    }

    pub fn parse_server_sync(&mut self, mut msg: msgs::ServerSync) {
        if msg.has_welcome_text() {
            self.welcome_text = Some(msg.take_welcome_text());
        }
    }

    pub fn parse_channel_state(&mut self, msg: msgs::ChannelState) {
        if !msg.has_channel_id() {
            warn!("Can't parse channel state without channel id");
            return;
        }
        match self.channels.entry(msg.get_channel_id()) {
            Entry::Vacant(e) => {
                e.insert(Channel::new(msg));
            }
            Entry::Occupied(mut e) => e.get_mut().parse_channel_state(msg),
        }
    }

    pub fn parse_channel_remove(&mut self, msg: msgs::ChannelRemove) {
        if !msg.has_channel_id() {
            warn!("Can't parse channel remove without channel id");
            return;
        }
        match self.channels.entry(msg.get_channel_id()) {
            Entry::Vacant(_) => {
                warn!("Attempted to remove channel that doesn't exist");
            }
            Entry::Occupied(e) => {
                e.remove();
            }
        }
    }

    pub fn parse_user_state(&mut self, msg: msgs::UserState) {
        if !msg.has_session() {
            warn!("Can't parse user state without session");
            return;
        }
        match self.users.entry(msg.get_session()) {
            Entry::Vacant(e) => {
                e.insert(User::new(msg));
            }
            Entry::Occupied(mut e) => e.get_mut().parse_user_state(msg),
        }
    }

    pub fn channels(&self) -> &HashMap<u32, Channel> {
        &self.channels
    }

    pub fn users(&self) -> &HashMap<u32, User> {
        &self.users
    }

    pub fn username(&self) -> Option<&str> {
        self.username.as_ref().map(|e| e.as_str())
    }
}

impl From<&Server> for mumlib::state::Server {
    fn from(server: &Server) -> Self {
        mumlib::state::Server {
            channels: into_channel(server.channels(), server.users()),
            welcome_text: server.welcome_text.clone(),
            username: server.username.clone().unwrap(),
            host: server.host.as_ref().unwrap().clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Channel {
    description: Option<String>,
    links: Vec<u32>,
    max_users: u32,
    name: String,
    parent: Option<u32>,
    position: i32,
}

impl Channel {
    pub fn new(mut msg: msgs::ChannelState) -> Self {
        Self {
            description: if msg.has_description() {
                Some(msg.take_description())
            } else {
                None
            },
            links: Vec::new(),
            max_users: msg.get_max_users(),
            name: msg.take_name(),
            parent: if msg.has_parent() {
                Some(msg.get_parent())
            } else {
                None
            },
            position: msg.get_position(),
        }
    }

    pub fn parse_channel_state(&mut self, mut msg: msgs::ChannelState) {
        if msg.has_description() {
            self.description = Some(msg.take_description());
        }
        self.links = msg.take_links();
        if msg.has_max_users() {
            self.max_users = msg.get_max_users();
        }
        if msg.has_name() {
            self.name = msg.take_name();
        }
        if msg.has_parent() {
            self.parent = Some(msg.get_parent());
        }
        if msg.has_position() {
            self.position = msg.get_position();
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self, channels: &HashMap<u32, Channel>) -> String {
        match &self.parent {
            Some(t) => format!("{}/{}", channels.get(t).unwrap().path(channels), self.name),
            None => self.name.clone(),
        }
    }
}

#[derive(Debug)]
struct ProtoTree<'a> {
    channel: Option<&'a Channel>,
    children: HashMap<u32, ProtoTree<'a>>,
    users: Vec<&'a User>,
}

impl<'a> ProtoTree<'a> {
    fn walk_and_add(
        &mut self,
        channel: &'a Channel,
        users: &HashMap<u32, Vec<&'a User>>,
        walk: &[u32],
    ) {
        match walk {
            [] => unreachable!("Walks should always have at least one element"),
            &[node] => {
                let pt = self.children.entry(node).or_insert(ProtoTree {
                    channel: None,
                    children: HashMap::new(),
                    users: Vec::new(),
                });
                pt.channel = Some(channel);
                pt.users = users.get(&node).map(|e| e.clone()).unwrap_or(Vec::new());
            }
            longer => {
                self.children
                    .entry(longer[0])
                    .or_insert(ProtoTree {
                        channel: None,
                        children: HashMap::new(),
                        users: Vec::new(),
                    })
                    .walk_and_add(channel, users, &walk[1..]);
            }
        }
    }
}

impl<'a> From<&ProtoTree<'a>> for mumlib::state::Channel {
    fn from(tree: &ProtoTree<'a>) -> Self {
        let mut channel = mumlib::state::Channel::from(tree.channel.unwrap());
        let mut children = tree
            .children
            .iter()
            .map(|e| {
                (
                    e.1.channel.unwrap().position,
                    mumlib::state::Channel::from(e.1),
                )
            })
            .collect::<Vec<_>>();
        children.sort_by_key(|e| (e.0, e.1.name.clone()));
        channel.children = children.into_iter().map(|e| e.1).collect();
        channel.users = tree.users.iter().map(|e| (*e).into()).collect();
        channel
    }
}

pub fn into_channel(
    channels: &HashMap<u32, Channel>,
    users: &HashMap<u32, User>,
) -> mumlib::state::Channel {
    let mut walks = Vec::new();

    let mut channel_lookup = HashMap::new();

    for user in users.values() {
        channel_lookup
            .entry(user.channel)
            .or_insert(Vec::new())
            .push(user);
    }

    for (channel_id, channel) in channels {
        let mut walk = Vec::new();
        let mut current = *channel_id;
        while let Some(next) = channels.get(&current).unwrap().parent {
            walk.push(current);
            current = next;
        }
        walk.reverse();

        if walk.len() > 0 {
            walks.push((walk, channel));
        }
    }

    //root node is ignored because of how walk_and_add is implemented on ProtoTree
    let mut proto_tree = ProtoTree {
        channel: Some(channels.get(&0).unwrap()),
        children: HashMap::new(),
        users: channel_lookup
            .get(&0)
            .map(|e| e.clone())
            .unwrap_or(Vec::new()),
    };

    for (walk, channel) in walks {
        proto_tree.walk_and_add(channel, &channel_lookup, &walk);
    }

    (&proto_tree).into()
}

impl From<&Channel> for mumlib::state::Channel {
    fn from(channel: &Channel) -> Self {
        mumlib::state::Channel {
            description: channel.description.clone(),
            links: Vec::new(),
            max_users: channel.max_users,
            name: channel.name.clone(),
            children: Vec::new(),
            users: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct User {
    channel: u32,
    comment: Option<String>,
    hash: Option<String>,
    name: String,
    priority_speaker: bool,
    recording: bool,

    suppress: bool,  // by me
    self_mute: bool, // by self
    self_deaf: bool, // by self
    mute: bool,      // by admin
    deaf: bool,      // by admin
}

impl User {
    pub fn new(mut msg: msgs::UserState) -> Self {
        Self {
            channel: msg.get_channel_id(),
            comment: if msg.has_comment() {
                Some(msg.take_comment())
            } else {
                None
            },
            hash: if msg.has_hash() {
                Some(msg.take_hash())
            } else {
                None
            },
            name: msg.take_name(),
            priority_speaker: msg.has_priority_speaker() && msg.get_priority_speaker(),
            recording: msg.has_recording() && msg.get_recording(),
            suppress: msg.has_suppress() && msg.get_suppress(),
            self_mute: msg.has_self_mute() && msg.get_self_mute(),
            self_deaf: msg.has_self_deaf() && msg.get_self_deaf(),
            mute: msg.has_mute() && msg.get_mute(),
            deaf: msg.has_deaf() && msg.get_deaf(),
        }
    }

    pub fn parse_user_state(&mut self, mut msg: msgs::UserState) {
        if msg.has_channel_id() {
            self.channel = msg.get_channel_id();
        }
        if msg.has_comment() {
            self.comment = Some(msg.take_comment());
        }
        if msg.has_hash() {
            self.hash = Some(msg.take_hash());
        }
        if msg.has_name() {
            self.name = msg.take_name();
        }
        if msg.has_priority_speaker() {
            self.priority_speaker = msg.get_priority_speaker();
        }
        if msg.has_recording() {
            self.recording = msg.get_recording();
        }
        if msg.has_suppress() {
            self.suppress = msg.get_suppress();
        }
        if msg.has_self_mute() {
            self.self_mute = msg.get_self_mute();
        }
        if msg.has_self_deaf() {
            self.self_deaf = msg.get_self_deaf();
        }
        if msg.has_mute() {
            self.mute = msg.get_mute();
        }
        if msg.has_deaf() {
            self.deaf = msg.get_deaf();
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn channel(&self) -> u32 {
        self.channel
    }
}

impl From<&User> for mumlib::state::User {
    fn from(user: &User) -> Self {
        mumlib::state::User {
            comment: user.comment.clone(),
            hash: user.hash.clone(),
            name: user.name.clone(),
            priority_speaker: user.priority_speaker,
            recording: user.recording,
            suppress: user.suppress,
            self_mute: user.self_mute,
            self_deaf: user.self_deaf,
            mute: user.mute,
            deaf: user.deaf,
        }
    }
}
