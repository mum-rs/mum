use crate::audio::Audio;
use crate::state::Server;
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
    server: Arc<Mutex<Server>>,
    server_addr: SocketAddr,
    server_host: String,
    username: String,
    accept_invalid_cert: bool,
    crypt_state_sender: oneshot::Sender<ClientCryptState>,
    audio: Arc<Mutex<Audio>>,
) {
    let (sink, stream) = connect(server_addr, server_host, accept_invalid_cert).await;
    let sink = Arc::new(Mutex::new(sink));

    // Handshake (omitting `Version` message for brevity)
    authenticate(Arc::clone(&sink), username).await;

    info!("Logging in...");

    join!(
        send_pings(Arc::clone(&sink), 10),
        listen(server, sink, stream, crypt_state_sender, audio),
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

async fn authenticate(sink: Arc<Mutex<TcpSender>>, username: String) {
    let mut msg = msgs::Authenticate::new();
    msg.set_username(username);
    msg.set_opus(true);
    sink.lock().unwrap().send(msg.into()).await.unwrap();
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

async fn listen(
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
