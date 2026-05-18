//! ICMP echo sweep — pre-filters alive hosts before TCP probing.

use socket2::{Domain, Protocol, Socket, Type};
use std::{
    collections::HashSet,
    net::{Ipv4Addr, SocketAddr},
    time::{Duration, Instant},
};

const ICMP_ECHO_REQUEST: u8 = 8;
const ICMP_ECHO_REPLY: u8 = 0;
const IP_HEADER_LEN: usize = 20;
const ICMP_HEADER_LEN: usize = 8;

fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut chunks = data.chunks_exact(2);
    for c in chunks.by_ref() {
        sum += u32::from(u16::from_be_bytes([c[0], c[1]]));
    }
    if let Some(&b) = chunks.remainder().first() {
        sum += u32::from(b) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    // Truncation is intentional: ones-complement fold always produces a u16.
    #[allow(clippy::cast_possible_truncation)]
    let result = !(sum as u16);
    result
}

fn echo_request(id: u16, seq: u16) -> [u8; ICMP_HEADER_LEN] {
    let mut pkt = [ICMP_ECHO_REQUEST, 0u8, 0, 0, 0, 0, 0, 0];
    pkt[4..6].copy_from_slice(&id.to_be_bytes());
    pkt[6..8].copy_from_slice(&seq.to_be_bytes());
    let ck = checksum(&pkt);
    pkt[2..4].copy_from_slice(&ck.to_be_bytes());
    pkt
}

/// Send ICMP echo requests to all `hosts` and return those that reply.
///
/// Returns `Some(alive)` when raw sockets are available, `None` when the
/// caller lacks privilege to open a raw socket (`CAP_NET_RAW` / root /
/// administrator). An empty `Some(vec![])` means all hosts were unreachable.
#[must_use]
pub async fn ping_sweep(hosts: Vec<Ipv4Addr>, timeout: Duration) -> Option<Vec<Ipv4Addr>> {
    tokio::task::spawn_blocking(move || ping_sweep_sync(&hosts, timeout))
        .await
        .ok()
        .flatten()
}

fn ping_sweep_sync(hosts: &[Ipv4Addr], timeout: Duration) -> Option<Vec<Ipv4Addr>> {
    // Returns None if we cannot open a raw socket (no privilege).
    let sock = Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::ICMPV4)).ok()?;

    let id = (std::process::id() & 0xffff) as u16;
    for (seq, &host) in hosts.iter().enumerate() {
        // seq wraps at 65535; truncation is intentional.
        #[allow(clippy::cast_possible_truncation)]
        let pkt = echo_request(id, seq as u16);
        let dest = SocketAddr::from((host, 0));
        let _ = sock.send_to(&pkt, &dest.into());
    }

    let deadline = Instant::now() + timeout;
    let mut alive: HashSet<Ipv4Addr> = HashSet::new();
    // Pre-zeroed so bytes beyond what recv writes remain valid to read.
    let mut buf = [0u8; IP_HEADER_LEN + ICMP_HEADER_LEN];

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let _ = sock.set_read_timeout(Some(remaining.min(Duration::from_millis(10))));
        // SAFETY: [u8] and [MaybeUninit<u8>] have identical layout; buffer is
        // pre-zeroed so unwritten bytes are still valid to inspect.
        let uninit = unsafe {
            std::slice::from_raw_parts_mut(
                buf.as_mut_ptr().cast::<std::mem::MaybeUninit<u8>>(),
                buf.len(),
            )
        };
        match sock.recv_from(uninit) {
            Ok((n, _)) if n >= IP_HEADER_LEN + ICMP_HEADER_LEN => {
                // Source IP is in the IP header; ICMP header follows.
                let src_ip = Ipv4Addr::from([buf[12], buf[13], buf[14], buf[15]]);
                let icmp = &buf[IP_HEADER_LEN..IP_HEADER_LEN + ICMP_HEADER_LEN];
                let reply_id = u16::from_be_bytes([icmp[4], icmp[5]]);
                if icmp[0] == ICMP_ECHO_REPLY && reply_id == id && !src_ip.is_loopback() {
                    alive.insert(src_ip);
                }
            }
            Ok(_) => {}
            Err(_) => {
                if Instant::now() >= deadline {
                    break;
                }
            }
        }
    }

    Some(alive.into_iter().collect())
}
