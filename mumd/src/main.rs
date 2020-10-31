mod audio;
mod command;
mod network;
mod notify;
mod state;

use crate::network::ConnectionInfo;
use crate::state::State;

use futures::join;
use ipc_channel::ipc::{IpcOneShotServer, IpcSender};
use log::*;
use mumble_protocol::control::ControlPacket;
use mumble_protocol::crypt::ClientCryptState;
use mumble_protocol::voice::Serverbound;
use mumlib::command::{Command, CommandResponse};
use mumlib::setup_logger;
use std::fs;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};
use tokio::task::spawn_blocking;

#[tokio::main]
async fn main() {
    setup_logger(std::io::stderr(), true);
    notify::init();

    // Oneshot channel for setting UDP CryptState from control task
    // For simplicity we don't deal with re-syncing, real applications would have to.
    let (crypt_state_sender, crypt_state_receiver) = mpsc::channel::<ClientCryptState>(1); // crypt state should always be consumed before sending a new one
    let (packet_sender, packet_receiver) = mpsc::unbounded_channel::<ControlPacket<Serverbound>>();
    let (command_sender, command_receiver) = mpsc::unbounded_channel::<(
        Command,
        IpcSender<mumlib::error::Result<Option<CommandResponse>>>,
    )>();
    let (connection_info_sender, connection_info_receiver) =
        watch::channel::<Option<ConnectionInfo>>(None);
    let (response_sender, response_receiver) = mpsc::unbounded_channel();
    let (ping_request_sender, ping_request_receiver) = mpsc::unbounded_channel();

    let state = State::new(packet_sender, connection_info_sender);
    let state = Arc::new(Mutex::new(state));

    let (_, _, _, e, _) = join!(
        network::tcp::handle(
            Arc::clone(&state),
            connection_info_receiver.clone(),
            crypt_state_sender,
            packet_receiver,
            response_receiver,
        ),
        network::udp::handle(
            Arc::clone(&state),
            connection_info_receiver.clone(),
            crypt_state_receiver,
        ),
        command::handle(
            state,
            command_receiver,
            response_sender,
            ping_request_sender,
        ),
        spawn_blocking(move || {
            // IpcSender is blocking
            receive_oneshot_commands(command_sender);
        }),
        network::udp::handle_pings(
            ping_request_receiver
        ),
    );
    e.unwrap();
}

fn receive_oneshot_commands(
    command_sender: mpsc::UnboundedSender<(
        Command,
        IpcSender<mumlib::error::Result<Option<CommandResponse>>>,
    )>,
) {
    loop {
        // create listener
        let (server, server_name): (
            IpcOneShotServer<(
                Command,
                IpcSender<mumlib::error::Result<Option<CommandResponse>>>,
            )>,
            String,
        ) = IpcOneShotServer::new().unwrap();
        fs::write(mumlib::SOCKET_PATH, &server_name).unwrap();
        debug!("Listening to {}", server_name);

        // receive command and response channel
        let (_, conn): (
            _,
            (
                Command,
                IpcSender<mumlib::error::Result<Option<CommandResponse>>>,
            ),
        ) = server.accept().unwrap();
        debug!("Sending command {:?} to command handler", conn.0);
        command_sender.send(conn).unwrap();
    }
}
