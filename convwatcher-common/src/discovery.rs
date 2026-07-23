//! UDP-based discovery: the agent broadcasts [`Message::Beacon`] until a
//! coordinator replies with [`Message::BeaconAck`]; the coordinator listens for
//! beacons and answers with its TCP address.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use log::{debug, warn};
use tokio::net::UdpSocket;

use crate::protocol::Message;

/// Server side: bind the discovery UDP socket and answer beacons with the given
/// TCP address/port. Runs forever.
pub async fn serve_discovery(discovery_port: u16, tcp_addr: String, tcp_port: u16) -> Result<()> {
    let bind = format!("0.0.0.0:{discovery_port}");
    let sock = UdpSocket::bind(&bind)
        .await
        .with_context(|| format!("bind discovery socket on {bind}"))?;
    sock.set_broadcast(true).ok();
    log::info!("Discovery listening on udp://{bind}");

    let mut buf = vec![0u8; 4096];
    loop {
        let (n, peer) = match sock.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                warn!("discovery recv error: {e}");
                continue;
            }
        };
        match serde_json::from_slice::<Message>(&buf[..n]) {
            Ok(Message::Beacon { agent_id }) => {
                debug!("beacon from {agent_id} @ {peer}");
                let ack = Message::BeaconAck {
                    tcp_addr: tcp_addr.clone(),
                    tcp_port,
                };
                if let Ok(bytes) = serde_json::to_vec(&ack) {
                    if let Err(e) = sock.send_to(&bytes, peer).await {
                        warn!("failed to ack beacon to {peer}: {e}");
                    }
                }
            }
            Ok(_) => {}
            Err(e) => debug!("ignoring malformed discovery packet from {peer}: {e}"),
        }
    }
}

/// Agent side: broadcast beacons on the discovery port until a coordinator
/// replies. Returns the coordinator's TCP `SocketAddr`.
pub async fn discover_coordinator(
    discovery_port: u16,
    agent_id: &str,
    broadcast_interval: Duration,
) -> Result<SocketAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0")
        .await
        .context("bind agent discovery socket")?;
    sock.set_broadcast(true)
        .context("enable broadcast on discovery socket")?;

    let beacon = serde_json::to_vec(&Message::Beacon {
        agent_id: agent_id.to_string(),
    })?;
    let broadcast_addr = format!("255.255.255.255:{discovery_port}");

    let mut buf = vec![0u8; 4096];
    loop {
        if let Err(e) = sock.send_to(&beacon, &broadcast_addr).await {
            warn!("beacon broadcast failed: {e}");
        } else {
            debug!("broadcast beacon to {broadcast_addr}");
        }

        match tokio::time::timeout(broadcast_interval, sock.recv_from(&mut buf)).await {
            Ok(Ok((n, _peer))) => {
                if let Ok(Message::BeaconAck { tcp_addr, tcp_port }) =
                    serde_json::from_slice::<Message>(&buf[..n])
                {
                    let addr = format!("{tcp_addr}:{tcp_port}");
                    match addr.parse::<SocketAddr>() {
                        Ok(sa) => return Ok(sa),
                        Err(e) => warn!("coordinator sent invalid addr '{addr}': {e}"),
                    }
                }
            }
            Ok(Err(e)) => warn!("discovery recv error: {e}"),
            Err(_) => { /* timeout — loop and rebroadcast */ }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    // Verify the UDP beacon/discovery handshake: an agent broadcasting on the
    // loopback broadcast address must receive a BeaconAck carrying the
    // coordinator's advertised TCP address.
    #[tokio::test]
    async fn discovery_handshake() {
        // 127.255.255.255 is the loopback broadcast address.
        let discovery_port = 43887;
        let coordinator_port = 43888;
        let advertise = "127.0.0.1".to_string();

        let server = tokio::spawn(async move {
            serve_discovery(discovery_port, advertise, coordinator_port)
                .await
                .ok();
        });

        let found = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            discover_coordinator_addr(discovery_port, "agent-x"),
        )
        .await
        .expect("discovery timed out");

        assert_eq!(found.ip(), IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
        assert_eq!(found.port(), coordinator_port);

        server.abort();
    }

    // Helper that overrides the broadcast target to loopback for the test.
    async fn discover_coordinator_addr(discovery_port: u16, agent_id: &str) -> SocketAddr {
        use tokio::net::UdpSocket;
        let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sock.set_broadcast(true).ok();
        let beacon = serde_json::to_vec(&Message::Beacon {
            agent_id: agent_id.to_string(),
        })
        .unwrap();
        let broadcast_addr = format!("127.0.0.1:{discovery_port}");
        let _ = sock.set_broadcast(true).ok();
        let mut buf = vec![0u8; 4096];
        loop {
            sock.send_to(&beacon, &broadcast_addr).await.unwrap();
            if let Ok(Ok((n, _))) =
                tokio::time::timeout(std::time::Duration::from_secs(1), sock.recv_from(&mut buf))
                    .await
            {
                if let Ok(Message::BeaconAck { tcp_addr, tcp_port }) =
                    serde_json::from_slice::<Message>(&buf[..n])
                {
                    let addr = format!("{tcp_addr}:{tcp_port}");
                    return addr.parse().unwrap();
                }
            }
        }
    }
}
