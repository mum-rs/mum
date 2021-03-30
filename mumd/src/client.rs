use crate::command;
use crate::error::ClientError;
use crate::network::{tcp, udp, ConnectionInfo};
use crate::state::State;

use futures_util::{select, FutureExt};
use mumble_protocol::{Serverbound, control::ControlPacket, crypt::ClientCryptState};
use mumlib::command::{Command, CommandResponse};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, oneshot, watch};

pub async fn handle(
    state: State,
    command_receiver: mpsc::UnboundedReceiver<(
        Command,
        oneshot::Sender<mumlib::error::Result<Option<CommandResponse>>>,
    )>,
) -> Result<(), ClientError> {
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

    let state = Arc::new(Mutex::new(state));

    select! {
        r = tcp::handle(
            Arc::clone(&state),
            connection_info_receiver.clone(),
            crypt_state_sender,
            packet_sender.clone(),
            packet_receiver,
            response_receiver,
        ).fuse() => r.map_err(|e| ClientError::TcpError(e)),
        _ = udp::handle(
            Arc::clone(&state),
            connection_info_receiver.clone(),
            crypt_state_receiver,
        ).fuse() => Ok(()),
        _ = command::handle(
            state,
            command_receiver,
            response_sender,
            ping_request_sender,
            packet_sender,
            connection_info_sender,
        ).fuse() => Ok(()),
        _ = udp::handle_pings(ping_request_receiver).fuse() => Ok(()),
    }
}
