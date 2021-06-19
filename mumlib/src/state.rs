use serde::{Deserialize, Serialize};
use std::fmt;

/// The state of the currently connected Mumble server.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Server {
    /// State of the currently connected channel.
    pub channels: Channel,
    /// The welcome text we received when we connected.
    pub welcome_text: Option<String>,
    /// Our username.
    pub username: String,
    /// The host (ip:port) of the server.
    pub host: String,
}

/// A representation of a channel in a Mumble server.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Channel {
    /// The description of the channel, if set.
    pub description: Option<String>,
    /// The maximum number of allowed users in this channel.
    pub max_users: u32,
    /// The name of this channel.
    pub name: String,
    /// Any children this channel has.
    pub children: Vec<Channel>,
    /// This channel's connected users.
    pub users: Vec<User>,

    links: Vec<Vec<usize>>, //to represent several walks through the tree to find channels its linked to
}

impl Channel {
    /// Create a new Channel representation.
    pub fn new(
        name: String,
        description: Option<String>,
        max_users: u32,
    ) -> Self {
        Self {
            description,
            max_users,
            name,
            children: Vec::new(),
            users: Vec::new(),

            links: Vec::new(),
        }
    }

    /// Create an iterator over this channel and its children in a [pre-order
    /// traversal](https://en.wikipedia.org/wiki/Tree_traversal#Pre-order,_NLR)
    /// which ensures that parent channels are returned before any of its
    /// children.
    pub fn iter(&self) -> Iter<'_> {
        Iter {
            me: Some(&self),
            channel: if self.children.is_empty() {
                None
            } else {
                Some(0)
            },
            channels: self.children.iter().map(|e| e.iter()).collect(),
        }
    }

    /// Create an iterator over this channel and its childrens connected users
    /// in a pre-order traversal.
    pub fn users_iter(&self) -> UsersIter<'_> {
        UsersIter {
            channels: self.children.iter().map(|e| e.users_iter()).collect(),
            channel: if self.children.is_empty() {
                None
            } else {
                Some(0)
            },
            user: if self.users.is_empty() { None } else { Some(0) },
            users: &self.users,
        }
    }
}

/// An iterator over channels. Created by [Channel::iter].
#[derive(Debug)]
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

/// An iterator over users. Created by [Channel::users_iter].
#[derive(Debug)]
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

impl fmt::Display for User {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
