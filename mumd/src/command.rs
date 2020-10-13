use crate::state::State;

use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

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

pub async fn handle(
    state: Arc<Mutex<State>>,
    mut command_receiver: mpsc::UnboundedReceiver<Command>,
) {
    // wait until we can send packages
    let mut initialized_receiver = state.lock().unwrap().initialized_receiver();
    while matches!(initialized_receiver.recv().await, Some(false)) {}

    while let Some(command) = command_receiver.recv().await {
        state.lock().unwrap().handle_command(command).await;
    }
}
