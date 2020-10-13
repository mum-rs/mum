mod audio;
mod network;
mod command;
mod state;

use crate::state::State;
use crate::command::Command;

use argparse::ArgumentParser;
use argparse::Store;
use argparse::StoreTrue;
use colored::*;
use futures::channel::oneshot;
use futures::join;
use log::*;
use mumble_protocol::control::ControlPacket;
use mumble_protocol::crypt::ClientCryptState;
use mumble_protocol::voice::Serverbound;
use std::net::ToSocketAddrs;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() {
    // setup logger
    //TODO? add newline before message if it contains newlines
    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "{} {}:{} {}",
                //TODO runtime flag that disables color
                match record.level() {
                    Level::Error => "ERROR".red(),
                    Level::Warn  => "WARN ".yellow(),
                    Level::Info  => "INFO ".normal(),
                    Level::Debug => "DEBUG".green(),
                    Level::Trace => "TRACE".normal(),
                },
                record.file().unwrap(),
                record.line().unwrap(),
                message
            ))
        })
        .level(log::LevelFilter::Debug)
        .chain(std::io::stderr())
        .apply().unwrap();

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
    let server_addr = (server_host.as_ref(), server_port)
        .to_socket_addrs()
        .expect("Failed to parse server address")
        .next()
        .expect("Failed to resolve server address");

    // Oneshot channel for setting UDP CryptState from control task
    // For simplicity we don't deal with re-syncing, real applications would have to.
    let (crypt_state_sender, crypt_state_receiver) = oneshot::channel::<ClientCryptState>();
    let (packet_sender, packet_receiver) = mpsc::unbounded_channel::<ControlPacket<Serverbound>>();
    let (command_sender, command_receiver) = mpsc::unbounded_channel::<Command>();

    command_sender.send(Command::ChannelJoin{channel_id: 1}).unwrap();
    let state = State::new(packet_sender, command_sender, username);
    let state = Arc::new(Mutex::new(state));

    // Run it
    join!(
        network::tcp::handle(
            Arc::clone(&state),
            server_addr,
            server_host,
            accept_invalid_cert,
            crypt_state_sender,
            packet_receiver,
        ),
        network::udp::handle(
            Arc::clone(&state),
            server_addr,
            crypt_state_receiver,
        ),
        command::handle(
            state,
            command_receiver,
        ),
    );
}
