use futures::channel::oneshot;
use futures::StreamExt;
use futures_util::stream::{SplitSink, SplitStream};
use mumble_protocol::{Serverbound, Clientbound};
use mumble_protocol::control::ClientControlCodec;
use mumble_protocol::control::ControlCodec;
use mumble_protocol::control::ControlPacket;
use mumble_protocol::crypt::ClientCryptState;
use mumble_protocol::voice::VoicePacket;
use std::net::Ipv6Addr;
use std::net::SocketAddr;
use tokio::net::TcpStream;
use tokio::net::UdpSocket;
use tokio_tls::TlsConnector;
use tokio_tls::TlsStream;
use tokio_util::codec::Decoder;
use tokio_util::codec::Framed;
use tokio_util::udp::UdpFramed;

pub async fn connect_tcp(
    server_addr: SocketAddr,
    server_host: String,
    accept_invalid_cert: bool,
) -> (
    SplitSink<Framed<TlsStream<TcpStream>, ControlCodec<Serverbound, Clientbound>>, ControlPacket<Serverbound>>,
    SplitStream<Framed<TlsStream<TcpStream>, ControlCodec<Serverbound, Clientbound>>>
) {

    let stream = TcpStream::connect(&server_addr).await.expect("failed to connect to server:");
    println!("TCP connected");

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
    println!("TLS connected");

    // Wrap the TLS stream with Mumble's client-side control-channel codec
    ClientControlCodec::new().framed(tls_stream).split()
}

pub async fn connect_udp(
    server_addr: SocketAddr,
    crypt_state: oneshot::Receiver<ClientCryptState>,
) -> (
    SplitSink<UdpFramed<ClientCryptState>, (VoicePacket<Serverbound>, SocketAddr)>,
    SplitStream<UdpFramed<ClientCryptState>>
) {
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
    println!("UDP ready!");

    // Wrap the raw UDP packets in Mumble's crypto and voice codec (CryptState does both)
    UdpFramed::new(udp_socket, crypt_state).split()
}
