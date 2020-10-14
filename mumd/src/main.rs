mod audio;
mod command;
mod network;
mod state;

use crate::command::{Command, CommandResponse};
use crate::network::ConnectionInfo;
use crate::state::State;

use argparse::ArgumentParser;
use argparse::Store;
use argparse::StoreTrue;
use colored::*;
use futures::join;
use log::*;
use mumble_protocol::control::ControlPacket;
use mumble_protocol::crypt::ClientCryptState;
use mumble_protocol::voice::Serverbound;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, watch};

#[tokio::main]
async fn main() {
    // setup logger
    fern::Dispatch::new()
        .format(|out, message, record| {
            let message = message.to_string();
            out.finish(format_args!(
                "{} {}:{}{}{}",
                //TODO runtime flag that disables color
                match record.level() {
                    Level::Error => "ERROR".red(),
                    Level::Warn => "WARN ".yellow(),
                    Level::Info => "INFO ".normal(),
                    Level::Debug => "DEBUG".green(),
                    Level::Trace => "TRACE".normal(),
                },
                record.file().unwrap(),
                record.line().unwrap(),
                if message.chars().any(|e| e == '\n') {
                    "\n"
                } else {
                    " "
                },
                message
            ))
        })
        .level(log::LevelFilter::Debug)
        .chain(std::io::stderr())
        .apply()
        .unwrap();

    // Handle command line arguments
    let mut server_host = "".to_string();
    let mut server_port = 64738u16;
    let mut username = "EchoBot".to_string();
    let mut accept_invalid_cert = false;
    {
        let mut ap = ArgumentParser::new();
        ap.set_description("Run the echo client example");
        ap.refer(&mut server_host)
            .add_option(&["--host"], Store, "Hostname of mumble server")
            .required();
        ap.refer(&mut server_port)
            .add_option(&["--port"], Store, "Port of mumble server");
        ap.refer(&mut username)
            .add_option(&["--username"], Store, "User name used to connect");
        ap.refer(&mut accept_invalid_cert).add_option(
            &["--accept-invalid-cert"],
            StoreTrue,
            "Accept invalid TLS certificates",
        );
        ap.parse_args_or_exit();
    }

    // Oneshot channel for setting UDP CryptState from control task
    // For simplicity we don't deal with re-syncing, real applications would have to.
    let (crypt_state_sender, crypt_state_receiver) = mpsc::channel::<ClientCryptState>(1); // crypt state should always be consumed before sending a new one
    let (packet_sender, packet_receiver) = mpsc::unbounded_channel::<ControlPacket<Serverbound>>();
    let (command_sender, command_receiver) = mpsc::unbounded_channel::<Command>();
    let (command_response_sender, command_response_receiver) =
        mpsc::unbounded_channel::<Result<Option<CommandResponse>, ()>>();
    let (connection_info_sender, connection_info_receiver) =
        watch::channel::<Option<ConnectionInfo>>(None);

    let state = State::new(
        packet_sender,
        command_sender.clone(),
        connection_info_sender,
    );
    let state = Arc::new(Mutex::new(state));

    // Run it
    join!(
        network::tcp::handle(
            Arc::clone(&state),
            connection_info_receiver.clone(),
            crypt_state_sender,
            packet_receiver,
        ),
        network::udp::handle(
            Arc::clone(&state),
            connection_info_receiver.clone(),
            crypt_state_receiver,
        ),
        command::handle(state, command_receiver, command_response_sender,),
        send_commands(
            command_sender,
            Command::ServerConnect {
                host: server_host,
                port: server_port,
                username: username.clone(),
                accept_invalid_cert
            }
        ),
        receive_command_responses(command_response_receiver,),
    );
}

async fn send_commands(command_sender: mpsc::UnboundedSender<Command>, connect_command: Command) {
    command_sender.send(connect_command.clone()).unwrap();
    tokio::time::delay_for(Duration::from_secs(2)).await;
    command_sender.send(Command::ServerDisconnect).unwrap();
    tokio::time::delay_for(Duration::from_secs(2)).await;
    command_sender.send(connect_command.clone()).unwrap();
    tokio::time::delay_for(Duration::from_secs(2)).await;
    command_sender.send(Command::ServerDisconnect).unwrap();

    debug!("Finished sending commands");
}

async fn receive_command_responses(
    mut command_response_receiver: mpsc::UnboundedReceiver<Result<Option<CommandResponse>, ()>>,
) {
    while let Some(command_response) = command_response_receiver.recv().await {
        debug!("{:?}", command_response);
    }

    debug!("Finished receiving commands");
}
