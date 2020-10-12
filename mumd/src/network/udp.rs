use crate::audio::Audio;
use log::*;

use bytes::Bytes;
use futures::channel::oneshot;
use futures::{join, SinkExt, StreamExt};
use futures_util::stream::{SplitSink, SplitStream};
use mumble_protocol::crypt::ClientCryptState;
use mumble_protocol::voice::{VoicePacket, VoicePacketPayload};
use mumble_protocol::Serverbound;
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use tokio::net::UdpSocket;
use tokio_util::udp::UdpFramed;

type UdpSender = SplitSink<UdpFramed<ClientCryptState>, (VoicePacket<Serverbound>, SocketAddr)>;
type UdpReceiver = SplitStream<UdpFramed<ClientCryptState>>;

pub async fn connect(
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

async fn listen(
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

async fn send_ping(sink: &mut UdpSender, server_addr: SocketAddr) {
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

async fn send_voice(
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

pub async fn handle(
    server_addr: SocketAddr,
    crypt_state: oneshot::Receiver<ClientCryptState>,
    audio: Arc<Mutex<Audio>>,
) {
    let (mut sink, source) = connect(crypt_state).await;

    // Note: A normal application would also send periodic Ping packets, and its own audio
    //       via UDP. We instead trick the server into accepting us by sending it one
    //       dummy voice packet.
    send_ping(&mut sink, server_addr).await;

    let sink = Arc::new(Mutex::new(sink));
    join!(
        listen(Arc::clone(&sink), source, Arc::clone(&audio)),
        send_voice(sink, server_addr, audio)
    );
}
