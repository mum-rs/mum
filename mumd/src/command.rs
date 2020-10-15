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
    loop {
        debug!("Enter loop");
        let command = command_receiver.recv().await.unwrap();
        debug!("Received command {:?}", command.0);
        let mut state = state.lock().unwrap();
        debug!("Got mutex lock");
        let (wait_for_connected, command_response) = state.handle_command(command.0).await;
        if wait_for_connected {
            let mut watcher = state.phase_receiver();
            drop(state);
            debug!("Waiting to be connected");
            while !matches!(watcher.recv().await.unwrap(), StatePhase::Connected) {}
        }
        debug!("Sending response");
        command.1.send(command_response).unwrap();
        debug!("Sent response");
    }
    //TODO err if not connected
    //while let Some(command) = command_receiver.recv().await {
    //    debug!("Parsing command {:?}", command);
    //}

    //debug!("Finished handling commands");
}
