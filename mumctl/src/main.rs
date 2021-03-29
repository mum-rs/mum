use clap::{App, AppSettings, Arg, Shell, SubCommand, ArgMatches};
use colored::Colorize;
use log::{Record, Level, Metadata, LevelFilter, error, warn};
use mumlib::command::{Command, CommandResponse};
use mumlib::config::{self, ServerConfig, Config};
use mumlib::state::Channel;
use std::{io::{self, Read, BufRead, Write}, iter, fmt::{Display, Formatter}, os::unix::net::UnixStream};

const INDENTATION: &str = "  ";

macro_rules! error_if_err {
    ($func:expr) => {
        if let Err(e) = $func {
            error!("{}", e);
        }
    };
}

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
                record.args());
        }
    }

    fn flush(&self) {}
}

static LOGGER: SimpleLogger = SimpleLogger;

fn main() {
    log::set_logger(&LOGGER)
        .map(|()| log::set_max_level(LevelFilter::Info)).unwrap();
    let mut config = config::read_default_cfg();

    let mut app = App::new("mumctl")
        .setting(AppSettings::ArgRequiredElseHelp)
        .version(env!("VERSION"))
        .subcommand(
            SubCommand::with_name("connect")
                .about("Connect to a server")
                .arg(Arg::with_name("host").required(true))
                .arg(Arg::with_name("username"))
                .arg(Arg::with_name("password"))
                .arg(
                    Arg::with_name("port")
                        .long("port")
                        .short("p")
                        .takes_value(true),
                ),
        )
        .subcommand(
            SubCommand::with_name("disconnect")
                .about("Disconnect from the currently connected server"),
        )
        .subcommand(
            SubCommand::with_name("server")
                .setting(AppSettings::ArgRequiredElseHelp)
                .about("Handle servers")
                .subcommand(
                    SubCommand::with_name("config")
                        .about("Configure a saved server")
                        .arg(Arg::with_name("server_name"))
                        .arg(Arg::with_name("var_name"))
                        .arg(Arg::with_name("var_value")),
                )
                .subcommand(
                    SubCommand::with_name("rename")
                        .about("Rename a saved server")
                        .arg(Arg::with_name("prev_name").required(true))
                        .arg(Arg::with_name("next_name").required(true)),
                )
                .subcommand(
                    SubCommand::with_name("add")
                        .about("Add a new saved server")
                        .arg(Arg::with_name("name").required(true))
                        .arg(Arg::with_name("host").required(true))
                        .arg(
                            Arg::with_name("port")
                                .long("port")
                                .takes_value(true)
                                .default_value("64738"),
                        )
                        .arg(
                            Arg::with_name("username")
                                .long("username")
                                .takes_value(true),
                        )
                        .arg(
                            Arg::with_name("password")
                                .long("password")
                                .takes_value(true),
                        ),
                )
                .subcommand(
                    SubCommand::with_name("remove")
                        .about("Remove a saved server")
                        .arg(Arg::with_name("name").required(true)),
                )
                .subcommand(
                    SubCommand::with_name("list")
                        .about("List saved servers and number of people connected"),
                ),
        )
        .subcommand(
            SubCommand::with_name("channel")
                .about("Handle channels in the connected server")
                .setting(AppSettings::ArgRequiredElseHelp)
                .subcommand(
                    SubCommand::with_name("list")
                        .about("List all channels")
                        .arg(Arg::with_name("short").long("short").short("s")),
                )
                .subcommand(
                    SubCommand::with_name("connect")
                        .about("Connect to another channel")
                        .arg(Arg::with_name("channel").required(true)),
                ),
        )
        .subcommand(SubCommand::with_name("status").about("Show current status"))
        .subcommand(
            SubCommand::with_name("config")
                .about("Change config values")
                .arg(Arg::with_name("name").required(true))
                .arg(Arg::with_name("value").required(true)),
        )
        .subcommand(
            SubCommand::with_name("config-reload").about("Force a reload of the config file"),
        )
        .subcommand(
            SubCommand::with_name("completions")
                .about("Generate CLI completions")
                .arg(Arg::with_name("zsh").long("zsh"))
                .arg(Arg::with_name("bash").long("bash"))
                .arg(Arg::with_name("fish").long("fish")),
        )
        .subcommand(
            SubCommand::with_name("volume")
                .about("Change volume of either you or someone else")
                .subcommand(
                    SubCommand::with_name("set")
                        .arg(Arg::with_name("user").required(true))
                        .arg(Arg::with_name("volume").required(true)),
                )
                .arg(Arg::with_name("user").required(true))
                .setting(AppSettings::SubcommandsNegateReqs),
        )
        .subcommand(
            SubCommand::with_name("mute")
                .about("Mute/unmute yourself")
                .subcommand(SubCommand::with_name("true").alias("1"))
                .subcommand(SubCommand::with_name("false").alias("0"))
                .subcommand(SubCommand::with_name("toggle"))
                .setting(AppSettings::SubcommandRequiredElseHelp),
        )
        .subcommand(
            SubCommand::with_name("deafen")
                .about("Deafen/undeafen yourself")
                .subcommand(SubCommand::with_name("true").alias("1"))
                .subcommand(SubCommand::with_name("false").alias("0"))
                .subcommand(SubCommand::with_name("toggle"))
                .setting(AppSettings::SubcommandRequiredElseHelp),
        )
        .subcommand(
            SubCommand::with_name("user")
                .about("Configure someone else")
                .arg(Arg::with_name("user").required(true))
                .subcommand(
                    SubCommand::with_name("mute")
                        .subcommand(SubCommand::with_name("true").alias("1"))
                        .subcommand(SubCommand::with_name("false").alias("0"))
                        .subcommand(SubCommand::with_name("toggle"))
                        .setting(AppSettings::SubcommandRequiredElseHelp),
                )
                .setting(AppSettings::SubcommandRequiredElseHelp),
        );

    let matches = app.clone().get_matches();

    if let Err(e) = process_matches(matches, &mut config, &mut app) {
        error!("{}", e);
    }

    if !config::cfg_exists() {
        println!(
            "Config file not found. Create one in {}? [Y/n]",
            config::get_creatable_cfg_path()
        );
        let stdin = std::io::stdin();
        let response = stdin.lock().lines().next();
        if let Some(Ok(true)) = response.map(|e| e.map(|e| &e == "Y")) {
            config.write_default_cfg(true).unwrap();
        }
    } else {
        config.write_default_cfg(false).unwrap();
    }
}

fn process_matches(matches: ArgMatches, config: &mut Config, app: &mut App) -> Result<(), Error> {
    if let Some(matches) = matches.subcommand_matches("connect") {
        match_server_connect(matches, &config)?;
    } else if let Some(_) = matches.subcommand_matches("disconnect") {
        match send_command(Command::ServerDisconnect) {
            Ok(v) => error_if_err!(v),
            Err(e) => error!("{}", e),
        }
        // error_if_err!(send_command(Command::ServerDisconnect));
    } else if let Some(matches) = matches.subcommand_matches("server") {
        if let Some(matches) = matches.subcommand_matches("config") {
            match_server_config(matches, config);
        } else if let Some(matches) = matches.subcommand_matches("rename") {
            match_server_rename(matches, config);
        } else if let Some(matches) = matches.subcommand_matches("remove") {
            match_server_remove(matches, config);
        } else if let Some(matches) = matches.subcommand_matches("add") {
            match_server_add(matches, config);
        } else if let Some(_) = matches.subcommand_matches("list") {
            if config.servers.is_empty() {
                warn!("No servers in config");
            }
            let query = config
                .servers
                .iter()
                .map(|e| {
                    let response = send_command(Command::ServerStatus {
                        host: e.host.clone(),
                        port: e.port.unwrap_or(mumlib::DEFAULT_PORT),
                    });
                    response.map(|f| (e, f))
                })
                .collect::<Result<Vec<_>, _>>()?;
            for (server, response) in query.into_iter()
                .filter(|e| e.1.is_ok())
                .map(|e| (e.0, e.1.unwrap().unwrap())) {
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
    } else if let Some(matches) = matches.subcommand_matches("channel") {
        if let Some(_matches) = matches.subcommand_matches("list") {
            match send_command(Command::ChannelList)? {
                Ok(res) => match res {
                    Some(CommandResponse::ChannelList { channels }) => {
                        print_channel(&channels, 0);
                    }
                    _ => unreachable!(),
                },
                Err(e) => error!("{}", e),
            }
        } else if let Some(matches) = matches.subcommand_matches("connect") {
            error_if_err!(send_command(Command::ChannelJoin {
                channel_identifier: matches.value_of("channel").unwrap().to_string()
            }));
        }
    } else if let Some(_) = matches.subcommand_matches("status") {
        match send_command(Command::Status)? {
            Ok(res) => match res {
                Some(CommandResponse::Status { server_state }) => {
                    parse_status(&server_state);
                }
                _ => unreachable!(),
            },
            Err(e) => error!("{}", e),
        }
    } else if let Some(matches) = matches.subcommand_matches("config") {
        let name = matches.value_of("name").unwrap();
        let value = matches.value_of("value").unwrap();
        match name {
            "audio.input_volume" => {
                if let Ok(volume) = value.parse() {
                    send_command(Command::InputVolumeSet(volume))?.unwrap();
                    config.audio.input_volume = Some(volume);
                }
            }
            "audio.output_volume" => {
                if let Ok(volume) = value.parse() {
                    send_command(Command::OutputVolumeSet(volume))?.unwrap();
                    config.audio.output_volume = Some(volume);
                }
            }
            _ => {
                error!("unknown config value {}", name);
            }
        }
    } else if matches.subcommand_matches("config-reload").is_some() {
        send_command(Command::ConfigReload)?.unwrap();
    } else if let Some(matches) = matches.subcommand_matches("completions") {
        app.gen_completions_to(
            "mumctl",
            match matches.value_of("shell").unwrap_or("zsh") {
                "bash" => Shell::Bash,
                "fish" => Shell::Fish,
                _ => Shell::Zsh,
            },
            &mut io::stdout(),
        );
    } else if let Some(matches) = matches.subcommand_matches("volume") {
        if let Some(matches) = matches.subcommand_matches("set") {
            let user = matches.value_of("user").unwrap();
            let volume = matches.value_of("volume").unwrap();
            if let Ok(val) = volume.parse() {
                error_if_err!(send_command(Command::UserVolumeSet(user.to_string(), val)))
            } else {
                error!("invalid volume value: {}", volume);
            }
        } else {
            let _user = matches.value_of("user").unwrap();
            //TODO implement me
            //needs work on mumd to implement
        }
    } else if let Some(matches) = matches.subcommand_matches("mute") {
        let command =
            Command::MuteSelf(if let Some(_matches) = matches.subcommand_matches("true") {
                Some(true)
            } else if let Some(_matches) = matches.subcommand_matches("false") {
                Some(false)
            } else if let Some(_matches) = matches.subcommand_matches("toggle") {
                None
            } else {
                unreachable!()
            });
        match send_command(command)? {
            Ok(Some(CommandResponse::MuteStatus { is_muted })) => println!("{}", if is_muted {
                "Muted"
            } else {
                "Unmuted"
            }),
            Ok(_) => {},
            Err(e) => error!("{}", e),
        }
    } else if let Some(matches) = matches.subcommand_matches("deafen") {
        let command =
            Command::DeafenSelf(if let Some(_matches) = matches.subcommand_matches("true") {
                Some(true)
            } else if let Some(_matches) = matches.subcommand_matches("false") {
                Some(false)
            } else if let Some(_matches) = matches.subcommand_matches("toggle") {
                None
            } else {
                unreachable!()
            });
        match send_command(command)? {
            Ok(Some(CommandResponse::DeafenStatus { is_deafened })) => println!("{}", if is_deafened {
                "Deafened"
            } else {
                "Undeafened"
            }),
            Ok(_) => {},
            Err(e) => error!("{}", e),
        }
    } else if let Some(matches) = matches.subcommand_matches("user") {
        let name = matches.value_of("user").unwrap();
        if let Some(matches) = matches.subcommand_matches("mute") {
            let toggle = if let Some(_matches) = matches.subcommand_matches("true") {
                Some(true)
            } else if let Some(_matches) = matches.subcommand_matches("false") {
                Some(false)
            } else if let Some(_matches) = matches.subcommand_matches("toggle") {
                None
            } else {
                unreachable!()
            };
            error_if_err!(send_command(Command::MuteOther(name.to_string(), toggle)));
        } else {
            unreachable!();
        }
    };

    Ok(())
}

fn match_server_connect(matches: &clap::ArgMatches<'_>, config: &mumlib::config::Config) -> Result<(), Error> {
    let host = matches.value_of("host").unwrap();
    let username = matches.value_of("username");
    let password = matches.value_of("password");
    let port = match matches.value_of("port").map(|e| e.parse()) {
        None => Some(mumlib::DEFAULT_PORT),
        Some(Err(_)) => None,
        Some(Ok(v)) => Some(v),
    };
    if let Some(port) = port {
        let (host, port, username, password) = match config.servers.iter().find(|e| e.name == host) {
            Some(server_config) => {
                let host = server_config.host.as_str();
                let port = server_config.port.unwrap_or(port);
                let username = server_config
                    .username
                    .as_deref()
                    .or(username);
                if username.is_none() {
                    error!("no username specified");
                    return Ok(()); //TODO? return as error
                }
                let password = server_config
                    .password
                    .as_deref()
                    .or(password);
                (host, port, username.unwrap(), password)
            }
            None => {
                if username.is_none() {
                    error!("no username specified");
                    return Ok(()); //TODO? return as error
                }
                (host, port, username.unwrap(), password)
            }
        };
        let response = send_command(Command::ServerConnect {
            host: host.to_string(),
            port,
            username: username.to_string(),
            password: password.map(|x| x.to_string()),
            accept_invalid_cert: true, //TODO
        })?
        .map(|e| (e, host));
        match response {
            Ok((e, host)) => {
                if let Some(CommandResponse::ServerConnect { welcome_message }) = e {
                    println!("Connected to {}", host);
                    if let Some(message) = welcome_message {
                        println!("Welcome: {}", message);
                    }
                }
            }
            Err(e) => {
                error!("{}", e);
            }
        };
    }

    Ok(())
}

fn match_server_config(matches: &clap::ArgMatches<'_>, config: &mut mumlib::config::Config) {
    if let Some(server_name) = matches.value_of("server_name") {
        let server = config.servers.iter_mut().find(|s| s.name == server_name);
        if let Some(server) = server {
            if let Some(var_name) = matches.value_of("var_name") {
                if let Some(var_value) = matches.value_of("var_value") {
                    // save var_value in var_name (if it is valid)
                    match var_name {
                        "name" => {
                            error!("use mumctl server rename instead!");
                        }
                        "host" => {
                            server.host = var_value.to_string();
                        }
                        "port" => {
                            server.port = Some(var_value.parse().unwrap());
                        }
                        "username" => {
                            server.username = Some(var_value.to_string());
                        }
                        "password" => {
                            server.password = Some(var_value.to_string()); //TODO ask stdin if empty
                        }
                        _ => {
                            error!("variable {} not found", var_name);
                        }
                    };
                } else {
                    // var_value is None
                    // print value of var_name
                    println!(
                        "{}",
                        match var_name {
                            "name" => {
                                server.name.to_string()
                            }
                            "host" => {
                                server.host.to_string()
                            }
                            "port" => {
                                server
                                    .port
                                    .map(|s| s.to_string())
                                    .unwrap_or(format!("{} not set", "error:".red()))
                            }
                            "username" => {
                                server
                                    .username
                                    .as_ref()
                                    .map(|s| s.to_string())
                                    .unwrap_or(format!("{} not set", "error:".red()))
                            }
                            "password" => {
                                server
                                    .password
                                    .as_ref()
                                    .map(|s| s.to_string())
                                    .unwrap_or(format!("{} not set", "error:".red()))
                            }
                            _ => {
                                format!("{} unknown variable", "error:".red())
                            }
                        }
                    );
                }
            } else {
                // var_name is None
                // print server config
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
                )
            }
        } else {
            // server is None
            error!("server {} not found", server_name);
        }
    } else {
        for server in config.servers.iter() {
            println!("{}", server.name);
        }
    }
}

fn match_server_rename(matches: &clap::ArgMatches<'_>, config: &mut mumlib::config::Config) {
    let prev_name = matches.value_of("prev_name").unwrap();
    let next_name = matches.value_of("next_name").unwrap();
    if let Some(server) = config.servers.iter_mut().find(|s| s.name == prev_name) {
        server.name = next_name.to_string();
    } else {
        error!("server {} not found", prev_name);
    }
}

fn match_server_remove(matches: &clap::ArgMatches<'_>, config: &mut mumlib::config::Config) {
    let name = matches.value_of("name").unwrap();
    match config.servers.iter().position(|server| server.name == name) {
        Some(idx) => {
            config.servers.remove(idx);
        }
        None => {
            error!("server {} not found", name);
        }
    };
}

fn match_server_add(matches: &clap::ArgMatches<'_>, config: &mut mumlib::config::Config) {
    let name = matches.value_of("name").unwrap().to_string();
    let host = matches.value_of("host").unwrap().to_string();
    // optional arguments map None to None
    let port = matches.value_of("port").map(|s| s.parse().unwrap());
    let username = matches.value_of("username").map(|s| s.to_string());
    let password = matches.value_of("password").map(|s| s.to_string());
    if config.servers.iter().any(|s| s.name == name) {
        error!("a server named {} already exists", name);
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

fn parse_status(server_state: &mumlib::state::Server) {
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

fn send_command(command: Command) -> Result<mumlib::error::Result<Option<CommandResponse>>, crate::Error> {
    let mut connection = UnixStream::connect(mumlib::SOCKET_PATH).map_err(|_| Error::ConnectionError)?;

    let serialized = bincode::serialize(&command).unwrap();

    connection.write(&(serialized.len() as u32).to_be_bytes()).map_err(|_| Error::ConnectionError)?;
    connection.write(&serialized).map_err(|_| Error::ConnectionError)?;

    connection.read_exact(&mut [0; 4]).map_err(|_| Error::ConnectionError)?;
    bincode::deserialize_from(&mut connection).map_err(|_| Error::ConnectionError)
}

fn print_channel(channel: &Channel, depth: usize) {
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

#[derive(Debug)]
enum Error {
    ConnectionError
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Unable to connect to mumd. Is mumd running?")
    }
}
