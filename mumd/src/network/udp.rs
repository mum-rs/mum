use crate::error::UdpError;
use crate::state::State;
use crate::audio::output::AudioOutputMessage;

use futures_util::{SinkExt, StreamExt};
use log::*;
use mumble_protocol::crypt::ClientCryptState;
use mumble_protocol::voice::VoicePacket;
use mumble_protocol::control::msgs::CryptSetup;
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::convert::TryInto;
use tokio::net::UdpSocket;
use tokio::sync::broadcast;
use tokio::sync::{watch, RwLock};
use tokio::time::{timeout, Duration};
use tokio_util::udp::UdpFramed;
use tokio::select;

pub async fn handle(state: Arc<RwLock<State>>) -> Result<(), UdpError> {
    let state_lock = state.read().await;
    let mut server = state_lock.server_recv();
    let mut connected = state_lock.connected_recv();
    let mut crypt = state_lock.crypt_recv();
    drop(state_lock);

    loop {
        // wait not being in idle state
        if server.borrow().is_none() {
            server.changed().await.unwrap();
            continue;
        }

        //wait the server connection
        if !*connected.borrow() {
            connected.changed().await.unwrap();
            continue;
        }

        //wait the crypt
        if crypt.borrow().is_none() {
            crypt.changed().await.unwrap();
            continue;
        }

        let udp_framed = connect(&crypt).await?;

        let addr = match server.borrow().as_ref() {
            Some(server) => server.addr.clone(),
            None => continue, //disconnected
        };

        select!(
            _ = tokio::spawn(handle_socket(Arc::clone(&state), udp_framed, addr)) => {},
            _ = tokio::spawn(send_pings(Arc::clone(&state))) => {},
            _ = tokio::spawn(check_changed_server(Arc::clone(&state), addr)) => {}
        );
        debug!("Fully disconnected UDP stream");
    }
}

//udp only care if we disconnect or addr changed
async fn check_changed_server(
    state: Arc<RwLock<State>>,
    addr: SocketAddr,
) {
    let mut server = state.read().await.server_recv();
    while let Ok(()) = server.changed().await {
        match server.borrow().as_ref() {
            //unchanged
            Some(server) if server.addr == addr => {},
            //changed
            _ => break,
        };
    }
}

async fn connect(
    crypt_state: &watch::Receiver<Option<Box<CryptSetup>>>,
) -> Result<UdpFramed<ClientCryptState>, UdpError> {
    // Bind UDP socket
    let udp_socket = UdpSocket::bind((Ipv6Addr::from(0u128), 0u16))
        .await?;

    // Wait for initial CryptState
    let crypt_state = new_crypt_state(crypt_state)?;
    debug!("UDP connected");

    // Wrap the raw UDP packets in Mumble's crypto and voice codec (CryptState does both)
    let udp_framed = UdpFramed::new(udp_socket, crypt_state);
    Ok(udp_framed)
}

fn new_crypt_state(
    crypt: &watch::Receiver<Option<Box<CryptSetup>>>,
) -> Result<ClientCryptState, UdpError> {
    let key = crypt.borrow().to_owned();
    // disconnected before we received the CryptSetup packet, oh well
    let key = key.ok_or_else(|| UdpError::DisconnectBeforeCryptSetup)?;
    Ok(ClientCryptState::new_from(
        key.get_key()
            .try_into()
            .expect("Server sent private key with incorrect size"),
        key.get_client_nonce()
            .try_into()
            .expect("Server sent client_nonce with incorrect size"),
        key.get_server_nonce()
            .try_into()
            .expect("Server sent server_nonce with incorrect size"),
    ))
}

//TODO break this function in three: send packet, recv packet, crypt
async fn handle_socket(
    state: Arc<RwLock<State>>,
    mut source: UdpFramed<ClientCryptState>,
    addr: SocketAddr,
) -> Result<(), UdpError> {
    let state_lock = state.read().await;

    let audio_send = state.read().await.get_audio_sink_sender();
    let mut udp_audio = state_lock.get_udp_sink_receiver().await.unwrap();
    let mut crypt = state_lock.crypt_recv();

    drop(state_lock);

    //carefull not to return without giving udp_audio back
    let error = loop {
        select!(
            packet_net = source.next() => {
                let (packet, _src_addr) = match packet_net {
                    Some(Ok(packet)) => packet,
                    Some(Err(err)) => {
                        warn!("Got an invalid UDP packet: {}", err);
                        // To be expected, considering this is the internet, just ignore it
                        continue;
                    },
                    None => {
                        //socket closed
                        break UdpError::GenericError;
                    },
                };
                match packet {
                    VoicePacket::Ping { timestamp } => {
                        state.read().await.udp_ping_broadcast_send(timestamp);
                    }
                    VoicePacket::Audio {
                        session_id,
                        //target,
                        seq_num, //TODO check packet seq
                        payload,
                        // position_info,
                        ..
                    } => {
                        //TODO verify seq_num
                        audio_send.send(AudioOutputMessage::VoicePacket {
                            user_id: session_id,
                            seq_num,
                            data: payload,
                        }).unwrap();
                    }
                }
            },
            packet = udp_audio.recv() => {
                //safe because there always be a state.udp_audio_send
                let packet = packet.unwrap();
                if let Err(_) = source.send((packet, addr)).await {
                    break UdpError::GenericError;
                }
            },
            _ = crypt.changed() => {
                //update crypt on the fly
                match new_crypt_state(&crypt) {
                    Err(_) => break UdpError::GenericError,
                    Ok(crypt_state) => *source.codec_mut() = crypt_state,
                }
            }
        );
    };
    state.read().await.set_udp_sink_receiver(udp_audio).await;
    Err(error)
}

async fn recv_ping(
    last_ping: u64,
    ping_recv: &mut broadcast::Receiver<u64>,
) {
    loop {
        match ping_recv.recv().await {
            //ping received, return
            Ok(ping) if ping == last_ping => return,
            //wrong response, older ping received? wait for next
            Ok(_) => continue,
            Err(_) => panic!("Udp ping broadcast closed"),
        }
    }
}

async fn send_pings(state: Arc<RwLock<State>>) {
    let state_lock = state.read().await;

    let send_packets = state_lock.get_udp_sink_sender();
    let mut ping_recv = state_lock.udp_ping_broadcast_receiver();

    drop(state_lock);

    let mut last_send = 0;
    loop {
        //send the ping
        send_packets.send(VoicePacket::Ping { timestamp: last_send }).unwrap();
        //check if we receive the packet before the timeout
        let received = timeout(
            Duration::from_secs(1),          //timeout is 1s
            recv_ping(last_send, &mut ping_recv), //return if correct ping received
        ).await;
        match received {
            Ok(()) => {
                //ping received, using UDP
                state.read().await.set_link_udp(true).await;
            },
            Err(_) => {
                //timeout, use TCP instead
                state.read().await.set_link_udp(false).await;
            }
        }
        //change the ping id to avoid overlapping responses
        last_send = last_send.wrapping_add(1);
    }
}
