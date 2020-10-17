use clap::{App, AppSettings, Arg, Shell, SubCommand};
use ipc_channel::ipc::{self, IpcSender};
use mumlib::command::{Command, CommandResponse};
use mumlib::setup_logger;
use std::{fs, io, iter};
use colored::Colorize;
use mumlib::state::Channel;

macro_rules! err_print {
    ($func:expr) => {
        if let Err(e) = $func {
            println!("{} {}", "error:".red(), e);
        }
    };
}

fn main() {
    setup_logger();

    let mut app = App::new("mumctl")
        .setting(AppSettings::ArgRequiredElseHelp)
        .subcommand(SubCommand::with_name("server")
                    .setting(AppSettings::ArgRequiredElseHelp)
                    .subcommand(SubCommand::with_name("connect")
                                .setting(AppSettings::ArgRequiredElseHelp)
                                .arg(Arg::with_name("host")
                                     .required(true)
                                     .index(1))
                                .arg(Arg::with_name("username")
                                     .required(true)
                                     .index(2)))
                    .subcommand(SubCommand::with_name("disconnect")))
        .subcommand(SubCommand::with_name("channel")
                    .setting(AppSettings::ArgRequiredElseHelp)
                    .subcommand(SubCommand::with_name("list")
                                .arg(Arg::with_name("short")
                                     .short("s")
                                     .long("short")))
                    .subcommand(SubCommand::with_name("connect")
                                .arg(Arg::with_name("channel")
                                     .required(true))))
        .subcommand(SubCommand::with_name("status"))
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
            err_print!(send_command(Command::ServerConnect {
                host: host.to_string(),
                port: 64738u16, //TODO
                username: username.to_string(),
                accept_invalid_cert: true, //TODO
            }));
        } else if let Some(_) = matches.subcommand_matches("disconnect") {
            err_print!(send_command(Command::ServerDisconnect));
        }
    } else if let Some(matches) = matches.subcommand_matches("channel") {
        if let Some(_matches) = matches.subcommand_matches("list") {
            match send_command(Command::ChannelList) {
                Ok(res) => {
                    match res {
                        Some(CommandResponse::ChannelList { channels }) => {
                            print_channel(&channels, 0);
                        }
                        _ => unreachable!(),
                    }
                }
                Err(e) => println!("{} {}", "error:".red(), e),
            }
        } else if let Some(matches) = matches.subcommand_matches("connect") {
            err_print!(send_command(Command::ChannelJoin {
                channel_id: matches.value_of("channel").unwrap().parse::<u32>().unwrap()
            }));
        }
    } else if let Some(_matches) = matches.subcommand_matches("status") {
        match send_command(Command::Status) {
            Ok(res) => {
                println!("{:#?}", res.unwrap());
            }
            Err(e) => println!("{} {}", "error:".red(), e),
        }
    } else if let Some(matches) = matches.subcommand_matches("completions") {
            app.gen_completions_to("mumctl",
                                   match matches.value_of("shell").unwrap_or("zsh") {
                                       "bash" => {
                                           Shell::Bash
                                       },
                                       "fish" => {
                                           Shell::Fish
                                       },
                                       _ => {
                                           Shell::Zsh
                                       },
                                   },
                                   &mut io::stdout());
            return;
    };
}

fn send_command(command: Command) -> mumlib::error::Result<Option<CommandResponse>> {
    let (tx_client, rx_client) = ipc::channel::<mumlib::error::Result<Option<CommandResponse>>>().unwrap();

    let server_name = fs::read_to_string("/var/tmp/mumd-oneshot").unwrap(); //TODO don't panic

    let tx0 = IpcSender::connect(server_name).unwrap();

    tx0.send((command, tx_client)).unwrap();

    rx_client.recv().unwrap()
}

fn print_channel(channel: &Channel, depth: usize) {
    println!("{}{}{}", iter::repeat("  ").take(depth).collect::<String>(), channel.name.bold(), if channel.max_users != 0 {
        format!(" {}/{}", channel.users.len(), channel.max_users)
    } else {
        "".to_string()
    });
    for user in &channel.users {
        println!("{}-{}", iter::repeat("  ").take(depth + 1).collect::<String>(), user.name);
    }
    for child in &channel.children {
        print_channel(child, depth + 1);
    }
}