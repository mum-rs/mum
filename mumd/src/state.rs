pub mod channel;
pub mod server;
pub mod user;

use crate::audio::output::{AudioOutputMessage, NotificationEvents};
use crate::error::{StateError, ConnectionError};
use crate::state::channel::Channel;
use crate::state::server::Server;
use crate::state::user::User;

use log::*;
use mumble_protocol::control::msgs::{self, CryptSetup};
use mumble_protocol::control::ControlPacket;
use mumble_protocol::voice::{VoicePacket, Serverbound};
use mumlib::config::Config;
use std::sync::atomic::{Ordering, AtomicU32};
use std::collections::HashMap;
use tokio::sync::{mpsc, watch, RwLock, Mutex, broadcast};
use tokio::time::{timeout, Duration};

pub struct State {
    pub config: RwLock<Config>,

    pub channels: RwLock<HashMap<u32, RwLock<Channel>>>,
    pub users: RwLock<HashMap<u32, RwLock<User>>>,
    pub welcome_text: RwLock<Option<String>>,

    //None is idle state, Some(server) is try to connected to server
    server_recv: watch::Receiver<Option<Server>>,
    server_send: Mutex<watch::Sender<Option<Server>>>,

    input_volume_state_recv: watch::Receiver<f32>,
    input_volume_state_send: Mutex<watch::Sender<f32>>,

    output_volume_state_recv: watch::Receiver<f32>,
    output_volume_state_send: Mutex<watch::Sender<f32>>,

    muted_recv: watch::Receiver<bool>,
    muted_send: Mutex<watch::Sender<bool>>,
    deaf_recv: watch::Receiver<bool>,
    deaf_send: Mutex<watch::Sender<bool>>,
    connected_recv: watch::Receiver<bool>,
    connected_send: Mutex<watch::Sender<bool>>,
    //TODO use a AtomicBool?
    link_udp_recv: watch::Receiver<bool>,
    link_udp_send: Mutex<watch::Sender<bool>>,
    //valid if connected, so no need for Option
    session_id: AtomicU32,

    crypt_recv: watch::Receiver<Option<Box<CryptSetup>>>,
    crypt_send: Mutex<watch::Sender<Option<Box<CryptSetup>>>>,

    audio_sink_sender: mpsc::UnboundedSender<AudioOutputMessage>,
    audio_sink_receiver: Mutex<Option<mpsc::UnboundedReceiver<AudioOutputMessage>>>,

    tcp_sink_sender: mpsc::UnboundedSender<ControlPacket<Serverbound>>,
    tcp_sink_receiver: Mutex<Option<mpsc::UnboundedReceiver<ControlPacket<Serverbound>>>>,

    udp_sink_sender: mpsc::UnboundedSender<VoicePacket<Serverbound>>,
    udp_sink_receiver: Mutex<Option<mpsc::UnboundedReceiver<VoicePacket<Serverbound>>>>,

    connection_broadcast: broadcast::Sender<Result<(), ConnectionError>>,
    tcp_ping_broadcast: broadcast::Sender<Box<msgs::Ping>>,
    tcp_tunnel_ping_broadcast: broadcast::Sender<u64>,
    udp_ping_broadcast: broadcast::Sender<u64>,
}

impl State {
    pub async fn new(server: Option<Server>) -> Result<Self, StateError> {
        let config = mumlib::config::read_default_cfg()?;

        let (server_send, server_recv) = watch::channel(server);
        let (input_volume_state_send, input_volume_state_recv) = watch::channel(1.0);
        let (output_volume_state_send, output_volume_state_recv) = watch::channel(1.0);
        let (audio_sink_sender, audio_sink_receiver) = mpsc::unbounded_channel();

        let (muted_send, muted_recv) = watch::channel(false);
        let (deaf_send, deaf_recv) = watch::channel(false);
        let (connected_send, connected_recv) = watch::channel(false);
        let (crypt_send, crypt_recv) = watch::channel(None);
        let (link_udp_send, link_udp_recv) = watch::channel(false);
        let (tcp_sink_sender, tcp_sink_receiver) = mpsc::unbounded_channel();
        let (udp_sink_sender, udp_sink_receiver) = mpsc::unbounded_channel();
        let (connection_broadcast, _) = broadcast::channel(10);
        let (tcp_ping_broadcast, _) = broadcast::channel(10);
        let (tcp_tunnel_ping_broadcast, _) = broadcast::channel(10);
        let (udp_ping_broadcast, _) = broadcast::channel(10);

        let state = Self {
            config: RwLock::new(config),
            server_recv,
            server_send: Mutex::new(server_send),
            channels: RwLock::new(HashMap::new()),
            users: RwLock::new(HashMap::new()),
            welcome_text: RwLock::new(None),
            input_volume_state_recv,
            input_volume_state_send: Mutex::new(input_volume_state_send),
            output_volume_state_recv,
            output_volume_state_send: Mutex::new(output_volume_state_send),
            audio_sink_sender,
            audio_sink_receiver: Mutex::new(Some(audio_sink_receiver)),
            muted_recv,
            muted_send: Mutex::new(muted_send),
            deaf_recv,
            deaf_send: Mutex::new(deaf_send),
            connected_recv,
            connected_send: Mutex::new(connected_send),
            link_udp_recv,
            link_udp_send: Mutex::new(link_udp_send),
            session_id: AtomicU32::new(0),
            crypt_recv,
            crypt_send: Mutex::new(crypt_send),
            tcp_sink_sender,
            tcp_sink_receiver: Mutex::new(Some(tcp_sink_receiver)),
            udp_sink_sender,
            udp_sink_receiver: Mutex::new(Some(udp_sink_receiver)),
            connection_broadcast,
            tcp_ping_broadcast,
            tcp_tunnel_ping_broadcast,
            udp_ping_broadcast,
        };
        state.reload_config().await?;
        Ok(state)
    }


    pub fn server_recv(&self) -> watch::Receiver<Option<Server>> {
        self.server_recv.clone()
    }

    pub async fn wait_connected_state(&self, duration: Duration, state: bool) -> bool {
        let mut connected = self.connected_recv();
        timeout(duration, async {
            while *connected.borrow() == state {
                connected.changed().await.unwrap();
            }
        }).await.is_ok()
    }

    pub fn is_idle(&self) -> bool {
        self.server_recv.borrow().is_none()
    }

    pub async fn set_disconnected(&self, reason: ConnectionError) {
        self.channels.write().await.clear();
        self.users.write().await.clear();
        let _ = self.connection_broadcast.send(Err(reason));
    }

    //TODO create an error type for state
    pub async fn set_connected(&self, mut msg: Box<msgs::ServerSync>) -> Result<(), StateError>{
        debug!("Connected to server: {:?}", msg);
        if !msg.has_session() {
            return Err(StateError::GenericError);
        }
        self.set_session_id(msg.get_session());
        if msg.has_welcome_text() {
            let message = msg.take_welcome_text();
            info!("Welcome: {}", message);
            *self.welcome_text.write().await = Some(message);
        }
        self.connected_send.lock().await.send(true).unwrap();
        let _ = self.connection_broadcast.send(Ok(()));
        Ok(())
    }

    pub async fn set_server(&self, server: Option<Server>) {
        debug!("State: new server received: {:?}", server);
        self.server_send.lock().await.send(server).unwrap();
    }

    pub fn get_input_volume_state_recv(&self) -> watch::Receiver<f32> {
        self.input_volume_state_recv.clone()
    }

    pub async fn set_input_volume(&self, volume: f32) {
        debug!("State: new input volume: {}", volume);
        self.input_volume_state_send.lock().await.send(volume).unwrap();
    }

    pub fn get_output_volume_state_recv(&self) -> watch::Receiver<f32> {
        self.output_volume_state_recv.clone()
    }

    pub async fn set_output_volume(&self, volume: f32) {
        debug!("State: new output volume: {}", volume);
        self.output_volume_state_send.lock().await.send(volume).unwrap();
    }

    pub fn get_audio_sink_sender(&self) -> mpsc::UnboundedSender<AudioOutputMessage> {
        self.audio_sink_sender.clone()
    }

    pub async fn get_audio_sink_receiver(&self) -> Option<mpsc::UnboundedReceiver<AudioOutputMessage>> {
        self.audio_sink_receiver.lock().await.take()
    }

    pub async fn set_audio_sink_receiver(&self, value: mpsc::UnboundedReceiver<AudioOutputMessage>) {
        *self.audio_sink_receiver.lock().await = Some(value);
    }

    pub async fn channel_parse(&self, msg: Box<msgs::ChannelState>) -> Result<(), StateError> {
        debug!("State: parse channel: {:?}", msg);
        if !msg.has_channel_id() {
            //TODO panic?
            warn!("Can't parse channel state without channel id");
            return Err(StateError::GenericError);
        }
        //carefull with the deadlock here
        let channels_lock = self.channels.read().await;
        let channel = channels_lock.get(&msg.get_channel_id());
        match channel {
            Some(channel) => {
                //get a write lock so we can update the channel
                channel.write().await.parse_state(msg);
                Ok(())
            },
            None => {
                //get a write lock to create a new entry
                drop(channel);
                drop(channels_lock);
                let mut channels = self.channels.write().await;
                channels.insert(msg.get_channel_id(), RwLock::new(Channel::new(msg)));
                Ok(())
            }
        }
    }

    pub async fn channel_remove(&self, id: u32) {
        debug!("State: remove channel: {}", id);
        //TODO: remove our own channel?
        if let None = self.channels.write().await.remove(&id) {
            warn!("Attempted to remove channel that doesn't exist");
        }
    }

    pub async fn user_parse(&self, msg: Box<msgs::UserState>) -> Result<(), StateError>{
        debug!("State: parse user: {:?}", msg);
        if !msg.has_session() {
            warn!("Can't parse user state without session");
            return Err(StateError::GenericError);
        }
        let id = msg.get_session();
        //carefull with the deadlock here
        let users_lock = self.users.read().await;
        let user = users_lock.get(&id);
        match user {
            Some(user) => {
                //get a write lock so we can update the user
                user.write().await.parse_state(msg);
            },
            None => {
                drop(user);
                drop(users_lock);
                //get a write lock to create a new entry
                self.users.write().await.insert(id, RwLock::new(User::new(msg)));
                if *self.connected_recv().borrow() {
                    //only notify if connected, during the connection process
                    //we will receive all the users, we only want to make the
                    //notification after the connection is finished
                    self.audio_sink_sender.send(AudioOutputMessage::Effects(
                        NotificationEvents::UserConnected
                    )).map_err(|_| StateError::GenericError)?;
                    //notifications::send(format!(
                    //    "{} connected and joined {}",
                    //    &msg.get_name(),
                    //    channel.name()
                    //));
                }
            }
        }
        Ok(())
    }

    pub async fn user_remove(&self, id: u32) -> Result<(), StateError> {
        //let this_channel = self.get_users_channel(self.server().unwrap().session_id().unwrap());
        //let other_channel = self.get_users_channel(msg.get_session());
        //if this_channel == other_channel {
        //    self.audio_output.play_effect(NotificationEvents::UserDisconnected);
        //    if let Some(user) = self.server().unwrap().users().get(&msg.get_session()) {
        //        notifications::send(format!("{} disconnected", &user.name()));
        //    }
        //}

        debug!("State: remove user: {}", id);
        //TODO: remove yourself?
        if let None = self.users.write().await.remove(&id) {
            return Err(StateError::GenericError);
        }
        info!("User {} disconnected", id);
        Ok(())
    }

    pub fn session_id(&self) -> u32 {
        self.session_id.load(Ordering::SeqCst)
    }

    pub fn set_session_id(&self, id: u32) {
        self.session_id.store(id, Ordering::SeqCst)
    }

    pub fn deaf_recv(&self) -> watch::Receiver<bool> {
        self.deaf_recv.clone()
    }

    pub async fn set_deaf(&self, deaf: bool) {
        self.deaf_send.lock().await.send(deaf).unwrap()
    }

    pub fn connected_recv(&self) -> watch::Receiver<bool> {
        self.connected_recv.clone()
    }

    pub fn is_connected(&self) -> bool {
        *self.connected_recv.borrow()
    }

    pub fn muted_recv(&self) -> watch::Receiver<bool> {
        self.muted_recv.clone()
    }

    pub async fn set_mute(&self, muted: bool) {
        self.muted_send.lock().await.send(muted).unwrap()
    }

    pub fn link_udp_recv(&self) -> watch::Receiver<bool> {
        self.link_udp_recv.clone()
    }

    pub async fn set_link_udp(&self, value: bool) {
        self.link_udp_send.lock().await.send(value).unwrap()
    }

    pub fn crypt_recv(&self) -> watch::Receiver<Option<Box<CryptSetup>>> {
        self.crypt_recv.clone()
    }

    pub async fn set_crypt(&self, value: Option<Box<CryptSetup>>) {
        self.crypt_send.lock().await.send(value).unwrap()
    }

    pub fn get_tcp_sink_sender(&self) -> mpsc::UnboundedSender<ControlPacket<Serverbound>> {
        self.tcp_sink_sender.clone()
    }

    pub async fn get_tcp_sink_receiver(&self) -> Option<mpsc::UnboundedReceiver<ControlPacket<Serverbound>>> {
        self.tcp_sink_receiver.lock().await.take()
    }

    pub async fn set_tcp_sink_receiver(&self, value: mpsc::UnboundedReceiver<ControlPacket<Serverbound>>) {
        *self.tcp_sink_receiver.lock().await = Some(value);
    }

    pub fn get_udp_sink_sender(&self) -> mpsc::UnboundedSender<VoicePacket<Serverbound>> {
        self.udp_sink_sender.clone()
    }

    pub async fn get_udp_sink_receiver(&self) -> Option<mpsc::UnboundedReceiver<VoicePacket<Serverbound>>> {
        self.udp_sink_receiver.lock().await.take()
    }

    pub async fn set_udp_sink_receiver(&self, value: mpsc::UnboundedReceiver<VoicePacket<Serverbound>>) {
        *self.udp_sink_receiver.lock().await = Some(value)
    }

    pub fn connection_broadcast_receiver(&self)
    -> broadcast::Receiver<Result<(), ConnectionError>> {
        self.connection_broadcast.subscribe()
    }

    pub fn tcp_ping_broadcast_send(&self, ping: Box<msgs::Ping>) {
        let _ = self.tcp_ping_broadcast.send(ping);
    }

    pub fn tcp_ping_broadcast_receiver(&self)
    -> broadcast::Receiver<Box<msgs::Ping>> {
        self.tcp_ping_broadcast.subscribe()
    }

    pub fn tcp_tunnel_ping_broadcast_send(&self, ping: u64) {
        let _ = self.tcp_tunnel_ping_broadcast.send(ping);
    }

    pub fn tcp_tunnel_ping_broadcast_receiver(&self)
    -> broadcast::Receiver<u64> {
        self.tcp_tunnel_ping_broadcast.subscribe()
    }

    pub fn udp_ping_broadcast_send(&self, ping: u64) {
        let _ = self.udp_ping_broadcast.send(ping);
    }

    pub fn udp_ping_broadcast_receiver(&self)
    -> broadcast::Receiver<u64> {
        self.udp_ping_broadcast.subscribe()
    }

    pub async fn reload_config(&self) -> Result<(), StateError> {
        debug!("State: reload config");
        let config = mumlib::config::read_default_cfg().map_err(|_| StateError::GenericError)?;

        if let Some(input_volume) = config.audio.input_volume {
            self.set_input_volume(input_volume).await;
        }
        if let Some(output_volume) = config.audio.output_volume {
            self.set_output_volume(output_volume).await;
        }
        if let Some(sound_effects) = &config.audio.sound_effects {
            self.audio_sink_sender.send(
                AudioOutputMessage::LoadSoundEffects(sound_effects.to_owned())
            ).unwrap();
        }

        *self.config.write().await = config;
        Ok(())
    }
}
