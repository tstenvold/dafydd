use socket2::{Domain, Protocol, Socket, Type};
use std::{
    collections::HashSet,
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};
use tokio::{net::UdpSocket, time::Instant};

const MDNS_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const MDNS_PORT: u16 = 5353;
const BUF_SIZE: usize = 1500;

/// DNS PTR query for `_services._dns-sd._udp.local` in mDNS wire format.
///
/// Sending this to 224.0.0.251:5353 causes all mDNS-capable devices on the
/// LAN to respond with their service announcements, enabling active discovery
/// rather than waiting for unsolicited broadcasts.
const DNS_SD_QUERY: &[u8] = &[
    // Header: ID=0, standard query, QDCOUNT=1, all other counts=0
    0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    // QNAME: _services._dns-sd._udp.local
    0x09, b'_', b's', b'e', b'r', b'v', b'i', b'c', b'e', b's', 0x07, b'_', b'd', b'n', b's', b'-',
    b's', b'd', 0x04, b'_', b'u', b'd', b'p', 0x05, b'l', b'o', b'c', b'a', b'l',
    0x00, // end of name
    // QTYPE=PTR (12), QCLASS=IN (1)
    0x00, 0x0c, 0x00, 0x01,
];

/// Send an active DNS-SD query to the mDNS multicast group, then listen for
/// `duration` and return all unique IPv4 source addresses that respond.
///
/// Falls back to an empty `Vec` on any setup, send, or receive error — never
/// propagates errors to the caller.
#[must_use]
pub async fn active_mdns_hosts(duration: Duration) -> Vec<Ipv4Addr> {
    try_active_mdns_hosts(duration).await.unwrap_or_default()
}

async fn try_active_mdns_hosts(duration: Duration) -> Option<Vec<Ipv4Addr>> {
    let socket = build_socket().ok()?;

    socket
        .join_multicast_v4(MDNS_ADDR, Ipv4Addr::UNSPECIFIED)
        .ok()?;

    let dest = SocketAddr::from((MDNS_ADDR, MDNS_PORT));
    // Send the DNS-SD query to wake up mDNS devices before listening.
    let _ = socket.send_to(DNS_SD_QUERY, &dest).await;

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

fn build_socket() -> Result<UdpSocket, ()> {
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).map_err(|_| ())?;
    sock.set_reuse_address(true).map_err(|_| ())?;
    #[cfg(not(target_os = "windows"))]
    sock.set_reuse_port(true).map_err(|_| ())?;
    sock.set_nonblocking(true).map_err(|_| ())?;
    let bind_addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, MDNS_PORT));
    sock.bind(&bind_addr.into()).map_err(|_| ())?;
    UdpSocket::from_std(std::net::UdpSocket::from(sock)).map_err(|_| ())
}
