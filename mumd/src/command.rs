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
        let mut state = state.lock().unwrap();
        let (event, generator) = state.handle_command(command).await;
        drop(state);
        if let Some(event) = event {
            let (tx, rx) = oneshot::channel();
            //TODO handle this error
            let _ = tcp_event_register_sender.send((event, Box::new(move |e| {
                let response = generator(Some(e));
                response_sender.send(response).unwrap();
                tx.send(()).unwrap();
            })));

            rx.await.unwrap();
        } else {
            response_sender.send(generator(None)).unwrap();
        }
    }
}
