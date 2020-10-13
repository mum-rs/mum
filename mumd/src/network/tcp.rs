use crate::state::State;
use log::*;

use futures::channel::oneshot;
use futures::{join, SinkExt, StreamExt};
use futures_util::stream::{SplitSink, SplitStream};
use mumble_protocol::control::{msgs, ClientControlCodec, ControlCodec, ControlPacket};
use mumble_protocol::crypt::ClientCryptState;
use mumble_protocol::{Clientbound, Serverbound};
use std::convert::{Into, TryInto};
use std::net::{SocketAddr};
use std::sync::{Arc, Mutex};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{self, Duration};
use tokio_tls::{TlsConnector, TlsStream};
use tokio_util::codec::{Decoder, Framed};

type TcpSender = SplitSink<
    Framed<TlsStream<TcpStream>, ControlCodec<Serverbound, Clientbound>>,
    ControlPacket<Serverbound>,
>;
type TcpReceiver =
    SplitStream<Framed<TlsStream<TcpStream>, ControlCodec<Serverbound, Clientbound>>>;

pub async fn handle(
    state: Arc<Mutex<State>>,
    server_addr: SocketAddr,
    server_host: String,
    accept_invalid_cert: bool,
    crypt_state_sender: oneshot::Sender<ClientCryptState>,
    packet_receiver: mpsc::UnboundedReceiver<ControlPacket<Serverbound>>,
) {
    let (mut sink, stream) = connect(server_addr, server_host, accept_invalid_cert).await;

    // Handshake (omitting `Version` message for brevity)
    authenticate(&mut sink, state.lock().unwrap().username().to_string()).await;

    info!("Logging in...");

    join!(
        send_pings(state.lock().unwrap().packet_sender(), 10),
        listen(state, stream, crypt_state_sender),
        send_packets(sink, packet_receiver),
    );
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

async fn send_pings(packet_sender: mpsc::UnboundedSender<ControlPacket<Serverbound>>,
                    delay_seconds: u64) {
    let mut interval = time::interval(Duration::from_secs(delay_seconds));
    loop {
        interval.tick().await;
        trace!("Sending ping");
        let msg = msgs::Ping::new();
        packet_sender.send(msg.into()).unwrap();
    }
}

async fn send_packets(mut sink: TcpSender,
                      mut packet_receiver: mpsc::UnboundedReceiver<ControlPacket<Serverbound>>) {

    while let Some(packet) = packet_receiver.recv().await {
        sink.send(packet).await.unwrap();
    }
}

async fn listen(
    state: Arc<Mutex<State>>,
    mut stream: TcpReceiver,
    crypt_state_sender: oneshot::Sender<ClientCryptState>,
) {
    let mut crypt_state = None;
    let mut crypt_state_sender = Some(crypt_state_sender);

    while let Some(packet) = stream.next().await {
        //TODO handle types separately
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
                if let Some(sender) = crypt_state_sender.take() {
                    let _ = sender.send(
                        crypt_state
                            .take()
                            .expect("Server didn't send us any CryptSetup packet!"),
                    );
                }
                let mut state = state.lock().unwrap();
                let server = state.server_mut();
                server.parse_server_sync(msg);
                match &server.welcome_text {
                    Some(s) => info!("Welcome: {}", s),
                    None => info!("No welcome received"),
                }
                for (_, channel) in server.channels() {
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
                state.audio_mut().add_client(msg.get_session()); //TODO
                if *state.initialized_receiver().borrow() {
                    state.server_mut().parse_user_state(msg);
                } else {
                    state.parse_initial_user_state(msg);
                }
                let server = state.server_mut();
                let user = server.users().get(&session).unwrap();
                info!("User {} connected to {}",
                         user.name(),
                         user.channel());
            }
            ControlPacket::UserRemove(msg) => {
                info!("User {} left", msg.get_session());
                state.lock().unwrap().audio_mut().remove_client(msg.get_session());
            }
            ControlPacket::ChannelState(msg) => {
                debug!("Channel state received");
                state.lock().unwrap().server_mut().parse_channel_state(msg); //TODO parse initial if initial
            }
            ControlPacket::ChannelRemove(msg) => {
                state.lock().unwrap().server_mut().parse_channel_remove(msg);
            }
            _ => {}
        }
    }
}
