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

/// Listen passively on the mDNS multicast group for `duration` and return all
/// unique IPv4 source addresses observed.
///
/// Falls back to an empty `Vec` on any setup error (bind failure, multicast
/// join failure, etc.) — never propagates errors to the caller.
#[must_use]
pub async fn passive_mdns_hosts(duration: Duration) -> Vec<Ipv4Addr> {
    try_passive_mdns_hosts(duration).await.unwrap_or_default()
}

async fn try_passive_mdns_hosts(duration: Duration) -> Option<Vec<Ipv4Addr>> {
    let socket = build_socket().ok()?;

    socket
        .join_multicast_v4(MDNS_ADDR, Ipv4Addr::UNSPECIFIED)
        .ok()?;

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
