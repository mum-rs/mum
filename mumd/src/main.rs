mod audio;
mod client;
mod command;
mod network;
mod notify;
mod state;

use futures::join;
use ipc_channel::ipc::{self, IpcOneShotServer, IpcSender};
use log::*;
use mumlib::command::{Command, CommandResponse};
use mumlib::setup_logger;
use std::fs;
use tokio::sync::mpsc;
use tokio::task::spawn_blocking;

#[tokio::main(worker_threads = 4)]
async fn main() {
    setup_logger(std::io::stderr(), true);
    notify::init();

    // check if another instance is live
    let (tx_client, rx_client) =
        ipc::channel::<mumlib::error::Result<Option<CommandResponse>>>().unwrap();
    if let Ok(server_name) = fs::read_to_string(mumlib::SOCKET_PATH) {
        if let Ok(tx0) = IpcSender::connect(server_name) {
            if tx0.send((Command::Ping, tx_client)).is_ok() {
                match rx_client.recv() {
                    Ok(Ok(Some(CommandResponse::Pong))) => {
                        error!("Another instance of mumd is already running");
                        return;
                    },
                    resp => {
                        warn!("Ping with weird response. Continuing...");
                        debug!("Response was {:?}", resp);
                    }
                }
            }
        }
    }

    let (command_sender, command_receiver) = mpsc::unbounded_channel::<(
        Command,
        IpcSender<mumlib::error::Result<Option<CommandResponse>>>,
    )>();

    let (_, e) = join!(
        client::handle(command_receiver),
        spawn_blocking(move || {
            // IpcSender is blocking
            receive_oneshot_commands(command_sender);
        }),
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
