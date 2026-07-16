//! SSDP — the discovery half of UPnP. Two jobs:
//!
//!  * answer `M-SEARCH` probes unicast on udp/1900 (bound with
//!    SO_REUSEADDR/SO_REUSEPORT so we coexist with other UPnP stacks), and
//!  * multicast `NOTIFY ssdp:alive` to 239.255.255.250:1900 on startup and
//!    every `NOTIFY_INTERVAL` so idle renderers keep us in their lists.
//!
//! Every packet's LOCATION points at `/rootDesc.xml` on this machine, using
//! an address routable from the peer's subnet (multi-homed hosts advertise
//! per-interface).

use super::{CD_SERVICE_TYPE, CM_SERVICE_TYPE, DEVICE_TYPE, SERVER_HEADER};
use anyhow::{Context, Result};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;
use tokio::net::UdpSocket;

const SSDP_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const SSDP_PORT: u16 = 1900;
const MAX_AGE_SECS: u64 = 1800;
/// Re-announce well inside max-age so a single lost multicast doesn't expire us.
const NOTIFY_INTERVAL: Duration = Duration::from_secs(MAX_AGE_SECS / 3);

pub async fn run(uuid: String, http_port: u16) -> Result<()> {
    let socket = bind_ssdp_socket().context("binding SSDP socket on udp/1900")?;

    for ip in local_v4_ips() {
        if let Err(e) = socket.join_multicast_v4(SSDP_ADDR, ip) {
            tracing::debug!("ssdp: join multicast on {ip}: {e}");
        }
    }
    // Fallback join on the default interface in case iface enumeration
    // missed the active one.
    let _ = socket.join_multicast_v4(SSDP_ADDR, Ipv4Addr::UNSPECIFIED);

    tracing::info!("upnp: SSDP listener on udp/0.0.0.0:{SSDP_PORT} (uuid:{uuid})");

    {
        let uuid = uuid.clone();
        tokio::spawn(async move {
            announce_loop(uuid, http_port).await;
        });
    }

    let mut buf = vec![0u8; 2048];
    loop {
        let (n, peer) = match socket.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("ssdp recv: {e}");
                continue;
            }
        };
        let msg = String::from_utf8_lossy(&buf[..n]);
        if !msg.starts_with("M-SEARCH") {
            continue;
        }
        let Some(st) = header_value(&msg, "ST") else {
            continue;
        };
        let responses = matching_targets(&st, &uuid);
        if responses.is_empty() {
            continue;
        }
        let Some(local_ip) = local_ip_for_peer(peer.ip()) else {
            continue;
        };
        let location = format!("http://{local_ip}:{http_port}/rootDesc.xml");
        for (st, usn) in responses {
            let reply = format!(
                "HTTP/1.1 200 OK\r\n\
                 CACHE-CONTROL: max-age={MAX_AGE_SECS}\r\n\
                 DATE: {date}\r\n\
                 EXT:\r\n\
                 LOCATION: {location}\r\n\
                 SERVER: {SERVER_HEADER}\r\n\
                 ST: {st}\r\n\
                 USN: {usn}\r\n\
                 Content-Length: 0\r\n\r\n",
                date = chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S GMT"),
            );
            if let Err(e) = socket.send_to(reply.as_bytes(), peer).await {
                tracing::debug!("ssdp reply to {peer}: {e}");
            }
        }
    }
}

/// udp/1900 with SO_REUSEADDR (+ SO_REUSEPORT on unix): other UPnP daemons
/// on this host may already own the port.
fn bind_ssdp_socket() -> Result<UdpSocket> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    let addr: SocketAddr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, SSDP_PORT).into();
    socket.bind(&addr.into())?;
    socket.set_nonblocking(true)?;
    Ok(UdpSocket::from_std(socket.into())?)
}

async fn announce_loop(uuid: String, http_port: u16) {
    // Give the HTTP side a beat to bind before pointing renderers at it.
    tokio::time::sleep(Duration::from_secs(2)).await;
    loop {
        send_alive(&uuid, http_port).await;
        tokio::time::sleep(NOTIFY_INTERVAL).await;
    }
}

async fn send_alive(uuid: &str, http_port: u16) {
    for ip in local_v4_ips() {
        let socket = match UdpSocket::bind((ip, 0)).await {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!("ssdp notify bind {ip}: {e}");
                continue;
            }
        };
        let _ = socket.set_multicast_ttl_v4(2);
        let location = format!("http://{ip}:{http_port}/rootDesc.xml");
        // The spec suggests repeating each announcement; two sends paper
        // over single-datagram loss on busy WiFi.
        for _ in 0..2 {
            for (nt, usn) in notify_targets(uuid) {
                let packet = format!(
                    "NOTIFY * HTTP/1.1\r\n\
                     HOST: {SSDP_ADDR}:{SSDP_PORT}\r\n\
                     CACHE-CONTROL: max-age={MAX_AGE_SECS}\r\n\
                     LOCATION: {location}\r\n\
                     NT: {nt}\r\n\
                     NTS: ssdp:alive\r\n\
                     SERVER: {SERVER_HEADER}\r\n\
                     USN: {usn}\r\n\r\n",
                );
                if let Err(e) = socket
                    .send_to(packet.as_bytes(), (SSDP_ADDR, SSDP_PORT))
                    .await
                {
                    tracing::debug!("ssdp notify from {ip}: {e}");
                }
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }
}

/// (NT, USN) pairs a MediaServer:1 advertises.
fn notify_targets(uuid: &str) -> Vec<(String, String)> {
    let root = format!("uuid:{uuid}");
    vec![
        (
            "upnp:rootdevice".to_string(),
            format!("{root}::upnp:rootdevice"),
        ),
        (root.clone(), root.clone()),
        (DEVICE_TYPE.to_string(), format!("{root}::{DEVICE_TYPE}")),
        (
            CD_SERVICE_TYPE.to_string(),
            format!("{root}::{CD_SERVICE_TYPE}"),
        ),
        (
            CM_SERVICE_TYPE.to_string(),
            format!("{root}::{CM_SERVICE_TYPE}"),
        ),
    ]
}

/// (ST, USN) pairs an M-SEARCH for `st` should be answered with.
fn matching_targets(st: &str, uuid: &str) -> Vec<(String, String)> {
    let st = st.trim();
    if st == "ssdp:all" {
        return notify_targets(uuid);
    }
    notify_targets(uuid)
        .into_iter()
        .filter(|(nt, _)| nt == st)
        .collect()
}

fn header_value(msg: &str, name: &str) -> Option<String> {
    for line in msg.lines() {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case(name) {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

fn local_v4_ips() -> Vec<Ipv4Addr> {
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    ifaces
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.ip() {
            IpAddr::V4(v4) if v4.is_private() => Some(v4),
            _ => None,
        })
        .collect()
}

/// Pick a local IPv4 on the same /24 as the probing peer, falling back to
/// any private IPv4 — same heuristic the Jellyfin discovery responder uses.
fn local_ip_for_peer(peer: IpAddr) -> Option<String> {
    let peer_v4 = match peer {
        IpAddr::V4(v) => Some(v),
        IpAddr::V6(_) => None,
    };
    let ips = local_v4_ips();
    if let Some(peer_v4) = peer_v4 {
        let p = peer_v4.octets();
        for ip in &ips {
            let b = ip.octets();
            if p[0] == b[0] && p[1] == b[1] && p[2] == b[2] {
                return Some(ip.to_string());
            }
        }
    }
    ips.first().map(|ip| ip.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_value_is_case_insensitive() {
        let msg =
            "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nst: ssdp:all\r\nMX: 2\r\n\r\n";
        assert_eq!(header_value(msg, "ST").as_deref(), Some("ssdp:all"));
        assert_eq!(header_value(msg, "MX").as_deref(), Some("2"));
        assert_eq!(header_value(msg, "Missing"), None);
    }

    #[test]
    fn ssdp_all_matches_every_target() {
        assert_eq!(matching_targets("ssdp:all", "abc").len(), 5);
    }

    #[test]
    fn single_target_matches() {
        let m = matching_targets("upnp:rootdevice", "abc");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].1, "uuid:abc::upnp:rootdevice");

        let m = matching_targets(DEVICE_TYPE, "abc");
        assert_eq!(m.len(), 1);

        let m = matching_targets("uuid:abc", "abc");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].1, "uuid:abc");

        assert!(matching_targets("urn:unrelated:thing", "abc").is_empty());
    }
}
