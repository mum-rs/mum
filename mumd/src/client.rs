use crate::command;
use crate::network::{tcp, udp, ConnectionInfo};
use crate::state::State;

use futures_util::join;
use ipc_channel::ipc::IpcSender;
use mumble_protocol::{Serverbound, control::ControlPacket, crypt::ClientCryptState};
use mumlib::command::{Command, CommandResponse};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};

pub async fn handle(
    command_receiver: mpsc::UnboundedReceiver<(
        Command,
        IpcSender<mumlib::error::Result<Option<CommandResponse>>>,
    )>,
) {
    let (connection_info_sender, connection_info_receiver) =
        watch::channel::<Option<ConnectionInfo>>(None);
    let (crypt_state_sender, crypt_state_receiver) =
        mpsc::channel::<ClientCryptState>(1);
    let (packet_sender, packet_receiver) =
        mpsc::unbounded_channel::<ControlPacket<Serverbound>>();
    let (ping_request_sender, ping_request_receiver) =
        mpsc::unbounded_channel();
    let (response_sender, response_receiver) =
        mpsc::unbounded_channel();

    let state = State::new(packet_sender, connection_info_sender);
    let state = Arc::new(Mutex::new(state));
    join!(
        tcp::handle(
            Arc::clone(&state),
            connection_info_receiver.clone(),
            crypt_state_sender,
            packet_receiver,
            response_receiver,
        ),
        udp::handle(
            Arc::clone(&state),
            connection_info_receiver.clone(),
            crypt_state_receiver,
        ),
        command::handle(
            state,
            command_receiver,
            response_sender,
            ping_request_sender,
        ),
        udp::handle_pings(ping_request_receiver),
    );
}
