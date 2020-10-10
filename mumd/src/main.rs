mod audio;
mod network;
use crate::audio::Audio;

use argparse::ArgumentParser;
use argparse::Store;
use argparse::StoreTrue;
use bytes::Bytes;

use futures::channel::oneshot;
use futures::join;
use futures::StreamExt;
use futures::SinkExt;

use mumble_protocol::control::msgs;
use mumble_protocol::control::ControlPacket;
use mumble_protocol::crypt::ClientCryptState;
use mumble_protocol::voice::VoicePacket;
use mumble_protocol::voice::VoicePacketPayload;

use std::convert::Into;
use std::convert::TryInto;

use std::net::SocketAddr;
use std::net::ToSocketAddrs;

use tokio::time::{self, Duration};

use std::sync::Arc;
use std::sync::Mutex;

use cpal::traits::StreamTrait;

use opus::Channels;

async fn connect(
    server_addr: SocketAddr,
    server_host: String,
    user_name: String,
    accept_invalid_cert: bool,
    crypt_state_sender: oneshot::Sender<ClientCryptState>,
) {
    // Wrap crypt_state_sender in Option, so we can call it only once
    let mut crypt_state_sender = Some(crypt_state_sender);

    let (sink, mut stream) = network::connect_tcp(server_addr, server_host, accept_invalid_cert).await;
    let sink = Arc::new(Mutex::new(sink));

    // Handshake (omitting `Version` message for brevity)
    let mut msg = msgs::Authenticate::new();
    msg.set_username(user_name);
    msg.set_opus(true);
    let mut lock = sink.lock().unwrap();
    lock.send(msg.into()).await.unwrap();
    drop(lock);

    println!("Logging in..");
    let mut crypt_state = None;

    let ping_sink = Arc::clone(&sink);
    let handle = async {
        let mut interval = time::interval(Duration::from_secs(10));
        let sink = ping_sink;

        loop {
            interval.tick().await;
            let msg = msgs::Ping::new();
            let mut lock = sink.lock().unwrap();
            lock.send(msg.into()).await.unwrap();
        }
    };

    // Handle incoming packets
    let receive = async {
        while let Some(packet) = stream.next().await {
            match packet.unwrap() {
                ControlPacket::TextMessage(mut msg) => {
                    println!(
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
                    println!("crypt setup");
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
                ControlPacket::ServerSync(_) => {
                    println!("Logged in!");
                    if let Some(sender) = crypt_state_sender.take() {
                        let _ = sender.send(
                            crypt_state
                                .take()
                                .expect("Server didn't send us any CryptSetup packet!"),
                        );
                    }
                }
                ControlPacket::Reject(msg) => {
                    println!("Login rejected: {:?}", msg);
                }
                _ => {},
            }
        }
    };
    join!(handle, receive);
}

async fn handle_udp(
    server_addr: SocketAddr,
    crypt_state: oneshot::Receiver<ClientCryptState>,
) {
    let audio = Audio::new();
    audio.output_stream.play().unwrap();

    // create opus decoder (might be expensive)
    let mut opus_decoder = opus::Decoder::new(
        audio.output_config.sample_rate.0 as u32,
        match audio.output_config.channels {
            1 => Channels::Mono,
            2 => Channels::Stereo,
            _ => unimplemented!("ljudnörd (got {} channels, need 1 or 2)", audio.output_config.channels)
        }
    ).unwrap();

    let (mut sink, mut source) = network::connect_udp(server_addr, crypt_state).await;

    // Note: A normal application would also send periodic Ping packets, and its own audio
    //       via UDP. We instead trick the server into accepting us by sending it one
    //       dummy voice packet.
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
    )).await.unwrap();

    // Handle incoming UDP packets
    while let Some(packet) = source.next().await {
        let (packet, _src_addr) = match packet {
            Ok(packet) => packet,
            Err(err) => {
                eprintln!("Got an invalid UDP packet: {}", err);
                // To be expected, considering this is the internet, just ignore it
                continue
            }
        };
        match packet {
            VoicePacket::Ping { .. } => {
                // Note: A normal application would handle these and only use UDP for voice
                //       once it has received one.
                continue
            }
            VoicePacket::Audio {
                // seq_num,
                payload,
                // position_info,
                ..
            } => {
                match payload {
                    VoicePacketPayload::Opus(bytes, _eot) => {
                        let mut out: Vec<f32> = vec![0.0; bytes.len() * audio.output_config.channels as usize * 4];
                        opus_decoder.decode_float(&bytes[..], &mut out, false).expect("error decoding");
                        let mut lock = audio.output_buffer.lock().unwrap();
                        lock.extend(out);
                    },
                    _ => { unimplemented!("något fint"); }
                }

                // decode paylout and put it in buffer

                // Got audio, naively echo it back
                //let reply = VoicePacket::Audio {
                //    _dst: std::marker::PhantomData,
                //    target: 0,      // normal speech
                //    session_id: (), // unused for server-bound packets
                //    seq_num,
                //    payload,
                //    position_info,
                //};
                //sink.send((reply, src_addr)).await.unwrap();
            }
        }
    }
}

#[tokio::main]
async fn main() {
    // Handle command line arguments
    let mut server_host = "".to_string();
    let mut server_port = 64738u16;
    let mut user_name = "EchoBot".to_string();
    let mut accept_invalid_cert = false;
    {
        let mut ap = ArgumentParser::new();
        ap.set_description("Run the echo client example");
        ap.refer(&mut server_host)
            .add_option(&["--host"], Store, "Hostname of mumble server")
            .required();
        ap.refer(&mut server_port)
            .add_option(&["--port"], Store, "Port of mumble server");
        ap.refer(&mut user_name)
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

    // Run it
    join!(
        connect(
            server_addr,
            server_host,
            user_name,
            accept_invalid_cert,
            crypt_state_sender,
        ),
        handle_udp(server_addr, crypt_state_receiver)
    );
}
