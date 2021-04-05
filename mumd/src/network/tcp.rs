use crate::error::{ServerSendError, TcpError};
use crate::network::ConnectionInfo;
use crate::state::{State, StatePhase};
use log::*;

use futures_util::{FutureExt, SinkExt, StreamExt};
use futures_util::select;
use futures_util::stream::{SplitSink, SplitStream, Stream};
use mumble_protocol::control::{msgs, ClientControlCodec, ControlCodec, ControlPacket};
use mumble_protocol::crypt::ClientCryptState;
use mumble_protocol::voice::VoicePacket;
use mumble_protocol::{Clientbound, Serverbound};
use std::collections::HashMap;
use std::convert::{Into, TryInto};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch, Mutex};
use tokio::time::{self, Duration};
use tokio_native_tls::{TlsConnector, TlsStream};
use tokio_util::codec::{Decoder, Framed};

use super::{run_until, VoiceStreamType};

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

#[derive(Clone)]
pub enum TcpEventData<'a> {
    Connected(Result<&'a msgs::ServerSync, mumlib::Error>),
    Disconnected,
}

impl<'a> From<&TcpEventData<'a>> for TcpEvent {
    fn from(t: &TcpEventData) -> Self {
        match t {
            TcpEventData::Connected(_) => TcpEvent::Connected,
            TcpEventData::Disconnected => TcpEvent::Disconnected,
        }
    }
}

#[derive(Clone)]
struct TcpEventQueue {
    handlers: Arc<Mutex<HashMap<TcpEvent, Vec<TcpEventCallback>>>>,
}

impl TcpEventQueue {
    fn new() -> Self {
        Self {
            handlers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn register(&self, at: TcpEvent, callback: TcpEventCallback) {
        self.handlers.lock().await.entry(at).or_default().push(callback);
    }

    async fn resolve<'a>(&self, data: TcpEventData<'a>) {
        if let Some(vec) = self.handlers.lock().await.get_mut(&TcpEvent::from(&data)) {
            let old = std::mem::take(vec);
            for handler in old {
                handler(data.clone());
            }
        }
    }
}

pub async fn handle(
    state: Arc<Mutex<State>>,
    mut connection_info_receiver: watch::Receiver<Option<ConnectionInfo>>,
    crypt_state_sender: mpsc::Sender<ClientCryptState>,
    packet_sender: mpsc::UnboundedSender<ControlPacket<Serverbound>>,
    mut packet_receiver: mpsc::UnboundedReceiver<ControlPacket<Serverbound>>,
    mut tcp_event_register_receiver: mpsc::UnboundedReceiver<(TcpEvent, TcpEventCallback)>,
) -> Result<(), TcpError> {
    loop {
        let connection_info = 'data: loop {
            while connection_info_receiver.changed().await.is_ok() {
                if let Some(data) = connection_info_receiver.borrow().clone() {
                    break 'data data;
                }
            }
            return Err(TcpError::NoConnectionInfoReceived);
        };
        let (mut sink, stream) = connect(
            connection_info.socket_addr,
            connection_info.hostname,
            connection_info.accept_invalid_cert,
        )
        .await?;

        // Handshake (omitting `Version` message for brevity)
        let state_lock = state.lock().await;
        let username = state_lock.username().unwrap().to_string();
        let password = state_lock.password().map(|x| x.to_string());
        authenticate(&mut sink, username, password).await?;
        let phase_watcher = state_lock.phase_receiver();
        let input_receiver = state_lock.audio_input().receiver();
        drop(state_lock);
        let event_queue = TcpEventQueue::new();

        info!("Logging in...");

        let phase_watcher_inner = phase_watcher.clone();

        run_until(
            |phase| matches!(phase, StatePhase::Disconnected),
            async {
                select! {
                    r = send_pings(packet_sender.clone(), 10).fuse() => r,
                    r = listen(
                        Arc::clone(&state),
                        stream,
                        crypt_state_sender.clone(),
                        event_queue.clone(),
                    ).fuse() => r,
                    r = send_voice(
                        packet_sender.clone(),
                        Arc::clone(&input_receiver),
                        phase_watcher_inner,
                    ).fuse() => r,
                    r = send_packets(sink, &mut packet_receiver).fuse() => r,
                    _ = register_events(&mut tcp_event_register_receiver, event_queue.clone()).fuse() => Ok(()),
                }
            },
            phase_watcher,
        ).await.unwrap_or(Ok(()))?;

        event_queue.resolve(TcpEventData::Disconnected).await;

        debug!("Fully disconnected TCP stream, waiting for new connection info");
    }
}

async fn connect(
    server_addr: SocketAddr,
    server_host: String,
    accept_invalid_cert: bool,
) -> Result<(TcpSender, TcpReceiver), TcpError> {
    let stream = TcpStream::connect(&server_addr).await?;
    debug!("TCP connected");

    let mut builder = native_tls::TlsConnector::builder();
    builder.danger_accept_invalid_certs(accept_invalid_cert);
    let connector: TlsConnector = builder
        .build()
        .map_err(|e| TcpError::TlsConnectorBuilderError(e))?
        .into();
    let tls_stream = connector
        .connect(&server_host, stream)
        .await
        .map_err(|e| TcpError::TlsConnectError(e))?;
    debug!("TLS connected");

    // Wrap the TLS stream with Mumble's client-side control-channel codec
    Ok(ClientControlCodec::new().framed(tls_stream).split())
}

async fn authenticate(
    sink: &mut TcpSender,
    username: String,
    password: Option<String>
) -> Result<(), TcpError> {
    let mut msg = msgs::Authenticate::new();
    msg.set_username(username);
    if let Some(password) = password {
        msg.set_password(password);
    }
    msg.set_opus(true);
    sink.send(msg.into()).await?;
    Ok(())
}

async fn send_pings(
    packet_sender: mpsc::UnboundedSender<ControlPacket<Serverbound>>,
    delay_seconds: u64,
) -> Result<(), TcpError> {
    let mut interval = time::interval(Duration::from_secs(delay_seconds));
    loop {
        interval.tick().await;
        trace!("Sending TCP ping");
        let msg = msgs::Ping::new();
        packet_sender.send(msg.into())?;
    }
}

async fn send_packets(
    mut sink: TcpSender,
    packet_receiver: &mut mpsc::UnboundedReceiver<ControlPacket<Serverbound>>,
) -> Result<(), TcpError> {
    loop {
        // Safe since we always have at least one sender alive.
        let packet = packet_receiver.recv().await.unwrap();
        sink.send(packet).await?;
    }
}

async fn send_voice(
    packet_sender: mpsc::UnboundedSender<ControlPacket<Serverbound>>,
    receiver: Arc<Mutex<Box<(dyn Stream<Item = VoicePacket<Serverbound>> + Unpin)>>>,
    phase_watcher: watch::Receiver<StatePhase>,
) -> Result<(), TcpError> {
    loop {
        let mut inner_phase_watcher = phase_watcher.clone();
        loop {
            inner_phase_watcher.changed().await.unwrap();
            if matches!(*inner_phase_watcher.borrow(), StatePhase::Connected(VoiceStreamType::TCP)) {
                break;
            }
        }
        run_until(
            |phase| !matches!(phase, StatePhase::Connected(VoiceStreamType::TCP)),
            async {
                loop {
                    packet_sender.send(
                        receiver
                            .lock()
                            .await
                            .next()
                            .await
                            .expect("No audio stream")
                            .into())?;
                }
            },
            inner_phase_watcher.clone(),
        ).await.unwrap_or(Ok::<(), ServerSendError>(()))?;
    }
}

async fn listen(
    state: Arc<Mutex<State>>,
    mut stream: TcpReceiver,
    crypt_state_sender: mpsc::Sender<ClientCryptState>,
    event_queue: TcpEventQueue,
) -> Result<(), TcpError> {
    let mut crypt_state = None;
    let mut crypt_state_sender = Some(crypt_state_sender);

    loop {
        let packet = match stream.next().await {
            Some(Ok(packet)) => packet,
            Some(Err(e)) => {
                error!("TCP error: {:?}", e);
                continue; //TODO Break here? Maybe look at the error and handle it
            }
            None => {
                // We end up here if the login was rejected. We probably want
                // to exit before that.
                warn!("TCP stream gone");
                state.lock().await.broadcast_phase(StatePhase::Disconnected);
                break;
            }
        };
        match packet {
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
                if let Some(sender) = crypt_state_sender.take() {
                    let _ = sender
                        .send(
                            crypt_state
                                .take()
                                .expect("Server didn't send us any CryptSetup packet!"),
                        )
                        .await;
                }
                event_queue.resolve(TcpEventData::Connected(Ok(&msg))).await;
                let mut state = state.lock().await;
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
                debug!("Login rejected: {:?}", msg);
                match msg.get_field_type() {
                    msgs::Reject_RejectType::WrongServerPW => {
                        event_queue.resolve(TcpEventData::Connected(Err(mumlib::Error::InvalidServerPassword))).await;
                    }
                    ty => {
                        warn!("Unhandled reject type: {:?}", ty);
                    }
                }
            }
            ControlPacket::UserState(msg) => {
                state.lock().await.parse_user_state(*msg);
            }
            ControlPacket::UserRemove(msg) => {
                state.lock().await.remove_client(*msg);
            }
            ControlPacket::ChannelState(msg) => {
                debug!("Channel state received");
                state
                    .lock()
                    .await
                    .server_mut()
                    .unwrap()
                    .parse_channel_state(*msg); //TODO parse initial if initial
            }
            ControlPacket::ChannelRemove(msg) => {
                state
                    .lock()
                    .await
                    .server_mut()
                    .unwrap()
                    .parse_channel_remove(*msg);
            }
            ControlPacket::UDPTunnel(msg) => {
                match *msg {
                    VoicePacket::Ping { .. } => {}
                    VoicePacket::Audio {
                        session_id,
                        // seq_num,
                        payload,
                        // position_info,
                        ..
                    } => {
                        state
                            .lock()
                            .await
                            .audio_output()
                            .decode_packet_payload(
                                VoiceStreamType::TCP,
                                session_id,
                                payload);
                    }
                }
            }
            packet => {
                debug!("Received unhandled ControlPacket {:#?}", packet);
            }
        }
    }
    Ok(())
}

async fn register_events(
    tcp_event_register_receiver: &mut mpsc::UnboundedReceiver<(TcpEvent, TcpEventCallback)>,
    event_queue: TcpEventQueue,
) {
    loop {
        let (event, handler) = tcp_event_register_receiver.recv().await.unwrap();
        event_queue.register(event, handler).await;
    }
}
