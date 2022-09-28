use crate::error::{ServerSendError, TcpError};
use crate::network::ConnectionInfo;
use crate::notifications;
use crate::state::server::Server;
use crate::state::{State, StatePhase};

use futures_util::select;
use futures_util::stream::{SplitSink, SplitStream, Stream};
use futures_util::{FutureExt, SinkExt, StreamExt};
use log::*;
use mumble_protocol::control::{msgs, ClientControlCodec, ControlCodec, ControlPacket};
use mumble_protocol::crypt::ClientCryptState;
use mumble_protocol::voice::VoicePacket;
use mumble_protocol::{Clientbound, Serverbound};
use mumlib::command::MumbleEventKind;
use std::collections::HashMap;
use std::convert::Into;
use std::fmt::Debug;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
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

pub(crate) type TcpEventCallback = Box<dyn FnOnce(TcpEventData<'_>)>;
pub(crate) type TcpEventSubscriber = Box<dyn FnMut(TcpEventData<'_>) -> bool>; //the bool indicates if it should be kept or not

/// Why the TCP was disconnected.
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub enum DisconnectedReason {
    InvalidTls,
    User,
    TcpError,
}

/// Something a callback can register to. Data is sent via a respective [TcpEventData].
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub enum TcpEvent {
    Connected,                        // fires when the client has connected to a server
    Disconnected(DisconnectedReason), // fires when the client has disconnected from a server
    TextMessage,                      // fires when a text message comes in
    Ping,                             // fires when the server sends a ping with packet stats
}

/// When a [TcpEvent] occurs, this contains the data for the event.
///
/// The events are picked up by a [crate::state::ExecutionContext].
///
/// Having two different types might feel a bit confusing. Essentially, a
/// callback _registers_ to a [TcpEvent] but _takes_ a [TcpEventData] as
/// parameter.
#[derive(Clone, Debug)]
pub enum TcpEventData<'a> {
    Connected(Result<&'a msgs::ServerSync, mumlib::Error>),
    Disconnected(DisconnectedReason),
    TextMessage(&'a msgs::TextMessage),
    Ping {
        good: u32,
        late: u32,
        lost: u32,
        resync: u32,
        total_good: u32,
        total_late: u32,
        total_lost: u32,
        total_resync: u32,
    },
}

impl From<&TcpEventData<'_>> for TcpEvent {
    fn from(t: &TcpEventData<'_>) -> Self {
        match t {
            TcpEventData::Connected(_) => TcpEvent::Connected,
            TcpEventData::Disconnected(reason) => TcpEvent::Disconnected(*reason),
            TcpEventData::TextMessage(_) => TcpEvent::TextMessage,
            TcpEventData::Ping { .. } => TcpEvent::Ping,
        }
    }
}

#[derive(Clone, Default)]
pub struct TcpEventQueue {
    callbacks: Arc<RwLock<HashMap<TcpEvent, Vec<TcpEventCallback>>>>,
    subscribers: Arc<RwLock<HashMap<TcpEvent, Vec<TcpEventSubscriber>>>>,
}

impl TcpEventQueue {
    /// Creates a new `TcpEventQueue`.
    pub fn new() -> Self {
        Self {
            callbacks: Arc::new(RwLock::new(HashMap::new())),
            subscribers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Registers a new callback to be triggered when an event is fired.
    pub fn register_callback(&self, at: TcpEvent, callback: TcpEventCallback) {
        self.callbacks
            .write()
            .unwrap()
            .entry(at)
            .or_default()
            .push(callback);
    }

    /// Registers a new callback to be triggered when an event is fired.
    pub fn register_subscriber(&self, at: TcpEvent, callback: TcpEventSubscriber) {
        self.subscribers
            .write()
            .unwrap()
            .entry(at)
            .or_default()
            .push(callback);
    }

    /// Fires all callbacks related to a specific TCP event and removes them from the event queue.
    /// Also calls all event subscribers, but keeps them in the queue
    pub fn resolve(&self, data: TcpEventData<'_>) {
        if let Some(vec) = self
            .callbacks
            .write()
            .unwrap()
            .get_mut(&TcpEvent::from(&data))
        {
            let old = std::mem::take(vec);
            for handler in old {
                handler(data.clone());
            }
        }
        if let Some(vec) = self
            .subscribers
            .write()
            .unwrap()
            .get_mut(&TcpEvent::from(&data))
        {
            let old = std::mem::take(vec);
            for mut e in old {
                if e(data.clone()) {
                    vec.push(e)
                }
            }
        }
    }
}

impl Debug for TcpEventQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpEventQueue").finish()
    }
}

pub async fn handle(
    state: Arc<RwLock<State>>,
    mut connection_info_receiver: watch::Receiver<Option<ConnectionInfo>>,
    crypt_state_sender: mpsc::Sender<ClientCryptState>,
    packet_sender: mpsc::UnboundedSender<ControlPacket<Serverbound>>,
    mut packet_receiver: mpsc::UnboundedReceiver<ControlPacket<Serverbound>>,
    event_queue: TcpEventQueue,
) -> Result<(), TcpError> {
    loop {
        let connection_info = loop {
            if connection_info_receiver.changed().await.is_ok() {
                if let Some(data) = connection_info_receiver.borrow().clone() {
                    break data;
                }
            } else {
                return Err(TcpError::NoConnectionInfoReceived);
            }
        };
        let connect_result = connect(
            connection_info.socket_addr,
            connection_info.hostname,
            connection_info.accept_invalid_cert,
        )
        .await;

        let (mut sink, stream) = match connect_result {
            Ok(ok) => ok,
            Err(TcpError::TlsConnectError(_)) => {
                warn!("Invalid TLS");
                state
                    .read()
                    .unwrap()
                    .broadcast_phase(StatePhase::Disconnected);
                event_queue.resolve(TcpEventData::Disconnected(DisconnectedReason::InvalidTls));
                continue;
            }
            Err(e) => {
                return Err(e);
            }
        };

        // Handshake (omitting `Version` message for brevity)
        let (username, password) = {
            let state_lock = state.read().unwrap();
            (
                state_lock.username().unwrap().to_string(),
                state_lock.password().map(|x| x.to_string()),
            )
        };
        authenticate(&mut sink, username, password).await?;
        let (phase_watcher, input_receiver) = {
            let state_lock = state.read().unwrap();
            (
                state_lock.phase_receiver(),
                state_lock.audio_input().receiver(),
            )
        };

        info!("Logging in...");

        let phase_watcher_inner = phase_watcher.clone();

        let result = run_until(
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
                }
            },
            phase_watcher,
        )
        .await
        .unwrap_or(Ok(()));

        match result {
            Ok(()) => event_queue.resolve(TcpEventData::Disconnected(DisconnectedReason::User)),
            Err(_) => event_queue.resolve(TcpEventData::Disconnected(DisconnectedReason::TcpError)),
        }

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
        .map_err(TcpError::TlsConnectorBuilderError)?
        .into();
    let tls_stream = connector
        .connect(&server_host, stream)
        .await
        .map_err(TcpError::TlsConnectError)?;
    debug!("TLS connected");

    // Wrap the TLS stream with Mumble's client-side control-channel codec
    Ok(ClientControlCodec::new().framed(tls_stream).split())
}

async fn authenticate(
    sink: &mut TcpSender,
    username: String,
    password: Option<String>,
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
            if matches!(
                *inner_phase_watcher.borrow(),
                StatePhase::Connected(VoiceStreamType::Tcp)
            ) {
                break;
            }
        }
        debug!("Sending voice over TCP");
        run_until(
            |phase| !matches!(phase, StatePhase::Connected(VoiceStreamType::Tcp)),
            async {
                loop {
                    packet_sender.send(
                        receiver
                            .lock()
                            .await
                            .next()
                            .await
                            .expect("No audio stream")
                            .into(),
                    )?;
                }
            },
            inner_phase_watcher.clone(),
        )
        .await
        .unwrap_or(Ok::<(), ServerSendError>(()))?;
        debug!("No longer sending voice over TCP");
    }
}

async fn listen(
    state: Arc<RwLock<State>>,
    mut stream: TcpReceiver,
    crypt_state_sender: mpsc::Sender<ClientCryptState>,
    event_queue: TcpEventQueue,
) -> Result<(), TcpError> {
    let mut crypt_state = None;
    let mut crypt_state_sender = Some(crypt_state_sender);

    let mut total_good = 0;
    let mut total_late = 0;
    let mut total_lost = 0;
    let mut total_resync = 0;

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
                state
                    .read()
                    .unwrap()
                    .broadcast_phase(StatePhase::Disconnected);
                break;
            }
        };
        match packet {
            ControlPacket::TextMessage(msg) => {
                let mut state = state.write().unwrap();
                let server = state.server();
                let user = (if let Server::Connected(s) = server {
                    Some(s)
                } else {
                    None
                })
                .and_then(|server| server.users().get(&msg.get_actor()))
                .map(|user| user.name());
                if let Some(user) = user {
                    notifications::send(format!("{}: {}", user, msg.get_message()));
                    //TODO: probably want a config flag for this
                    let user = user.to_string();
                    state.push_event(MumbleEventKind::TextMessageReceived(user))
                    //TODO also include message target
                }
                state.register_message((msg.get_message().to_owned(), msg.get_actor()));
                drop(state);
                event_queue.resolve(TcpEventData::TextMessage(&*msg));
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
                let mut state = state.write().unwrap();
                let server = state.server_mut();
                if let Server::Connecting(sb) = server {
                    let s = sb.clone().server_sync(*msg.clone());
                    *server = Server::Connected(s);
                    state.initialized();
                } else {
                    warn!(
                        "Got a ServerSync packet while not connecting. Current state is:\n{:#?}",
                        server
                    );
                }
                drop(state);
                event_queue.resolve(TcpEventData::Connected(Ok(&msg)));
            }
            ControlPacket::Reject(msg) => {
                debug!("Login rejected: {:?}", msg);
                match msg.get_field_type() {
                    msgs::Reject_RejectType::WrongServerPW => {
                        event_queue.resolve(TcpEventData::Connected(Err(
                            mumlib::Error::InvalidServerPassword,
                        )));
                    }
                    ty => {
                        warn!("Unhandled reject type: {:?}", ty);
                    }
                }
            }
            ControlPacket::UserState(msg) => {
                state.write().unwrap().user_state(*msg);
            }
            ControlPacket::UserRemove(msg) => {
                state.write().unwrap().remove_user(*msg);
            }
            ControlPacket::ChannelState(msg) => {
                if let Server::Connecting(sb) = state.write().unwrap().server_mut() {
                    sb.channel_state(*msg);
                }
            }
            ControlPacket::ChannelRemove(msg) => match state.write().unwrap().server_mut() {
                Server::Connecting(sb) => sb.channel_remove(*msg),
                Server::Connected(server) => server.channel_remove(*msg),
                Server::Disconnected => warn!("Got ChannelRemove packet while disconnected"),
            },
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
                        state.read().unwrap().audio_output().decode_packet_payload(
                            VoiceStreamType::Tcp,
                            session_id,
                            payload,
                        );
                    }
                }
            }
            ControlPacket::Ping(msg) => {
                trace!("Received Ping {:?}", *msg);

                // The packets contain the sums.
                let new_good = msg.get_good();
                let new_late = msg.get_late();
                let new_lost = msg.get_lost();
                let new_resync = msg.get_resync();

                // Changes since the last ping.
                let good = new_good - total_good;
                let late = new_late - total_late;
                let lost = new_lost - total_lost;
                let resync = new_resync - total_resync;

                // Totals for this session.
                total_good = new_good;
                total_late = new_late;
                total_lost = new_lost;
                total_resync = new_resync;

                event_queue.resolve(TcpEventData::Ping {
                    good,
                    late,
                    lost,
                    resync,
                    total_good,
                    total_late,
                    total_lost,
                    total_resync,
                });

                macro_rules! format_if_nonzero {
                    ($value:expr) => {
                        if $value != 0 {
                            format!("\n  {}: {}", stringify!($value), $value)
                        } else {
                            String::new()
                        }
                    };
                }

                if late != 0 || lost != 0 || resync != 0 {
                    debug!(
                        "Ping:{}{}{}",
                        format_if_nonzero!(late),
                        format_if_nonzero!(lost),
                        format_if_nonzero!(resync),
                    );
                }
            }
            packet => {
                debug!("Received unhandled ControlPacket {:#?}", packet);
            }
        }
    }
    Ok(())
}
