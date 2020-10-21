use mumble_protocol::control::msgs;
use serde::{Deserialize, Serialize};

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
