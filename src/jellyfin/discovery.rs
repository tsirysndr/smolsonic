//! Jellyfin's "client discovery" UDP protocol: clients send the literal
//! string `"Who is JellyfinServer?"` to UDP port 7359 (broadcast on the LAN)
//! and the server replies with a JSON payload identifying itself.
//!
//! See: https://jellyfin.org/docs/general/networking/index.html#auto-discovery

use anyhow::{Context, Result};
use serde_json::json;
use tokio::net::UdpSocket;

pub const DISCOVERY_PORT: u16 = 7359;
const PROBE: &str = "Who is JellyfinServer?";

pub async fn run(
    server_name: String,
    server_id: String,
    http_port: u16,
) -> Result<()> {
    let socket = UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT))
        .await
        .with_context(|| format!("binding UDP {DISCOVERY_PORT} for jellyfin discovery"))?;
    socket
        .set_broadcast(true)
        .context("enabling SO_BROADCAST on jellyfin discovery socket")?;

    tracing::info!(
        "jellyfin: discovery listener on udp/0.0.0.0:{DISCOVERY_PORT} as {server_name} ({server_id})"
    );

    let mut buf = vec![0u8; 1024];
    loop {
        let (n, peer) = match socket.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("jellyfin discovery recv: {e}");
                continue;
            }
        };
        let msg = String::from_utf8_lossy(&buf[..n]);
        let trimmed = msg.trim();
        if trimmed != PROBE {
            tracing::debug!("jellyfin discovery: ignoring probe from {peer}: {trimmed:?}");
            continue;
        }

        let local_ip = local_ip_for_peer(peer.ip()).unwrap_or_else(|| peer.ip().to_string());
        let address = format!("http://{}:{}", local_ip, http_port);
        let reply = json!({
            "Address": address,
            "Id": server_id,
            "Name": server_name,
            "EndpointAddress": null,
        });
        let body = reply.to_string();
        if let Err(e) = socket.send_to(body.as_bytes(), peer).await {
            tracing::warn!("jellyfin discovery reply to {peer}: {e}");
        } else {
            tracing::debug!("jellyfin discovery replied to {peer}: {body}");
        }
    }
}

/// Pick a local IPv4 address on the same subnet as the probing client, falling
/// back to any RFC1918 IPv4. Returned as a string for direct interpolation
/// into the discovery URL.
fn local_ip_for_peer(peer: std::net::IpAddr) -> Option<String> {
    let ifaces = if_addrs::get_if_addrs().ok()?;
    let peer_v4 = match peer {
        std::net::IpAddr::V4(v) => Some(v),
        std::net::IpAddr::V6(_) => None,
    };

    let mut best: Option<std::net::Ipv4Addr> = None;
    for iface in ifaces {
        if iface.is_loopback() {
            continue;
        }
        let std::net::IpAddr::V4(v4) = iface.ip() else {
            continue;
        };
        if !v4.is_private() {
            continue;
        }
        if let Some(peer_v4) = peer_v4 {
            // Cheap "same /24" heuristic to prefer matching subnet.
            let a = peer_v4.octets();
            let b = v4.octets();
            if a[0] == b[0] && a[1] == b[1] && a[2] == b[2] {
                return Some(v4.to_string());
            }
        }
        best.get_or_insert(v4);
    }
    best.map(|v| v.to_string())
}
