use mumble_protocol::control::msgs;
use serde::export::Formatter;
use serde::{Deserialize, Serialize};
use std::fmt::Display;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Server {
    pub channels: Channel,
    pub welcome_text: Option<String>,
    pub username: String,
    pub host: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Channel {
    pub description: Option<String>,
    pub links: Vec<Vec<usize>>, //to represent several walks through the tree to find channels its linked to
    pub max_users: u32,
    pub name: String,
    pub children: Vec<Channel>,
    pub users: Vec<User>,
}

impl Channel {
    pub fn iter(&self) -> Iter<'_> {
        Iter {
            me: Some(&self),
            channel: if self.children.len() > 0 {
                Some(0)
            } else {
                None
            },
            channels: self.children.iter().map(|e| e.iter()).collect(),
        }
    }

    pub fn users_iter(&self) -> UsersIter<'_> {
        UsersIter {
            channels: self.children.iter().map(|e| e.users_iter()).collect(),
            channel: if self.children.len() > 0 {
                Some(0)
            } else {
                None
            },
            user: if self.users.len() > 0 { Some(0) } else { None },
            users: &self.users,
        }
    }
}

pub struct Iter<'a> {
    me: Option<&'a Channel>,
    channel: Option<usize>,
    channels: Vec<Iter<'a>>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Channel;

    fn next(&mut self) -> Option<Self::Item> {
        if self.me.is_some() {
            self.me.take()
        } else if let Some(mut c) = self.channel {
            let mut n = self.channels[c].next();
            while n.is_none() {
                c += 1;
                if c >= self.channels.len() {
                    self.channel = None;
                    return None;
                }
                n = self.channels[c].next();
            }
            self.channel = Some(c);
            n
        } else {
            None
        }
    }
}

pub struct UsersIter<'a> {
    channel: Option<usize>,
    channels: Vec<UsersIter<'a>>,
    user: Option<usize>,
    users: &'a [User],
}

impl<'a> Iterator for UsersIter<'a> {
    type Item = &'a User;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(u) = self.user {
            let ret = Some(&self.users[u]);
            if u + 1 < self.users.len() {
                self.user = Some(u + 1);
            } else {
                self.user = None;
            }
            ret
        } else if let Some(mut c) = self.channel {
            let mut n = self.channels[c].next();
            while n.is_none() {
                c += 1;
                if c >= self.channels.len() {
                    self.channel = None;
                    return None;
                }
                n = self.channels[c].next();
            }
            self.channel = Some(c);
            n
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct User {
    pub comment: Option<String>, //TODO not option, empty string instead
    pub hash: Option<String>,
    pub name: String,
    pub priority_speaker: bool,
    pub recording: bool,

    pub suppress: bool,  // by me
    pub self_mute: bool, // by self
    pub self_deaf: bool, // by self
    pub mute: bool,      // by admin
    pub deaf: bool,      // by admin
}

macro_rules! true_to_str {
    ($condition:expr, $res:expr) => {
        if $condition {
            $res
        } else {
            ""
        }
    };
}

impl Display for User {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {}{}{}{}{}",
            self.name,
            true_to_str!(self.suppress, "s"),
            true_to_str!(self.self_mute, "M"),
            true_to_str!(self.self_deaf, "D"),
            true_to_str!(self.mute, "m"),
            true_to_str!(self.deaf, "d")
        )
    }
}

#[derive(Debug, Default)]
pub struct UserDiff {
    pub comment: Option<String>,
    pub hash: Option<String>,
    pub name: Option<String>,
    pub priority_speaker: Option<bool>,
    pub recording: Option<bool>,

    pub suppress: Option<bool>,  // by me
    pub self_mute: Option<bool>, // by self
    pub self_deaf: Option<bool>, // by self
    pub mute: Option<bool>,      // by admin
    pub deaf: Option<bool>,      // by admin

    pub channel_id: Option<u32>,
}

impl UserDiff {
    pub fn new() -> Self {
        UserDiff::default()
    }
}

impl From<msgs::UserState> for UserDiff {
    fn from(mut msg: msgs::UserState) -> Self {
        let mut ud = UserDiff::new();
        if msg.has_comment() {
            ud.comment = Some(msg.take_comment());
        }
        if msg.has_hash() {
            ud.hash = Some(msg.take_hash());
        }
        if msg.has_name() {
            ud.name = Some(msg.take_name());
        }
        if msg.has_priority_speaker() {
            ud.priority_speaker = Some(msg.get_priority_speaker());
        }
        if msg.has_recording() {
            ud.recording = Some(msg.get_recording());
        }
        if msg.has_suppress() {
            ud.suppress = Some(msg.get_suppress());
        }
        if msg.has_self_mute() {
            ud.self_mute = Some(msg.get_self_mute());
        }
        if msg.has_self_deaf() {
            ud.self_deaf = Some(msg.get_self_deaf());
        }
        if msg.has_mute() {
            ud.mute = Some(msg.get_mute());
        }
        if msg.has_deaf() {
            ud.deaf = Some(msg.get_deaf());
        }
        if msg.has_channel_id() {
            ud.channel_id = Some(msg.get_channel_id());
        }
        ud
    }
}
