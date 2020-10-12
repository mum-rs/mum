use crate::audio::Audio;
use crate::state::Server;

use bytes::Bytes;
use futures::channel::oneshot;
use futures::{join, SinkExt, StreamExt};
use futures_util::stream::{SplitSink, SplitStream};
use log::*;
use mumble_protocol::control::{msgs, ClientControlCodec, ControlCodec, ControlPacket};
use mumble_protocol::crypt::ClientCryptState;
use mumble_protocol::voice::{VoicePacket, VoicePacketPayload};
use mumble_protocol::{Clientbound, Serverbound};
use std::convert::{Into, TryInto};
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::{self, Duration};
use tokio_tls::{TlsConnector, TlsStream};
use tokio_util::codec::{Decoder, Framed};
use tokio_util::udp::UdpFramed;

type TcpSender = SplitSink<
    Framed<TlsStream<TcpStream>, ControlCodec<Serverbound, Clientbound>>,
    ControlPacket<Serverbound>,
>;
type TcpReceiver =
    SplitStream<Framed<TlsStream<TcpStream>, ControlCodec<Serverbound, Clientbound>>>;
type UdpSender = SplitSink<UdpFramed<ClientCryptState>, (VoicePacket<Serverbound>, SocketAddr)>;
type UdpReceiver = SplitStream<UdpFramed<ClientCryptState>>;

async fn connect_tcp(
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

pub async fn connect_udp(
    crypt_state: oneshot::Receiver<ClientCryptState>,
) -> (UdpSender, UdpReceiver) {
    // Bind UDP socket
    let udp_socket = UdpSocket::bind((Ipv6Addr::from(0u128), 0u16))
        .await
        .expect("Failed to bind UDP socket");

    // Wait for initial CryptState
    let crypt_state = match crypt_state.await {
        Ok(crypt_state) => crypt_state,
        // disconnected before we received the CryptSetup packet, oh well
        Err(_) => panic!("disconnect before crypt packet received"), //TODO exit gracefully
    };
    debug!("UDP connected");

    // Wrap the raw UDP packets in Mumble's crypto and voice codec (CryptState does both)
    UdpFramed::new(udp_socket, crypt_state).split()
}

async fn send_pings(sink: Arc<Mutex<TcpSender>>, delay_seconds: u64) {
    let mut interval = time::interval(Duration::from_secs(delay_seconds));
    loop {
        interval.tick().await;
        trace!("Sending ping");
        let msg = msgs::Ping::new();
        sink.lock().unwrap().send(msg.into()).await.unwrap();
    }
}

async fn authenticate(sink: Arc<Mutex<TcpSender>>, username: String) {
    let mut msg = msgs::Authenticate::new();
    msg.set_username(username);
    msg.set_opus(true);
    sink.lock().unwrap().send(msg.into()).await.unwrap();
}

async fn listen_tcp(
    server: Arc<Mutex<Server>>,
    sink: Arc<Mutex<TcpSender>>,
    mut stream: TcpReceiver,
    crypt_state_sender: oneshot::Sender<ClientCryptState>,
    audio: Arc<Mutex<Audio>>,
) {
    let mut crypt_state = None;
    let mut crypt_state_sender = Some(crypt_state_sender);

    while let Some(packet) = stream.next().await {
        //TODO handle types separately
        match packet.unwrap() {
            ControlPacket::TextMessage(mut msg) => {
                info!(
                    "Got message from user with session ID {}: {}",
                    msg.get_actor(),
                    msg.get_message()
                );
                // Send reply back to server
                let mut response = msgs::TextMessage::new();
                response.mut_session().push(msg.get_actor());
                response.set_message(msg.take_message());
                let mut lock = sink.lock().unwrap();
                lock.send(response.into()).await.unwrap();
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
                    let _ = sender.send(
                        crypt_state
                            .take()
                            .expect("Server didn't send us any CryptSetup packet!"),
                    );
                }
                let mut server = server.lock().unwrap();
                server.parse_server_sync(msg);
                match &server.welcome_text {
                    Some(s) => info!("Welcome: {}", s),
                    None => info!("No welcome received"),
                }
                for (_, channel) in server.channels() {
                    info!("Found channel {}", channel.name());
                }
                sink.lock().unwrap().send(msgs::UserList::new().into()).await.unwrap();
            }
            ControlPacket::Reject(msg) => {
                warn!("Login rejected: {:?}", msg);
            }
            ControlPacket::UserState(msg) => {
                audio.lock().unwrap().add_client(msg.get_session());
                let mut server = server.lock().unwrap();
                let session = msg.get_session();
                server.parse_user_state(msg);
                let user = server.users().get(&session).unwrap();
                info!("User {} connected to {}",
                         user.name(),
                         user.channel());
            }
            ControlPacket::UserRemove(msg) => {
                info!("User {} left", msg.get_session());
                audio.lock().unwrap().remove_client(msg.get_session());
            }
            ControlPacket::ChannelState(msg) => {
                debug!("Channel state received");
                server.lock().unwrap().parse_channel_state(msg);
            }
            ControlPacket::ChannelRemove(msg) => {
                server.lock().unwrap().parse_channel_remove(msg);
            }
            _ => {}
        }
    }
}

pub async fn handle_tcp(
    server: Arc<Mutex<Server>>,
    server_addr: SocketAddr,
    server_host: String,
    username: String,
    accept_invalid_cert: bool,
    crypt_state_sender: oneshot::Sender<ClientCryptState>,
    audio: Arc<Mutex<Audio>>,
) {
    let (sink, stream) = connect_tcp(server_addr, server_host, accept_invalid_cert).await;
    let sink = Arc::new(Mutex::new(sink));

    // Handshake (omitting `Version` message for brevity)
    authenticate(Arc::clone(&sink), username).await;

    info!("Logging in...");

    join!(
        send_pings(Arc::clone(&sink), 10),
        listen_tcp(server, sink, stream, crypt_state_sender, audio),
    );
}

async fn listen_udp(
    _sink: Arc<Mutex<UdpSender>>,
    mut source: UdpReceiver,
    audio: Arc<Mutex<Audio>>,
) {
    while let Some(packet) = source.next().await {
        let (packet, _src_addr) = match packet {
            Ok(packet) => packet,
            Err(err) => {
                warn!("Got an invalid UDP packet: {}", err);
                // To be expected, considering this is the internet, just ignore it
                continue;
            }
        };
        match packet {
            VoicePacket::Ping { .. } => {
                // Note: A normal application would handle these and only use UDP for voice
                //       once it has received one.
                continue;
            }
            VoicePacket::Audio {
                session_id,
                // seq_num,
                payload,
                // position_info,
                ..
            } => {
                audio.lock().unwrap().decode_packet(session_id, payload);
            }
        }
    }
}

async fn send_ping_udp(sink: &mut UdpSender, server_addr: SocketAddr) {
    sink.send((
        VoicePacket::Audio {
            _dst: std::marker::PhantomData,
            target: 0,
            session_id: (),
            seq_num: 0,
            payload: VoicePacketPayload::Opus(Bytes::from([0u8; 128].as_ref()), true),
            position_info: None,
        },
        server_addr,
    ))
    .await
    .unwrap();
}

async fn send_voice_udp(
    sink: Arc<Mutex<UdpSender>>,
    server_addr: SocketAddr,
    audio: Arc<Mutex<Audio>>,
) {
    let mut receiver = audio.lock().unwrap().take_receiver().unwrap();

    let mut count = 0;
    while let Some(payload) = receiver.recv().await {
        let reply = VoicePacket::Audio {
            _dst: std::marker::PhantomData,
            target: 0,      // normal speech
            session_id: (), // unused for server-bound packets
            seq_num: count,
            payload,
            position_info: None,
        };
        count += 1;
        sink.lock()
            .unwrap()
            .send((reply, server_addr))
            .await
            .unwrap();
    }
}

pub async fn handle_udp(
    server_addr: SocketAddr,
    crypt_state: oneshot::Receiver<ClientCryptState>,
    audio: Arc<Mutex<Audio>>,
) {
    let (mut sink, source) = connect_udp(crypt_state).await;

    // Note: A normal application would also send periodic Ping packets, and its own audio
    //       via UDP. We instead trick the server into accepting us by sending it one
    //       dummy voice packet.
    send_ping_udp(&mut sink, server_addr).await;

    let sink = Arc::new(Mutex::new(sink));
    join!(
        listen_udp(Arc::clone(&sink), source, Arc::clone(&audio)),
        send_voice_udp(sink, server_addr, audio)
    );
}
