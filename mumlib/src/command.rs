//! [Command]s can be sent from a controller to mumd which might respond with a
//! [CommandResponse].

use crate::state::{Channel, Server};

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Something that happened in our channel at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MumbleEvent {
    /// When the event occured.
    pub timestamp: NaiveDateTime,
    /// What occured.
    pub kind: MumbleEventKind
}

impl fmt::Display for MumbleEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.timestamp.format("%d %b %H:%M"), self.kind)
    }
}

/// The different kinds of events that can happen.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MumbleEventKind {
    /// A user connected to the server and joined our channel. Contains `(user, channel)`.
    UserConnected(String, String),
    /// A user disconnected from the server while in our channel. Contains `(user, channel)`.
    UserDisconnected(String, String),
    /// A user {un,}{muted,deafened}. Contains a rendered message with what changed and the user.
    UserMuteStateChanged(String),  // This logic is kinda weird so we only store the rendered message.
    /// A text message was received. Contains who sent the message.
    TextMessageReceived(String),
    /// A user switched to our channel from some other channel. Contains `(user, previous-channel)`.
    UserJoinedChannel(String, String),
    /// A user switched from our channel to some other channel. Contains `(user, new-channel)`.
    UserLeftChannel(String, String),
}

//TODO These strings are (mostly) duplicated with their respective notifications.
//     The only difference is that the text message event doesn't contain the text message.
impl fmt::Display for MumbleEventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MumbleEventKind::UserConnected(user, channel) => {
                write!(f, "{} connected to {}", user, channel)
            }
            MumbleEventKind::UserDisconnected(user, channel) => {
                write!(f, "{} disconnected from {}", user, channel)
            }
            MumbleEventKind::UserMuteStateChanged(message) => {
                write!(f, "{}", message)
            }
            MumbleEventKind::TextMessageReceived(user) => {
                write!(f, "{} sent a text message", user)
            }
            MumbleEventKind::UserJoinedChannel(name, from) => {
                write!(f, "{} moved to your channel from {}", name, from)
            }
            MumbleEventKind::UserLeftChannel(name, to) => {
                write!(f, "{} moved to {}", name, to)
            }

        }
    }
}

/// Sent by a controller to mumd who might respond with a [CommandResponse]. Not
/// all commands receive a response.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Command {
    /// No response.
    ChannelJoin {
        channel_identifier: String,
    },

    /// Response: [CommandResponse::ChannelList].
    ChannelList,

    /// Force reloading of config file from disk. No response.
    ConfigReload,
    /// Response: [CommandResponse::DeafenStatus]. Toggles if None.
    DeafenSelf(Option<bool>),
    Events {
        block: bool
    },
    /// Set the outgoing audio volume (i.e. from you to the server). No response.
    InputVolumeSet(f32),

    /// Response: [CommandResponse::MuteStatus]. Toggles if None.
    MuteOther(String, Option<bool>),

    /// Response: [CommandResponse::MuteStatus]. Toggles if None.
    MuteSelf(Option<bool>),

    /// Set the master incoming audio volume (i.e. from the server to you).
    /// No response.
    OutputVolumeSet(f32),

    PastMessages {
        block: bool,
    },

    /// Response: [CommandResponse::Pong]. Used to test existance of a
    /// mumd-instance.
    Ping,

    SendMessage {
        message: String,
        targets: MessageTarget,
    },
    /// Connect to the specified server. Response: [CommandResponse::ServerConnect].
    ServerConnect {
        host: String,
        port: u16,
        username: String,
        password: Option<String>,
        accept_invalid_cert: bool,
    },

    /// No response.
    ServerDisconnect,

    /// Send a server status request via UDP (e.g. not requiring a TCP connection).
    /// Response: [CommandResponse::ServerStatus].
    ServerStatus {
        host: String,
        port: u16,
    },

    /// Response: [CommandResponse::Status].
    Status,

    /// No response.
    UserVolumeSet(String, f32),
}

/// A response to a sent [Command].
#[derive(Debug, Deserialize, Serialize)]
pub enum CommandResponse {
    ChannelList {
        channels: Channel,
    },

    DeafenStatus {
        is_deafened: bool,
    },

    Event {
        event: MumbleEvent,
    },

    MuteStatus {
        is_muted: bool,
    },

    PastMessage {
        message: (NaiveDateTime, String, String),
    },

    Pong,

    ServerConnect {
        welcome_message: Option<String>,
        server_state: Server,
    },

    ServerStatus {
        version: u32,
        users: u32,
        max_users: u32,
        bandwidth: u32,
    },

    Status {
        server_state: Server,
    },
}

/// Messages sent to channels can be sent either to a named channel or the
/// currently connected channel.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum ChannelTarget {
    Current,
    Named(String)
}

/// Messages can be sent to either channels or specific users.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum MessageTarget {
    Channel(Vec<(ChannelTarget, bool)>),  // (target, recursive)
    User(Vec<String>),
}
