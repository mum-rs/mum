use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Server {
    channels: Vec<Channel>,
    welcome_text: Option<String>,
}

impl Server {
    pub fn new() -> Self {
        Self {
            channels: Vec::new(),
            welcome_text: None,
        }
    }
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