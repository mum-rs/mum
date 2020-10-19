use clap::{App, AppSettings, Arg, Shell, SubCommand};
use colored::Colorize;
use ipc_channel::ipc::{self, IpcSender};
use log::*;
use mumlib::command::{Command, CommandResponse};
use mumlib::config;
use mumlib::config::ServerConfig;
use mumlib::setup_logger;
use mumlib::state::Channel;
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
                        .setting(AppSettings::ArgRequiredElseHelp)
                        .arg(Arg::with_name("host").required(true))
                        .arg(Arg::with_name("username").required(true))
                        .arg(Arg::with_name("port").short("p").long("port").takes_value(true)))
                .subcommand(SubCommand::with_name("disconnect"))
                .subcommand(
                    SubCommand::with_name("config")
                        .setting(AppSettings::ArgRequiredElseHelp)
                        .arg(Arg::with_name("server_name").required(true))
                        .arg(Arg::with_name("var_name").required(true))
                        .arg(Arg::with_name("var_value").required(true)))
                .subcommand(
                    SubCommand::with_name("rename")
                        .setting(AppSettings::ArgRequiredElseHelp)
                        .arg(Arg::with_name("prev_name").required(true))
                        .arg(Arg::with_name("next_name").required(true)))
                .subcommand(
                    SubCommand::with_name("add")
                        .setting(AppSettings::ArgRequiredElseHelp)
                        .arg(Arg::with_name("name").required(true))
                        .arg(Arg::with_name("host").required(true))
                        .arg(Arg::with_name("port").long("port").takes_value(true).default_value("64738"))
                        .arg(Arg::with_name("username").long("username").takes_value(true))
                        .arg(Arg::with_name("password").long("password").takes_value(true))))
        .subcommand(
            SubCommand::with_name("channel")
                .setting(AppSettings::ArgRequiredElseHelp)
                .subcommand(
                    SubCommand::with_name("list")
                        .arg(Arg::with_name("short").short("s").long("short")))
                .subcommand(
                    SubCommand::with_name("connect").arg(Arg::with_name("channel").required(true))))
        .subcommand(SubCommand::with_name("status"))
        .subcommand(SubCommand::with_name("config")
                    .arg(Arg::with_name("name")
                         .required(true))
                    .arg(Arg::with_name("value")
                         .required(true)))
        .subcommand(SubCommand::with_name("completions")
                    .arg(Arg::with_name("zsh")
                         .long("zsh"))
                    .arg(Arg::with_name("bash")
                         .long("bash"))
                    .arg(Arg::with_name("fish")
                         .long("fish")));

    let matches = app.clone().get_matches();

    if let Some(matches) = matches.subcommand_matches("server") {
        if let Some(matches) = matches.subcommand_matches("connect") {
            let host = matches.value_of("host").unwrap();
            let username = matches.value_of("username").unwrap();
            let port = match matches.value_of("port").map(|e| e.parse()) {
                None => Some(64738),
                Some(Err(_)) => None,
                Some(Ok(v)) => Some(v),
            };
            if let Some(port) = port {
                err_print!(send_command(Command::ServerConnect {
                    host: host.to_string(),
                    port,
                    username: username.to_string(),
                    accept_invalid_cert: true, //TODO
                }));
            }
        } else if let Some(_) = matches.subcommand_matches("disconnect") {
            err_print!(send_command(Command::ServerDisconnect));
        } else if let Some(matches) = matches.subcommand_matches("config") {
            let server_name = matches.value_of("server_name").unwrap();
            let var_name = matches.value_of("var_name").unwrap();
            let var_value = matches.value_of("var_value").unwrap();
            if let Some(ref mut servers) = config.servers {
                let server = servers
                    .iter_mut()
                    .find(
                        |s| s.name == server_name);
                if server.is_none() {
                    println!("{} server {} not found", "error:".red(), server_name);
                } else {
                    let server = server.unwrap();
                    match var_name {
                        "name" => {
                            println!("use mumctl server rename instead!");
                        },
                        "host" => {
                            server.host = var_value.to_string();
                        },
                        "port" => {
                            server.port = var_value.parse().unwrap();
                        },
                        "username" => {
                            server.username = Some(var_value.to_string());
                        },
                        "password" => {
                            server.password = Some(var_value.to_string()); //TODO ask stdin if empty
                        },
                        &_ => {
                            println!("{} variable {} not found", "error:".red(), var_name);
                        },
                    };
                }
            } else {
                println!("{} no servers found in configuration", "error:".red());
            }
        } else if let Some(matches) = matches.subcommand_matches("rename") {
            if let Some(ref mut servers) = config.servers {
                let prev_name = matches.value_of("prev_name").unwrap();
                let next_name = matches.value_of("next_name").unwrap();
                let server = servers
                    .iter_mut()
                    .find(
                        |s| s.name == prev_name
                    );
                if server.is_none() {
                    println!("{} server {} not found", "error:".red(), prev_name);
                } else {
                    server.unwrap().name = next_name.to_string();
                }
            }
        } else if let Some(matches) = matches.subcommand_matches("add") {
            let name = matches.value_of("name").unwrap().to_string();
            let host = matches.value_of("host").unwrap().to_string();
            let port = matches.value_of("port").unwrap().parse().unwrap();
            let username = if let Some(username) = matches.value_of("username") {
                Some(username.to_string())
            } else {
                None
            };
            let password = if let Some(password) = matches.value_of("password") {
                Some(password.to_string())
            } else {
                None
            };
            if let Some(ref mut servers) = config.servers {
                if servers.into_iter().any(|s| s.name == name) {
                    println!("{} a server named {} already exists", "error:".red(), name);
                } else {
                    servers.push(ServerConfig {
                        name,
                        host,
                        port,
                        username,
                        password,
                    });
                }
            } else {
                config.servers = Some(vec![ServerConfig {
                    name,
                    host,
                    port,
                    username,
                    password,
                }]);
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
    } else if let Some(_matches) = matches.subcommand_matches("status") {
        match send_command(Command::Status) {
            Ok(res) => match res {
                Some(CommandResponse::Status { server_state }) => {
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
                }
            },
            _ => {
                println!("{} Unknown config value {}", "error:".red(), name);
            }
        }
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
    };

    config.write_default_cfg();
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
            "{}-{}",
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
