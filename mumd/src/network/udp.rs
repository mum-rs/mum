use crate::network::ConnectionInfo;
use crate::state::{State, StatePhase};
use log::*;

use bytes::Bytes;
use futures::{join, select, pin_mut, SinkExt, StreamExt, FutureExt};
use futures_util::stream::{SplitSink, SplitStream};
use mumble_protocol::crypt::ClientCryptState;
use mumble_protocol::voice::{VoicePacket, VoicePacketPayload};
use mumble_protocol::Serverbound;
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use tokio::net::UdpSocket;
use tokio::sync::{watch, oneshot};
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

pub async fn handle(
    state: Arc<Mutex<State>>,
    mut connection_info_receiver: watch::Receiver<Option<ConnectionInfo>>,
    crypt_state: oneshot::Receiver<ClientCryptState>,
) {
    let connection_info = loop {
        match connection_info_receiver.recv().await {
            None => { return; }
            Some(None) => {}
            Some(Some(connection_info)) => { break connection_info; }
        }
    };
    let (mut sink, source) = connect(crypt_state).await;

    // Note: A normal application would also send periodic Ping packets, and its own audio
    //       via UDP. We instead trick the server into accepting us by sending it one
    //       dummy voice packet.
    send_ping(&mut sink, connection_info.socket_addr).await;

    let sink = Arc::new(Mutex::new(sink));

    let phase_watcher = state.lock().unwrap().phase_receiver();
    join!(
        listen(Arc::clone(&state), source, phase_watcher.clone()),
        send_voice(state, sink, connection_info.socket_addr, phase_watcher),
    );

    debug!("Fully disconnected UPD stream");
}

async fn listen(
    state: Arc<Mutex<State>>,
    mut source: UdpReceiver,
    mut phase_watcher: watch::Receiver<StatePhase>,
) {
    let (tx, rx) = oneshot::channel();
    let phase_transition_block = async {
        while !matches!(phase_watcher.recv().await.unwrap(), StatePhase::Disconnected) {}
        tx.send(true);
    };

    let main_block = async {
        let rx = rx.fuse();
        pin_mut!(rx);
        loop {
            let packet_recv = source.next().fuse();
            pin_mut!(packet_recv);
            let exitor = select! {
                data = packet_recv => Some(data),
                _ = rx => None
            };
            match exitor {
                None => {
                    break;
                }
                Some(None) => {
                    warn!("Channel closed before disconnect command");
                    break;
                }
                Some(Some(packet)) => {
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
                            state.lock().unwrap().audio().decode_packet(session_id, payload);
                        }
                    }
                }
            }
        }
    };

    join!(main_block, phase_transition_block);

    debug!("UDP listener process killed");

    /*while let Some(packet) = source.next().await {
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
                state.lock().unwrap().audio().decode_packet(session_id, payload);
            }
        }
    }*/
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
    state: Arc<Mutex<State>>,
    sink: Arc<Mutex<UdpSender>>,
    server_addr: SocketAddr,
    mut phase_watcher: watch::Receiver<StatePhase>,
) {
    let mut receiver = state.lock().unwrap().audio_mut().take_receiver().unwrap();

    let (tx, rx) = oneshot::channel();
    let phase_transition_block = async {
        while !matches!(phase_watcher.recv().await.unwrap(), StatePhase::Disconnected) {}
        tx.send(true);
    };

    let main_block = async {
        let rx = rx.fuse();
        pin_mut!(rx);
        let mut count = 0;
        loop {
            let packet_recv = receiver.recv().fuse();
            pin_mut!(packet_recv);
            let exitor = select! {
                data = packet_recv => Some(data),
                _ = rx => None
            };
            match exitor {
                None => {
                    break;
                }
                Some(None) => {
                    warn!("Channel closed before disconnect command");
                    break;
                }
                Some(Some(payload)) => {
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
        }
    };

    join!(main_block, phase_transition_block);

    debug!("UDP listener process killed");
}

