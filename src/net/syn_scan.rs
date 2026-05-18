//! Raw TCP SYN scan — detects open ports without completing the handshake.
//!
//! On Linux (with root / `CAP_NET_RAW`), sends crafted SYN packets and
//! collects SYN-ACK replies to identify open ports in a single RTT without
//! holding a connection open. On all other platforms the function always
//! returns `None`, causing the caller to fall back to the ICMP-only path.

/// Send raw SYN packets to every `(ip, port)` in `targets` and return those
/// that replied with SYN-ACK within `timeout`.
///
/// Returns `Some(open_pairs)` when raw TCP sockets are available, `None`
/// otherwise (not Linux, or insufficient privilege).
#[must_use]
// `os::syn_scan` is non-const on Linux (real impl) and const on other targets
// (stub returns `None`). The wrapper must be non-const on all targets so the
// Linux build links; non-Linux targets see this as "could be const" which we
// silence below.
#[allow(clippy::missing_const_for_fn)]
pub fn syn_scan(
    src_ip: std::net::Ipv4Addr,
    targets: &[(std::net::Ipv4Addr, u16)],
    timeout: std::time::Duration,
) -> Option<Vec<(std::net::Ipv4Addr, u16)>> {
    os::syn_scan(src_ip, targets, timeout)
}

// ── Linux implementation ─────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
mod os {
    use socket2::{Domain, Protocol, Socket, Type};
    use std::{
        collections::{HashMap, HashSet},
        net::{Ipv4Addr, SocketAddr},
        time::{Duration, Instant},
    };

    const IP_HDR_LEN: usize = 20;
    const TCP_HDR_LEN: usize = 20;
    const PKT_LEN: usize = IP_HDR_LEN + TCP_HDR_LEN;
    const TCP_SYN: u8 = 0x02;
    const TCP_SYN_ACK: u8 = 0x12;

    fn ones_complement_sum(data: &[u8]) -> u16 {
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

    // tcp_hdr is always the 20-byte TCP header slice from `build_syn`; the
    // length cast to u16 is statically safe.
    #[allow(clippy::cast_possible_truncation)]
    fn tcp_checksum(src: Ipv4Addr, dst: Ipv4Addr, tcp_hdr: &[u8]) -> u16 {
        let len = tcp_hdr.len() as u16;
        // Pseudo-header: src(4) + dst(4) + zero(1) + proto=6(1) + tcp_len(2)
        let mut pseudo = [0u8; 12];
        pseudo[..4].copy_from_slice(&src.octets());
        pseudo[4..8].copy_from_slice(&dst.octets());
        pseudo[9] = 6;
        pseudo[10..12].copy_from_slice(&len.to_be_bytes());
        let combined: Vec<u8> = pseudo.iter().chain(tcp_hdr).copied().collect();
        ones_complement_sum(&combined)
    }

    // PKT_LEN = 40 (IP_HDR_LEN + TCP_HDR_LEN); the cast to u16 is statically
    // safe.
    #[allow(clippy::cast_possible_truncation)]
    fn build_syn(
        src_ip: Ipv4Addr,
        dst_ip: Ipv4Addr,
        src_port: u16,
        dst_port: u16,
        seq: u32,
    ) -> [u8; PKT_LEN] {
        let mut pkt = [0u8; PKT_LEN];

        // IP header
        pkt[0] = 0x45; // version=4, IHL=5
        pkt[2..4].copy_from_slice(&(PKT_LEN as u16).to_be_bytes());
        pkt[6] = 0x40; // DF flag
        pkt[8] = 64; // TTL
        pkt[9] = 6; // protocol TCP
        pkt[12..16].copy_from_slice(&src_ip.octets());
        pkt[16..20].copy_from_slice(&dst_ip.octets());
        let ip_ck = ones_complement_sum(&pkt[..IP_HDR_LEN]);
        pkt[10..12].copy_from_slice(&ip_ck.to_be_bytes());

        // TCP header
        pkt[IP_HDR_LEN..IP_HDR_LEN + 2].copy_from_slice(&src_port.to_be_bytes());
        pkt[IP_HDR_LEN + 2..IP_HDR_LEN + 4].copy_from_slice(&dst_port.to_be_bytes());
        pkt[IP_HDR_LEN + 4..IP_HDR_LEN + 8].copy_from_slice(&seq.to_be_bytes());
        pkt[IP_HDR_LEN + 12] = 0x50; // data offset = 5 (20 bytes), reserved = 0
        pkt[IP_HDR_LEN + 13] = TCP_SYN;
        pkt[IP_HDR_LEN + 14..IP_HDR_LEN + 16].copy_from_slice(&65535u16.to_be_bytes());
        let tcp_ck = tcp_checksum(src_ip, dst_ip, &pkt[IP_HDR_LEN..]);
        pkt[IP_HDR_LEN + 16..IP_HDR_LEN + 18].copy_from_slice(&tcp_ck.to_be_bytes());

        pkt
    }

    // `i as u16` and `i as u32` below are intentionally truncating: the
    // `wrapping_add` / `wrapping_mul` next to each cast spells the intent.
    // Going through `try_from` would force a branch for the practically-never
    // case of more than 65 535 targets.
    #[allow(clippy::cast_possible_truncation)]
    pub(super) fn syn_scan(
        src_ip: Ipv4Addr,
        targets: &[(Ipv4Addr, u16)],
        timeout: Duration,
    ) -> Option<Vec<(Ipv4Addr, u16)>> {
        if targets.is_empty() {
            return Some(Vec::new());
        }

        let sock = Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::TCP)).ok()?;
        sock.set_header_included_v4(true).ok()?;

        // Use a source port range derived from the process ID to avoid collisions.
        let base_port = 40000u16.wrapping_add((std::process::id() & 0x0fff) as u16);

        // src_port → (dst_ip, dst_port) so we can match SYN-ACK responses.
        let port_map: HashMap<u16, (Ipv4Addr, u16)> = targets
            .iter()
            .enumerate()
            .map(|(i, &pair)| (base_port.wrapping_add(i as u16), pair))
            .collect();

        for (i, &(dst_ip, dst_port)) in targets.iter().enumerate() {
            let src_port = base_port.wrapping_add(i as u16);
            // Vary sequence numbers so replies are distinguishable from noise.
            let seq = (i as u32).wrapping_mul(7_919);
            let pkt = build_syn(src_ip, dst_ip, src_port, dst_port, seq);
            let dest = SocketAddr::from((dst_ip, 0));
            let _ = sock.send_to(&pkt, &dest.into());
        }

        let deadline = Instant::now() + timeout;
        let mut open: HashSet<(Ipv4Addr, u16)> = HashSet::new();
        // 80 bytes: max 60-byte IP header + 20-byte TCP header.
        let mut buf = [0u8; 80];

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let _ = sock.set_read_timeout(Some(remaining.min(Duration::from_millis(10))));
            // SAFETY: [u8] and [MaybeUninit<u8>] have identical layout; buffer is
            // pre-zeroed so unwritten bytes are valid to inspect.
            let uninit = unsafe {
                std::slice::from_raw_parts_mut(
                    buf.as_mut_ptr().cast::<std::mem::MaybeUninit<u8>>(),
                    buf.len(),
                )
            };
            match sock.recv_from(uninit) {
                Ok((n, _)) if n >= IP_HDR_LEN + TCP_HDR_LEN => {
                    let ihl = ((buf[0] & 0x0f) as usize) * 4;
                    if n < ihl + TCP_HDR_LEN {
                        continue;
                    }
                    let tcp = &buf[ihl..ihl + TCP_HDR_LEN];
                    // Incoming SYN-ACK: tcp[0..2] = remote src port, tcp[2..4] = our src port.
                    if tcp[13] != TCP_SYN_ACK {
                        continue;
                    }
                    let our_port = u16::from_be_bytes([tcp[2], tcp[3]]);
                    if let Some(&target) = port_map.get(&our_port) {
                        let src_ip = Ipv4Addr::from([buf[12], buf[13], buf[14], buf[15]]);
                        if src_ip == target.0 {
                            open.insert(target);
                        }
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

        Some(open.into_iter().collect())
    }
}

// ── Non-Linux stub ────────────────────────────────────────────────────────────

#[cfg(not(target_os = "linux"))]
mod os {
    use std::{net::Ipv4Addr, time::Duration};

    pub(super) const fn syn_scan(
        _src_ip: Ipv4Addr,
        _targets: &[(Ipv4Addr, u16)],
        _timeout: Duration,
    ) -> Option<Vec<(Ipv4Addr, u16)>> {
        None
    }
}
