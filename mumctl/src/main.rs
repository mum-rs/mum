use clap::{App, AppSettings, Arg, Shell, SubCommand};
use colored::Colorize;
use ipc_channel::ipc::{self, IpcSender};
use mumlib::command::{Command, CommandResponse};
use mumlib::config;
use mumlib::config::ServerConfig;
use mumlib::setup_logger;
use mumlib::state::Channel;
use std::io::BufRead;
use std::{fs, io, iter};

const INDENTATION: &str = "  ";

macro_rules! err_print {
    ($func:expr) => {
        if let Err(e) = $func {
            println!("{} {}", "error:".red(), e);
        }
    };
}

fn main() {
    setup_logger(io::stderr(), true);
    let mut config = config::read_default_cfg();

    let mut app = App::new("mumctl")
        .setting(AppSettings::ArgRequiredElseHelp)
        .subcommand(
            SubCommand::with_name("server")
                .setting(AppSettings::ArgRequiredElseHelp)
                .subcommand(
                    SubCommand::with_name("connect")
                        .arg(Arg::with_name("host").required(true))
                        .arg(Arg::with_name("username"))
                        .arg(
                            Arg::with_name("port")
                                .long("port")
                                .short("p")
                                .takes_value(true),
                        ),
                )
                .subcommand(SubCommand::with_name("disconnect"))
                .subcommand(
                    SubCommand::with_name("config")
                        .arg(Arg::with_name("server_name"))
                        .arg(Arg::with_name("var_name"))
                        .arg(Arg::with_name("var_value")),
                )
                .subcommand(
                    SubCommand::with_name("rename")
                        .arg(Arg::with_name("prev_name").required(true))
                        .arg(Arg::with_name("next_name").required(true)),
                )
                .subcommand(
                    SubCommand::with_name("add")
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
                    SubCommand::with_name("remove").arg(Arg::with_name("name").required(true)),
                )
                .subcommand(SubCommand::with_name("list")),
        )
        .subcommand(
            SubCommand::with_name("channel")
                .setting(AppSettings::ArgRequiredElseHelp)
                .subcommand(
                    SubCommand::with_name("list")
                        .arg(Arg::with_name("short").long("short").short("s")),
                )
                .subcommand(
                    SubCommand::with_name("connect").arg(Arg::with_name("channel").required(true)),
                ),
        )
        .subcommand(SubCommand::with_name("status"))
        .subcommand(
            SubCommand::with_name("config")
                .arg(Arg::with_name("name").required(true))
                .arg(Arg::with_name("value").required(true)),
        )
        .subcommand(SubCommand::with_name("config-reload"))
        .subcommand(
            SubCommand::with_name("completions")
                .arg(Arg::with_name("zsh").long("zsh"))
                .arg(Arg::with_name("bash").long("bash"))
                .arg(Arg::with_name("fish").long("fish")),
        )
        .subcommand(
            SubCommand::with_name("volume")
                .subcommand(
                    SubCommand::with_name("set")
                        .arg(Arg::with_name("user").required(true))
                        .arg(Arg::with_name("volume").required(true)),
                )
                .arg(Arg::with_name("user").required(true))
                .setting(AppSettings::SubcommandsNegateReqs),
        )
        .subcommand(SubCommand::with_name("mute")
            .subcommand(SubCommand::with_name("true")
                .alias("1"))
            .subcommand(SubCommand::with_name("false")
                .alias("0"))
            .subcommand(SubCommand::with_name("toggle"))
            .setting(AppSettings::SubcommandRequiredElseHelp))
        .subcommand(SubCommand::with_name("deafen")
            .subcommand(SubCommand::with_name("true")
                .alias("1"))
            .subcommand(SubCommand::with_name("false")
                .alias("0"))
            .subcommand(SubCommand::with_name("toggle"))
            .setting(AppSettings::SubcommandRequiredElseHelp))
        .subcommand(SubCommand::with_name("user")
            .arg(Arg::with_name("user").required(true))
            .subcommand(SubCommand::with_name("mute")
                .subcommand(SubCommand::with_name("true")
                    .alias("1"))
                .subcommand(SubCommand::with_name("false")
                    .alias("0"))
                .subcommand(SubCommand::with_name("toggle"))
                .setting(AppSettings::SubcommandRequiredElseHelp))
            .setting(AppSettings::SubcommandRequiredElseHelp));

    let matches = app.clone().get_matches();

    if let Some(matches) = matches.subcommand_matches("server") {
        if let Some(matches) = matches.subcommand_matches("connect") {
            match_server_connect(matches, &config);
        } else if let Some(_) = matches.subcommand_matches("disconnect") {
            err_print!(send_command(Command::ServerDisconnect));
        } else if let Some(matches) = matches.subcommand_matches("config") {
            match_server_config(matches, &mut config);
        } else if let Some(matches) = matches.subcommand_matches("rename") {
            match_server_rename(matches, &mut config);
        } else if let Some(matches) = matches.subcommand_matches("remove") {
            match_server_remove(matches, &mut config);
        } else if let Some(matches) = matches.subcommand_matches("add") {
            match_server_add(matches, &mut config);
        } else if let Some(_) = matches.subcommand_matches("list") {
            if config.servers.len() == 0 {
                println!("{} No servers in config", "warning:".yellow());
            }
            for (server, response) in config.servers
                .iter()
                .map(|e| {
                    let response = send_command(Command::ServerStatus {
                        host: e.host.clone(),
                        port: e.port.unwrap_or(mumlib::DEFAULT_PORT),
                    });
                    (e, response)
                })
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
    } else if let Some(matches) = matches.subcommand_matches("channel") {
        if let Some(_matches) = matches.subcommand_matches("list") {
            match send_command(Command::ChannelList) {
                Ok(res) => match res {
                    Some(CommandResponse::ChannelList { channels }) => {
                        print_channel(&channels, 0);
                    }
                    _ => unreachable!(),
                },
                Err(e) => println!("{} {}", "error:".red(), e),
            }
        } else if let Some(matches) = matches.subcommand_matches("connect") {
            err_print!(send_command(Command::ChannelJoin {
                channel_identifier: matches.value_of("channel").unwrap().to_string()
            }));
        }
    } else if let Some(_) = matches.subcommand_matches("status") {
        match send_command(Command::Status) {
            Ok(res) => match res {
                Some(CommandResponse::Status { server_state }) => {
                    parse_status(&server_state);
                }
                _ => unreachable!(),
            },
            Err(e) => println!("{} {}", "error:".red(), e),
        }
    } else if let Some(matches) = matches.subcommand_matches("config") {
        let name = matches.value_of("name").unwrap();
        let value = matches.value_of("value").unwrap();
        match name {
            "audio.input_volume" => {
                if let Ok(volume) = value.parse() {
                    send_command(Command::InputVolumeSet(volume)).unwrap();
                    config.audio.input_volume = Some(volume);
                }
            }
            "audio.output_volume" => {
                if let Ok(volume) = value.parse() {
                    send_command(Command::OutputVolumeSet(volume)).unwrap();
                    config.audio.output_volume = Some(volume);
                }
            }
            _ => {
                println!("{} Unknown config value {}", "error:".red(), name);
            }
        }
    } else if matches.subcommand_matches("config-reload").is_some() {
        send_command(Command::ConfigReload).unwrap();
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
        return;
    } else if let Some(matches) = matches.subcommand_matches("volume") {
        if let Some(matches) = matches.subcommand_matches("set") {
            let user = matches.value_of("user").unwrap();
            let volume = matches.value_of("volume").unwrap();
            if let Ok(val) = volume.parse() {
                err_print!(send_command(Command::UserVolumeSet(user.to_string(), val)))
            } else {
                println!("{} Invalid volume value: {}", "error:".red(), volume);
            }
        } else {
            let _user = matches.value_of("user").unwrap();
            //TODO implement me
            //needs work on mumd to implement
        }
    } else if let Some(matches) = matches.subcommand_matches("mute") {
        let command = Command::MuteSelf(
            if let Some(_matches) = matches.subcommand_matches("true") {
                Some(true)
            } else if let Some(_matches) = matches.subcommand_matches("false") {
                Some(false)
            } else if let Some(_matches) = matches.subcommand_matches("toggle") {
                None
            } else {
                unreachable!()
            });
        err_print!(send_command(command));
    } else if let Some(matches) = matches.subcommand_matches("deafen") {
        let command = Command::DeafenSelf(
            if let Some(_matches) = matches.subcommand_matches("true") {
                Some(true)
            } else if let Some(_matches) = matches.subcommand_matches("false") {
                Some(false)
            } else if let Some(_matches) = matches.subcommand_matches("toggle") {
                None
            } else {
                unreachable!()
            });
        err_print!(send_command(command));
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
            err_print!(send_command(Command::MuteOther(name.to_string(), toggle)));
        } else {
            unreachable!();
        }
    };

    if !config::cfg_exists() {
        println!(
            "Config file not found. Create one in {}? [Y/n]",
            config::get_creatable_cfg_path()
        );
        let stdin = std::io::stdin();
        let response = stdin.lock().lines().next();
        match response.map(|e| e.map(|e| &e == "Y")) {
            Some(Ok(true)) => {
                config.write_default_cfg(true).unwrap();
            }
            _ => {}
        }
    } else {
        config.write_default_cfg(false).unwrap();
    }
}

fn match_server_connect(matches: &clap::ArgMatches<'_>, config: &mumlib::config::Config) {
    let host = matches.value_of("host").unwrap();
    let username = matches.value_of("username");
    let port = match matches.value_of("port").map(|e| e.parse()) {
        None => Some(mumlib::DEFAULT_PORT),
        Some(Err(_)) => None,
        Some(Ok(v)) => Some(v),
    };
    if let Some(port) = port {
        let (host, port, username) = match config.servers.iter().find(|e| e.name == host) {
            Some(server_config) => {
                let host = server_config.host.as_str();
                let port = server_config.port.unwrap_or(port);
                let username = server_config
                    .username
                    .as_ref()
                    .map(|e| e.as_str())
                    .or(username);
                if username.is_none() {
                    println!("{} no username specified", "error:".red());
                    return;
                }
                (host, port, username.unwrap())
            }
            None => {
                if username.is_none() {
                    println!("{} no username specified", "error:".red());
                    return;
                }
                (host, port, username.unwrap())
            }
        };
        let response = send_command(Command::ServerConnect {
            host: host.to_string(),
            port,
            username: username.to_string(),
            accept_invalid_cert: true, //TODO
        })
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
                println!("{} {}", "error:".red(), e);
            }
        };
    }
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
                            println!("{} use mumctl server rename instead!", "error:".red());
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
                            println!("{} variable {} not found", "error:".red(), var_name);
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
                        .unwrap_or("".to_string()),
                    server
                        .username
                        .as_ref()
                        .map(|s| format!("username: {}\n", s))
                        .unwrap_or("".to_string()),
                    server
                        .password
                        .as_ref()
                        .map(|s| format!("password: {}\n", s))
                        .unwrap_or("".to_string()),
                )
            }
        } else {
            // server is None
            println!("{} server {} not found", "error:".red(), server_name);
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
        println!("{} server {} not found", "error:".red(), prev_name);
    }
}

fn match_server_remove(matches: &clap::ArgMatches<'_>, config: &mut mumlib::config::Config) {
    let name = matches.value_of("name").unwrap();
    match config.servers.iter().position(|server| server.name == name) {
        Some(idx) => {
            config.servers.remove(idx);
        }
        None => {
            println!("{} server {} not found", "error:".red(), name);
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
        println!("{} a server named {} already exists", "error:".red(), name);
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

fn send_command(command: Command) -> mumlib::error::Result<Option<CommandResponse>> {
    let (tx_client, rx_client) =
        ipc::channel::<mumlib::error::Result<Option<CommandResponse>>>().unwrap();

    let server_name = fs::read_to_string(mumlib::SOCKET_PATH).unwrap(); //TODO don't panic

    let tx0 = IpcSender::connect(server_name).unwrap();

    tx0.send((command, tx_client)).unwrap();

    rx_client.recv().unwrap()
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
