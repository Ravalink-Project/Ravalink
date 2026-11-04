//! Live Discord Voice Gateway websocket driver.
//!
//! The protocol state machine remains independently testable in
//! voice_gateway.rs. This module owns the websocket, performs UDP discovery,
//! dispatches DAVE MLS events, and keeps gateway heartbeats alive.

use crate::{
    AudioError, DaveGatewayCommand, DaveVoiceController, DiscordUdpTransport, VoicePacketPipeline,
    voice_gateway::{VoiceGatewayAction, VoiceGatewayMachine, voice_gateway_url},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::TcpStream,
    sync::{Mutex, mpsc, watch},
    time::Interval,
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

type VoiceWebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// A negotiated Discord voice connection. The websocket control loop runs in
/// its own task while the caller drives the audio player's 20 ms cadence.
pub struct DiscordVoiceConnection {
    pub transport: Arc<DiscordUdpTransport>,
    pub pipeline: VoicePacketPipeline,
    pub dave: Arc<Mutex<DaveVoiceController>>,
    connected: Arc<AtomicBool>,
    shutdown: watch::Sender<bool>,
    speaking_tx: mpsc::UnboundedSender<bool>,
}

impl DiscordVoiceConnection {
    #[allow(clippy::too_many_arguments)]
    pub async fn connect(
        guild_id: u64,
        user_id: u64,
        channel_id: u64,
        discord_session_id: &str,
        token: &str,
        endpoint: &str,
    ) -> Result<Self, AudioError> {
        let url =
            voice_gateway_url(endpoint).map_err(|error| AudioError::invalid(error.to_string()))?;
        let (mut socket, _) = connect_async(url).await.map_err(|error| {
            AudioError::invalid(format!(
                "Discord voice websocket connection failed: {error}"
            ))
        })?;
        let mut machine = VoiceGatewayMachine::new(
            guild_id.to_string(),
            user_id.to_string(),
            discord_session_id,
            token,
        )
        .map_err(|error| AudioError::invalid(error.to_string()))?;

        let mut heartbeat: Option<Interval> = None;
        let mut transport: Option<DiscordUdpTransport> = None;
        loop {
            let incoming = if let Some(interval) = heartbeat.as_mut() {
                tokio::select! {
                    _ = interval.tick() => {
                        send_voice_json(&mut socket, machine.heartbeat(unix_time_ms())).await?;
                        continue;
                    }
                    incoming = socket.next() => incoming,
                }
            } else {
                socket.next().await
            };
            let Some(incoming) = incoming else {
                return Err(AudioError::invalid(
                    "Discord closed the voice websocket during handshake",
                ));
            };
            let message = incoming.map_err(|error| {
                AudioError::invalid(format!("Discord voice websocket handshake failed: {error}"))
            })?;

            match message {
                Message::Text(text) => {
                    let actions = machine
                        .handle_message(text.as_str(), unix_time_ms())
                        .map_err(|error| AudioError::invalid(error.to_string()))?;
                    for action in actions {
                        match action {
                            VoiceGatewayAction::StartHeartbeat { interval_ms } => {
                                let mut interval =
                                    tokio::time::interval(Duration::from_millis(interval_ms));
                                interval.tick().await;
                                heartbeat = Some(interval);
                            }
                            VoiceGatewayAction::Send(payload) => {
                                send_voice_json(&mut socket, payload).await?;
                            }
                            VoiceGatewayAction::DiscoverUdp { remote, ssrc } => {
                                let discovered = DiscordUdpTransport::discover(
                                    remote,
                                    ssrc,
                                    Duration::from_secs(5),
                                )
                                .await?;
                                let select_protocol = machine
                                    .on_udp_discovered(
                                        discovered.external_address(),
                                        discovered.external_port(),
                                    )
                                    .map_err(|error| AudioError::invalid(error.to_string()))?;
                                send_voice_json(&mut socket, select_protocol).await?;
                                transport = Some(discovered);
                            }
                            VoiceGatewayAction::StartVoiceOutput {
                                ssrc,
                                mode,
                                secret_key,
                                dave_protocol_version,
                            } => {
                                let pipeline =
                                    VoicePacketPipeline::new(ssrc, 0, 0, secret_key, mode)?;
                                let mut dave = DaveVoiceController::new(
                                    user_id,
                                    channel_id,
                                    dave_protocol_version,
                                )
                                .map_err(|error| AudioError::invalid(error.to_string()))?;
                                dave.on_clients_connect(
                                    machine.connected_user_ids().iter().copied(),
                                );
                                let transport = transport.ok_or_else(|| {
                                    AudioError::invalid(
                                        "Discord voice session completed without UDP discovery",
                                    )
                                })?;
                                let transport = Arc::new(transport);
                                let dave = Arc::new(Mutex::new(dave));
                                let connected = Arc::new(AtomicBool::new(true));
                                let (shutdown, shutdown_rx) = watch::channel(false);
                                tokio::spawn(run_voice_udp_keepalive(
                                    transport.clone(),
                                    connected.clone(),
                                    shutdown_rx.clone(),
                                ));
                                let (speaking_tx, speaking_rx) = mpsc::unbounded_channel();
                                let heartbeat = heartbeat.ok_or_else(|| {
                                    AudioError::invalid(
                                        "Discord voice session completed without heartbeat setup",
                                    )
                                })?;
                                tokio::spawn(run_voice_gateway_socket(
                                    socket,
                                    machine,
                                    heartbeat,
                                    dave.clone(),
                                    connected.clone(),
                                    shutdown_rx,
                                    speaking_rx,
                                    endpoint.to_owned(),
                                ));
                                return Ok(Self {
                                    transport,
                                    pipeline,
                                    dave,
                                    connected,
                                    shutdown,
                                    speaking_tx,
                                });
                            }
                            VoiceGatewayAction::ClientsConnect { .. }
                            | VoiceGatewayAction::ClientDisconnect { .. }
                            | VoiceGatewayAction::DaveEvent { .. }
                            | VoiceGatewayAction::DaveBinaryEvent { .. }
                            | VoiceGatewayAction::Resumed
                            | VoiceGatewayAction::HeartbeatAck => {}
                        }
                    }
                }
                Message::Ping(payload) => {
                    socket.send(Message::Pong(payload)).await.map_err(|error| {
                        AudioError::invalid(format!("Discord voice websocket pong failed: {error}"))
                    })?;
                }
                Message::Close(_) => {
                    return Err(AudioError::invalid(
                        "Discord closed the voice websocket during handshake",
                    ));
                }
                Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
            }
        }
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    pub fn set_speaking(&self, speaking: bool) -> Result<(), AudioError> {
        self.speaking_tx
            .send(speaking)
            .map_err(|_| AudioError::invalid("Discord voice websocket control task has stopped"))
    }
}

impl Drop for DiscordVoiceConnection {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

async fn run_voice_udp_keepalive(
    transport: Arc<DiscordUdpTransport>,
    connected: Arc<AtomicBool>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    interval.tick().await;
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            _ = interval.tick() => {
                if let Err(error) = transport.send_keepalive(unix_time_ms()).await {
                    tracing::warn!(%error, "Discord voice UDP keepalive failed");
                    connected.store(false, Ordering::Release);
                    break;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_voice_gateway_socket(
    mut socket: VoiceWebSocket,
    mut machine: VoiceGatewayMachine,
    mut heartbeat: Interval,
    dave: Arc<Mutex<DaveVoiceController>>,
    connected: Arc<AtomicBool>,
    mut shutdown: watch::Receiver<bool>,
    mut speaking_rx: mpsc::UnboundedReceiver<bool>,
    endpoint: String,
) {
    let mut heartbeat_timeout = heartbeat.period().saturating_mul(2);
    let mut last_heartbeat_ack = tokio::time::Instant::now();
    loop {
        let mut connection_lost = false;
        'read: loop {
            let incoming = tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break 'read;
                    }
                    continue;
                }
                speaking = speaking_rx.recv() => {
                    if let Some(speaking) = speaking {
                        match machine.speaking_state(speaking) {
                            Ok(payload) => {
                                if let Err(error) = send_voice_json(&mut socket, payload).await {
                                    tracing::warn!(%error, "Discord voice speaking update failed");
                                    connection_lost = true;
                                    break 'read;
                                }
                            }
                            Err(error) => {
                                tracing::warn!(%error, "Discord voice speaking update rejected");
                                connection_lost = true;
                                break 'read;
                            }
                        }
                    }
                    continue;
                }
                _ = heartbeat.tick() => {
                    if let Err(error) =
                        send_voice_json(&mut socket, machine.heartbeat(unix_time_ms())).await
                    {
                        tracing::warn!(%error, "Discord voice heartbeat failed");
                        connection_lost = true;
                        break 'read;
                    }
                    continue;
                }
                _ = tokio::time::sleep_until(last_heartbeat_ack + heartbeat_timeout) => {
                    tracing::warn!("Discord voice heartbeat acknowledgement timed out");
                    connection_lost = true;
                    break 'read;
                }
                incoming = socket.next() => incoming,
            };
            let Some(incoming) = incoming else {
                connection_lost = true;
                break;
            };
            let message = match incoming {
                Ok(message) => message,
                Err(error) => {
                    tracing::warn!(%error, "Discord voice websocket failed");
                    connection_lost = true;
                    break;
                }
            };

            let actions = match message {
                Message::Text(text) => machine.handle_message(text.as_str(), unix_time_ms()),
                Message::Binary(bytes) => machine.handle_binary_message(&bytes),
                Message::Ping(payload) => {
                    if let Err(error) = socket.send(Message::Pong(payload)).await {
                        tracing::warn!(%error, "Discord voice websocket pong failed");
                        connection_lost = true;
                        break;
                    }
                    continue;
                }
                Message::Close(frame) => {
                    tracing::debug!(?frame, "Discord voice websocket closed");
                    connection_lost = true;
                    break;
                }
                Message::Pong(_) | Message::Frame(_) => continue,
            };
            let actions = match actions {
                Ok(actions) => actions,
                Err(error) => {
                    tracing::warn!(%error, "Invalid Discord voice gateway event");
                    connection_lost = true;
                    break;
                }
            };

            for action in actions {
                let result = match action {
                    VoiceGatewayAction::Send(payload) => {
                        send_voice_json(&mut socket, payload).await
                    }
                    VoiceGatewayAction::DaveEvent { opcode, data } => {
                        let commands = {
                            let mut dave = dave.lock().await;
                            dave.handle_json_event(opcode, &data)
                        };
                        match commands {
                            Ok(commands) => send_dave_commands(&mut socket, commands).await,
                            Err(error) => {
                                tracing::warn!(%error, opcode, "DAVE JSON event was rejected");
                                Ok(())
                            }
                        }
                    }
                    VoiceGatewayAction::DaveBinaryEvent {
                        sequence,
                        opcode,
                        payload,
                    } => {
                        let commands = {
                            let mut dave = dave.lock().await;
                            dave.handle_binary_event(opcode, &payload)
                        };
                        match commands {
                            Ok(commands) => send_dave_commands(&mut socket, commands).await,
                            Err(error) => {
                                tracing::warn!(%error, opcode, sequence, "DAVE MLS event was rejected");
                                if matches!(opcode, 29 | 30) && payload.len() >= 2 {
                                    let transition_id =
                                        u16::from_be_bytes([payload[0], payload[1]]);
                                    send_dave_commands(
                                        &mut socket,
                                        vec![DaveGatewayCommand::Binary {
                                            opcode: 31,
                                            payload: transition_id.to_be_bytes().to_vec(),
                                        }],
                                    )
                                    .await
                                } else {
                                    Ok(())
                                }
                            }
                        }
                    }
                    VoiceGatewayAction::ClientsConnect { user_ids } => {
                        dave.lock().await.on_clients_connect(user_ids);
                        Ok(())
                    }
                    VoiceGatewayAction::ClientDisconnect { user_id } => {
                        dave.lock().await.on_client_disconnect(user_id);
                        Ok(())
                    }
                    VoiceGatewayAction::HeartbeatAck => {
                        last_heartbeat_ack = tokio::time::Instant::now();
                        Ok(())
                    }
                    VoiceGatewayAction::StartHeartbeat { .. }
                    | VoiceGatewayAction::DiscoverUdp { .. }
                    | VoiceGatewayAction::StartVoiceOutput { .. }
                    | VoiceGatewayAction::Resumed => Ok(()),
                };
                if let Err(error) = result {
                    tracing::warn!(%error, "Failed to handle Discord voice gateway action");
                    connection_lost = true;
                    break;
                }
            }
            if connection_lost {
                break;
            }
        }

        if *shutdown.borrow() || shutdown.has_changed().is_err() {
            break;
        }
        if !connection_lost {
            break;
        }

        let mut retry_delay = Duration::from_secs(1);
        let mut resumed = false;
        loop {
            match resume_voice_gateway(&endpoint, &mut machine, &dave, &mut shutdown).await {
                Ok((resumed_socket, resumed_heartbeat)) => {
                    socket = resumed_socket;
                    heartbeat_timeout = resumed_heartbeat.period().saturating_mul(2);
                    heartbeat = resumed_heartbeat;
                    last_heartbeat_ack = tokio::time::Instant::now();
                    tracing::info!("Discord voice websocket session resumed");
                    resumed = true;
                    break;
                }
                Err(error) => {
                    tracing::warn!(
                        %error,
                        retry_in_ms = retry_delay.as_millis(),
                        "Discord voice session resume failed; retrying"
                    );
                    tokio::select! {
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() {
                                break;
                            }
                        }
                        _ = tokio::time::sleep(retry_delay) => {}
                    }
                    if *shutdown.borrow() {
                        break;
                    }
                    retry_delay = next_resume_backoff(retry_delay);
                }
            }
        }
        if !resumed {
            break;
        }
    }
    connected.store(false, Ordering::Release);
}

fn next_resume_backoff(current: Duration) -> Duration {
    current.saturating_mul(2).min(Duration::from_secs(30))
}

async fn resume_voice_gateway(
    endpoint: &str,
    machine: &mut VoiceGatewayMachine,
    dave: &Arc<Mutex<DaveVoiceController>>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(VoiceWebSocket, Interval), AudioError> {
    let url =
        voice_gateway_url(endpoint).map_err(|error| AudioError::invalid(error.to_string()))?;
    let connect = tokio::time::timeout(Duration::from_secs(15), connect_async(url));
    let (mut socket, _) = tokio::select! {
        changed = shutdown.changed() => {
            if changed.is_err() || *shutdown.borrow() {
                return Err(AudioError::invalid("voice resume was cancelled"));
            }
            return Err(AudioError::invalid("voice resume was interrupted"));
        }
        result = connect => match result {
            Ok(Ok(connection)) => connection,
            Ok(Err(error)) => {
                return Err(AudioError::invalid(format!(
                    "voice websocket resume connect failed: {error}"
                )));
            }
            Err(_) => {
                return Err(AudioError::invalid("voice websocket resume connection timed out"));
            }
        }
    };
    machine.request_resume();
    let mut heartbeat: Option<Interval> = None;
    let resume_deadline = tokio::time::Instant::now() + Duration::from_secs(15);

    loop {
        let incoming = if let Some(interval) = heartbeat.as_mut() {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return Err(AudioError::invalid("voice resume was cancelled"));
                    }
                    continue;
                }
                _ = tokio::time::sleep_until(resume_deadline) => {
                    return Err(AudioError::invalid("Discord voice session resume timed out"));
                }
                _ = interval.tick() => {
                    send_voice_json(&mut socket, machine.heartbeat(unix_time_ms())).await?;
                    continue;
                }
                incoming = socket.next() => incoming,
            }
        } else {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return Err(AudioError::invalid("voice resume was cancelled"));
                    }
                    continue;
                }
                _ = tokio::time::sleep_until(resume_deadline) => {
                    return Err(AudioError::invalid("Discord voice session resume timed out"));
                }
                incoming = socket.next() => incoming,
            }
        };
        let Some(incoming) = incoming else {
            return Err(AudioError::invalid(
                "Discord closed the voice websocket during resume",
            ));
        };
        let message = incoming.map_err(|error| {
            AudioError::invalid(format!("voice websocket resume failed: {error}"))
        })?;
        let actions = match message {
            Message::Text(text) => machine
                .handle_message(text.as_str(), unix_time_ms())
                .map_err(|error| AudioError::invalid(error.to_string()))?,
            Message::Binary(bytes) => machine
                .handle_binary_message(&bytes)
                .map_err(|error| AudioError::invalid(error.to_string()))?,
            Message::Ping(payload) => {
                socket.send(Message::Pong(payload)).await.map_err(|error| {
                    AudioError::invalid(format!("voice websocket resume pong failed: {error}"))
                })?;
                continue;
            }
            Message::Close(_) => {
                return Err(AudioError::invalid(
                    "Discord rejected the voice session resume",
                ));
            }
            Message::Pong(_) | Message::Frame(_) => continue,
        };

        let mut resumed = false;
        for action in actions {
            match action {
                VoiceGatewayAction::StartHeartbeat { interval_ms } => {
                    let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
                    interval.tick().await;
                    heartbeat = Some(interval);
                }
                VoiceGatewayAction::Send(payload) => {
                    send_voice_json(&mut socket, payload).await?;
                }
                VoiceGatewayAction::DaveEvent { opcode, data } => {
                    let commands = dave
                        .lock()
                        .await
                        .handle_json_event(opcode, &data)
                        .map_err(|error| AudioError::invalid(error.to_string()))?;
                    send_dave_commands(&mut socket, commands).await?;
                }
                VoiceGatewayAction::DaveBinaryEvent {
                    opcode, payload, ..
                } => {
                    let commands = dave
                        .lock()
                        .await
                        .handle_binary_event(opcode, &payload)
                        .map_err(|error| AudioError::invalid(error.to_string()))?;
                    send_dave_commands(&mut socket, commands).await?;
                }
                VoiceGatewayAction::ClientsConnect { user_ids } => {
                    dave.lock().await.on_clients_connect(user_ids);
                }
                VoiceGatewayAction::ClientDisconnect { user_id } => {
                    dave.lock().await.on_client_disconnect(user_id);
                }
                VoiceGatewayAction::Resumed => resumed = true,
                VoiceGatewayAction::DiscoverUdp { .. }
                | VoiceGatewayAction::StartVoiceOutput { .. }
                | VoiceGatewayAction::HeartbeatAck => {}
            }
        }
        if resumed {
            let heartbeat = heartbeat.ok_or_else(|| {
                AudioError::invalid("Discord resumed the voice session without heartbeat setup")
            })?;
            return Ok((socket, heartbeat));
        }
    }
}

async fn send_dave_commands(
    socket: &mut VoiceWebSocket,
    commands: Vec<DaveGatewayCommand>,
) -> Result<(), AudioError> {
    for command in commands {
        match command {
            DaveGatewayCommand::Json(payload) => send_voice_json(socket, payload).await?,
            DaveGatewayCommand::Binary { opcode, payload } => {
                let frame = encode_dave_binary_frame(opcode, &payload);
                socket
                    .send(Message::Binary(frame.into()))
                    .await
                    .map_err(|error| {
                        AudioError::invalid(format!("DAVE binary gateway send failed: {error}"))
                    })?;
            }
        }
    }
    Ok(())
}

fn encode_dave_binary_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(1 + payload.len());
    frame.push(opcode);
    frame.extend_from_slice(payload);
    frame
}

async fn send_voice_json(socket: &mut VoiceWebSocket, payload: Value) -> Result<(), AudioError> {
    socket
        .send(Message::Text(payload.to_string().into()))
        .await
        .map_err(|error| AudioError::invalid(format!("Discord voice gateway send failed: {error}")))
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{encode_dave_binary_frame, next_resume_backoff};
    use std::time::Duration;

    #[test]
    fn client_dave_binary_messages_start_with_the_opcode_without_server_sequence() {
        assert_eq!(
            encode_dave_binary_frame(26, &[0xaa, 0xbb]),
            vec![26, 0xaa, 0xbb]
        );
    }

    #[test]
    fn voice_gateway_resume_backoff_doubles_and_caps_at_thirty_seconds() {
        assert_eq!(
            next_resume_backoff(Duration::from_secs(1)),
            Duration::from_secs(2)
        );
        assert_eq!(
            next_resume_backoff(Duration::from_secs(16)),
            Duration::from_secs(30)
        );
        assert_eq!(
            next_resume_backoff(Duration::from_secs(30)),
            Duration::from_secs(30)
        );
    }
}
