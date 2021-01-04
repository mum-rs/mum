use crate::network::ConnectionInfo;
use crate::state::{State, StatePhase};
use log::*;

use bytes::Bytes;
use futures::{join, pin_mut, select, FutureExt, SinkExt, StreamExt, Stream};
use futures_util::stream::{SplitSink, SplitStream};
use mumble_protocol::crypt::ClientCryptState;
use mumble_protocol::ping::{PingPacket, PongPacket};
use mumble_protocol::voice::{VoicePacket, VoicePacketPayload};
use mumble_protocol::Serverbound;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::net::{Ipv6Addr, SocketAddr};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::udp::UdpFramed;

type UdpSender = SplitSink<UdpFramed<ClientCryptState>, (VoicePacket<Serverbound>, SocketAddr)>;
type UdpReceiver = SplitStream<UdpFramed<ClientCryptState>>;

pub async fn handle(
    state: Arc<Mutex<State>>,
    mut connection_info_receiver: watch::Receiver<Option<ConnectionInfo>>,
    mut crypt_state_receiver: mpsc::Receiver<ClientCryptState>,
) {
    let receiver = state.lock().unwrap().audio_mut().take_receiver();

    loop {
        let connection_info = 'data: loop {
            while connection_info_receiver.changed().await.is_ok() {
                if let Some(data) = connection_info_receiver.borrow().clone() {
                    break 'data data;
                }
            }
            return;
        };
        let (mut sink, source) = connect(&mut crypt_state_receiver).await;

        // Note: A normal application would also send periodic Ping packets, and its own audio
        //       via UDP. We instead trick the server into accepting us by sending it one
        //       dummy voice packet.
        send_ping(&mut sink, connection_info.socket_addr).await;

        let sink = Arc::new(Mutex::new(sink));
        let source = Arc::new(Mutex::new(source));

        let phase_watcher = state.lock().unwrap().phase_receiver();
        let mut audio_receiver_lock = receiver.lock().unwrap();
        join!(
            listen(Arc::clone(&state), Arc::clone(&source), phase_watcher.clone()),
            send_voice(
                Arc::clone(&sink),
                connection_info.socket_addr,
                phase_watcher,
                &mut *audio_receiver_lock
            ),
            new_crypt_state(&mut crypt_state_receiver, sink, source)
        );

        debug!("Fully disconnected UDP stream, waiting for new connection info");
    }
}

async fn connect(
    crypt_state: &mut mpsc::Receiver<ClientCryptState>,
) -> (UdpSender, UdpReceiver) {
    // Bind UDP socket
    let udp_socket = UdpSocket::bind((Ipv6Addr::from(0u128), 0u16))
        .await
        .expect("Failed to bind UDP socket");

    // Wait for initial CryptState
    let crypt_state = match crypt_state.recv().await {
        Some(crypt_state) => crypt_state,
        // disconnected before we received the CryptSetup packet, oh well
        None => panic!("Disconnect before crypt packet received"), //TODO exit gracefully
    };
    debug!("UDP connected");

    // Wrap the raw UDP packets in Mumble's crypto and voice codec (CryptState does both)
    UdpFramed::new(udp_socket, crypt_state).split()
}

async fn new_crypt_state(
    crypt_state: &mut mpsc::Receiver<ClientCryptState>,
    sink: Arc<Mutex<UdpSender>>,
    source: Arc<Mutex<UdpReceiver>>,
) {
    loop {
        match crypt_state.recv().await {
            Some(crypt_state) => {
                info!("Received new crypt state");
                let udp_socket = UdpSocket::bind((Ipv6Addr::from(0u128), 0u16))
                    .await
                    .expect("Failed to bind UDP socket");
                let (new_sink, new_source) = UdpFramed::new(udp_socket, crypt_state).split();
                *sink.lock().unwrap() = new_sink;
                *source.lock().unwrap() = new_source;
            },
            None => {},
        }
    }
}

async fn listen(
    state: Arc<Mutex<State>>,
    source: Arc<Mutex<UdpReceiver>>,
    mut phase_watcher: watch::Receiver<StatePhase>,
) {
    let (tx, rx) = oneshot::channel();
    let phase_transition_block = async {
        loop {
            phase_watcher.changed().await.unwrap();
            if matches!(*phase_watcher.borrow(), StatePhase::Disconnected) {
                break;
            }
        }
        tx.send(true).unwrap();
    };

    let main_block = async {
        let rx = rx.fuse();
        pin_mut!(rx);
        loop {
            let mut source = source.lock().unwrap();
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
                            state
                                .lock()
                                .unwrap()
                                .audio()
                                .decode_packet(session_id, payload);
                        }
                    }
                }
            }
        }
    };

    join!(main_block, phase_transition_block);

    debug!("UDP listener process killed");
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
    mut phase_watcher: watch::Receiver<StatePhase>,
    receiver: &mut (dyn Stream<Item = VoicePacket<Serverbound>> + Unpin),
) {
    pin_mut!(receiver);
    let (tx, rx) = oneshot::channel();
    let phase_transition_block = async {
        loop {
            phase_watcher.changed().await.unwrap();
            if matches!(*phase_watcher.borrow(), StatePhase::Disconnected) {
                break;
            }
        }
        tx.send(true).unwrap();
    };

    let main_block = async {
        let rx = rx.fuse();
        pin_mut!(rx);
        loop {
            let packet_recv = receiver.next().fuse();
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
                Some(Some(reply)) => {
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

    debug!("UDP sender process killed");
}

pub async fn handle_pings(
    mut ping_request_receiver: mpsc::UnboundedReceiver<(
        u64,
        SocketAddr,
        Box<dyn FnOnce(PongPacket)>,
    )>,
) {
    let udp_socket = UdpSocket::bind((Ipv6Addr::from(0u128), 0u16))
        .await
        .expect("Failed to bind UDP socket");

    let pending = Rc::new(Mutex::new(HashMap::new()));

    let sender_handle = async {
        while let Some((id, socket_addr, handle)) = ping_request_receiver.recv().await {
            let packet = PingPacket { id };
            let packet: [u8; 12] = packet.into();
            udp_socket.send_to(&packet, &socket_addr).await.unwrap();
            pending.lock().unwrap().insert(id, handle);
        }
    };

    let receiver_handle = async {
        let mut buf = vec![0; 24];
        while let Ok(read) = udp_socket.recv(&mut buf).await {
            assert_eq!(read, 24);

            let packet = PongPacket::try_from(buf.as_slice()).unwrap();

            if let Some(handler) = pending.lock().unwrap().remove(&packet.id) {
                handler(packet);
            }
        }
    };

    debug!("Waiting for ping requests");

    join!(sender_handle, receiver_handle);
}
