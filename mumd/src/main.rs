mod audio;
mod network;
use crate::audio::Audio;

use argparse::ArgumentParser;
use argparse::Store;
use argparse::StoreTrue;
use cpal::traits::StreamTrait;
use futures::channel::oneshot;
use futures::join;
use mumble_protocol::crypt::ClientCryptState;
use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::sync::Mutex;

#[tokio::main]
async fn main() {
    // Handle command line arguments
    let mut server_host = "".to_string();
    let mut server_port = 64738u16;
    let mut username = "EchoBot".to_string();
    let mut accept_invalid_cert = false;
    {
        let mut ap = ArgumentParser::new();
        ap.set_description("Run the echo client example");
        ap.refer(&mut server_host)
            .add_option(&["--host"], Store, "Hostname of mumble server")
            .required();
        ap.refer(&mut server_port)
            .add_option(&["--port"], Store, "Port of mumble server");
        ap.refer(&mut username)
            .add_option(&["--username"], Store, "User name used to connect");
        ap.refer(&mut accept_invalid_cert).add_option(
            &["--accept-invalid-cert"],
            StoreTrue,
            "Accept invalid TLS certificates",
        );
        ap.parse_args_or_exit();
    }
    let server_addr = (server_host.as_ref(), server_port)
        .to_socket_addrs()
        .expect("Failed to parse server address")
        .next()
        .expect("Failed to resolve server address");

    // Oneshot channel for setting UDP CryptState from control task
    // For simplicity we don't deal with re-syncing, real applications would have to.
    let (crypt_state_sender, crypt_state_receiver) = oneshot::channel::<ClientCryptState>();

    let audio = Audio::new();
    audio.output_stream.play().unwrap();
    let audio = Arc::new(Mutex::new(audio));

    // Run it
    join!(
        network::handle_tcp(
            server_addr,
            server_host,
            username,
            accept_invalid_cert,
            crypt_state_sender,
            Arc::clone(&audio),
        ),
        network::handle_udp(
            server_addr,
            crypt_state_receiver,
            audio,
        )
    );
}
