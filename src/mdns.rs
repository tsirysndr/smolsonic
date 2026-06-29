use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::net::{IpAddr, Ipv4Addr};

const SUBSONIC_SERVICE: &str = "_subsonic._tcp.local.";
const S3_SERVICE: &str = "_s3._tcp.local.";
const JELLYFIN_SERVICE: &str = "_jellyfin._tcp.local.";

pub struct Handle {
    daemon: ServiceDaemon,
    fullnames: Vec<String>,
}

impl Drop for Handle {
    fn drop(&mut self) {
        for fullname in &self.fullnames {
            let _ = self.daemon.unregister(fullname);
        }
        let _ = self.daemon.shutdown();
    }
}

pub fn start(
    instance_name: &str,
    subsonic_port: u16,
    s3: Option<(String, u16)>,
    jellyfin: Option<(String, u16, String)>,
) -> Result<Handle> {
    let ips = local_network_ips();
    if ips.is_empty() {
        anyhow::bail!("mdns: no usable local network IPv4 addresses found");
    }

    let daemon = ServiceDaemon::new().context("mdns: failed to create service daemon")?;
    let hostname = format!("{instance_name}.local.");
    let mut fullnames = Vec::new();

    let subsonic = ServiceInfo::new(
        SUBSONIC_SERVICE,
        instance_name,
        &hostname,
        ips.as_slice(),
        subsonic_port,
        None,
    )
    .context("mdns: failed to build subsonic ServiceInfo")?;
    let subsonic_full = subsonic.get_fullname().to_string();
    daemon
        .register(subsonic)
        .context("mdns: failed to register subsonic service")?;
    fullnames.push(subsonic_full);
    tracing::info!(
        "mdns: broadcasting {SUBSONIC_SERVICE} as {instance_name} on {ips:?}:{subsonic_port}"
    );

    if let Some((_, s3_port)) = s3 {
        let s3_instance = format!("{instance_name}-s3");
        let s3_info = ServiceInfo::new(
            S3_SERVICE,
            &s3_instance,
            &hostname,
            ips.as_slice(),
            s3_port,
            None,
        )
        .context("mdns: failed to build s3 ServiceInfo")?;
        let s3_full = s3_info.get_fullname().to_string();
        daemon
            .register(s3_info)
            .context("mdns: failed to register s3 service")?;
        fullnames.push(s3_full);
        tracing::info!(
            "mdns: broadcasting {S3_SERVICE} as {s3_instance} on {ips:?}:{s3_port}"
        );
    }

    if let Some((_, jellyfin_port, server_id)) = jellyfin {
        let jellyfin_instance = format!("{instance_name}-jellyfin");
        let mut txt = std::collections::HashMap::new();
        txt.insert("ID".to_string(), server_id.clone());
        let jellyfin_info = ServiceInfo::new(
            JELLYFIN_SERVICE,
            &jellyfin_instance,
            &hostname,
            ips.as_slice(),
            jellyfin_port,
            Some(txt),
        )
        .context("mdns: failed to build jellyfin ServiceInfo")?;
        let jellyfin_full = jellyfin_info.get_fullname().to_string();
        daemon
            .register(jellyfin_info)
            .context("mdns: failed to register jellyfin service")?;
        fullnames.push(jellyfin_full);
        tracing::info!(
            "mdns: broadcasting {JELLYFIN_SERVICE} as {jellyfin_instance} on {ips:?}:{jellyfin_port} (ID={server_id})"
        );
    }

    Ok(Handle { daemon, fullnames })
}

fn local_network_ips() -> Vec<IpAddr> {
    let ifaces = match if_addrs::get_if_addrs() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("mdns: failed to enumerate interfaces: {e}");
            return Vec::new();
        }
    };
    ifaces
        .into_iter()
        .filter_map(|iface| {
            if iface.is_loopback() {
                return None;
            }
            if is_virtual_iface(&iface.name) {
                tracing::debug!("mdns: skipping virtual interface {}", iface.name);
                return None;
            }
            match iface.ip() {
                IpAddr::V4(v4) if is_local_network_v4(v4) => Some(IpAddr::V4(v4)),
                _ => None,
            }
        })
        .collect()
}

fn is_local_network_v4(ip: Ipv4Addr) -> bool {
    if ip.is_unspecified() || ip.is_loopback() || ip.is_link_local() {
        return false;
    }
    if !ip.is_private() {
        return false;
    }
    let oct = ip.octets();
    // Docker default bridge network (docker0): 172.17.0.0/16.
    if oct[0] == 172 && oct[1] == 17 {
        return false;
    }
    true
}

fn is_virtual_iface(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    const PREFIXES: &[&str] = &[
        // Docker / container bridges
        "docker", "br-", "veth", "cni", "flannel", "cali", "weave",
        // VM hypervisors
        "vboxnet", "vmnet", "vnic", "virbr", "vif", "vmk",
        // Tunnels / VPN / mesh
        "tun", "tap", "utun", "wg", "zt", "tailscale", "ppp", "gif", "stf",
        // Apple-specific virtual interfaces
        "awdl", "llw", "anpi", "ap", "bridge",
    ];
    PREFIXES.iter().any(|p| lower.starts_with(p))
}
