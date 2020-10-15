use crate::state::{State, StatePhase};

use ipc_channel::ipc::IpcSender;
use log::*;
use mumlib::command::{Command, CommandResponse};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

pub async fn handle(
    state: Arc<Mutex<State>>,
    mut command_receiver: mpsc::UnboundedReceiver<(Command, IpcSender<mumlib::error::Result<Option<CommandResponse>>>)>,
) {
    debug!("Begin listening for commands");
    while let Some(command) = command_receiver.recv().await {
        debug!("Received command {:?}", command.0);
        let mut state = state.lock().unwrap();
        let (wait_for_connected, command_response) = state.handle_command(command.0).await;
        if wait_for_connected {
            let mut watcher = state.phase_receiver();
            drop(state);
            while !matches!(watcher.recv().await.unwrap(), StatePhase::Connected) {}
        }
        command.1.send(command_response).unwrap();
    }
    //TODO err if not connected
    //while let Some(command) = command_receiver.recv().await {
    //    debug!("Parsing command {:?}", command);
    //}

    //debug!("Finished handling commands");
}
