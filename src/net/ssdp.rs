//! SSDP/UPnP M-SEARCH — discovers UPnP-capable devices on the LAN.

use std::{
    collections::HashSet,
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};
use tokio::{net::UdpSocket, time::Instant};

const SSDP_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const SSDP_PORT: u16 = 1900;
const BUF_SIZE: usize = 2048;

// MX: 1 requests responses within 1 second — fast enough for LAN discovery.
const MSEARCH: &str = "M-SEARCH * HTTP/1.1\r\n\
    HOST: 239.255.255.250:1900\r\n\
    MAN: \"ssdp:discover\"\r\n\
    MX: 1\r\n\
    ST: ssdp:all\r\n\
    \r\n";

/// Send an SSDP M-SEARCH query and return all unique IPv4 source addresses
/// that respond within `duration`. Falls back to empty `Vec` on any error.
#[must_use]
pub async fn active_ssdp_hosts(duration: Duration) -> Vec<Ipv4Addr> {
    try_active_ssdp_hosts(duration).await.unwrap_or_default()
}

async fn try_active_ssdp_hosts(duration: Duration) -> Option<Vec<Ipv4Addr>> {
    let socket = UdpSocket::bind("0.0.0.0:0").await.ok()?;
    socket.set_multicast_ttl_v4(4).ok()?;

    let dest = SocketAddr::from((SSDP_ADDR, SSDP_PORT));
    socket.send_to(MSEARCH.as_bytes(), dest).await.ok()?;

    let deadline = Instant::now() + duration;
    let mut buf = [0u8; BUF_SIZE];
    let mut seen: HashSet<Ipv4Addr> = HashSet::new();

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, socket.recv_from(&mut buf)).await {
            Ok(Ok((_n, addr))) => {
                if let SocketAddr::V4(v4) = addr {
                    let ip = *v4.ip();
                    if !ip.is_loopback() && !ip.is_link_local() {
                        seen.insert(ip);
                    }
                }
            }
            _ => break,
        }
    }

    Some(seen.into_iter().collect())
}
