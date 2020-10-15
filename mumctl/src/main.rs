use clap::{App, Arg, SubCommand};
use ipc_channel::ipc::{self, IpcReceiver, IpcSender};
use log::*;
use mumlib::command::{Command, CommandResponse};
use mumlib::setup_logger;
use std::fs;

fn main() {
    setup_logger();

    let matches = App::new("mumctl")
        .subcommand(SubCommand::with_name("server")
                    .subcommand(SubCommand::with_name("connect")
                                .arg(Arg::with_name("host")
                                     .required(true)
                                     .index(1))
                                .arg(Arg::with_name("username")
                                     .required(true)
                                     .index(2)))
                    .subcommand(SubCommand::with_name("disconnect")))
        .subcommand(SubCommand::with_name("channel")
                    .subcommand(SubCommand::with_name("list")
                                .arg(Arg::with_name("short")
                                     .short("s")
                                     .long("short")))
                    .subcommand(SubCommand::with_name("connect")
                                .arg(Arg::with_name("channel")
                                     .required(true))))
        .subcommand(SubCommand::with_name("status"))
        .get_matches();

    let command =
        if let Some(matches) = matches.subcommand_matches("server") {
            if let Some(matches) = matches.subcommand_matches("connect") {
                let host = matches.value_of("host").unwrap();
                let username = matches.value_of("username").unwrap();
                Some(Command::ServerConnect {
                    host: host.to_string(),
                    port: 64738u16, //TODO
                    username: username.to_string(),
                    accept_invalid_cert: true, //TODO
                })
            } else {
                None
            }
        } else if let Some(matches) = matches.subcommand_matches("channel") {
            if let Some(matches) = matches.subcommand_matches("list") {
                if matches.is_present("short") {
                    None //TODO
                } else {
                    None //TODO
                }
            } else if let Some(_matches) = matches.subcommand_matches("connect") {
                None //TODO
            } else {
                None
            }
        } else if let Some(_matches) = matches.subcommand_matches("status") {
            None //TODO
        } else {
            None
        };

    debug!("Creating channel");
    let (tx_client, rx_client): (IpcSender<Result<Option<CommandResponse>, ()>>,
                                 IpcReceiver<Result<Option<CommandResponse>, ()>>) = ipc::channel().unwrap();

    let server_name = fs::read_to_string("/var/tmp/mumd-oneshot").unwrap(); //TODO don't panic
    info!("Sending {:#?}", command);
    let tx0 = IpcSender::connect(server_name).unwrap();
    tx0.send((command.unwrap(), tx_client)).unwrap();

    debug!("Reading response");
    let response = rx_client.recv().unwrap();
    debug!("\n{:#?}", response);
}
