use crate::audio::VoiceStream;
use crate::network::ConnectionInfo;
use crate::state::{State, StatePhase};
use log::*;

use futures::{join, pin_mut, select, FutureExt, SinkExt, StreamExt};
use futures_util::stream::{SplitSink, SplitStream};
use mumble_protocol::control::{msgs, ClientControlCodec, ControlCodec, ControlPacket};
use mumble_protocol::crypt::ClientCryptState;
use mumble_protocol::voice::VoicePacket;
use mumble_protocol::{Clientbound, Serverbound};
use std::cell::RefCell;
use std::collections::HashMap;
use std::convert::{Into, TryInto};
use std::future::Future;
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{self, Duration};
use tokio_tls::{TlsConnector, TlsStream};
use tokio_util::codec::{Decoder, Framed};

type TcpSender = SplitSink<
    Framed<TlsStream<TcpStream>, ControlCodec<Serverbound, Clientbound>>,
    ControlPacket<Serverbound>,
>;
type TcpReceiver =
    SplitStream<Framed<TlsStream<TcpStream>, ControlCodec<Serverbound, Clientbound>>>;

pub(crate) type TcpEventCallback = Box<dyn FnOnce(TcpEventData)>;

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum TcpEvent {
    Connected,    //fires when the client has connected to a server
    Disconnected, //fires when the client has disconnected from a server
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
    phase_watcher: watch::Receiver<StatePhase>,
) {
    let interval = Rc::new(RefCell::new(time::interval(Duration::from_secs(
        delay_seconds,
    ))));
    let packet_sender = Rc::new(RefCell::new(packet_sender));

    run_until_disconnection(
        || async { Some(interval.borrow_mut().tick().await) },
        |_| async {
            trace!("Sending ping");
            let msg = msgs::Ping::new();
            packet_sender.borrow_mut().send(msg.into()).unwrap();
        },
        || async {},
        phase_watcher,
    )
    .await;

    debug!("Ping sender process killed");
}

async fn send_packets(
    sink: TcpSender,
    packet_receiver: &mut mpsc::UnboundedReceiver<ControlPacket<Serverbound>>,
    phase_watcher: watch::Receiver<StatePhase>,
) {
    let sink = Rc::new(RefCell::new(sink));
    let packet_receiver = Rc::new(RefCell::new(packet_receiver));
    run_until_disconnection(
        || async { packet_receiver.borrow_mut().recv().await },
        |packet| async {
            sink.borrow_mut().send(packet).await.unwrap();
        },
        || async {
            //clears queue of remaining packets
            while packet_receiver.borrow_mut().try_recv().is_ok() {}

            sink.borrow_mut().close().await.unwrap();
        },
        phase_watcher,
    )
    .await;

    debug!("TCP packet sender killed");
}

async fn listen(
    state: Arc<Mutex<State>>,
    stream: TcpReceiver,
    crypt_state_sender: mpsc::Sender<ClientCryptState>,
    event_queue: Arc<Mutex<HashMap<TcpEvent, Vec<TcpEventCallback>>>>,
    phase_watcher: watch::Receiver<StatePhase>,
) {
    let crypt_state = Rc::new(RefCell::new(None));
    let crypt_state_sender = Rc::new(RefCell::new(Some(crypt_state_sender)));

    let stream = Rc::new(RefCell::new(stream));
    run_until_disconnection(
        || async { stream.borrow_mut().next().await },
        |packet| async {
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
                    *crypt_state.borrow_mut() = Some(ClientCryptState::new_from(
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
                    if let Some(mut sender) = crypt_state_sender.borrow_mut().take() {
                        let _ = sender
                            .send(
                                crypt_state
                                    .borrow_mut()
                                    .take()
                                    .expect("Server didn't send us any CryptSetup packet!"),
                            )
                            .await;
                    }
                    if let Some(vec) = event_queue.lock().unwrap().get_mut(&TcpEvent::Connected) {
                        let old = std::mem::take(vec);
                        for handler in old {
                            handler(TcpEventData::Connected(&msg));
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
                    state.lock().unwrap().parse_user_state(*msg);
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
                ControlPacket::UDPTunnel(msg) => {
                    match *msg {
                        VoicePacket::Ping { .. } => {
                            //TODO handle tcp/udp
                        }
                        VoicePacket::Audio {
                            session_id,
                            // seq_num,
                            payload,
                            // position_info,
                            ..
                        } => {
                            state
                                .lock()
                                .unwrap()
                                .audio()
                                .decode_packet(VoiceStream::TCP, session_id, payload);
                        }
                    }
                }
                packet => {
                    debug!("Received unhandled ControlPacket {:#?}", packet);
                }
            }
        },
        || async {
            if let Some(vec) = event_queue.lock().unwrap().get_mut(&TcpEvent::Disconnected) {
                let old = std::mem::take(vec);
                for handler in old {
                    handler(TcpEventData::Disconnected);
                }
            }
        },
        phase_watcher,
    )
    .await;

    debug!("Killing TCP listener block");
}

async fn register_events(
    tcp_event_register_receiver: &mut mpsc::UnboundedReceiver<(TcpEvent, TcpEventCallback)>,
    event_data: Arc<Mutex<HashMap<TcpEvent, Vec<TcpEventCallback>>>>,
    phase_watcher: watch::Receiver<StatePhase>,
) {
    let tcp_event_register_receiver = Rc::new(RefCell::new(tcp_event_register_receiver));
    run_until_disconnection(
        || async { tcp_event_register_receiver.borrow_mut().recv().await },
        |(event, handler)| async {
            event_data
                .lock()
                .unwrap()
                .entry(event)
                .or_default()
                .push(handler);
        },
        || async {},
        phase_watcher,
    )
    .await;
}

async fn run_until_disconnection<T, F, G, H>(
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
