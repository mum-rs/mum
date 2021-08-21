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
    pub kind: MumbleEventKind,
}

impl fmt::Display for MumbleEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] {}",
            self.timestamp.format("%d %b %H:%M"),
            self.kind
        )
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
    UserMuteStateChanged(String), // This logic is kinda weird so we only store the rendered message.
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
        block: bool,
    },
    /// Set the outgoing audio volume (i.e. from you to the server). No response.
    InputVolumeSet(f32),

    /// Response: [CommandResponse::MuteStatus]. Toggles mute state if None.
    MuteOther(String, Option<bool>),

    /// Response: [CommandResponse::MuteStatus]. Toggles mute state if None.
    MuteSelf(Option<bool>),

    /// Set the master incoming audio volume (i.e. from the server to you).
    /// No response.
    OutputVolumeSet(f32),

    /// Request a list of past messages. Blocks while waiting for more messages
    /// if block is true. Response: multiple [CommandResponse::PastMessage].
    PastMessages {
        block: bool,
    },

    /// Response: [CommandResponse::Pong]. Used to test existance of a
    /// mumd-instance.
    Ping,

    /// Send a message to some [MessageTarget].
    SendMessage {
        /// The message to send.
        message: String,
        /// The target(s) to send the message to.
        targets: MessageTarget,
    },

    /// Connect to the specified server. Response: [CommandResponse::ServerConnect].
    ServerConnect {
        /// The URL or IP-adress to connect to.
        host: String,
        /// The port to connect to.
        port: u16,
        /// The username to connect with.
        username: String,
        /// The server password, if applicable. Not sent if None.
        password: Option<String>,
        /// Whether to accept an invalid server certificate or not.
        accept_invalid_cert: bool,
    },

    /// Disconnect from the currently connected server. No response.
    ServerDisconnect,

    /// Send a server status request via UDP (e.g. not requiring a TCP connection).
    /// Response: [CommandResponse::ServerStatus].
    ServerStatus {
        host: String,
        port: u16,
    },

    /// Request the status of the current server. Response: [CommandResponse::Status].
    Status,

    /// The the volume of the specified user. No response.
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
    Named(String),
}

/// Messages can be sent to either channels or specific users.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum MessageTarget {
    Channel(Vec<(ChannelTarget, bool)>), // (target, recursive)
    User(Vec<String>),
}
