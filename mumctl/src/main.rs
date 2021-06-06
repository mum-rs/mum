use colored::Colorize;
use log::*;
use mumlib::command::{Command as MumCommand, CommandResponse, MessageTarget};
use mumlib::config::{self, Config, ServerConfig};
use mumlib::state::Channel as MumChannel;
use serde::de::DeserializeOwned;
use std::fmt;
use std::io::{self, BufRead, Read, Write};
use std::iter;
use std::marker::PhantomData;
use std::os::unix::net::UnixStream;
use std::thread;
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
}

#[derive(Debug, StructOpt)]
enum Target {
    Channel {
        /// The message to send
        message: String,
        /// If the message should be sent recursivley to sub-channels
        #[structopt(short = "r", long = "recursive")]
        recursive: bool,
        /// Which channels to send to
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
enum Server {
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
    let mut config = config::read_cfg(&config::default_cfg_path())?;

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
                return Err(CliError::ConfigKeyNotFound(key).into());
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
        Command::Messages { follow } => {
            for response in send_command_multi(MumCommand::PastMessages { block: follow })? {
                match response {
                    Ok(Some(CommandResponse::PastMessage { message })) => {
                        println!("{}: {}", message.1, message.0)
                    }
                    Ok(_) => unreachable!("Response should only be a Some(PastMessages)"),
                    Err(e) => error!("{}", e),
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
                    targets: names
                        .into_iter()
                        .map(|name| MessageTarget::Channel { name, recursive })
                        .collect(),
                };
                send_command(msg)??;
            }
            Target::User { message, names } => {
                let msg = MumCommand::SendMessage {
                    message,
                    targets: names
                        .into_iter()
                        .map(|name| MessageTarget::User { name })
                        .collect(),
                };
                send_command(msg)??;
            }
        },
    }

    let config_path = config::default_cfg_path();
    if !config_path.exists() {
        println!(
            "Config file not found. Create one in {}? [Y/n]",
            config_path.display(),
        );
        let stdin = std::io::stdin();
        let response = stdin.lock().lines().next();
        if let Some(Ok(true)) = response.map(|e| e.map(|e| &e == "Y")) {
            config.write(&config_path, true)?;
        }
    } else {
        config.write(&config_path, false)?;
    }
    Ok(())
}

fn match_server_command(server_command: Server, config: &mut Config) -> Result<(), Error> {
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
                return Err(CliError::ServerAlreadyExists(name).into());
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
                return Err(CliError::NoServers.into());
            }

            let longest = config
                .servers
                .iter()
                .map(|s| s.name.len())
                .max()
                .unwrap()  // ok since !config.servers.is_empty() above
                + 1;

            let queries: Vec<_> = config
                .servers
                .iter()
                .map(|s| {
                    let query = MumCommand::ServerStatus {
                        host: s.host.clone(),
                        port: s.port.unwrap_or(mumlib::DEFAULT_PORT),
                    };
                    thread::spawn(move || send_command(query))
                })
                .collect();

            for (server, response) in config.servers.iter().zip(queries) {
                match response.join().unwrap() {
                    Ok(Ok(Some(response))) => {
                        if let CommandResponse::ServerStatus {
                            users, max_users, ..
                        } = response
                        {
                            println!(
                                "{0:<1$} [{2:}/{3:}]",
                                server.name, longest, users, max_users
                            );
                        } else {
                            unreachable!();
                        }
                    }
                    Ok(Ok(None)) => {
                        println!("{0:<1$} offline", server.name, longest);
                    }
                    Ok(Err(e)) => {
                        error!("{}", e);
                        return Err(e.into());
                    }
                    Err(e) => {
                        error!("{}", e);
                        return Err(e.into());
                    }
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

/// Tries to find a running mumd instance and receive one response from it.
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

/// Tries to find a running mumd instance and send a single command to it. Returns an iterator which
/// yields all responses that mumd sends for that particular command.
fn send_command_multi(
    command: MumCommand,
) -> Result<impl Iterator<Item = mumlib::error::Result<Option<CommandResponse>>>, CliError> {
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
        .shutdown(std::net::Shutdown::Write)
        .map_err(|_| CliError::ConnectionError)?;

    Ok(BincodeIter::new(connection))
}

/// A struct to represent an iterator that deserializes bincode-encoded data from a `Reader`.
struct BincodeIter<R, I> {
    reader: R,
    phantom: PhantomData<*const I>,
}

impl<R, I> BincodeIter<R, I> {
    /// Creates a new `BincodeIter` from a reader.
    fn new(reader: R) -> Self {
        Self {
            reader,
            phantom: PhantomData,
        }
    }
}

impl<R, I> Iterator for BincodeIter<R, I>
where
    R: Read,
    I: DeserializeOwned,
{
    type Item = I;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.reader.read_exact(&mut [0; 4]).ok()?;
        bincode::deserialize_from(&mut self.reader).ok()
    }
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
