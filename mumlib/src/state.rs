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
    pub comment: Option<String>,
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

impl Display for User {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}
