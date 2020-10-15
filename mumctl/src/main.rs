use ipc_channel::ipc::{self, IpcSender};
use log::*;
use mumlib::command::{Command, CommandResponse};
use mumlib::setup_logger;
use std::fs;

fn main() {
    setup_logger();

    // MUMCTL
    //temp send command and channel to listener
    debug!("Creating channel");
    let (tx_client, rx_client) = ipc::channel::<mumlib::error::Result<Option<CommandResponse>>>().unwrap();

    let server_name = fs::read_to_string("/var/tmp/mumd-oneshot").unwrap(); //TODO don't panic
    debug!("Connecting to mumd at {}", server_name);
    let tx0 = IpcSender::connect(server_name).unwrap();
    let connect_command = Command::ServerConnect {
        host: "icahasse.se".to_string(),
        port: 64738u16,
        username: "gustav-mumd".to_string(),
        accept_invalid_cert: true,
    };
    debug!("Sending {:?} to mumd", connect_command);
    tx0.send((
        connect_command,
        tx_client)) .unwrap();

    debug!("Reading response");
    let response = rx_client.recv().unwrap();
    debug!("{:?}", response);
}
