use crate::network::ConnectionInfo;
use crate::state::{State, StatePhase};
use log::*;

use futures::{join, pin_mut, select, FutureExt, SinkExt, StreamExt};
use futures_util::stream::{SplitSink, SplitStream};
use mumble_protocol::control::{msgs, ClientControlCodec, ControlCodec, ControlPacket};
use mumble_protocol::crypt::ClientCryptState;
use mumble_protocol::{Clientbound, Serverbound};
use std::convert::{Into, TryInto};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{self, Duration};
use tokio_tls::{TlsConnector, TlsStream};
use tokio_util::codec::{Decoder, Framed};
use std::collections::HashMap;

type TcpSender = SplitSink<
    Framed<TlsStream<TcpStream>, ControlCodec<Serverbound, Clientbound>>,
    ControlPacket<Serverbound>,
>;
type TcpReceiver =
    SplitStream<Framed<TlsStream<TcpStream>, ControlCodec<Serverbound, Clientbound>>>;

pub(crate) type TcpEventCallback = Box<dyn FnOnce(&TcpEventData)>;

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum TcpEvent {
    Connected,
    Disconnected,
}

pub enum TcpEventData<'a> {
    Connected(&'a msgs::ServerSync),
    Disconnected,
}

pub async fn handle(
    state: Arc<Mutex<State>>,
    mut connection_info_receiver: watch::Receiver<Option<ConnectionInfo>>,
    crypt_state_sender: mpsc::Sender<ClientCryptState>,
    mut packet_receiver: mpsc::UnboundedReceiver<ControlPacket<Serverbound>>,
    mut tcp_event_register_receiver: mpsc::UnboundedReceiver<(TcpEvent, TcpEventCallback)>,
) {
    loop {
        let connection_info = loop {
            match connection_info_receiver.recv().await {
                None => {
                    return;
                }
                Some(None) => {}
                Some(Some(connection_info)) => {
                    break connection_info;
                }
            }
        };
        let (mut sink, stream) = connect(
            connection_info.socket_addr,
            connection_info.hostname,
            connection_info.accept_invalid_cert,
        )
        .await;

        // Handshake (omitting `Version` message for brevity)
        let state_lock = state.lock().unwrap();
        authenticate(&mut sink, state_lock.username().unwrap().to_string()).await;
        let phase_watcher = state_lock.phase_receiver();
        let packet_sender = state_lock.packet_sender();
        drop(state_lock);
        let event_queue = Arc::new(Mutex::new(HashMap::new()));

        info!("Logging in...");

        join!(
            send_pings(packet_sender, 10, phase_watcher.clone()),
            listen(
                Arc::clone(&state),
                stream,
                crypt_state_sender.clone(),
                Arc::clone(&event_queue),
                phase_watcher.clone(),
            ),
            send_packets(sink, &mut packet_receiver, phase_watcher.clone()),
            register_events(&mut tcp_event_register_receiver, event_queue, phase_watcher),
        );

        debug!("Fully disconnected TCP stream, waiting for new connection info");
    }
}

async fn connect(
    server_addr: SocketAddr,
    server_host: String,
    accept_invalid_cert: bool,
) -> (TcpSender, TcpReceiver) {
    let stream = TcpStream::connect(&server_addr)
        .await
        .expect("failed to connect to server:");
    debug!("TCP connected");

    let mut builder = native_tls::TlsConnector::builder();
    builder.danger_accept_invalid_certs(accept_invalid_cert);
    let connector: TlsConnector = builder
        .build()
        .expect("failed to create TLS connector")
        .into();
    let tls_stream = connector
        .connect(&server_host, stream)
        .await
        .expect("failed to connect TLS: {}");
    debug!("TLS connected");

    // Wrap the TLS stream with Mumble's client-side control-channel codec
    ClientControlCodec::new().framed(tls_stream).split()
}

async fn authenticate(sink: &mut TcpSender, username: String) {
    let mut msg = msgs::Authenticate::new();
    msg.set_username(username);
    msg.set_opus(true);
    sink.send(msg.into()).await.unwrap();
}

async fn send_pings(
    packet_sender: mpsc::UnboundedSender<ControlPacket<Serverbound>>,
    delay_seconds: u64,
    mut phase_watcher: watch::Receiver<StatePhase>,
) {
    let (tx, rx) = oneshot::channel();
    let phase_transition_block = async {
        while !matches!(
            phase_watcher.recv().await.unwrap(),
            StatePhase::Disconnected
        ) {}
        tx.send(true).unwrap();
    };

    let mut interval = time::interval(Duration::from_secs(delay_seconds));
    let main_block = async {
        let rx = rx.fuse();
        pin_mut!(rx);
        loop {
            let interval_waiter = interval.tick().fuse();
            pin_mut!(interval_waiter);
            let exitor = select! {
                data = interval_waiter => Some(data),
                _ = rx => None
            };

            match exitor {
                Some(_) => {
                    trace!("Sending ping");
                    let msg = msgs::Ping::new();
                    packet_sender.send(msg.into()).unwrap();
                }
                None => break,
            }
        }
    };

    join!(main_block, phase_transition_block);

    debug!("Ping sender process killed");
}

async fn send_packets(
    mut sink: TcpSender,
    packet_receiver: &mut mpsc::UnboundedReceiver<ControlPacket<Serverbound>>,
    mut phase_watcher: watch::Receiver<StatePhase>,
) {
    let (tx, rx) = oneshot::channel();
    let phase_transition_block = async {
        while !matches!(
            phase_watcher.recv().await.unwrap(),
            StatePhase::Disconnected
        ) {}
        tx.send(true).unwrap();
    };

    let main_block = async {
        let rx = rx.fuse();
        pin_mut!(rx);
        loop {
            let packet_recv = packet_receiver.recv().fuse();
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
                    warn!("Channel closed before disconnect command");
                    break;
                }
                Some(Some(packet)) => {
                    sink.send(packet).await.unwrap();
                }
            }
        }

        //clears queue of remaining packets
        while packet_receiver.try_recv().is_ok() {}

        sink.close().await.unwrap();
    };

    join!(main_block, phase_transition_block);

    debug!("TCP packet sender killed");
}

async fn listen(
    state: Arc<Mutex<State>>,
    mut stream: TcpReceiver,
    crypt_state_sender: mpsc::Sender<ClientCryptState>,
    event_data: Arc<Mutex<HashMap<TcpEvent, Vec<TcpEventCallback>>>>,
    mut phase_watcher: watch::Receiver<StatePhase>,
) {
    let mut crypt_state = None;
    let mut crypt_state_sender = Some(crypt_state_sender);

    let (tx, rx) = oneshot::channel();
    let phase_transition_block = async {
        while !matches!(
            phase_watcher.recv().await.unwrap(),
            StatePhase::Disconnected
        ) {}
        tx.send(true).unwrap();
    };

    let listener_block = async {
        let rx = rx.fuse();
        pin_mut!(rx);
        loop {
            let packet_recv = stream.next().fuse();
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
                    warn!("Channel closed before disconnect command");
                    break;
                }
                Some(Some(packet)) => {
                    match packet.unwrap() {
                        ControlPacket::TextMessage(msg) => {
                            info!(
                                "Got message from user with session ID {}: {}",
                                msg.get_actor(),
                                msg.get_message()
                            );
                        }
                        ControlPacket::CryptSetup(msg) => {
                            debug!("Crypt setup");
                            // Wait until we're fully connected before initiating UDP voice
                            crypt_state = Some(ClientCryptState::new_from(
                                msg.get_key()
                                    .try_into()
                                    .expect("Server sent private key with incorrect size"),
                                msg.get_client_nonce()
                                    .try_into()
                                    .expect("Server sent client_nonce with incorrect size"),
                                msg.get_server_nonce()
                                    .try_into()
                                    .expect("Server sent server_nonce with incorrect size"),
                            ));
                        }
                        ControlPacket::ServerSync(msg) => {
                            info!("Logged in");
                            if let Some(mut sender) = crypt_state_sender.take() {
                                let _ = sender
                                    .send(
                                        crypt_state
                                            .take()
                                            .expect("Server didn't send us any CryptSetup packet!"),
                                    )
                                    .await;
                            }
                            if let Some(vec) = event_data.lock().unwrap().get_mut(&TcpEvent::Connected) {
                                let old = std::mem::take(vec);
                                for handler in old {
                                    handler(&TcpEventData::Connected(&msg));
                                }
                            }
                            let mut state = state.lock().unwrap();
                            let server = state.server_mut().unwrap();
                            server.parse_server_sync(*msg);
                            match &server.welcome_text {
                                Some(s) => info!("Welcome: {}", s),
                                None => info!("No welcome received"),
                            }
                            for channel in server.channels().values() {
                                info!("Found channel {}", channel.name());
                            }
                            state.initialized();
                        }
                        ControlPacket::Reject(msg) => {
                            warn!("Login rejected: {:?}", msg);
                        }
                        ControlPacket::UserState(msg) => {
                            let mut state = state.lock().unwrap();
                            let session = msg.get_session();

                            let user_state_diff = state.parse_user_state(*msg);
                            //TODO do something with user state diff
                            debug!("user state diff: {:#?}", &user_state_diff);

                            let server = state.server_mut().unwrap();
                            let user = server.users().get(&session).unwrap();
                            info!("User {} connected to {}", user.name(), user.channel());
                        }
                        ControlPacket::UserRemove(msg) => {
                            state.lock().unwrap().remove_client(*msg);
                        }
                        ControlPacket::ChannelState(msg) => {
                            debug!("Channel state received");
                            state
                                .lock()
                                .unwrap()
                                .server_mut()
                                .unwrap()
                                .parse_channel_state(*msg); //TODO parse initial if initial
                        }
                        ControlPacket::ChannelRemove(msg) => {
                            state
                                .lock()
                                .unwrap()
                                .server_mut()
                                .unwrap()
                                .parse_channel_remove(*msg);
                        }
                        _ => {}
                    }
                }
            }
        }

        if let Some(vec) = event_data.lock().unwrap().get_mut(&TcpEvent::Disconnected) {
            let old = std::mem::take(vec);
            for handler in old {
                handler(&TcpEventData::Disconnected);
            }
        }

        //TODO? clean up stream
    };

    join!(phase_transition_block, listener_block);

    debug!("Killing TCP listener block");
}

async fn register_events(
    tcp_event_register_receiver: &mut mpsc::UnboundedReceiver<(TcpEvent, TcpEventCallback)>,
    event_data: Arc<Mutex<HashMap<TcpEvent, Vec<TcpEventCallback>>>>,
    mut phase_watcher: watch::Receiver<StatePhase>,
) {
    let (tx, rx) = oneshot::channel();
    let phase_transition_block = async {
        while !matches!(
            phase_watcher.recv().await.unwrap(),
            StatePhase::Disconnected
        ) {}
        tx.send(true).unwrap();
    };

    let main_block = async {
        let rx = rx.fuse();
        pin_mut!(rx);
        loop {
            let packet_recv = tcp_event_register_receiver.recv().fuse();
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
                    warn!("Channel closed before disconnect command");
                    break;
                }
                Some(Some((event, handler))) => {
                    event_data.lock().unwrap().entry(event).or_default().push(handler);
                }
            }
        }
    };

    join!(main_block, phase_transition_block);
}