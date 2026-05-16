//! Async TCP host scanning with semaphore-bounded concurrency.

use crate::{
    error::{DafyddError, Result},
    types::{DeviceMatch, Transport},
};
use if_addrs::IfAddr;
use ipnet::{IpNet, Ipv4Net};
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::Semaphore,
    task::JoinSet,
    time::timeout,
};

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

/// Attempt a single TCP connection, optionally sending `probe` and collecting
/// the response.
///
/// When `probe` is `None`, a successful connection alone counts as a match
/// (reachability ping). When `probe` is `Some`, the probe bytes are written
/// and any non-empty response is returned as a match. The raw response is
/// stored in `DeviceMatch.info["response"]`.
///
/// `connect_timeout` bounds the TCP handshake; `io_timeout` bounds the probe
/// write + response read once connected.
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
    let Ok(Ok(stream)) = timeout(connect_timeout, TcpStream::connect(addr)).await else {
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
///
/// Reads in a loop to handle responses that arrive across multiple TCP
/// segments.
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
                Ok(0) | Err(_) => break,
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

/// Resolve `host` to socket addresses and probe each one concurrently.
///
/// Returns the first matching address. DNS failure or no response yields
/// `Ok(None)`, allowing the caller to fall back to a subnet sweep.
///
/// # Errors
///
/// Propagates [`DafyddError::Io`] for unexpected I/O failures during probing.
pub async fn probe_host(
    host: &str,
    port: u16,
    probe: Option<&[u8]>,
    connect_timeout: Duration,
    io_timeout: Duration,
) -> Result<Option<DeviceMatch>> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host(format!("{host}:{port}"))
        .await
        .map(Iterator::collect)
        .unwrap_or_default();

    if addrs.is_empty() {
        return Ok(None);
    }

    let mut set: JoinSet<Result<Option<DeviceMatch>>> = JoinSet::new();
    for addr in addrs {
        let probe = probe.map(Vec::from);
        let host = host.to_owned();
        set.spawn(async move {
            probe_addr(
                addr,
                probe.as_deref(),
                connect_timeout,
                io_timeout,
                Some(&host),
            )
            .await
        });
    }

    while let Some(result) = set.join_next().await {
        if let Ok(Ok(Some(m))) = result {
            set.abort_all();
            return Ok(Some(m));
        }
    }
    Ok(None)
}

/// Scan all hosts across multiple `subnets` on `port` in parallel.
///
/// Subnets are validated up-front before any I/O begins. Hosts are then
/// yielded lazily — no full `Vec<SocketAddr>` is materialised upfront — and
/// processed in batches to bound peak memory and task count. Each batch is
/// bounded by `max_concurrent × 4` addresses (minimum 1 024), and the
/// semaphore caps live connections within each batch.
///
/// `connect_timeout` bounds the TCP handshake per host; `io_timeout` bounds
/// the probe write + response read once a connection is established.
///
/// # Errors
///
/// Returns [`DafyddError::InvalidSubnet`] if any element of `subnets` is not
/// valid CIDR notation.
pub async fn scan_subnets(
    subnets: &[String],
    port: u16,
    probe: Option<&[u8]>,
    connect_timeout: Duration,
    io_timeout: Duration,
    max_concurrent: usize,
) -> Result<Vec<DeviceMatch>> {
    let sem = Arc::new(Semaphore::new(max_concurrent));
    // Arc<[u8]> lets every spawned task share the probe bytes with a pointer
    // bump rather than a full Vec clone — O(1) regardless of probe size.
    let probe_arc: Option<Arc<[u8]>> = probe.map(Arc::from);

    // Validate all subnets upfront so bad CIDR strings fail immediately.
    let nets: Vec<IpNet> = subnets
        .iter()
        .map(|s| {
            s.parse()
                .map_err(|e: ipnet::AddrParseError| DafyddError::InvalidSubnet(e.to_string()))
        })
        .collect::<Result<_>>()?;

    // Batch size: large enough to keep the semaphore saturated, small enough
    // to avoid holding millions of live SocketAddrs in RAM for /8 subnets.
    let batch_size = max_concurrent.saturating_mul(4).max(1024);
    let mut matches = Vec::new();

    // Lazily generate SocketAddrs — no upfront allocation for large ranges.
    let mut host_iter = nets
        .into_iter()
        .flat_map(move |net| net.hosts().map(move |h| SocketAddr::new(h, port)));

    loop {
        let batch: Vec<SocketAddr> = host_iter.by_ref().take(batch_size).collect();
        if batch.is_empty() {
            break;
        }
        let mut set: JoinSet<Result<Option<DeviceMatch>>> = JoinSet::new();

        for sock_addr in batch {
            let sem = Arc::clone(&sem);
            let probe = probe_arc.clone(); // pointer bump only
            set.spawn(async move {
                // Permit kept alive until task completes — intentional named binding.
                let _permit = sem.acquire_owned().await;
                probe_addr(
                    sock_addr,
                    probe.as_deref(),
                    connect_timeout,
                    io_timeout,
                    None,
                )
                .await
            });
        }

        while let Some(result) = set.join_next().await {
            if let Ok(Ok(Some(m))) = result {
                matches.push(m);
            }
        }
    }

    Ok(matches)
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
        // Ensuring the logic safely handles the host machine's interface layout
        let _subnets = local_subnets();
    }
}
