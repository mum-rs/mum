mod audio;
mod client;
mod command;
mod network;
mod notify;
mod state;

use futures_util::StreamExt;
//use ipc_channel::ipc::{self, IpcOneShotServer, IpcSender};
use log::*;
use mumlib::command::{Command, CommandResponse};
use mumlib::setup_logger;
use tokio::{join, net::UnixListener, sync::{mpsc, oneshot}};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use bytes::{BufMut, BytesMut};

#[tokio::main]
async fn main() {
    if std::env::args().find(|s| s.as_str() == "--version").is_some() {
        println!("mumd {}", env!("VERSION"));
        return;
    }

    setup_logger(std::io::stderr(), true);
    notify::init();

    // check if another instance is live
    /*let (tx_client, rx_client) =
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
    }*/

    let (command_sender, command_receiver) = mpsc::unbounded_channel();

    join!(
        client::handle(command_receiver),
        receive_commands(command_sender),
    );
}

async fn receive_commands(
    command_sender: mpsc::UnboundedSender<(
        Command,
        oneshot::Sender<mumlib::error::Result<Option<CommandResponse>>>,
    )>,
) {
    let socket = UnixListener::bind("/tmp/mumd").unwrap();

    loop {
        if let Ok((incoming, _)) = socket.accept().await {
            let (reader, writer) = incoming.into_split();
            let reader = FramedRead::new(reader, LengthDelimitedCodec::new());
            let writer = FramedWrite::new(writer, LengthDelimitedCodec::new());

            reader.filter_map(|buf| async {
                buf.ok()
            })
            .map(|buf| bincode::deserialize::<Command>(&buf).unwrap())
            .filter_map(|command| async {
                let (tx, rx) = oneshot::channel();

                command_sender.send((command, tx)).unwrap();

                let response = rx.await.unwrap();
                let mut serialized = BytesMut::new();
                bincode::serialize_into((&mut serialized).writer(), &response).unwrap();
                Some(Ok(serialized.freeze()))
            }).forward(writer).await.unwrap();
        }
    }
}