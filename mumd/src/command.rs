use crate::network::{
    ConnectionInfo,
    tcp::{TcpEvent, TcpEventCallback},
    udp::PingRequest
};
use crate::state::{ExecutionContext, State};

use log::*;
use mumble_protocol::{Serverbound, control::ControlPacket};
use mumlib::command::{Command, CommandResponse};
use std::sync::{atomic::{AtomicU64, Ordering}, Arc, RwLock};
use tokio::sync::{mpsc, oneshot, watch};

pub async fn handle(
    state: Arc<RwLock<State>>,
    mut command_receiver: mpsc::UnboundedReceiver<(
        Command,
        oneshot::Sender<mumlib::error::Result<Option<CommandResponse>>>,
    )>,
    tcp_event_register_sender: mpsc::UnboundedSender<(TcpEvent, TcpEventCallback)>,
    ping_request_sender: mpsc::UnboundedSender<PingRequest>,
    mut packet_sender: mpsc::UnboundedSender<ControlPacket<Serverbound>>,
    mut connection_info_sender: watch::Sender<Option<ConnectionInfo>>,
) {
    debug!("Begin listening for commands");
    let ping_count = AtomicU64::new(0);
    while let Some((command, response_sender)) = command_receiver.recv().await {
        debug!("Received command {:?}", command);
        let mut state = state.write().unwrap();
        let event = state.handle_command(command, &mut packet_sender, &mut connection_info_sender);
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
                let ret = generator();
                debug!("Ping generated: {:?}", ret);
                match ret {
                    Ok(addr) => {
                        let id = ping_count.fetch_add(1, Ordering::Relaxed);
                        let res = ping_request_sender.send((
                            id,
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
