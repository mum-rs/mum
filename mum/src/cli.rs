use crate::state::State;
use mumlib::command::{ChannelTarget, Command as MumCommand, CommandResponse,  MessageTarget};
use mumlib::config::{self, Config, ServerConfig};
use mumlib::state::Channel as MumChannel;

use colored::Colorize;
use futures_util::FutureExt;
use log::{debug, error, warn};
use std::fmt;
use std::io::BufRead;
use std::iter;
use structopt::StructOpt;
use tokio::select;
use tokio::sync::mpsc;

const INDENTATION: &str = "  ";

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

// TODO duplicated
type CommandSender = mpsc::UnboundedSender<(
    MumCommand,
    mpsc::UnboundedSender<mumlib::error::Result<Option<CommandResponse>>>,
)>;

pub async fn match_args(opt: Mum, command_sender: CommandSender, output: mpsc::UnboundedSender<String>) -> Result<(), Error> {
    let (tx, mut rx) = mpsc::unbounded_channel();

    macro_rules! send_command {
        ($command:expr) => {
            command_sender.send(($command, tx)).unwrap();
        };
    }

    macro_rules! response {
        () => {
            rx.recv().await.unwrap()
        };
    }

    let mut config = config::read_cfg(&config::default_cfg_path())?;

    match opt.command {
        Command::Connect {
            host,
            username,
            password,
            port,
            accept_invalid_cert: cli_accept_invalid_cert,
        } => {
            let port = port.unwrap_or(mumlib::DEFAULT_PORT);

            let (host, username, password, port, server_accept_invalid_cert) =
                match config.servers.iter().find(|e| e.name == host) {
                    Some(server) => (
                        &server.host,
                        server
                            .username
                            .as_ref()
                            .or_else(|| username.as_ref())
                            .ok_or(CliError::NoUsername)?,
                        server.password.as_ref().or_else(|| password.as_ref()),
                        server.port.unwrap_or(port),
                        server.accept_invalid_cert,
                    ),
                    None => (
                        &host,
                        username.as_ref().ok_or(CliError::NoUsername)?,
                        password.as_ref(),
                        port,
                        None,
                    ),
                };

            let config_accept_invalid_cert =
                server_accept_invalid_cert.or(config.allow_invalid_server_cert);
            let specified_accept_invalid_cert =
                cli_accept_invalid_cert || config_accept_invalid_cert.is_some();

            send_command!(MumCommand::ServerConnect {
                host: host.to_string(),
                port,
                username: username.to_string(),
                password: password.map(|x| x.to_string()),
                accept_invalid_cert: cli_accept_invalid_cert
                    || config_accept_invalid_cert.unwrap_or(false),
            });
            match response!() {
                Ok(Some(CommandResponse::ServerConnect {
                    welcome_message,
                    server_state,
                })) => {
                    parse_state(&server_state, output.clone());
                    if let Some(message) = welcome_message {
                        output.send(format!("\nWelcome: {}", message)).unwrap();
                    }
                }
                Err(mumlib::error::Error::ServerCertReject) => {
                    output.send(format!("Connection rejected since the server supplied an invalid certificate.\nhelp: If you trust this server anyway, you can do any of the following to connect:\n  1. Temporarily trust this server by passing --accept-invalid-cert when connecting.\n  2. Permanently trust this server by setting accept_invalid_cert=true in the server's config.  3. Permantently trust all invalid certificates by setting accept_all_invalid_certs=true globally.")).unwrap();
                }
                Ok(other) => unreachable!(
                    "Response should only be a ServerConnect or ServerCertReject. Got {:?}",
                    other
                ),
                Err(e) => return Err(e.into()),
            }
        }
        Command::Disconnect => {
            send_command!(MumCommand::ServerDisconnect);
            rx.recv().await.unwrap()?;
        }
        Command::Server(server_command) => {
            match_server_command(server_command, &mut config, command_sender, output).await?;
        }
        Command::Channel(channel_command) => {
            match channel_command {
                Channel::List { short: _short } => {
                    //TODO actually use this
                    send_command!(MumCommand::ChannelList);
                    match response!()? {
                        Some(CommandResponse::ChannelList { channels }) => {
                            output.send(print_channel(&channels, 0)).unwrap();
                        }
                        _ => unreachable!("Response should only be a ChannelList"),
                    }
                }
                Channel::Connect { name } => {
                    send_command!(MumCommand::ChannelJoin {
                        channel_identifier: name,
                    });
                    response!()?;
                }
            }
        }
        Command::Status => {
            send_command!(MumCommand::Status);
            match response!()? {
                Some(CommandResponse::Status { server_state }) => {
                    parse_state(&server_state, output);
                }
                _ => unreachable!("Response should only be a Status"),
            }
        }
        Command::Config { key, value } => match key.as_str() {
            "audio.input_volume" => {
                if let Ok(volume) = value.parse() {
                    send_command!(MumCommand::InputVolumeSet(volume));
                    response!()?;
                    config.audio.input_volume = Some(volume);
                }
            }
            "audio.output_volume" => {
                if let Ok(volume) = value.parse() {
                    send_command!(MumCommand::OutputVolumeSet(volume));
                    response!()?;
                    config.audio.output_volume = Some(volume);
                }
            }
            "accept_all_invalid_certs" => {
                if let Ok(b) = value.parse() {
                    config.allow_invalid_server_cert = Some(b);
                }
            }
            _ => {
                return Err(CliError::ConfigKeyNotFound(key).into());
            }
        },
        Command::ConfigReload => {
            send_command!(MumCommand::ConfigReload);
            response!()?;
        }
        Command::Completions(_completions) => {
            todo!("remove this from cli.rs and branch repl/cli earlier")
        }
        Command::Volume { user, volume } => {
            if let Some(volume) = volume {
                send_command!(MumCommand::UserVolumeSet(user, volume));
                response!()?;
            } else {
                //TODO report volume of user
                //     needs work on mumd
                output.send("Currently unimplemented".to_string()).unwrap();
            }
        }
        Command::Mute { user } => match user {
            Some(user) => {
                send_command!(MumCommand::MuteOther(user, Some(true)));
                response!()?;
            }
            None => {
                send_command!(MumCommand::MuteSelf(Some(true)));
                response!()?;
            }
        },
        Command::Unmute { user } => match user {
            Some(user) => {
                send_command!(MumCommand::MuteOther(user, Some(false)));
                response!()?;
            }
            None => {
                send_command!(MumCommand::MuteSelf(Some(false)));
                response!()?;
            }
        },
        Command::Deafen => {
            send_command!(MumCommand::DeafenSelf(Some(true)));
            response!()?;
        }
        Command::Undeafen => {
            send_command!(MumCommand::DeafenSelf(Some(false)));
            response!()?;
        }
        Command::Messages { follow } => {
            send_command!(MumCommand::PastMessages { block: follow });
            while let Some(response) = rx.recv().await {
                match response {
                    Ok(Some(CommandResponse::PastMessage { message })) => {
                        output.send(format!(
                            "[{}] {}: {}",
                            message.0.format("%d %b %H:%M"),
                            message.2,
                            message.1
                        )).unwrap()
                    }
                    Ok(_) => unreachable!("Response should only be a Some(PastMessages)"),
                    Err(e) => output.send(format!("error: {}", e)).unwrap(),
                }
            }
        }
        Command::Message(target) => match target {
            Target::Channel {
                message,
                recursive,
                names,
            } => {
                let msg = MumCommand::SendMessage {
                    message,
                    targets: if names.is_empty() {
                        MessageTarget::Channel(vec![(ChannelTarget::Current, recursive)])
                    } else {
                        MessageTarget::Channel(
                            names
                                .into_iter()
                                .map(|name| (ChannelTarget::Named(name), recursive))
                                .collect(),
                        )
                    },
                };
                send_command!(msg);
                response!()?;
            }
            Target::User { message, names } => {
                let msg = MumCommand::SendMessage {
                    message,
                    targets: MessageTarget::User(names),
                };
                send_command!(msg);
                response!()?;
            }
        },
        Command::Events { follow } => {
            send_command!(MumCommand::Events { block: follow });
            while let Some(response) = rx.recv().await {
                match response {
                    Ok(Some(CommandResponse::Event { event })) => {
                        output.send(format!("{}", event)).unwrap()
                    }
                    Ok(_) => unreachable!("Response should only be a Some(Event)"),
                    Err(e) => output.send(format!("error: {}", e)).unwrap(),
                }
            }
        }
    }

    let config_path = config::default_cfg_path();
    if config_path.exists() {
        // TODO always overwrite?
        config.write(&config_path, false)?;
    }
    Ok(())
}

async fn match_server_command(
    server_command: Server,
    config: &mut Config,
    command_sender: CommandSender,
    output: mpsc::UnboundedSender<String>,
) -> Result<(), Error> {
    match server_command {
        Server::Config {
            server_name,
            key,
            value,
        } => {
            let server_name = match server_name {
                Some(server_name) => server_name,
                None => {
                    for server in config.servers.iter() {
                        output.send(format!("{}", server.name)).unwrap();
                    }
                    return Ok(());
                }
            };
            let server = config
                .servers
                .iter_mut()
                .find(|s| s.name == server_name)
                .ok_or(CliError::NoServerFound(server_name))?;

            match (key.as_deref(), value) {
                (None, _) => {
                    output.send(format!(
                        "{}{}{}{}{}",
                        format!("host: {}", server.host.to_string()),
                        server
                            .port
                            .map(|s| format!("port: {}\n", s))
                            .unwrap_or_else(|| "".to_string()),
                        server
                            .username
                            .as_ref()
                            .map(|s| format!("username: {}\n", s))
                            .unwrap_or_else(|| "".to_string()),
                        server
                            .password
                            .as_ref()
                            .map(|s| format!("password: {}\n", s))
                            .unwrap_or_else(|| "".to_string()),
                        server
                            .accept_invalid_cert
                            .map(|b| format!(
                                "accept_invalid_cert: {}\n",
                                if b { "true" } else { "false" }
                            ))
                            .unwrap_or_else(|| "".to_string()),
                    )).unwrap();
                }
                (Some("name"), None) => {
                    output.send(format!("{}", server.name)).unwrap();
                }
                (Some("host"), None) => {
                    output.send(format!("{}", server.host)).unwrap();
                }
                (Some("port"), None) => {
                    output.send(format!(
                        "{}",
                        server
                            .port
                            .ok_or_else(|| CliError::NotSet("port".to_string()))?
                    )).unwrap();
                }
                (Some("username"), None) => {
                    output.send(format!(
                        "{}",
                        server
                            .username
                            .as_ref()
                            .ok_or_else(|| CliError::NotSet("username".to_string()))?
                    )).unwrap();
                }
                (Some("password"), None) => {
                    output.send(format!(
                        "{}",
                        server
                            .password
                            .as_ref()
                            .ok_or_else(|| CliError::NotSet("password".to_string()))?
                    )).unwrap();
                }
                (Some("accept_invalid_cert"), None) => {
                    output.send(format!(
                        "{}",
                        server
                            .accept_invalid_cert
                            .map(|b| b.to_string())
                            .ok_or_else(|| CliError::NotSet("accept_invalid_cert".to_string()))?
                    )).unwrap();
                }
                (Some("name"), Some(_)) => {
                    return Err(CliError::UseServerRename.into());
                }
                (Some("host"), Some(value)) => {
                    server.host = value;
                }
                (Some("port"), Some(value)) => {
                    server.port = Some(value.parse().unwrap());
                }
                (Some("username"), Some(value)) => {
                    server.username = Some(value);
                }
                (Some("password"), Some(value)) => {
                    server.password = Some(value);
                }
                (Some("accept_invalid_cert"), Some(value)) => match value.parse() {
                    Ok(b) => server.accept_invalid_cert = Some(b),
                    Err(e) => output.send(format!("error: {}", e)).unwrap(),
                },
                (Some(_), _) => {
                    return Err(CliError::ConfigKeyNotFound(key.unwrap()).into());
                }
            }
        }
        Server::Rename { old_name, new_name } => {
            config
                .servers
                .iter_mut()
                .find(|s| s.name == old_name)
                .ok_or(CliError::NoServerFound(old_name))?
                .name = new_name;
        }
        Server::Add {
            name,
            host,
            port,
            username,
            password,
        } => {
            if config.servers.iter().any(|s| s.name == name) {
                return Err(CliError::ServerAlreadyExists(name).into());
            } else {
                config.servers.push(ServerConfig {
                    name,
                    host,
                    port,
                    username,
                    password,
                    accept_invalid_cert: None,
                });
            }
        }
        Server::Remove { name } => {
            let idx = config
                .servers
                .iter()
                .position(|s| s.name == name)
                .ok_or(CliError::NoServerFound(name))?;
            config.servers.remove(idx);
        }
        Server::List => {
            if config.servers.is_empty() {
                return Err(CliError::NoServers.into());
            }

            let longest = config
                .servers
                .iter()
                .map(|s| s.name.len())
                .max()
                .unwrap()  // ok since !config.servers.is_empty() above
                + 1;

            let queries = config.servers.iter().map(|s| MumCommand::ServerStatus {
                host: s.host.clone(),
                port: s.port.unwrap_or(mumlib::DEFAULT_PORT),
            });

            let mut responses = vec![];
            for query in queries {
                let (tx, mut rx) = mpsc::unbounded_channel();
                command_sender.send((query, tx)).unwrap();
                responses.push(rx.recv().await.unwrap()?);
            }

            for (server, response) in config.servers.iter().zip(responses) {
                if let CommandResponse::ServerStatus {
                    users, max_users, ..
                } = response.unwrap()
                {
                    output.send(format!(
                        "{0:<1$} [{2:}/{3:}]",
                        server.name, longest, users, max_users
                    )).unwrap();
                } else {
                    unreachable!();
                }
            }
        }
    }
    Ok(())
}

fn parse_state(server_state: &mumlib::state::Server, output: mpsc::UnboundedSender<String>) {
    output.send(format!(
        "Connected to {} as {}",
        server_state.host, server_state.username
    )).unwrap();
    let own_channel = server_state
        .channels
        .iter()
        .find(|e| e.users.iter().any(|e| e.name == server_state.username))
        .unwrap();
    output.send(format!(
        "Currently in {} with {} other client{}:",
        own_channel.name,
        own_channel.users.len() - 1,
        if own_channel.users.len() == 2 {
            ""
        } else {
            "s"
        }
    )).unwrap();
    output.send(format!("{}{}", INDENTATION, own_channel.name)).unwrap();
    for user in &own_channel.users {
        output.send(format!("{}{}{}", INDENTATION, INDENTATION, user)).unwrap();
    }
}

fn print_channel(channel: &MumChannel, depth: usize) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "{}{}{}",
        iter::repeat(INDENTATION).take(depth).collect::<String>(),
        channel.name.bold(),
        if channel.max_users != 0 {
            format!(" {}/{}", channel.users.len(), channel.max_users)
        } else {
            "".to_string()
        }
    ));
    for user in &channel.users {
        s.push_str(&format!(
            "{}- {}",
            iter::repeat(INDENTATION)
                .take(depth + 1)
                .collect::<String>(),
            user
        ));
    }
    for child in &channel.children {
        s.push_str(&print_channel(child, depth + 1));
    }
    return s;
}
