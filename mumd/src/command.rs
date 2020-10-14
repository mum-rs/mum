use crate::state::{Channel, Server, State, StatePhase};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use log::*;

#[derive(Debug)]
pub enum Command {
    ChannelJoin {
        channel_id: u32,
    },
    ChannelList,
    ServerConnect {
        host: String,
        port: u16,
        username: String,
        accept_invalid_cert: bool, //TODO ask when connecting
    },
    ServerDisconnect,
    Status,
}

#[derive(Debug)]
pub enum CommandResponse {
    ChannelList {
        channels: HashMap<u32, Channel>,
    },
    Status {
        username: String,
        server_state: Server,
    }
}

pub async fn handle(
    state: Arc<Mutex<State>>,
    mut command_receiver: mpsc::UnboundedReceiver<Command>,
    command_response_sender: mpsc::UnboundedSender<Result<Option<CommandResponse>, ()>>,
) {
    //TODO err if not connected
    while let Some(command) = command_receiver.recv().await {
        debug!("Parsing command {:?}", command);
        let mut state = state.lock().unwrap();
        let (wait_for_connected, _) = state.handle_command(command).await;
        if wait_for_connected {
            let mut watcher = state.phase_receiver();
            drop(state);
            while !matches!(watcher.recv().await.unwrap(), StatePhase::Connected) {}
        }
    }
}
