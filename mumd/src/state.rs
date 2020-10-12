use log::*;
use mumble_protocol::control::msgs;
use std::collections::HashMap;
use std::collections::hash_map::Entry;

pub struct Server {
    channels: HashMap<u32, Channel>,
    users: HashMap<u32, User>,
    pub welcome_text: Option<String>,
}

impl Server {
    pub fn new() -> Self {
        Self {
            channels: HashMap::new(),
            users: HashMap::new(),
            welcome_text: None,
        }
    }

    pub fn parse_server_sync(&mut self, mut msg: Box<msgs::ServerSync>) {
        if msg.has_welcome_text() {
            self.welcome_text = Some(msg.take_welcome_text());
        }
    }

    pub fn parse_channel_state(&mut self, msg: Box<msgs::ChannelState>) {
        if !msg.has_channel_id() {
            warn!("Can't parse channel state without channel id");
            return;
        }
        match self.channels.entry(msg.get_channel_id()) {
            Entry::Vacant(e) => { e.insert(Channel::new(msg)); },
            Entry::Occupied(mut e) => e.get_mut().parse_channel_state(msg),
        }
    }

    pub fn parse_channel_remove(&mut self, msg: Box<msgs::ChannelRemove>) {
        if !msg.has_channel_id() {
            warn!("Can't parse channel remove without channel id");
            return;
        }
        match self.channels.entry(msg.get_channel_id()) {
            Entry::Vacant(_) => { warn!("Attempted to remove channel that doesn't exist"); }
            Entry::Occupied(e) => { e.remove(); }
        }
    }

    pub fn parse_user_state(&mut self, msg: Box<msgs::UserState>) {
        if !msg.has_session() {
            warn!("Can't parse user state without session");
            return;
        }
        match self.users.entry(msg.get_session()) {
            Entry::Vacant(e) => { e.insert(User::new(msg)); },
            Entry::Occupied(mut e) => e.get_mut().parse_user_state(msg),
        }
    }

    pub fn channels(&self) -> &HashMap<u32, Channel> {
        &self.channels
    }

    pub fn users(&self) -> &HashMap<u32, User> {
        &self.users
    }
}


pub struct Channel {
    description: Option<String>,
    links: Vec<u32>,
    max_users: u32,
    name: String,
    parent: Option<u32>,
    position: i32,
}

impl Channel {
    pub fn new(mut msg: Box<msgs::ChannelState>) -> Self {
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

    pub fn parse_channel_state(&mut self, mut msg: Box<msgs::ChannelState>) {
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
}

pub struct User {
    channel: u32,
    comment: Option<String>,
    hash: Option<String>,
    name: String,
    priority_speaker: bool,
    recording: bool,

    suppress: bool, // by me
    self_mute: bool, // by self
    self_deaf: bool, // by self
    mute: bool, // by admin
    deaf: bool, // by admin
}

impl User {
    pub fn new(mut msg: Box<msgs::UserState>) -> Self {
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
            priority_speaker: msg.has_priority_speaker()
                              && msg.get_priority_speaker(),
            recording: msg.has_recording()
                       && msg.get_recording(),
            suppress: msg.has_suppress()
                      && msg.get_suppress(),
            self_mute: msg.has_self_mute()
                       && msg.get_self_mute(),
            self_deaf: msg.has_self_deaf()
                       && msg.get_self_deaf(),
            mute: msg.has_mute()
                  && msg.get_mute(),
            deaf: msg.has_deaf()
                  && msg.get_deaf(),
        }
    }

    pub fn parse_user_state(&mut self, mut msg: Box<msgs::UserState>) {
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
