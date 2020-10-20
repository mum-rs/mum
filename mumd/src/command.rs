use crate::state::State;

use ipc_channel::ipc::IpcSender;
use log::*;
use mumlib::command::{Command, CommandResponse};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use crate::network::tcp::{TcpEvent, TcpEventCallback};

pub async fn handle(
    state: Arc<Mutex<State>>,
    mut command_receiver: mpsc::UnboundedReceiver<(
        Command,
        IpcSender<mumlib::error::Result<Option<CommandResponse>>>,
    )>,
    tcp_event_register_sender: mpsc::UnboundedSender<(TcpEvent, TcpEventCallback)>,
) {
    debug!("Begin listening for commands");
    while let Some((command, response_sender)) = command_receiver.recv().await {
        debug!("Received command {:?}", command);
        let mut statee = state.lock().unwrap();
        let (event_data, command_response) = statee.handle_command(command).await;
        drop(statee);
        if let Some((event, callback)) = event_data {
            let (tx, rx) = oneshot::channel();
            tcp_event_register_sender.send((event, Box::new(move |e| {
                println!("något hände");
                callback(e);
                response_sender.send(command_response).unwrap();
                tx.send(());
            })));

            rx.await;
        } else {
            response_sender.send(command_response).unwrap();
        }
    }
}
