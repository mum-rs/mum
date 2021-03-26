mod audio;
mod client;
mod command;
mod network;
mod notify;
mod state;

use futures_util::{SinkExt, StreamExt};
//use ipc_channel::ipc::{self, IpcOneShotServer, IpcSender};
use log::*;
use mumlib::command::{Command, CommandResponse};
use mumlib::setup_logger;
use tokio::{join, net::{UnixListener, UnixStream}, sync::{mpsc, oneshot}};
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
    let connection = UnixStream::connect("/tmp/mumd").await;
    match connection {
        Ok(stream) => {
            let (reader, writer) = stream.into_split();
            let mut reader = FramedRead::new(reader, LengthDelimitedCodec::new());
            let mut writer = FramedWrite::new(writer, LengthDelimitedCodec::new());
            let mut command = BytesMut::new();
            bincode::serialize_into((&mut command).writer(), &Command::Ping).unwrap();
            if let Ok(()) = writer.send(command.freeze()).await {
                if let Some(Ok(buf)) = reader.next().await {
                    if let Ok(Ok::<Option<CommandResponse>, mumlib::error::Error>(Some(CommandResponse::Pong))) = bincode::deserialize(&buf) {
                        error!("Another instance of mumd is already running");
                        return;
                    }
                }
            }
            debug!("a dead socket was found, removing");
            tokio::fs::remove_file("/tmp/mumd").await.unwrap();
        }
        Err(e) => {
            if matches!(e.kind(), std::io::ErrorKind::ConnectionRefused) {
                debug!("a dead socket was found, removing");
                tokio::fs::remove_file("/tmp/mumd").await.unwrap();
            }
        }
    }

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
            .map(|buf| bincode::deserialize::<Command>(&buf))
            .filter_map(|e| async { e.ok() })
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