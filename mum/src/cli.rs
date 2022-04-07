#[derive(Debug, StructOpt)]
pub struct Mum {
    #[structopt(subcommand)]
    pub command: Command,
}

#[derive(Debug, StructOpt)]
pub enum Command {
    /// Connect to a server
    Connect {
        host: String,
        username: Option<String>,
        password: Option<String>,
        #[structopt(short = "p", long = "port")]
        port: Option<u16>,
        #[structopt(long = "accept-invalid-cert")]
        accept_invalid_cert: bool,
    },
    /// Disconnect from the currently connected server
    Disconnect,
    /// Handle servers
    Server(Server),
    /// Handle channels in the connected server
    Channel(Channel),
    /// Show current status
    Status,
    /// Change config values
    Config { key: String, value: String },
    /// Reload the config file
    ConfigReload,
    /// Output CLI completions
    Completions(Completions),
    /// Change volume of either you or someone else
    Volume { user: String, volume: Option<f32> },
    /// Mute someone/yourself
    Mute { user: Option<String> },
    /// Unmute someone/yourself
    Unmute { user: Option<String> },
    /// Deafen yourself
    Deafen,
    /// Undeafen yourself
    Undeafen,
    /// Get messages sent to the server you're currently connected to
    Messages {
        #[structopt(short = "f", long = "follow")]
        follow: bool,
    },
    /// Send a message to a channel or a user
    Message(Target),
    /// Get events that have happened since we connected
    Events {
        #[structopt(short = "f", long = "follow")]
        follow: bool,
    },
}

#[derive(Debug, StructOpt)]
pub enum Target {
    Channel {
        /// The message to send
        message: String,
        /// If the message should be sent recursively to sub-channels
        #[structopt(short = "r", long = "recursive")]
        recursive: bool,
        /// Which channels to send to. Defaults to current channel if left empty
        names: Vec<String>,
    },
    User {
        /// The message to send
        message: String,
        /// Which channels to send to
        names: Vec<String>,
    },
}

#[derive(Debug, StructOpt)]
pub enum Server {
    /// Configure a saved server
    Config {
        server_name: Option<String>,
        key: Option<String>,
        value: Option<String>,
    },
    /// Rename a saved server
    Rename { old_name: String, new_name: String },
    /// Add a new saved server
    Add {
        name: String,
        host: String,
        #[structopt(short = "p", long = "port")]
        port: Option<u16>,
        username: Option<String>,
        #[structopt(requires = "username")]
        password: Option<String>,
    },
    /// Remove a saved server
    Remove { name: String },
    /// List saved servers and number of people connected
    List,
}

#[derive(Debug, StructOpt)]
pub enum Channel {
    List {
        #[structopt(short = "s", long = "short")]
        short: bool,
    },
    Connect {
        name: String,
    },
}

#[derive(Debug, StructOpt)]
pub enum Completions {
    Zsh,
    Bash,
    Fish,
}

#[derive(Debug)]
pub enum CliError {
    NoUsername,
    ConnectionError,
    NoServerFound(String),
    NotSet(String),
    UseServerRename,
    ConfigKeyNotFound(String),
    ServerAlreadyExists(String),
    NoServers,
}

#[derive(Debug)]
pub struct Error(Box<dyn std::error::Error>); // new type since otherwise we'd get an impl conflict with impl<T> From<T> for T below

use std::fmt;

use structopt::StructOpt;

impl<E> From<E> for Error
where
    E: std::error::Error + fmt::Display + 'static,
{
    fn from(e: E) -> Self {
        Error(Box::new(e))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl std::error::Error for CliError {}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::NoUsername => {
                write!(f, "No username specified")
            }
            CliError::ConnectionError => {
                write!(f, "Unable to connect to mumd. Is mumd running?")
            }
            CliError::NoServerFound(s) => {
                write!(f, "Server '{}' not found", s)
            }
            CliError::NotSet(s) => {
                write!(f, "Key '{}' not set", s)
            }
            CliError::UseServerRename => {
                write!(f, "Use 'server rename' instead")
            }
            CliError::ConfigKeyNotFound(s) => {
                write!(f, "Key '{}' not found", s)
            }
            CliError::ServerAlreadyExists(s) => {
                write!(f, "A server named '{}' already exists", s)
            }
            CliError::NoServers => {
                write!(f, "No servers found")
            }
        }
    }
}
