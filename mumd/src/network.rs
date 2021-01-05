pub mod tcp;
pub mod udp;

use std::net::SocketAddr;

use futures::Future;
use futures::FutureExt;
use futures::channel::oneshot;
use futures::join;
use futures::pin_mut;
use futures::select;
use tokio::sync::watch;

use crate::state::StatePhase;

#[derive(Clone, Debug)]
pub struct ConnectionInfo {
    socket_addr: SocketAddr,
    hostname: String,
    accept_invalid_cert: bool,
}

impl ConnectionInfo {
    pub fn new(socket_addr: SocketAddr, hostname: String, accept_invalid_cert: bool) -> Self {
        Self {
            socket_addr,
            hostname,
            accept_invalid_cert,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum VoiceStreamType {
    TCP,
    UDP,
}

async fn run_until<T, F, G, H>(
    phase_checker: impl Fn(StatePhase) -> bool,
    mut generator: impl FnMut() -> F,
    mut handler: impl FnMut(T) -> G,
    mut shutdown: impl FnMut() -> H,
    mut phase_watcher: watch::Receiver<StatePhase>,
) where
    F: Future<Output = Option<T>>,
    G: Future<Output = ()>,
    H: Future<Output = ()>,
{
    let (tx, rx) = oneshot::channel();
    let phase_transition_block = async {
        loop {
            phase_watcher.changed().await.unwrap();
            if phase_checker(*phase_watcher.borrow()) {
                break;
            }
        }
        tx.send(true).unwrap();
    };

    let main_block = async {
        let rx = rx.fuse();
        pin_mut!(rx);
        loop {
            let packet_recv = generator().fuse();
            pin_mut!(packet_recv);
            let exitor = select! {
                data = packet_recv => Some(data),
                _ = rx => None
            };
            match exitor {
                None => {
                    break;
                }
                Some(None) => {
                    //warn!("Channel closed before disconnect command"); //TODO make me informative
                    break;
                }
                Some(Some(data)) => {
                    handler(data).await;
                }
            }
        }

        shutdown().await;
    };

    join!(main_block, phase_transition_block);
}
