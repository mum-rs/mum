use crate::state::channel::{into_channel, Channel};
use crate::state::user::User;

use log::*;
use mumble_protocol::control::msgs;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::Entry;
use std::collections::HashMap;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Server {
    channels: HashMap<u32, Channel>,
    users: HashMap<u32, User>,
    pub welcome_text: Option<String>,

    username: Option<String>,
    session_id: Option<u32>,
    muted: bool,
    deafened: bool,

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
            muted: false,
            deafened: false,
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

    pub fn host_mut(&mut self) -> &mut Option<String> {
        &mut self.host
    }

    pub fn session_id(&self) -> Option<u32> {
        self.session_id
    }

    pub fn session_id_mut(&mut self) -> &mut Option<u32> {
        &mut self.session_id
    }

    pub fn users(&self) -> &HashMap<u32, User> {
        &self.users
    }

    pub fn users_mut(&mut self) -> &mut HashMap<u32, User> {
        &mut self.users
    }

    pub fn username(&self) -> Option<&str> {
        self.username.as_deref()
    }

    pub fn username_mut(&mut self) -> &mut Option<String> {
        &mut self.username
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    pub fn deafened(&self) -> bool {
        self.deafened
    }

    pub fn set_muted(&mut self, value: bool) {
        self.muted = value;
    }

    pub fn set_deafened(&mut self, value: bool) {
        self.deafened = value;
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
