//! Async TCP host scanning with semaphore-bounded concurrency.

use crate::{
    error::{DafyddError, Result},
    types::{CancellationToken, DeviceMatch, Transport},
};
use if_addrs::IfAddr;
use ipnet::{IpNet, Ipv4Net};
use socket2::{Domain, Protocol, Socket as S2Socket, Type};
#[cfg(unix)]
use std::os::fd::FromRawFd;
#[cfg(windows)]
use std::os::windows::io::FromRawSocket;
use std::{
    collections::{HashMap, HashSet},
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpSocket, TcpStream},
    sync::Semaphore,
    task::JoinSet,
    time::timeout,
};

/// Last octets probed first within each subnet before the linear sweep.
/// These are statistically common device addresses on embedded hardware.
const PRIORITY_LAST_OCTETS: &[u8] = &[1, 2, 10, 50, 100, 101, 200, 253, 254];

/// Return a deduplicated list of CIDR subnets for all active non-loopback
/// IPv4 network interfaces on the local machine.
///
/// Link-local addresses (`169.254.0.0/16`) and loopback are excluded.
/// IPv6 interfaces are skipped; they are uncommon in embedded-device contexts.
///
/// Returns an empty `Vec` if interface enumeration fails — callers should
/// treat this as "nothing to sweep" rather than a hard error.
///
/// # Panics
///
/// Never panics in practice; the `expect` guards a compile-time constant.
#[must_use]
pub fn local_subnets() -> Vec<String> {
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };

    let mut seen: HashSet<String> = HashSet::new();
    let link_local: Ipv4Net = "169.254.0.0/16".parse().expect("static CIDR is valid");

    ifaces
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| {
            let IfAddr::V4(v4) = i.addr else {
                return None;
            };
            let net = Ipv4Net::with_netmask(v4.ip, v4.netmask).ok()?;
            if link_local.contains(&v4.ip) {
                return None;
            }
            let cidr = format!("{}/{}", net.network(), net.prefix_len());
            if seen.insert(cidr.clone()) {
                Some(cidr)
            } else {
                None
            }
        })
        .collect()
}

/// Establish a TCP connection with optimised socket options.
///
/// Builds the socket via `socket2` to set `SO_LINGER = 0` before connecting.
/// This makes the kernel send RST on close instead of going through `TIME_WAIT`,
/// reclaiming the local port immediately. Critical for high-concurrency scans
/// that would otherwise exhaust the ephemeral port range on a /16 sweep.
/// Also sets `TCP_NODELAY` so probe bytes are sent in the first segment.
/// On Linux with the `tcp-fast-open` feature, `TCP_FASTOPEN_CONNECT` is set.
async fn connect_with_opts(addr: SocketAddr, connect_timeout: Duration) -> Option<TcpStream> {
    let domain = match addr {
        SocketAddr::V4(_) => Domain::IPV4,
        SocketAddr::V6(_) => Domain::IPV6,
    };

    let s2 = S2Socket::new(domain, Type::STREAM, Some(Protocol::TCP)).ok()?;
    // linger=0: RST on close, no TIME_WAIT.
    s2.set_linger(Some(Duration::ZERO)).ok()?;
    s2.set_nonblocking(true).ok()?;

    // Convert socket2::Socket → tokio::net::TcpSocket via raw handle.
    #[cfg(unix)]
    let socket = {
        use std::os::fd::IntoRawFd;
        let fd = s2.into_raw_fd();
        unsafe { TcpSocket::from_raw_fd(fd) }
    };
    #[cfg(windows)]
    let socket = {
        use std::os::windows::io::IntoRawSocket;
        let sock = s2.into_raw_socket();
        unsafe { TcpSocket::from_raw_socket(sock) }
    };

    #[cfg(all(target_os = "linux", feature = "tcp-fast-open"))]
    set_tcp_fast_open_connect(&socket);

    let stream = timeout(connect_timeout, socket.connect(addr))
        .await
        .ok()?
        .ok()?;

    // Suppress Nagle: send probe bytes in the first segment without waiting.
    let _ = stream.set_nodelay(true);

    Some(stream)
}

/// Enable `TCP_FASTOPEN_CONNECT` on Linux (feature-gated).
///
/// Sends the SYN with data attached, cutting connect + write to a single RTT.
/// The server must also support TFO; if it does not, the kernel falls back
/// to a normal three-way handshake transparently.
#[cfg(all(target_os = "linux", feature = "tcp-fast-open"))]
fn set_tcp_fast_open_connect(socket: &TcpSocket) {
    use socket2::Socket;
    use std::os::fd::AsRawFd;

    // SAFETY: we create a temporary socket2::Socket view of the fd without
    // taking ownership — mem::forget prevents double-close.
    let raw_fd = socket.as_raw_fd();
    let s2 = unsafe { Socket::from_raw_fd(raw_fd) };
    let _ = s2.set_tcp_fast_open_connect(true);
    std::mem::forget(s2);
}

/// Attempt a single TCP connection, optionally sending `probe` and collecting
/// the response.
///
/// When `probe` is `None`, a successful connection alone counts as a match
/// (reachability check). When `probe` is `Some`, the probe bytes are written
/// and any non-empty response is returned as a match.
///
/// Returns `Ok(Some(_))` on a match, `Ok(None)` on timeout, connection
/// refused, or an empty response to a probe.
///
/// # Errors
///
/// Returns [`DafyddError::Io`] for unexpected I/O failures during the probe.
pub async fn probe_addr(
    addr: SocketAddr,
    probe: Option<&[u8]>,
    connect_timeout: Duration,
    io_timeout: Duration,
    hostname_hint: Option<&str>,
) -> Result<Option<DeviceMatch>> {
    let Some(stream) = connect_with_opts(addr, connect_timeout).await else {
        return Ok(None);
    };

    if let Some(p) = probe {
        probe_stream(stream, addr, p, io_timeout, hostname_hint).await
    } else {
        Ok(Some(build_match(addr, b"", hostname_hint)))
    }
}

/// Write `probe` to `stream`, then read until the connection closes or
/// `io_timeout` elapses. Returns the accumulated response.
async fn probe_stream(
    mut stream: TcpStream,
    addr: SocketAddr,
    probe: &[u8],
    io_timeout: Duration,
    hostname_hint: Option<&str>,
) -> Result<Option<DeviceMatch>> {
    let result = timeout(io_timeout, async {
        stream.write_all(probe).await?;
        let mut response: Vec<u8> = Vec::with_capacity(4096);
        let mut buf = [0u8; 4096];
        loop {
            match stream.read(&mut buf).await {
                Ok(0) | Err(_) => {
                    core::hint::cold_path();
                    break;
                }
                Ok(n) => response.extend_from_slice(&buf[..n]),
            }
        }
        Ok::<Vec<u8>, std::io::Error>(response)
    })
    .await;

    let Ok(Ok(response)) = result else {
        return Ok(None);
    };

    if response.is_empty() {
        Ok(None)
    } else {
        Ok(Some(build_match(addr, &response, hostname_hint)))
    }
}

fn build_match(addr: SocketAddr, response: &[u8], hostname_hint: Option<&str>) -> DeviceMatch {
    let mut info: HashMap<String, String> = HashMap::new();
    let raw_response = if response.is_empty() {
        None
    } else {
        Some(response.to_vec())
    };
    if let Some(h) = hostname_hint {
        info.insert("hostname".to_owned(), h.to_owned());
    }
    DeviceMatch {
        transport: Transport::Tcp,
        address: addr.to_string(),
        response: raw_response,
        info,
    }
}

/// Resolve `host` to socket addresses and probe each (address × port) pair
/// concurrently.
///
/// Returns all matches found. DNS failure or no response yields `Ok(vec![])`,
/// allowing the caller to fall back to a subnet sweep.
///
/// # Errors
///
/// Propagates [`DafyddError::Io`] for unexpected I/O failures during probing.
pub async fn probe_host(
    host: &str,
    ports: &[u16],
    probe: Option<&[u8]>,
    connect_timeout: Duration,
    io_timeout: Duration,
) -> Result<Vec<DeviceMatch>> {
    if ports.is_empty() {
        return Ok(Vec::new());
    }

    // Resolve DNS for all ports simultaneously.
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host(format!("{host}:{}", ports[0]))
        .await
        .map(Iterator::collect)
        .unwrap_or_default();

    if addrs.is_empty() {
        return Ok(Vec::new());
    }

    // Probe every (resolved_addr × port) combination concurrently.
    let mut set: JoinSet<Result<Option<DeviceMatch>>> = JoinSet::new();
    let probe_arc: Option<Arc<[u8]>> = probe.map(Arc::from);
    let host_owned = host.to_owned();

    for addr in &addrs {
        for &port in ports {
            let sock_addr = SocketAddr::new(addr.ip(), port);
            let probe = probe_arc.clone();
            let host_hint = host_owned.clone();
            set.spawn(async move {
                probe_addr(
                    sock_addr,
                    probe.as_deref(),
                    connect_timeout,
                    io_timeout,
                    Some(&host_hint),
                )
                .await
            });
        }
    }

    let mut matches = Vec::new();
    while let Some(result) = set.join_next().await {
        if let Ok(Ok(Some(m))) = result {
            matches.push(m);
        }
    }
    Ok(matches)
}

/// Build the set of priority addresses to probe before the linear sweep.
///
/// Priority addresses come from two sources:
/// 1. The kernel ARP cache — hosts seen recently on the network.
/// 2. Common last-octet heuristics (`.1`, `.100`, `.254`, etc.) applied to
///    each subnet's network prefix.
///
/// Only addresses that fall within the validated subnets are included, so
/// we don't probe outside the declared scope.
fn build_priority_addrs(nets: &[IpNet], ports: &[u16]) -> Vec<SocketAddr> {
    let mut seen: HashSet<SocketAddr> = HashSet::new();
    let mut out = Vec::new();

    let add = |addr: Ipv4Addr, seen: &mut HashSet<SocketAddr>, out: &mut Vec<SocketAddr>| {
        for &port in ports {
            let sa = SocketAddr::from((addr, port));
            if seen.insert(sa) {
                out.push(sa);
            }
        }
    };

    // ARP cache: machines that have communicated on the LAN recently.
    for ip in crate::net::arp::arp_cache_hosts() {
        for net in nets {
            if net.contains(&std::net::IpAddr::V4(ip)) {
                add(ip, &mut seen, &mut out);
                break;
            }
        }
    }

    // Common last-octet heuristic within each subnet.
    for net in nets {
        let IpNet::V4(v4net) = net else { continue };
        let prefix_bits = v4net.network().octets();
        for &last in PRIORITY_LAST_OCTETS {
            let addr = Ipv4Addr::new(prefix_bits[0], prefix_bits[1], prefix_bits[2], last);
            if v4net.contains(&addr) && !addr.is_broadcast() {
                add(addr, &mut seen, &mut out);
            }
        }
    }

    out
}

/// Scan all hosts across multiple `subnets` on `ports` in parallel.
///
/// Before the main linear sweep, a priority probe is run against:
/// - Hosts currently in the kernel ARP cache (recently seen on the LAN).
/// - Common device addresses (`.1`, `.100`, `.254`, etc.) within each subnet.
///
/// Subnets are validated up-front before any I/O begins. Hosts are then
/// yielded lazily — no full `Vec<SocketAddr>` is materialised upfront — and
/// processed in batches to bound peak memory and task count.
///
/// `max_concurrent` caps live connections across both the priority probe and
/// the main sweep.
///
/// When `cancel` is `Some` and the token is cancelled between batches, the
/// sweep terminates early and returns whatever matches were found so far.
///
/// When `tx` is `Some`, each match is forwarded on the channel immediately as
/// it is found (streaming mode) in addition to being returned in the `Vec`.
///
/// # Errors
///
/// Returns [`DafyddError::InvalidSubnet`] if any element of `subnets` is not
/// valid CIDR notation.
#[allow(clippy::too_many_arguments)]
pub async fn scan_subnets(
    subnets: &[String],
    ports: &[u16],
    probe: Option<&[u8]>,
    connect_timeout: Duration,
    io_timeout: Duration,
    max_concurrent: usize,
    cancel: Option<&CancellationToken>,
    tx: Option<&std::sync::mpsc::SyncSender<DeviceMatch>>,
) -> Result<Vec<DeviceMatch>> {
    if ports.is_empty() {
        return Ok(Vec::new());
    }

    let sem = Arc::new(Semaphore::new(max_concurrent));
    let probe_arc: Option<Arc<[u8]>> = probe.map(Arc::from);

    let nets: Vec<IpNet> = subnets
        .iter()
        .map(|s| {
            s.parse()
                .map_err(|e: ipnet::AddrParseError| DafyddError::InvalidSubnet(e.to_string()))
        })
        .collect::<Result<_>>()?;

    let batch_size = max_concurrent.saturating_mul(4).max(1024);
    let mut matches = Vec::new();

    // Priority probe: ARP cache + common octets first.
    let priority = build_priority_addrs(&nets, ports);
    let priority_addrs: HashSet<SocketAddr> = priority.iter().copied().collect();
    if !priority.is_empty() {
        let priority_matches = run_batch(
            priority,
            &sem,
            probe_arc.as_ref(),
            connect_timeout,
            io_timeout,
            cancel,
        )
        .await;
        for m in priority_matches {
            if let Some(sender) = tx {
                let _ = sender.try_send(m.clone());
            }
            matches.push(m);
        }
    }

    if cancel.is_some_and(CancellationToken::is_cancelled) {
        return Ok(matches);
    }

    // Main sweep — lazily iterate all host addresses, skip priority already done.
    let mut host_iter = nets.into_iter().flat_map(move |net| {
        let ports = ports.to_vec();
        net.hosts().flat_map(move |h| {
            let ports = ports.clone();
            ports.into_iter().map(move |p| SocketAddr::new(h, p))
        })
    });

    loop {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            break;
        }

        let batch: Vec<SocketAddr> = host_iter
            .by_ref()
            .filter(|a| !priority_addrs.contains(a))
            .take(batch_size)
            .collect();

        if batch.is_empty() {
            break;
        }

        let batch_matches = run_batch(
            batch,
            &sem,
            probe_arc.as_ref(),
            connect_timeout,
            io_timeout,
            cancel,
        )
        .await;

        for m in batch_matches {
            if let Some(sender) = tx {
                let _ = sender.try_send(m.clone());
            }
            matches.push(m);
        }
    }

    Ok(matches)
}

/// Probe a batch of addresses under the given semaphore and return all matches.
async fn run_batch(
    addrs: Vec<SocketAddr>,
    sem: &Arc<Semaphore>,
    probe_arc: Option<&Arc<[u8]>>,
    connect_timeout: Duration,
    io_timeout: Duration,
    cancel: Option<&CancellationToken>,
) -> Vec<DeviceMatch> {
    let mut set: JoinSet<Result<Option<DeviceMatch>>> = JoinSet::new();

    for addr in addrs {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            break;
        }
        let sem = Arc::clone(sem);
        let probe = probe_arc.cloned();
        set.spawn(async move {
            let _permit = sem.acquire_owned().await;
            probe_addr(addr, probe.as_deref(), connect_timeout, io_timeout, None).await
        });
    }

    let mut matches = Vec::new();
    while let Some(result) = set.join_next().await {
        if let Ok(Ok(Some(m))) = result {
            matches.push(m);
        }
    }
    matches
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn test_build_match_empty_response() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
        let m = build_match(addr, &[], None);

        assert_eq!(m.address, "127.0.0.1:8080");
        assert_eq!(m.transport, Transport::Tcp);
        assert!(m.response.is_none());
        assert!(m.info.is_empty());
    }

    #[test]
    fn test_build_match_with_response_and_hostname() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 80);
        let m = build_match(addr, b"hello", Some("my-device"));

        assert_eq!(m.address, "192.168.1.100:80");
        assert_eq!(m.response, Some(b"hello".to_vec()));
        assert_eq!(
            m.info.get("hostname").map(String::as_str),
            Some("my-device")
        );
    }

    #[test]
    fn test_local_subnets_never_panics() {
        let _subnets = local_subnets();
    }

    #[test]
    fn test_priority_last_octets_reasonable() {
        // Sanity-check that none of the priority octets are broadcast (255).
        for &octet in PRIORITY_LAST_OCTETS {
            assert_ne!(octet, 255);
            assert_ne!(octet, 0);
        }
    }
}
