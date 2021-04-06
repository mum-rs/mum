use colored::Colorize;
use log::*;
use mumlib::command::{Command as MumCommand, CommandResponse};
use mumlib::config::{self, Config, ServerConfig};
use mumlib::state::Channel as MumChannel;
use std::{fmt,io::{self, BufRead, Read, Write}, iter, os::unix::net::UnixStream};
use structopt::{clap::Shell, StructOpt};

const INDENTATION: &str = "  ";

struct SimpleLogger;

impl log::Log for SimpleLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            println!(
                "{}{}",
                match record.level() {
                    Level::Error => "error: ".red(),
                    Level::Warn => "warning: ".yellow(),
                    _ => "".normal(),
                },
                record.args()
            );
        }
    }

    fn flush(&self) {}
}

static LOGGER: SimpleLogger = SimpleLogger;

#[derive(Debug, StructOpt)]
struct Mum {
    #[structopt(subcommand)]
    command: Command,
}

#[derive(Debug, StructOpt)]
enum Command {
    /// Connect to a server
    Connect {
        host: String,
        username: Option<String>,
        password: Option<String>,
        #[structopt(short = "p", long = "port")]
        port: Option<u16>,
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
    Config {
        key: String,
        value: String,
    },
    /// Reload the config file
    ConfigReload,
    /// Output CLI completions
    Completions(Completions),
    /// Change volume of either you or someone else
    Volume {
        user: String,
        volume: Option<f32>,
    },
    /// Mute someone/yourself
    Mute {
        user: Option<String>,
    },
    /// Unmute someone/yourself
    Unmute {
        user: Option<String>,
    },
    /// Deafen yourself
    Deafen,
    /// Undeafen yourself
    Undeafen,
}

#[derive(Debug, StructOpt)]
enum Server {
    /// Configure a saved server
    Config {
        server_name: Option<String>,
        key: Option<String>,
        value: Option<String>,
    },
    /// Rename a saved server
    Rename {
        old_name: String,
        new_name: String
    },
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
    Remove {
        name: String,
    },
    /// List saved servers and number of people connected
    List,
}

#[derive(Debug, StructOpt)]
enum Channel {
    List {
        #[structopt(short = "s", long = "short")]
        short: bool,
    },
    Connect {
        name: String,
    },
}

#[derive(Debug, StructOpt)]
enum Completions {
    Zsh,
    Bash,
    Fish,
}

#[derive(Debug)]
enum CliError {
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
struct Error(Box<dyn std::error::Error>); // new type since otherwise we'd get an impl conflict with impl<T> From<T> for T below

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

fn main() {
    log::set_logger(&LOGGER)
        .map(|()| log::set_max_level(LevelFilter::Info))
        .unwrap();
    if let Err(e) = match_opt() {
        error!("{}", e);
    }
}
fn match_opt() -> Result<(), Error> {
    let mut config = config::read_default_cfg()?;

    let opt = Mum::from_args();
    match opt.command {
        Command::Connect {
            host,
            username,
            password,
            port,
        } => {
            let port = port.unwrap_or(mumlib::DEFAULT_PORT);

            let (host, username, password, port) =
                match config.servers.iter().find(|e| e.name == host) {
                    Some(server) => (
                        &server.host,
                        server
                            .username
                            .as_ref()
                            .or(username.as_ref())
                            .ok_or(CliError::NoUsername)?,
                        server.password.as_ref().or(password.as_ref()),
                        server.port.unwrap_or(port),
                    ),
                    None => (
                        &host,
                        username.as_ref().ok_or(CliError::NoUsername)?,
                        password.as_ref(),
                        port,
                    ),
                };

            let response = send_command(MumCommand::ServerConnect {
                host: host.to_string(),
                port,
                username: username.to_string(),
                password: password.map(|x| x.to_string()),
                accept_invalid_cert: true, //TODO
            })??;
            if let Some(CommandResponse::ServerConnect { welcome_message }) = response {
                println!("Connected to {}", host);
                if let Some(message) = welcome_message {
                    println!("Welcome: {}", message);
                }
            }
        }
        Command::Disconnect => {
            send_command(MumCommand::ServerDisconnect)??;
        }
        Command::Server(server_command) => {
            match_server_command(server_command, &mut config)?;
        }
        Command::Channel(channel_command) => {
            match channel_command {
                Channel::List { short: _short } => {
                    //TODO actually use this
                    match send_command(MumCommand::ChannelList)?? {
                        Some(CommandResponse::ChannelList { channels }) => {
                            print_channel(&channels, 0);
                        }
                        _ => unreachable!("Response should only be a ChannelList"),
                    }
                }
                Channel::Connect { name } => {
                    send_command(MumCommand::ChannelJoin {
                        channel_identifier: name,
                    })??;
                }
            }
        }
        Command::Status => match send_command(MumCommand::Status)?? {
            Some(CommandResponse::Status { server_state }) => {
                parse_state(&server_state);
            }
            _ => unreachable!("Response should only be a Status"),
        },
        Command::Config { key, value } => match key.as_str() {
            "audio.input_volume" => {
                if let Ok(volume) = value.parse() {
                    send_command(MumCommand::InputVolumeSet(volume))??;
                    config.audio.input_volume = Some(volume);
                }
            }
            "audio.output_volume" => {
                if let Ok(volume) = value.parse() {
                    send_command(MumCommand::OutputVolumeSet(volume))??;
                    config.audio.output_volume = Some(volume);
                }
            }
            _ => {
                return Err(CliError::ConfigKeyNotFound(key))?;
            }
        },
        Command::ConfigReload => {
            send_command(MumCommand::ConfigReload)??;
        }
        Command::Completions(completions) => {
            Mum::clap().gen_completions_to(
                "mumctl",
                match completions {
                    Completions::Bash => Shell::Bash,
                    Completions::Fish => Shell::Fish,
                    _ => Shell::Zsh,
                },
                &mut io::stdout(),
            );
        }
        Command::Volume { user, volume } => {
            if let Some(volume) = volume {
                send_command(MumCommand::UserVolumeSet(user, volume))??;
            } else {
                //TODO report volume of user
                //     needs work on mumd
                todo!();
            }
        }
        Command::Mute { user } => match user {
            Some(user) => {
                send_command(MumCommand::MuteOther(user, Some(true)))??;
            }
            None => {
                send_command(MumCommand::MuteSelf(Some(true)))??;
            }
        },
        Command::Unmute { user } => match user {
            Some(user) => {
                send_command(MumCommand::MuteOther(user, Some(false)))??;
            }
            None => {
                send_command(MumCommand::MuteSelf(Some(false)))??;
            }
        },
        Command::Deafen => {
            send_command(MumCommand::DeafenSelf(Some(true)))??;
        }
        Command::Undeafen => {
            send_command(MumCommand::DeafenSelf(Some(false)))??;
        }
    }

    if !config::cfg_exists() {
        println!(
            "Config file not found. Create one in {}? [Y/n]",
            config::default_cfg_path().display(),
        );
        let stdin = std::io::stdin();
        let response = stdin.lock().lines().next();
        if let Some(Ok(true)) = response.map(|e| e.map(|e| &e == "Y")) {
            config.write_default_cfg(true)?;
        }
    } else {
        config.write_default_cfg(false)?;
    }
    Ok(())
}

fn match_server_command(server_command: Server, config: &mut Config) -> Result<(), CliError> {
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
                        println!("{}", server.name);
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
                    print!(
                        "{}{}{}{}",
                        format!("host: {}\n", server.host.to_string()),
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
                    );
                }
                (Some("name"), None) => {
                    println!("{}", server.name);
                }
                (Some("host"), None) => {
                    println!("{}", server.host);
                }
                (Some("port"), None) => {
                    println!(
                        "{}",
                        server.port.ok_or(CliError::NotSet("port".to_string()))?
                    );
                }
                (Some("username"), None) => {
                    println!(
                        "{}",
                        server
                            .username
                            .as_ref()
                            .ok_or(CliError::NotSet("username".to_string()))?
                    );
                }
                (Some("password"), None) => {
                    println!(
                        "{}",
                        server
                            .password
                            .as_ref()
                            .ok_or(CliError::NotSet("password".to_string()))?
                    );
                }
                (Some("name"), Some(_)) => {
                    return Err(CliError::UseServerRename)?;
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
                    //TODO ask stdin if empty
                }
                (Some(_), _) => {
                    return Err(CliError::ConfigKeyNotFound(key.unwrap()))?;
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
                return Err(CliError::ServerAlreadyExists(name))?;
            } else {
                config.servers.push(ServerConfig {
                    name,
                    host,
                    port,
                    username,
                    password,
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
                return Err(CliError::NoServers)?;
            }
            let query = config
                .servers
                .iter()
                .map(|e| {
                    let response = send_command(MumCommand::ServerStatus {
                        host: e.host.clone(),
                        port: e.port.unwrap_or(mumlib::DEFAULT_PORT),
                    });
                    response.map(|f| (e, f))
                })
                .collect::<Result<Vec<_>, _>>()?;
            for (server, response) in query
                .into_iter()
                .filter(|e| e.1.is_ok())
                .map(|e| (e.0, e.1.unwrap().unwrap()))
            {
                if let CommandResponse::ServerStatus {
                    users, max_users, ..
                } = response
                {
                    println!("{} [{}/{}]", server.name, users, max_users)
                } else {
                    unreachable!()
                }
            }
        }
    }
    Ok(())
}

fn parse_state(server_state: &mumlib::state::Server) {
    println!(
        "Connected to {} as {}",
        server_state.host, server_state.username
    );
    let own_channel = server_state
        .channels
        .iter()
        .find(|e| e.users.iter().any(|e| e.name == server_state.username))
        .unwrap();
    println!(
        "Currently in {} with {} other client{}:",
        own_channel.name,
        own_channel.users.len() - 1,
        if own_channel.users.len() == 2 {
            ""
        } else {
            "s"
        }
    );
    println!("{}{}", INDENTATION, own_channel.name);
    for user in &own_channel.users {
        println!("{}{}{}", INDENTATION, INDENTATION, user);
    }
}

fn send_command(
    command: MumCommand,
) -> Result<mumlib::error::Result<Option<CommandResponse>>, CliError> {
    let mut connection =
        UnixStream::connect(mumlib::SOCKET_PATH).map_err(|_| CliError::ConnectionError)?;

    let serialized = bincode::serialize(&command).unwrap();

    connection
        .write(&(serialized.len() as u32).to_be_bytes())
        .map_err(|_| CliError::ConnectionError)?;
    connection
        .write(&serialized)
        .map_err(|_| CliError::ConnectionError)?;

    connection
        .read_exact(&mut [0; 4])
        .map_err(|_| CliError::ConnectionError)?;
    bincode::deserialize_from(&mut connection).map_err(|_| CliError::ConnectionError)
}

fn print_channel(channel: &MumChannel, depth: usize) {
    println!(
        "{}{}{}",
        iter::repeat(INDENTATION).take(depth).collect::<String>(),
        channel.name.bold(),
        if channel.max_users != 0 {
            format!(" {}/{}", channel.users.len(), channel.max_users)
        } else {
            "".to_string()
        }
    );
    for user in &channel.users {
        println!(
            "{}- {}",
            iter::repeat(INDENTATION)
                .take(depth + 1)
                .collect::<String>(),
            user
        );
    }
    for child in &channel.children {
        print_channel(child, depth + 1);
    }
}
