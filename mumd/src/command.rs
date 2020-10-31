use crate::state::{ExecutionContext, State};

use crate::network::tcp::{TcpEvent, TcpEventCallback};
use ipc_channel::ipc::IpcSender;
use log::*;
use mumble_protocol::ping::PongPacket;
use mumlib::command::{Command, CommandResponse};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

pub async fn handle(
    state: Arc<Mutex<State>>,
    mut command_receiver: mpsc::UnboundedReceiver<(
        Command,
        IpcSender<mumlib::error::Result<Option<CommandResponse>>>,
    )>,
    tcp_event_register_sender: mpsc::UnboundedSender<(TcpEvent, TcpEventCallback)>,
    ping_request_sender: mpsc::UnboundedSender<(u64, SocketAddr, Box<dyn FnOnce(PongPacket)>)>,
) {
    debug!("Begin listening for commands");
    while let Some((command, response_sender)) = command_receiver.recv().await {
        debug!("Received command {:?}", command);
        let mut state = state.lock().unwrap();
        let event = state.handle_command(command);
        drop(state);
        match event {
            ExecutionContext::TcpEvent(event, generator) => {
                let (tx, rx) = oneshot::channel();
                //TODO handle this error
                let _ = tcp_event_register_sender.send((
                    event,
                    Box::new(move |e| {
                        let response = generator(e);
                        response_sender.send(response).unwrap();
                        tx.send(()).unwrap();
                    }),
                ));

                rx.await.unwrap();
            }
            ExecutionContext::Now(generator) => {
                response_sender.send(generator()).unwrap();
            }
            ExecutionContext::Ping(generator, converter) => {
                match generator() {
                    Ok(addr) => {
                        let res = ping_request_sender.send((
                            0,
                            addr,
                            Box::new(move |packet| {
                                response_sender.send(converter(packet)).unwrap();
                            }),
                        ));
                        if res.is_err() {
                            panic!();
                        }
                    }
                    Err(e) => {
                        response_sender.send(Err(e)).unwrap();
                    }
                };
            }
        }
    }
}
