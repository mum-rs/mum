use crate::network::ConnectionInfo;
use crate::state::{State, StatePhase};

use futures::{join, pin_mut, select, FutureExt, SinkExt, StreamExt, Stream};
use futures_util::stream::{SplitSink, SplitStream};
use log::*;
use mumble_protocol::crypt::ClientCryptState;
use mumble_protocol::ping::{PingPacket, PongPacket};
use mumble_protocol::voice::VoicePacket;
use mumble_protocol::Serverbound;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::net::{Ipv6Addr, SocketAddr};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{interval, Duration};
use tokio_util::udp::UdpFramed;

pub type PingRequest = (u64, SocketAddr, Box<dyn FnOnce(PongPacket)>);

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
        let (sink, source) = connect(&mut crypt_state_receiver).await;

        let sink = Arc::new(Mutex::new(sink));
        let source = Arc::new(Mutex::new(source));

        let phase_watcher = state.lock().unwrap().phase_receiver();
        let last_ping_recv = AtomicU64::new(0);
        let mut audio_receiver_lock = receiver.lock().unwrap();
        join!(
            listen(
                Arc::clone(&state),
                Arc::clone(&source),
                phase_watcher.clone(),
                &last_ping_recv,
            ),
            send_voice(
                Arc::clone(&sink),
                connection_info.socket_addr,
                phase_watcher,
                &mut *audio_receiver_lock,
            ),
            send_pings(
                Arc::clone(&state),
                Arc::clone(&sink),
                connection_info.socket_addr,
                &last_ping_recv,
            ),
            new_crypt_state(&mut crypt_state_receiver, sink, source),
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
        if let Some(crypt_state) = crypt_state.recv().await {
            info!("Received new crypt state");
            let udp_socket = UdpSocket::bind((Ipv6Addr::from(0u128), 0u16))
                .await
                .expect("Failed to bind UDP socket");
            let (new_sink, new_source) = UdpFramed::new(udp_socket, crypt_state).split();
            *sink.lock().unwrap() = new_sink;
            *source.lock().unwrap() = new_source;
        }
    }
}

async fn listen(
    state: Arc<Mutex<State>>,
    source: Arc<Mutex<UdpReceiver>>,
    mut phase_watcher: watch::Receiver<StatePhase>,
    last_ping_recv: &AtomicU64,
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
                        VoicePacket::Ping { timestamp } => {
                            state
                                .lock()
                                .unwrap()
                                .broadcast_phase(StatePhase::Connected);
                            last_ping_recv.store(timestamp, Ordering::Relaxed);
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

async fn send_pings(
    state: Arc<Mutex<State>>,
    sink: Arc<Mutex<UdpSender>>,
    server_addr: SocketAddr,
    last_ping_recv: &AtomicU64,
) {
    let mut last_send = None;
    let mut interval = interval(Duration::from_millis(1000));

    loop {
        let last_recv = last_ping_recv.load(Ordering::Relaxed);
        if last_send.is_some() && last_send.unwrap() != last_recv {
            state
                .lock()
                .unwrap()
                .broadcast_phase(StatePhase::Connected);
        }
        match sink
            .lock()
            .unwrap()
            .send((VoicePacket::Ping { timestamp: 0 }, server_addr))
            .await
        {
            Ok(_) => {
                last_send = Some(last_recv + 1);
            },
            Err(e) => {
                debug!("Error sending UDP ping: {}", e);
            }
        }
        interval.tick().await;
    }
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
    mut ping_request_receiver: mpsc::UnboundedReceiver<PingRequest>,
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
