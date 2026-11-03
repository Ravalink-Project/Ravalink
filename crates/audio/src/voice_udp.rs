//! Discord voice UDP discovery and encrypted RTP datagram transport.
//!
//! The gateway handshake and DAVE session are negotiated by the server layer.
//! This module handles the UDP discovery exchange and sends already-encrypted
//! RTP voice packets to the selected Discord voice endpoint.

use crate::AudioError;
use std::{net::SocketAddr, time::Duration};
use tokio::{net::UdpSocket, time::timeout};

const DISCOVERY_PACKET_BYTES: usize = 74;
const DISCOVERY_REQUEST_TYPE: u16 = 1;
const DISCOVERY_RESPONSE_TYPE: u16 = 2;

/// Connected UDP socket after Discord voice IP discovery.
pub struct DiscordUdpTransport {
    socket: UdpSocket,
    remote: SocketAddr,
    ssrc: u32,
    external_address: String,
    external_port: u16,
}

impl DiscordUdpTransport {
    /// Bind an ephemeral local UDP port, perform Discord's IP discovery
    /// exchange and retain the connected socket for voice packet transmission.
    pub async fn discover(
        remote: SocketAddr,
        ssrc: u32,
        discovery_timeout: Duration,
    ) -> Result<Self, AudioError> {
        let bind_address = if remote.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let socket = UdpSocket::bind(bind_address)
            .await
            .map_err(|error| AudioError::invalid(format!("voice UDP bind failed: {error}")))?;
        socket
            .connect(remote)
            .await
            .map_err(|error| AudioError::invalid(format!("voice UDP connect failed: {error}")))?;

        let mut request = [0_u8; DISCOVERY_PACKET_BYTES];
        request[..2].copy_from_slice(&DISCOVERY_REQUEST_TYPE.to_be_bytes());
        request[2..4].copy_from_slice(&70_u16.to_be_bytes());
        request[4..8].copy_from_slice(&ssrc.to_be_bytes());
        socket.send(&request).await.map_err(|error| {
            AudioError::invalid(format!("voice UDP discovery send failed: {error}"))
        })?;

        let mut response = [0_u8; 256];
        let received = timeout(discovery_timeout, socket.recv(&mut response))
            .await
            .map_err(|_| AudioError::invalid("voice UDP discovery timed out"))?
            .map_err(|error| {
                AudioError::invalid(format!("voice UDP discovery receive failed: {error}"))
            })?;
        let (external_address, external_port) =
            parse_discovery_response(&response[..received], ssrc)?;

        Ok(Self {
            socket,
            remote,
            ssrc,
            external_address,
            external_port,
        })
    }

    pub fn remote_addr(&self) -> SocketAddr {
        self.remote
    }

    pub fn ssrc(&self) -> u32 {
        self.ssrc
    }

    pub fn external_address(&self) -> &str {
        &self.external_address
    }

    pub fn external_port(&self) -> u16 {
        self.external_port
    }

    /// Send one RTP packet after encryption with the negotiated voice mode.
    pub async fn send_voice_packet(
        &self,
        encrypted_rtp_packet: &[u8],
    ) -> Result<usize, AudioError> {
        if encrypted_rtp_packet.len() <= 12 || encrypted_rtp_packet[0] >> 6 != 2 {
            return Err(AudioError::invalid(
                "voice UDP transport requires an encrypted RTP v2 packet",
            ));
        }
        self.socket
            .send(encrypted_rtp_packet)
            .await
            .map_err(|error| AudioError::invalid(format!("voice UDP packet send failed: {error}")))
    }
}

fn parse_discovery_response(
    packet: &[u8],
    expected_ssrc: u32,
) -> Result<(String, u16), AudioError> {
    if packet.len() != DISCOVERY_PACKET_BYTES {
        return Err(AudioError::invalid(
            "invalid Discord voice discovery response length",
        ));
    }
    let packet_type = u16::from_be_bytes([packet[0], packet[1]]);
    let packet_length = u16::from_be_bytes([packet[2], packet[3]]);
    let ssrc = u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]);
    if packet_type != DISCOVERY_RESPONSE_TYPE || packet_length != 70 || ssrc != expected_ssrc {
        return Err(AudioError::invalid(
            "invalid Discord voice discovery response header",
        ));
    }

    let address_bytes = &packet[8..72];
    let address_end = address_bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(address_bytes.len());
    let address = std::str::from_utf8(&address_bytes[..address_end])
        .map_err(|_| AudioError::invalid("voice discovery returned a non-UTF-8 IP address"))?;
    let address = address.trim();
    if address.is_empty()
        || address.len() > 63
        || address
            .bytes()
            .any(|byte| !(byte.is_ascii_digit() || byte == b'.'))
        || address.parse::<std::net::Ipv4Addr>().is_err()
    {
        return Err(AudioError::invalid(
            "voice discovery returned an invalid external IPv4 address",
        ));
    }

    let port = u16::from_be_bytes([packet[72], packet[73]]);
    if port == 0 {
        return Err(AudioError::invalid(
            "voice discovery returned an invalid external UDP port",
        ));
    }
    Ok((address.to_owned(), port))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(ssrc: u32, address: &str, port: u16) -> [u8; DISCOVERY_PACKET_BYTES] {
        let mut packet = [0_u8; DISCOVERY_PACKET_BYTES];
        packet[..2].copy_from_slice(&DISCOVERY_RESPONSE_TYPE.to_be_bytes());
        packet[2..4].copy_from_slice(&70_u16.to_be_bytes());
        packet[4..8].copy_from_slice(&ssrc.to_be_bytes());
        packet[8..8 + address.len()].copy_from_slice(address.as_bytes());
        packet[72..74].copy_from_slice(&port.to_be_bytes());
        packet
    }

    #[test]
    fn parses_discord_voice_ip_discovery_response() {
        let packet = response(0x1234_5678, "203.0.113.42", 50_000);
        assert_eq!(
            parse_discovery_response(&packet, 0x1234_5678).unwrap(),
            ("203.0.113.42".into(), 50_000)
        );
    }

    #[test]
    fn rejects_wrong_ssrc_or_malformed_discovery_response() {
        let packet = response(7, "192.0.2.1", 40_000);
        assert!(parse_discovery_response(&packet, 8).is_err());
        assert!(parse_discovery_response(&packet[..20], 7).is_err());
        let packet = response(7, "not-an-ip", 40_000);
        assert!(parse_discovery_response(&packet, 7).is_err());
        let packet = response(7, "192.0.2.1", 0);
        assert!(parse_discovery_response(&packet, 7).is_err());
    }
}
