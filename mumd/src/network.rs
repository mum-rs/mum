use futures::StreamExt;
use futures_util::stream::{SplitSink, SplitStream};
use mumble_protocol::{Serverbound, Clientbound};
use mumble_protocol::control::ClientControlCodec;
use mumble_protocol::control::ControlCodec;
use mumble_protocol::control::ControlPacket;
use std::net::SocketAddr;
use tokio::net::TcpStream;
use tokio_tls::TlsConnector;
use tokio_tls::TlsStream;
use tokio_util::codec::Decoder;
use tokio_util::codec::Framed;

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
