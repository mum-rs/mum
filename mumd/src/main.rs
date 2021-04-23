mod audio;
mod command;
mod error;
mod network;
mod notifications;
mod state;

use crate::state::State;
use crate::network::{tcp, udp};

use futures_util::{SinkExt, StreamExt};
use log::*;
use mumlib::command::{Command, CommandResponse};
use mumlib::setup_logger;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::net::UnixStream;
use tokio::select;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use bytes::{BufMut, BytesMut};

#[tokio::main]
async fn main() {
    if std::env::args().find(|s| s.as_str() == "--version").is_some() {
        println!("mumd {}", env!("VERSION"));
        return;
    }

    setup_logger(std::io::stderr(), true);
    notifications::init();

    // check if another instance is live
    let connection = UnixStream::connect(mumlib::SOCKET_PATH).await;
    match connection {
        Ok(stream) => {
            let (reader, writer) = stream.into_split();
            let mut reader = FramedRead::new(reader, LengthDelimitedCodec::new());
            let mut writer = FramedWrite::new(writer, LengthDelimitedCodec::new());
            let mut command = BytesMut::new();
            bincode::serialize_into((&mut command).writer(), &Command::Ping).unwrap();
            if let Ok(()) = writer.send(command.freeze()).await {
                if let Some(Ok(buf)) = reader.next().await {
                    if let Ok(Ok::<Option<CommandResponse>, mumlib::Error>(Some(CommandResponse::Pong))) = bincode::deserialize(&buf) {
                        error!("Another instance of mumd is already running");
                        return;
                    }
                }
            }
            debug!("a dead socket was found, removing");
            tokio::fs::remove_file(mumlib::SOCKET_PATH).await.unwrap();
        }
        Err(e) => {
            if matches!(e.kind(), std::io::ErrorKind::ConnectionRefused) {
                debug!("a dead socket was found, removing");
                tokio::fs::remove_file(mumlib::SOCKET_PATH).await.unwrap();
            }
        }
    }

    let state = match State::new(None).await {
        Ok(s) => s,
        Err(e) => {
            error!("Error instantiating mumd: {}", e);
            return;
        }
    };

    let state = Arc::new(RwLock::new(state));

    select! {
        _ = tcp::handle(Arc::clone(&state)) => unimplemented!(),
        _ = udp::handle(Arc::clone(&state)) => unimplemented!(),
        _ = audio::input::handle(Arc::clone(&state)) => unimplemented!(),
        _ = audio::output::handle(Arc::clone(&state)) => unimplemented!(),
        _ = command::receive_commands(Arc::clone(&state)) => unimplemented!(),
    };
}
