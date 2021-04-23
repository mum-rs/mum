use crate::audio::output::AudioOutputMessage;
use crate::error::{TcpError, ConnectionError};
use crate::state::State;
use crate::state::server::Server;
use log::*;

use futures_util::{SinkExt, StreamExt};
use futures_util::stream::{SplitSink, SplitStream};
use mumble_protocol::control::{ClientControlCodec, ControlCodec, ControlPacket};
use mumble_protocol::control::msgs;
use mumble_protocol::voice::VoicePacket;
use mumble_protocol::{Clientbound, Serverbound};
use std::convert::Into;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use tokio::time::{self, Duration};
use tokio::select;
use tokio_native_tls::{TlsConnector, TlsStream};
use tokio_util::codec::{Decoder, Framed};


type TcpSender = SplitSink<
    Framed<TlsStream<TcpStream>, ControlCodec<Serverbound, Clientbound>>,
    ControlPacket<Serverbound>,
>;
type TcpReceiver =
    SplitStream<Framed<TlsStream<TcpStream>, ControlCodec<Serverbound, Clientbound>>>;

pub async fn handle(state: Arc<RwLock<State>>) -> Result<(), TcpError> {
    let state_lock = state.read().await;
    let mut server_recv = state_lock.server_recv();
    let tcp_send = state_lock.get_tcp_sink_sender();
    drop(state_lock);

    loop {
        // wait not being in idle state
        if server_recv.borrow().is_none() {
            server_recv.changed().await.unwrap();
            continue;
        }

        let server = match server_recv.borrow().as_ref() {
            Some(server) => server.clone(),
            None => continue, //disconnected???
        };

        let (mut sink, stream) = connect(&server).await?;

        // TODO: Handshake (omitting `Version` message for brevity)
        authenticate(&mut sink, &server).await?;
        info!("Logging in...");

        //TODO handle errors
        let error = select! {
            _ = tokio::spawn(send_pings(tcp_send.clone(), 10)) => {
                //TODO: handle fatal return
                ConnectionError::GenericError
            },
            //TODO receive pings?
            _ = tokio::spawn(listen(Arc::clone(&state), stream)) => {
                //TODO: handle fatal return
                ConnectionError::GenericError
            },
            _ = tokio::spawn(send_packets(Arc::clone(&state), sink)) => {
                ConnectionError::GenericError
            },
            _ = tokio::spawn(check_changed_server(Arc::clone(&state), server)) => {
                //server changed, set to disconnected, and try to reconnect
                ConnectionError::GenericError
            }
        };
        state.read().await.set_disconnected(error).await;
        debug!("Fully disconnected Tcp");
    }
}

//tcp need to check all server fields
async fn check_changed_server(
    state: Arc<RwLock<State>>,
    current: Server,
) {
    let mut server_recv = state.read().await.server_recv();

    while let Ok(()) = server_recv.changed().await {
        match server_recv.borrow().as_ref() {
            //unchanged
            Some(server) if *server == current => {},
            //changed
            _ => break,
        };
    }
}

async fn connect(server: &Server) -> Result<(TcpSender, TcpReceiver), TcpError> {
    let stream = TcpStream::connect(&server.addr).await?;
    debug!("TCP connected");

    let mut builder = native_tls::TlsConnector::builder();
    builder.danger_accept_invalid_certs(server.accept_invalid_cert);
    let connector: TlsConnector = builder
        .build()
        .map_err(|e| TcpError::TlsConnectorBuilderError(e))?
        .into();
    let tls_stream = connector
        .connect(&server.host, stream)
        .await
        .map_err(|e| TcpError::TlsConnectError(e))?;
    debug!("TLS connected");

    // Wrap the TLS stream with Mumble's client-side control-channel codec
    Ok(ClientControlCodec::new().framed(tls_stream).split())
}

async fn authenticate(
    sink: &mut TcpSender,
    server: &Server,
) -> Result<(), TcpError> {
    let mut msg = msgs::Authenticate::new();
    msg.set_username(server.username.to_owned());
    if let Some(password) = server.password.as_ref() {
        msg.set_password(password.to_owned());
    }
    msg.set_opus(true);
    sink.send(msg.into()).await?;
    Ok(())
}

async fn send_pings(
    packet_sender: mpsc::UnboundedSender<ControlPacket<Serverbound>>,
    delay_seconds: u64,
) -> Result<(), TcpError> {
    let mut interval = time::interval(Duration::from_secs(delay_seconds));
    loop {
        interval.tick().await;
        trace!("Sending TCP ping");
        let msg = msgs::Ping::new();
        packet_sender.send(msg.into())?;
    }
}

async fn send_packets(
    state: Arc<RwLock<State>>,
    mut sink: TcpSender,
) -> Result<(), TcpError> {
    //need to take care, this receiver should be given back before returning
    let mut packet_receiver = state.read().await.get_tcp_sink_receiver().await
        .ok_or_else(|| TcpError::GenericError)?;
    let error = loop {
        // Safe since we always have at least one sender alive.
        let packet = packet_receiver.recv().await.unwrap();
        if let Err(_) = sink.send(packet).await {
            break TcpError::GenericError;
        }
    };
    state.read().await.set_tcp_sink_receiver(packet_receiver).await;
    Err(error)
}

async fn listen(
    state: Arc<RwLock<State>>,
    mut stream: TcpReceiver,
) -> Result<(), TcpError> {
    let audio_sink = state.read().await.get_audio_sink_sender();
    loop {
        let packet = match stream.next().await {
            Some(Ok(packet)) => packet,
            Some(Err(e)) => {
                error!("TCP error: {:?}", e);
                return Err(TcpError::GenericError);
                //TODO Break here? Maybe look at the error and handle it
            }
            None => {
                // We end up here if the login was rejected. We probably want
                // to exit before that.
                error!("TCP stream gone");
                return Err(TcpError::GenericError);
            }
        };
        match packet {
            ControlPacket::TextMessage(msg) => {
                info!(
                    "Got message from user with session ID {}: {}",
                    msg.get_actor(),
                    msg.get_message()
                );
            }
            ControlPacket::CryptSetup(msg) => {
                debug!("Crypt setup");
                state.read().await.set_crypt(Some(msg)).await;
            }
            ControlPacket::ServerSync(msg) => {
                info!("Logged in");
                //TODO check connection context
                //TODO check if error
                if let Err(_) = state.read().await.set_connected(msg).await {
                    return Err(TcpError::GenericError);
                }
            }
            ControlPacket::Reject(msg) => {
                debug!("Login rejected: {:?}", msg);
                //TODO check connection context
                match msg.get_field_type() {
                    msgs::Reject_RejectType::WrongServerPW => {
                        return Err(TcpError::GenericError);
                    }
                    ty => {
                        warn!("Unhandled reject type: {:?}", ty);
                        return Err(TcpError::GenericError);
                    }
                }
            }
            ControlPacket::UserState(msg) => {
                debug!("UserState received");
                let state = state.read().await;
                state.user_parse(msg).await.map_err(|_| TcpError::GenericError)?;
            }
            ControlPacket::UserRemove(msg) => {
                debug!("UserRemove received");
                if msg.has_session() {
                    let state = state.read().await;
                    state.user_remove(msg.get_session()).await
                        .map_err(|_| TcpError::GenericError)?;
                } else {
                    error!("Invalid UserRemove Message");
                    return Err(TcpError::GenericError);
                }
            }
            ControlPacket::ChannelState(msg) => {
                debug!("ChannelState received");
                state.read().await.channel_parse(msg).await
                    .map_err(|_| TcpError::GenericError)?;
            }
            ControlPacket::ChannelRemove(msg) => {
                debug!("ChannelRemove received");
                if msg.has_channel_id() {
                    state.read().await.channel_remove(msg.get_channel_id()).await;
                } else {
                    warn!("Invalid UserRemove Message");
                }
            }
            ControlPacket::Ping(ping) => {
                debug!("Ping tcp received");
                state.read().await.tcp_ping_broadcast_send(ping);
            }
            ControlPacket::UDPTunnel(msg) => {
                match *msg {
                    VoicePacket::Ping { timestamp } => {
                        state.read().await.tcp_tunnel_ping_broadcast_send(timestamp);
                    }
                    VoicePacket::Audio {
                        session_id,
                        //target,
                        seq_num,
                        payload,
                        // position_info,
                        ..
                    } => {
                        //TODO verify seq_num
                        audio_sink.send(AudioOutputMessage::VoicePacket {
                            user_id: session_id,
                            seq_num,
                            data: payload,
                            // position_info,
                        }).unwrap();
                    }
                }
            }
            packet => {
                warn!("Received unhandled ControlPacket {:#?}", packet);
            }
        }
    }
}
