use crate::state::State;
use crate::state::server::Server;
use crate::state::channel;
use crate::audio::output::{AudioOutputMessage, NotificationEvents};

use log::*;
use mumble_protocol::control::msgs;
use mumlib::command::{Command, CommandResponse};
use mumlib::error::Error;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::net::UnixListener;
use tokio::time::{timeout, Duration};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use futures_util::{SinkExt, StreamExt};
use bytes::{BufMut, BytesMut};

pub async fn receive_commands(state: Arc<RwLock<State>>) {
    let socket = UnixListener::bind(mumlib::SOCKET_PATH).unwrap();

    debug!("Begin listening for commands");
    loop {
        if let Ok((incoming, _)) = socket.accept().await {
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                let (reader, writer) = incoming.into_split();
                let mut reader = FramedRead::new(reader, LengthDelimitedCodec::new());
                let mut writer = FramedWrite::new(writer, LengthDelimitedCodec::new());
                while let Some(next) = reader.next().await {
                    let buf = match next {
                        Ok(buf) => buf,
                        Err(_) => continue, //TODO handle Err?
                    };

                    let command = match bincode::deserialize::<Command>(&buf) {
                        Ok(e) => e,
                        Err(_) => continue, //TODO handle Err?
                    };

                    let response = handle(
                        &state,
                        &command,
                    ).await;

                    let mut serialized = BytesMut::new();
                    bincode::serialize_into((&mut serialized).writer(), &response).unwrap();
                    let _ = writer.send(serialized.freeze()).await;
                }
            });
        }
    }
}

pub async fn handle(
    state: &Arc<RwLock<State>>,
    command: &Command,
    ) -> mumlib::error::Result<Option<CommandResponse>> {
        debug!("Received command {:?}", command);
        match command {
            Command::ChannelJoin { channel_identifier } => {
                let state_lock = state.read().await;
                if state_lock.is_idle() {
                    return Err(Error::GenericError);
                }

                if !state_lock.is_connected() {
                    return Err(Error::GenericError);
                }

                //TODO check for partial matchs?
                let mut id = None;
                let channels = state_lock.channels.read().await;
                for (channel_id, channel) in channels.iter() {
                    let channel = channel.read().await;
                    if channel.name() == channel_identifier {
                        id = Some(channel_id);
                    }
                }
                match id {
                    Some(id) => {
                        let tcp_sink = state_lock.get_tcp_sink_sender();
                        let mut msg = msgs::UserState::new();
                        msg.set_session(state_lock.session_id());
                        msg.set_channel_id(*id);
                        tcp_sink.send(msg.into()).map_err(|_| Error::GenericError)?;
                        //TODO wait for the confirmation packet
                        Ok(None)
                    },
                    None => {
                        Err(Error::Disconnected)
                    }
                }
            }
            Command::ChannelList => {
                let state_lock = state.read().await;
                if state_lock.is_idle() {
                    return Err(Error::GenericError);
                }

                if !state_lock.is_connected() {
                    return Err(Error::GenericError);
                }

                let list = channel::into_channel(state).await;
                Ok(Some(CommandResponse::ChannelList { channels: list }))
            }
            Command::ConfigReload => {
                state.write().await.reload_config().await
                    .map_err(|_| Error::GenericError)?;
                Ok(None)
            }
            Command::DeafenSelf(toggle) => {
                let state_lock = state.read().await;
                if state_lock.is_idle() {
                    return Err(Error::GenericError);
                }

                if !state_lock.is_connected() {
                    return Err(Error::GenericError);
                }
                //TODO set even if disconnected, once connected, deaf ourselfs?

                let deaf_state_recv = state_lock.deaf_recv();

                let deaf = *deaf_state_recv.borrow();
                let action = match (toggle, deaf) {
                    (Some(false), false) => None,
                    (Some(false), true) => Some(false),
                    (Some(true), false) => Some(true),
                    (Some(true), true) => None,
                    (None, false) => Some(true),
                    (None, true) => Some(false),
                };

                if let Some(deafen) = action {
                    state_lock.get_audio_sink_sender().send(
                        AudioOutputMessage::Effects(
                            if deafen {
                                NotificationEvents::Deafen
                            } else {
                                NotificationEvents::Undeafen
                            }
                        )
                    ).unwrap();

                    let tcp = state_lock.get_tcp_sink_sender();
                    state_lock.set_deaf(deafen).await;
                    let mut msg = msgs::UserState::new();
                    msg.set_self_deaf(deafen);
                    tcp.send(msg.into()).unwrap();
                    Ok(Some(CommandResponse::DeafenStatus {
                        is_deafened: deafen,
                    }))
                } else {
                    Ok(Some(CommandResponse::DeafenStatus {
                        is_deafened: deaf,
                    }))
                }
            },
            Command::InputVolumeSet(volume) => {
                let state_lock = state.read().await;
                state_lock.set_input_volume(*volume).await;
                Ok(None)
            },
            Command::MuteOther(username, toggle) => {
                let state_lock = state.read().await;
                if state_lock.is_idle() {
                    return Err(Error::GenericError);
                }

                if !state_lock.is_connected() {
                    return Err(Error::GenericError);
                }

                //search for the user
                let users = state_lock.users.read().await;
                for (_, user) in users.iter() {
                    //acquire user read lock
                    let user_lock = user.read().await;
                    if user_lock.name() == username {
                        //check if need to change the user mute state
                        let mute = match (user_lock.suppressed(), toggle) {
                            (false, Some(false)) | (true, Some(true)) => {
                                return Ok(None);
                            },
                            (false, Some(true)) | (false, None) => true,
                            (true, Some(false)) | (true, None) => false,
                        };
                        //drop user read lock
                        drop(user_lock);
                        //acquire user write lock, set the value, drop the lock
                        user.write().await.set_suppressed(mute);
                        return Ok(None);
                    }
                }
                //user not found
                Err(Error::GenericError)
            },
            Command::MuteSelf(toggle) => {
                let state_lock = state.read().await;
                if state_lock.is_idle() {
                    return Err(Error::GenericError);
                }

                if !state_lock.is_connected() {
                    return Err(Error::GenericError);
                }
                //TODO set even if disconnected, once connected, mute ourselfs?

                let muted_state_recv = state_lock.muted_recv();

                let mute = *muted_state_recv.borrow();
                let action = match (toggle, mute) {
                    (Some(false), false) => None,
                    (Some(false), true) => Some(false),
                    (Some(true), false) => Some(true),
                    (Some(true), true) => None,
                    (None, false) => Some(true),
                    (None, true) => Some(false),
                };

                if let Some(muted) = action {
                    state_lock.get_audio_sink_sender().send(AudioOutputMessage::Effects(if muted {
                        NotificationEvents::Mute
                    } else {
                        NotificationEvents::Unmute
                    })).unwrap();

                    state_lock.set_mute(muted).await;
                    let mut msg = msgs::UserState::new();
                    msg.set_self_mute(muted);
                    state_lock.get_tcp_sink_sender().send(msg.into()).unwrap();
                    Ok(Some(CommandResponse::MuteStatus {
                        is_muted: muted,
                    }))
                } else {
                    Ok(Some(CommandResponse::MuteStatus {
                        is_muted : mute,
                    }))
                }
            },
            Command::OutputVolumeSet(volume) => {
                let state_lock = state.read().await;
                state_lock.set_output_volume(*volume).await;
                Ok(None)
            },
            Command::Ping => {
                Ok(Some(CommandResponse::Pong))
            },
            Command::ServerConnect {
                host,
                port,
                username,
                password,
                accept_invalid_cert,
            } => {
                let server = Server::new(
                    host.to_owned(),
                    *port,
                    username.to_owned(),
                    password.to_owned(),
                    *accept_invalid_cert,
                ).await;
                let state_lock = state.read().await;
                let connected = state_lock.connected_recv();
                //if connected, force disconnection, avoid the race condition
                //of receing a server disconnection before update the server
                //state and after broadcast clone
                if *connected.borrow() {
                    debug!("Command ServerConnect, force disconnection");
                    state_lock.set_server(None).await;
                    //wait 2s for disconnection
                    if !state_lock.wait_connected_state(Duration::from_secs(2), false).await {
                        //didn't disconnected
                        info!("Command ServerConnect, didn't disconnected");
                        //TODO handle fatal error
                        return Err(Error::GenericError);
                    }
                }
                let mut connection = state_lock.connection_broadcast_receiver();

                state_lock.set_server(Some(server)).await;
                //wait 10s for a connection
                let connected = timeout(Duration::from_secs(10), connection.recv());
                //3 results, first for timeout, second for recv, last for connection
                match connected.await {
                    //connection ok
                    Ok(Ok(Ok(()))) => {
                        debug!("Command ServerConnect, Connected");
                        Ok(Some(CommandResponse::ServerConnect {
                            welcome_message: state_lock
                                .welcome_text
                                .read()
                                .await
                                .as_ref()
                                .cloned(),
                        }))
                    },
                    //connection err
                    Ok(Ok(Err(_))) => {
                        //back to idle state
                        debug!("Command ServerConnect, Unable to connect");
                        state_lock.set_server(None).await;
                        Err(Error::GenericError)
                    },
                    //broadcast closed
                    Ok(Err(_)) => {
                        //TODO handle fatal error
                        panic!("Command: Connected broadcast closed");
                    },
                    //timeout
                    Err(_) => {
                        //back to idle state
                        debug!("Command ServerConnect, server didn't respond");
                        state_lock.set_server(None).await;
                        Err(Error::GenericError)
                    }
                }
            },
            Command::ServerDisconnect => {
                let state_lock = state.read().await;
                if state_lock.is_idle() {
                    return Err(Error::GenericError);
                }

                let audio_sink = state_lock.get_audio_sink_sender();
                state_lock.set_server(None).await;
                audio_sink.send(
                    AudioOutputMessage::Effects(NotificationEvents::ServerDisconnect)
                ).unwrap();
                //TODO check connected state?
                Ok(None)
            },
            Command::ServerStatus { host, port } => {
                unimplemented!()
                //ExecutionContext::Ping(
                //Box::new(move || {
                //    match (host.as_str(), port)
                //        .to_socket_addrs()
                //        .map(|mut e| e.next())
                //    {
                //        Ok(Some(v)) => Ok(v),
                //        _ => Err(Error::InvalidServerAddr(host, port)),
                //    }
                //}),
                //Some(CommandResponse::ServerStatus {
                //    version: pong.version,
                //    users: pong.users,
                //    max_users: pong.max_users,
                //    bandwidth: pong.bandwidth,
                //}))
            },
            Command::Status => {
                let state_lock = state.read().await;
                if state_lock.is_idle() {
                    return Err(Error::GenericError);
                }

                unimplemented!()
                //Ok(Some(CommandResponse::Status {
                //    server_state: state, //guaranteed not to panic because if we are connected, server is guaranteed to be Some
                //}))
            }
            Command::UserVolumeSet(username, volume) => {
                let state_lock = state.read().await;
                if state_lock.is_idle() {
                    return Err(Error::GenericError);
                }

                if !state_lock.is_connected() {
                    return Err(Error::GenericError);
                }

                //search for the user
                let users = state_lock.users.read().await;
                for (_, user) in users.iter() {
                    //acquire user read lock
                    let user_lock = user.read().await;
                    if user_lock.name() == username {
                        let user_volume = user_lock.volume;
                        drop(user_lock); //drop user read lock
                        if user_volume != *volume {
                            let mut user_lock = user.write().await;
                            user_lock.volume = *volume;
                        }
                        return Ok(None);
                    }
                }
                //user not found
                Err(Error::GenericError)
            }
        }
}
