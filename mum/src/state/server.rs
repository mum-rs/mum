use crate::state::channel::{into_channel, Channel};
use crate::state::user::User;

use log::*;
use mumble_protocol_2x::control::msgs;
use mumlib::error::ChannelIdentifierError;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::Entry;
use std::collections::HashMap;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) enum Server {
    Disconnected,
    Connecting(ConnectingServer),
    Connected(ConnectedServer),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ConnectingServer {
    channels: HashMap<u32, Channel>,
    users: HashMap<u32, User>,
    welcome_text: Option<String>,

    username: String,
    password: Option<String>,

    host: String,
}

impl ConnectingServer {
    pub(crate) fn new(host: String, username: String, password: Option<String>) -> Self {
        Self {
            channels: HashMap::new(),
            users: HashMap::new(),
            welcome_text: None,
            username,
            password,
            host,
        }
    }

    pub(crate) fn channel_state(&mut self, msg: msgs::ChannelState) {
        channel_state(msg, &mut self.channels);
    }

    pub(crate) fn channel_remove(&mut self, msg: msgs::ChannelRemove) {
        channel_remove(msg, &mut self.channels);
    }

    pub(crate) fn user_state(&mut self, msg: msgs::UserState) {
        user_state(msg, &mut self.users);
    }

    pub(crate) fn user_remove(&mut self, msg: msgs::UserRemove) {
        self.users.remove(&msg.get_session());
    }

    pub(crate) fn server_sync(self, mut msg: msgs::ServerSync) -> ConnectedServer {
        ConnectedServer {
            channels: self.channels,
            users: self.users,
            welcome_text: msg.has_welcome_text().then(|| msg.take_welcome_text()),
            username: self.username,
            session_id: msg.get_session(),
            muted: false,
            deafened: false,
            host: self.host,
        }
    }

    pub(crate) fn username(&self) -> &str {
        &self.username
    }

    pub(crate) fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ConnectedServer {
    channels: HashMap<u32, Channel>,
    users: HashMap<u32, User>,
    welcome_text: Option<String>,

    username: String,
    session_id: u32,
    muted: bool,
    deafened: bool,

    host: String,
}

impl ConnectedServer {
    pub(crate) fn channel_remove(&mut self, msg: msgs::ChannelRemove) {
        channel_remove(msg, &mut self.channels);
    }

    pub(crate) fn user_state(&mut self, msg: msgs::UserState) {
        user_state(msg, &mut self.users);
    }

    pub(crate) fn user_remove(&mut self, msg: msgs::UserRemove) {
        self.users.remove(&msg.get_session());
    }

    pub(crate) fn channels(&self) -> &HashMap<u32, Channel> {
        &self.channels
    }

    /// Takes a channel name and returns either a tuple with the channel id and a reference to the
    /// channel struct if the channel name unambiguosly refers to a channel, or an error describing
    /// if the channel identifier was ambigous or invalid.
    /// ```
    /// use crate::state::channel::Channel;
    /// let mut server = Server::new();
    /// let channel = Channel {
    ///     name: "Foobar".to_owned(),
    ///     ..Default::default(),
    /// };
    /// server.channels.insert(0, channel.clone);
    /// assert_eq!(server.channel_name("Foobar"), Ok((0, &channel)));
    /// ```
    pub(crate) fn channel_name(
        &self,
        channel_name: &str,
    ) -> Result<(u32, &Channel), ChannelIdentifierError> {
        let matches = self
            .channels
            .iter()
            .map(|e| ((*e.0, e.1), e.1.path(&self.channels)))
            .filter(|e| e.1.ends_with(channel_name))
            .collect::<Vec<_>>();
        Ok(match matches.len() {
            0 => {
                let soft_matches = self
                    .channels
                    .iter()
                    .map(|e| ((*e.0, e.1), e.1.path(&self.channels).to_lowercase()))
                    .filter(|e| e.1.ends_with(&channel_name.to_lowercase()))
                    .collect::<Vec<_>>();
                match soft_matches.len() {
                    0 => return Err(ChannelIdentifierError::Invalid),
                    1 => soft_matches.get(0).unwrap().0,
                    _ => return Err(ChannelIdentifierError::Ambiguous),
                }
            }
            1 => matches.get(0).unwrap().0,
            _ => return Err(ChannelIdentifierError::Ambiguous),
        })
    }

    /// Returns the channel that our client is connected to.
    pub(crate) fn current_channel(&self) -> (u32, &Channel) {
        let channel_id = self.users().get(&self.session_id()).unwrap().channel();
        let channel = self.channels().get(&channel_id).unwrap();
        (channel_id, channel)
    }

    pub(crate) fn session_id(&self) -> u32 {
        self.session_id
    }

    pub(crate) fn users(&self) -> &HashMap<u32, User> {
        &self.users
    }

    pub(crate) fn users_mut(&mut self) -> &mut HashMap<u32, User> {
        &mut self.users
    }

    pub(crate) fn username(&self) -> &str {
        &self.username
    }

    pub(crate) fn muted(&self) -> bool {
        self.muted
    }

    pub(crate) fn deafened(&self) -> bool {
        self.deafened
    }

    pub(crate) fn set_muted(&mut self, value: bool) {
        self.muted = value;
    }

    pub(crate) fn set_deafened(&mut self, value: bool) {
        self.deafened = value;
    }

    pub(crate) fn users_channel(&self, user: u32) -> u32 {
        self.users().get(&user).unwrap().channel()
    }
}

fn user_state(msg: msgs::UserState, users: &mut HashMap<u32, User>) {
    if !msg.has_session() {
        warn!("Can't parse user state without session");
        return;
    }
    match users.entry(msg.get_session()) {
        Entry::Vacant(e) => {
            e.insert(User::new(msg));
        }
        Entry::Occupied(mut e) => e.get_mut().parse_user_state(msg),
    }
}

fn channel_state(msg: msgs::ChannelState, channels: &mut HashMap<u32, Channel>) {
    if !msg.has_channel_id() {
        warn!("Can't parse channel state without channel id");
        return;
    }
    match channels.entry(msg.get_channel_id()) {
        Entry::Vacant(e) => {
            e.insert(Channel::new(msg));
        }
        Entry::Occupied(mut e) => e.get_mut().parse_channel_state(msg),
    }
}

fn channel_remove(msg: msgs::ChannelRemove, channels: &mut HashMap<u32, Channel>) {
    if !msg.has_channel_id() {
        warn!("Can't parse channel remove without channel id");
        return;
    }
    match channels.entry(msg.get_channel_id()) {
        Entry::Vacant(_) => {
            warn!("Attempted to remove channel that doesn't exist");
        }
        Entry::Occupied(e) => {
            e.remove();
        }
    }
}

impl From<&ConnectedServer> for mumlib::state::Server {
    fn from(server: &ConnectedServer) -> Self {
        mumlib::state::Server {
            channels: into_channel(server.channels(), server.users()),
            welcome_text: server.welcome_text.clone(),
            username: server.username.clone(),
            host: server.host.clone(),
        }
    }
}
