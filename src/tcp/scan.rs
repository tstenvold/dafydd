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
    net::{IpAddr, Ipv4Addr, SocketAddr},
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
/// `max_prefix` is the broadest subnet width auto-detection will widen *to*:
/// an interface whose native prefix is wider than `max_prefix` (e.g. /16 on
/// a /24 limit) is clamped down to `max_prefix`. Narrower native prefixes
/// are honoured as-is. `max_prefix` must be in `[16, 32]`; anything wider
/// would sweep more than 65 k hosts per interface.
///
/// Returns an empty `Vec` if interface enumeration fails — callers should
/// treat this as "nothing to sweep" rather than a hard error.
///
/// # Errors
///
/// Returns [`DafyddError::InvalidSubnet`] when `max_prefix` is outside
/// `16..=32`.
///
/// # Panics
///
/// Never panics in practice; the `expect` guards a compile-time constant.
pub fn local_subnets(max_prefix: u8) -> Result<Vec<String>> {
    if !(16..=32).contains(&max_prefix) {
        return Err(DafyddError::InvalidSubnet(format!(
            "subnet prefix /{max_prefix} out of range; must be /16..=/32 (max 65k hosts)"
        )));
    }

    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return Ok(Vec::new());
    };

    let mut seen: HashSet<String> = HashSet::new();
    let link_local: Ipv4Net = "169.254.0.0/16".parse().expect("static CIDR is valid");

    Ok(ifaces
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
            // Clamp wider-than-max interfaces down to `max_prefix`; honour
            // narrower ones as-is. Prevents accidental /16 sweeps when the
            // caller asked for /24.
            let net = if net.prefix_len() < max_prefix {
                Ipv4Net::new(v4.ip, max_prefix).unwrap_or(net)
            } else {
                net
            };
            let cidr = format!("{}/{}", net.network(), net.prefix_len());
            if seen.insert(cidr.clone()) {
                Some(cidr)
            } else {
                None
            }
        })
        .collect())
}

/// Establish a TCP connection with optimised socket options.
///
/// When `linger` is `Some(Duration::ZERO)` the kernel sends RST on close
/// instead of going through `TIME_WAIT`, reclaiming the local port
/// immediately. Useful for very wide sweeps that would otherwise exhaust
/// the ephemeral port range. When `linger` is `None` no `SO_LINGER` is set
/// — the kernel performs a graceful FIN close (the polite default). Also
/// sets `TCP_NODELAY` so probe bytes are sent in the first segment. On
/// Linux with the `tcp-fast-open` feature, `TCP_FASTOPEN_CONNECT` is set.
#[allow(unsafe_code)]
async fn connect_with_opts(
    addr: SocketAddr,
    connect_timeout: Duration,
    linger: Option<Duration>,
) -> Option<TcpStream> {
    let domain = match addr {
        SocketAddr::V4(_) => Domain::IPV4,
        SocketAddr::V6(_) => Domain::IPV6,
    };

    let s2 = S2Socket::new(domain, Type::STREAM, Some(Protocol::TCP)).ok()?;
    if let Some(d) = linger {
        // linger=Some(0): RST on close, no TIME_WAIT (fast, antisocial).
        // linger=Some(n>0): block close for up to n seconds.
        s2.set_linger(Some(d)).ok()?;
    }
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
#[allow(unsafe_code)]
fn set_tcp_fast_open_connect(socket: &TcpSocket) {
    use socket2::Socket;
    use std::os::fd::AsRawFd;

    // SAFETY: ManuallyDrop prevents drop (and the fd close it would trigger)
    // on both normal exit and unwind — socket retains sole ownership of the fd.
    let raw_fd = socket.as_raw_fd();
    let s2 = std::mem::ManuallyDrop::new(unsafe { Socket::from_raw_fd(raw_fd) });
    let _ = s2.set_tcp_fast_open_connect(true);
}

/// Attempt a single TCP connection, optionally sending `probe` and collecting
/// the response.
///
/// When `probe` is `None`, a successful connection alone counts as a match
/// (reachability check). When `probe` is `Some`, the probe bytes are written
/// and any non-empty response (optionally matching `response_filter`) is
/// returned as a match.
///
/// Returns `Ok(Some(_))` on a match, `Ok(None)` on timeout, connection
/// refused, or a response that does not satisfy the filter.
///
/// # Errors
///
/// Returns [`DafyddError::Io`] for unexpected I/O failures during the probe.
#[allow(clippy::too_many_arguments)]
pub async fn probe_addr(
    addr: SocketAddr,
    probe: Option<&[u8]>,
    connect_timeout: Duration,
    io_timeout: Duration,
    hostname_hint: Option<&str>,
    response_filter: Option<&[u8]>,
    linger: Option<Duration>,
) -> Result<Option<DeviceMatch>> {
    let Some(stream) = connect_with_opts(addr, connect_timeout, linger).await else {
        return Ok(None);
    };

    if let Some(p) = probe {
        probe_stream(stream, addr, p, io_timeout, hostname_hint, response_filter).await
    } else {
        Ok(Some(build_match(addr, b"", hostname_hint)))
    }
}

/// Write `probe` to `stream`, then read until the connection closes or
/// `io_timeout` elapses. Returns the accumulated response if it is non-empty
/// and satisfies `response_filter` (when set).
async fn probe_stream(
    mut stream: TcpStream,
    addr: SocketAddr,
    probe: &[u8],
    io_timeout: Duration,
    hostname_hint: Option<&str>,
    response_filter: Option<&[u8]>,
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
        return Ok(None);
    }

    if let Some(f) = response_filter {
        if !response.windows(f.len()).any(|w| w == f) {
            return Ok(None);
        }
    }

    Ok(Some(build_match(addr, &response, hostname_hint)))
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
#[allow(clippy::too_many_arguments)]
pub async fn probe_host(
    host: &str,
    ports: &[u16],
    probe: Option<&[u8]>,
    connect_timeout: Duration,
    io_timeout: Duration,
    response_filter: Option<Arc<[u8]>>,
    linger: Option<Duration>,
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
            let filter = response_filter.clone();
            set.spawn(async move {
                probe_addr(
                    sock_addr,
                    probe.as_deref(),
                    connect_timeout,
                    io_timeout,
                    Some(&host_hint),
                    filter.as_deref(),
                    linger,
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

/// Build priority address lists to probe before the linear sweep.
///
/// Returns `(arp_addrs, arp_macs, heuristic_addrs)`:
/// - `arp_addrs`: Hosts from the kernel ARP cache, tagged `source=arp_cache`.
/// - `arp_macs`: `Ipv4Addr → MAC` map for stamping `info["mac"]` on matches.
/// - `heuristic_addrs`: Common last-octet addresses (`.1`, `.100`, `.254`, …)
///   not already covered by `arp_addrs`.
///
/// Only addresses within the validated subnets are included.
fn build_priority_addrs(
    nets: &[IpNet],
    ports: &[u16],
    use_arp: bool,
) -> (
    Vec<SocketAddr>,
    HashMap<Ipv4Addr, crate::net::arp::MacAddr>,
    Vec<SocketAddr>,
) {
    let mut arp_seen: HashSet<SocketAddr> = HashSet::new();
    let mut arp_out: Vec<SocketAddr> = Vec::new();
    let mut arp_macs: HashMap<Ipv4Addr, crate::net::arp::MacAddr> = HashMap::new();

    if use_arp {
        for (ip, mac) in crate::net::arp::arp_cache_hosts() {
            for net in nets {
                if net.contains(&std::net::IpAddr::V4(ip)) {
                    arp_macs.insert(ip, mac);
                    for &port in ports {
                        let sa = SocketAddr::from((ip, port));
                        if arp_seen.insert(sa) {
                            arp_out.push(sa);
                        }
                    }
                    break;
                }
            }
        }
    }

    let mut heuristic_seen: HashSet<SocketAddr> = HashSet::new();
    let mut heuristic_out: Vec<SocketAddr> = Vec::new();

    for net in nets {
        let IpNet::V4(v4net) = net else { continue };
        let prefix_bits = v4net.network().octets();
        for &last in PRIORITY_LAST_OCTETS {
            let addr = Ipv4Addr::new(prefix_bits[0], prefix_bits[1], prefix_bits[2], last);
            if v4net.contains(&addr) && !addr.is_broadcast() {
                for &port in ports {
                    let sa = SocketAddr::from((addr, port));
                    // Skip addresses already in the ARP list.
                    if !arp_seen.contains(&sa) && heuristic_seen.insert(sa) {
                        heuristic_out.push(sa);
                    }
                }
            }
        }
    }

    (arp_out, arp_macs, heuristic_out)
}

/// Use the UDP connect trick to determine the local IP used to reach `dst`.
fn local_ip_for(dst: Ipv4Addr) -> Option<Ipv4Addr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect(SocketAddr::from((dst, 80))).ok()?;
    match s.local_addr().ok()? {
        SocketAddr::V4(v4) => Some(*v4.ip()),
        SocketAddr::V6(_) => None,
    }
}

/// When raw-socket privilege is available (ICMP gave us alive hosts),
/// attempt a raw SYN scan to further narrow down to open ports. Falls back
/// to probing all alive × port combinations if SYN scan is unavailable.
async fn build_raw_probe_targets(
    alive: Vec<Ipv4Addr>,
    ports: &[u16],
    connect_timeout: Duration,
) -> Vec<SocketAddr> {
    let targets: Vec<(Ipv4Addr, u16)> = alive
        .iter()
        .flat_map(|&ip| ports.iter().map(move |&p| (ip, p)))
        .collect();

    let src = alive.first().and_then(|&ip| local_ip_for(ip));

    let open = if let Some(src_ip) = src {
        let t = targets.clone();
        let syn_timeout = connect_timeout.max(Duration::from_millis(100));
        tokio::task::spawn_blocking(move || crate::net::syn_scan::syn_scan(src_ip, &t, syn_timeout))
            .await
            .ok()
            .flatten()
    } else {
        None
    };

    open.map_or_else(
        || {
            targets
                .into_iter()
                .map(|(ip, p)| SocketAddr::from((ip, p)))
                .collect()
        },
        |pairs| {
            pairs
                .into_iter()
                .map(|(ip, p)| SocketAddr::from((ip, p)))
                .collect()
        },
    )
}

/// Scan all hosts across multiple `subnets` on `ports` in parallel.
///
/// Before the main sweep, a priority probe is run against ARP-cached hosts
/// and common device addresses. Then, if raw-socket privilege is available,
/// ICMP echo requests pre-filter the remaining hosts to only alive ones;
/// on Linux a raw SYN scan further narrows to open ports. Without privilege,
/// falls back to a full TCP sweep of all remaining addresses.
///
/// `max_concurrent` caps live connections across the entire scan.
///
/// # Errors
///
/// Returns [`DafyddError::InvalidSubnet`] if any element of `subnets` is not
/// valid CIDR notation.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub async fn scan_subnets(
    subnets: &[String],
    ports: &[u16],
    probe: Option<&[u8]>,
    connect_timeout: Duration,
    io_timeout: Duration,
    max_concurrent: usize,
    use_arp: bool,
    cancel: Option<&CancellationToken>,
    tx: Option<&std::sync::mpsc::SyncSender<DeviceMatch>>,
    response_filter: Option<Arc<[u8]>>,
    linger: Option<Duration>,
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

    // Priority probe: ARP cache first (tagged), then common-octet heuristics.
    let (arp_addrs, arp_macs, heuristic_addrs) = build_priority_addrs(&nets, ports, use_arp);
    let priority_addrs: HashSet<SocketAddr> = arp_addrs
        .iter()
        .chain(heuristic_addrs.iter())
        .copied()
        .collect();

    if !arp_addrs.is_empty() {
        run_priority_batch(
            arp_addrs,
            &sem,
            probe_arc.as_ref(),
            connect_timeout,
            io_timeout,
            cancel,
            response_filter.as_ref(),
            "arp_cache",
            tx,
            &mut matches,
            linger,
            Some(&arp_macs),
        )
        .await;
    }

    if cancel.is_some_and(CancellationToken::is_cancelled) {
        return Ok(matches);
    }

    if !heuristic_addrs.is_empty() {
        run_priority_batch(
            heuristic_addrs,
            &sem,
            probe_arc.as_ref(),
            connect_timeout,
            io_timeout,
            cancel,
            response_filter.as_ref(),
            "heuristic",
            tx,
            &mut matches,
            linger,
            None,
        )
        .await;
    }

    if cancel.is_some_and(CancellationToken::is_cancelled) {
        return Ok(matches);
    }

    // Collect remaining hosts (IPs only — decouple from ports for ICMP sweep).
    let remaining_ips: Vec<Ipv4Addr> = nets
        .iter()
        .flat_map(IpNet::hosts)
        .filter_map(|h| {
            if let IpAddr::V4(v4) = h {
                Some(v4)
            } else {
                None
            }
        })
        .filter(|ip| {
            ports
                .iter()
                .all(|&p| !priority_addrs.contains(&SocketAddr::from((*ip, p))))
        })
        .collect();

    // ICMP pre-filter: ping all remaining hosts to find alive ones.
    // Returns None when raw sockets are unavailable (no root/CAP_NET_RAW).
    let icmp_timeout = connect_timeout
        .saturating_mul(2)
        .max(Duration::from_millis(100));
    let alive_result = crate::net::icmp::ping_sweep(remaining_ips.clone(), icmp_timeout).await;

    match alive_result {
        Some(alive) if !alive.is_empty() => {
            // Raw socket available — restrict sweep to alive hosts,
            // then optionally to open ports via raw SYN.
            let probe_targets = build_raw_probe_targets(alive, ports, connect_timeout).await;

            if !probe_targets.is_empty() && !cancel.is_some_and(CancellationToken::is_cancelled) {
                for m in run_batch(
                    probe_targets,
                    &sem,
                    probe_arc.as_ref(),
                    connect_timeout,
                    io_timeout,
                    cancel,
                    response_filter.as_ref(),
                    linger,
                )
                .await
                {
                    if let Some(sender) = tx {
                        let _ = sender.try_send(m.clone());
                    }
                    matches.push(m);
                }
            }
        }
        Some(_) => {
            // ICMP available but no hosts replied — subnet is empty, skip sweep.
        }
        None => {
            // No raw-socket privilege: full TCP sweep of remaining addresses.
            let mut host_iter = nets.iter().flat_map(|net| {
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

                for m in run_batch(
                    batch,
                    &sem,
                    probe_arc.as_ref(),
                    connect_timeout,
                    io_timeout,
                    cancel,
                    response_filter.as_ref(),
                    linger,
                )
                .await
                {
                    if let Some(sender) = tx {
                        let _ = sender.try_send(m.clone());
                    }
                    matches.push(m);
                }
            }
        }
    }

    Ok(matches)
}

/// Probe a priority batch, tag each match with `source`, attach the MAC from
/// `arp_macs` when present, forward to `tx`, and append to `matches`.
#[allow(clippy::too_many_arguments)]
async fn run_priority_batch(
    addrs: Vec<SocketAddr>,
    sem: &Arc<Semaphore>,
    probe_arc: Option<&Arc<[u8]>>,
    connect_timeout: Duration,
    io_timeout: Duration,
    cancel: Option<&CancellationToken>,
    response_filter: Option<&Arc<[u8]>>,
    source_tag: &str,
    tx: Option<&std::sync::mpsc::SyncSender<DeviceMatch>>,
    matches: &mut Vec<DeviceMatch>,
    linger: Option<Duration>,
    arp_macs: Option<&HashMap<Ipv4Addr, crate::net::arp::MacAddr>>,
) {
    for mut m in run_batch(
        addrs,
        sem,
        probe_arc,
        connect_timeout,
        io_timeout,
        cancel,
        response_filter,
        linger,
    )
    .await
    {
        m.info.insert("source".to_owned(), source_tag.to_owned());
        if let Some(macs) = arp_macs {
            if let Ok(addr) = m.address.parse::<SocketAddr>() {
                if let IpAddr::V4(v4) = addr.ip() {
                    if let Some(mac) = macs.get(&v4) {
                        m.info
                            .insert("mac".to_owned(), crate::net::arp::format_mac(mac));
                    }
                }
            }
        }
        if let Some(sender) = tx {
            let _ = sender.try_send(m.clone());
        }
        matches.push(m);
    }
}

/// Probe a batch of addresses under the given semaphore and return all matches.
#[allow(clippy::too_many_arguments)]
async fn run_batch(
    addrs: Vec<SocketAddr>,
    sem: &Arc<Semaphore>,
    probe_arc: Option<&Arc<[u8]>>,
    connect_timeout: Duration,
    io_timeout: Duration,
    cancel: Option<&CancellationToken>,
    response_filter: Option<&Arc<[u8]>>,
    linger: Option<Duration>,
) -> Vec<DeviceMatch> {
    let mut set: JoinSet<Result<Option<DeviceMatch>>> = JoinSet::new();

    for addr in addrs {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            break;
        }
        let sem = Arc::clone(sem);
        let probe = probe_arc.cloned();
        let filter = response_filter.cloned();
        set.spawn(async move {
            let _permit = sem.acquire_owned().await;
            probe_addr(
                addr,
                probe.as_deref(),
                connect_timeout,
                io_timeout,
                None,
                filter.as_deref(),
                linger,
            )
            .await
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
    fn test_local_subnets_default_never_panics() {
        let _subnets = local_subnets(24).expect("/24 is in [16, 32]");
    }

    #[test]
    fn test_local_subnets_rejects_out_of_range() {
        assert!(local_subnets(15).is_err());
        assert!(local_subnets(33).is_err());
        assert!(local_subnets(0).is_err());
    }

    #[test]
    fn test_local_subnets_accepts_full_range() {
        for p in 16u8..=32 {
            assert!(local_subnets(p).is_ok(), "/{p} should be accepted");
        }
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
