//! TCP/IP device discovery.

pub mod scan;

use crate::{
    runtime::runtime,
    types::{CancellationToken, DeviceMatch},
};
use pyo3::{exceptions::PyValueError, prelude::*};
use std::{collections::HashSet, time::Duration};
use tokio::{sync::Semaphore, task::JoinSet};

/// Return CIDR strings for all active non-loopback IPv4 interfaces.
///
/// This is the same list [`TcpDiscovery`] uses when no subnets are configured.
/// Link-local (`169.254.x.x`) and loopback addresses are excluded. IPv6
/// interfaces are skipped.
#[must_use]
#[pyfunction]
pub fn local_subnets() -> Vec<String> {
    scan::local_subnets()
}

/// Discovers devices reachable over TCP/IP.
///
/// When `probe_command` is set, it is written to each connection and the raw
/// response is returned in `DeviceMatch.response`. Any non-empty response
/// counts as a match. When `probe_command` is not set, a successful TCP
/// connection alone counts as a match (port-open check).
///
/// When `preferred_host` is set (hostname or IP address) it is resolved via DNS
/// and probed first. Only if no match is found does the library fall back to
/// sweeping the provided subnets. Use `preferred_retry` to control how many
/// times the preferred host is retried before falling back.
///
/// Before the full subnet sweep, the library probes a priority set of addresses
/// drawn from the kernel ARP cache and common device address heuristics
/// (`.1`, `.100`, `.254`, etc.). This typically finds active devices within
/// milliseconds on a LAN without scanning the full subnet.
///
/// When no subnets are provided, the library automatically discovers all
/// active network interfaces and sweeps their connected subnets.
#[pyclass]
pub struct TcpDiscovery {
    ports: Vec<u16>,
    subnets: Vec<String>,
    probe_command: Option<Vec<u8>>,
    connect_timeout_ms: u64,
    io_timeout_ms: u64,
    max_concurrent: usize,
    preferred_host: Option<String>,
    preferred_retry: u32,
    preferred_retry_delay_ms: u64,
    use_arp_cache: bool,
    use_mdns: bool,
    mdns_timeout_ms: u64,
    cancellation_token: Option<CancellationToken>,
}

#[pymethods]
impl TcpDiscovery {
    /// Create a new [`TcpDiscovery`].
    ///
    /// Args:
    ///   `port`: Single TCP port to connect to on each host. Use `ports` for
    ///     multiple ports. At least one of `port` or `ports` must be set.
    ///   `ports`: Multiple TCP ports to probe per host (e.g. `[8080, 502]`).
    ///   `subnets`: CIDR subnets to sweep (e.g. `["192.168.1.0/24"]`).
    ///     When empty, all connected network interface subnets are used.
    ///   `probe_command`: Optional bytes to send after connecting. When set,
    ///     only hosts that respond with any bytes are returned as matches.
    ///     When omitted, every host that accepts a TCP connection is a match.
    ///   `connect_timeout_ms`: Per-host TCP handshake timeout in milliseconds
    ///     (default 200).
    ///   `io_timeout_ms`: Per-host probe write + response read timeout in
    ///     milliseconds (default 500).
    ///   `max_concurrent`: Maximum simultaneous open connections (default 500).
    ///   `preferred_host`: Hostname or IP to probe before sweeping subnets.
    ///   `preferred_retry`: Number of times to retry `preferred_host` before
    ///     falling back to a full sweep (default 0).
    ///   `preferred_retry_delay_ms`: Delay between preferred retries in
    ///     milliseconds (default 500).
    ///   `use_arp_cache`: Probe ARP-cached hosts first before the full sweep
    ///     (default `True`).
    ///   `use_mdns`: Listen for mDNS announcements before scanning and probe
    ///     those hosts with higher priority (default `False`; adds latency
    ///     equal to `mdns_timeout_ms`).
    ///   `mdns_timeout_ms`: Duration to listen for mDNS in milliseconds
    ///     (default 1000). Only used when `use_mdns=True`.
    ///   `cancellation_token`: Optional token to cancel an in-progress
    ///     discovery. Call `.cancel()` from another thread to stop the sweep.
    #[must_use]
    #[new]
    #[pyo3(signature = (
        port = None,
        ports = vec![],
        subnets = vec![],
        probe_command = None,
        connect_timeout_ms = 200,
        io_timeout_ms = 500,
        max_concurrent = 500,
        preferred_host = None,
        preferred_retry = 0,
        preferred_retry_delay_ms = 500,
        use_arp_cache = true,
        use_mdns = false,
        mdns_timeout_ms = 1000,
        cancellation_token = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        port: Option<u16>,
        ports: Vec<u16>,
        subnets: Vec<String>,
        probe_command: Option<Vec<u8>>,
        connect_timeout_ms: u64,
        io_timeout_ms: u64,
        max_concurrent: usize,
        preferred_host: Option<String>,
        preferred_retry: u32,
        preferred_retry_delay_ms: u64,
        use_arp_cache: bool,
        use_mdns: bool,
        mdns_timeout_ms: u64,
        cancellation_token: Option<CancellationToken>,
    ) -> Self {
        // Merge `port` + `ports`, deduplicate while preserving order.
        // Vec::dedup() only removes consecutive duplicates, so use a HashSet.
        let mut seen = std::collections::HashSet::new();
        let all_ports: Vec<u16> = port
            .into_iter()
            .chain(ports)
            .filter(|p| seen.insert(*p))
            .collect();
        Self {
            ports: all_ports,
            subnets,
            probe_command,
            connect_timeout_ms,
            io_timeout_ms,
            max_concurrent,
            preferred_host,
            preferred_retry,
            preferred_retry_delay_ms,
            use_arp_cache,
            use_mdns,
            mdns_timeout_ms,
            cancellation_token,
        }
    }

    /// Run discovery and return a list of matching devices.
    ///
    /// # Errors
    ///
    /// Returns a [`pyo3::PyErr`] if no ports are configured or a subnet
    /// string is not valid CIDR notation.
    pub fn discover(&self, py: Python<'_>) -> PyResult<Vec<DeviceMatch>> {
        if self.ports.is_empty() {
            return Err(PyValueError::new_err(
                "TcpDiscovery requires at least one port (use port= or ports=)",
            ));
        }

        let ports = self.ports.clone();
        let subnets = self.subnets.clone();
        let probe = self.probe_command.clone();
        let connect_timeout = Duration::from_millis(self.connect_timeout_ms);
        let io_timeout = Duration::from_millis(self.io_timeout_ms);
        let max_concurrent = self.max_concurrent;
        let preferred = self.preferred_host.clone();
        let preferred_retry = self.preferred_retry;
        let preferred_retry_delay = Duration::from_millis(self.preferred_retry_delay_ms);
        let use_arp = self.use_arp_cache;
        let use_mdns = self.use_mdns;
        let mdns_timeout = Duration::from_millis(self.mdns_timeout_ms);
        let cancel = self.cancellation_token.clone();

        py.detach(|| {
            match runtime().block_on(async move {
                run_discovery(
                    ports,
                    subnets,
                    probe,
                    connect_timeout,
                    io_timeout,
                    max_concurrent,
                    preferred,
                    preferred_retry,
                    preferred_retry_delay,
                    use_arp,
                    use_mdns,
                    mdns_timeout,
                    cancel.as_ref(),
                    None,
                )
                .await
            }) {
                Ok(v) => Ok(v),
                Err(e) => Err(PyErr::from(e)),
            }
        })
    }

    /// Run discovery and call `callback(match)` for every device found.
    ///
    /// Unlike `discover()`, this method calls the Python callback as soon as
    /// each device is found rather than waiting for the full sweep to finish.
    /// Useful for large subnet scans where you want to act on results immediately.
    ///
    /// The GIL is held while calling the callback. Other Python threads are
    /// blocked until the callback returns for each match.
    ///
    /// # Errors
    ///
    /// Returns a [`pyo3::PyErr`] if no ports are configured, a subnet string
    /// is invalid CIDR, or the callback raises an exception.
    // PyO3 requires owned Py<PyAny> for extraction from Python callables.
    #[allow(clippy::needless_pass_by_value)]
    pub fn discover_streaming(
        &self,
        py: Python<'_>,
        callback: Py<PyAny>,
    ) -> PyResult<Vec<DeviceMatch>> {
        if self.ports.is_empty() {
            return Err(PyValueError::new_err(
                "TcpDiscovery requires at least one port (use port= or ports=)",
            ));
        }

        let ports = self.ports.clone();
        let subnets = self.subnets.clone();
        let probe = self.probe_command.clone();
        let connect_timeout = Duration::from_millis(self.connect_timeout_ms);
        let io_timeout = Duration::from_millis(self.io_timeout_ms);
        let max_concurrent = self.max_concurrent;
        let preferred = self.preferred_host.clone();
        let preferred_retry = self.preferred_retry;
        let preferred_retry_delay = Duration::from_millis(self.preferred_retry_delay_ms);
        let use_arp = self.use_arp_cache;
        let use_mdns = self.use_mdns;
        let mdns_timeout = Duration::from_millis(self.mdns_timeout_ms);
        let cancel = self.cancellation_token.clone();

        // Bounded channel: backpressure prevents match accumulation when Python
        // callback is slow and the scan is fast.
        let (tx, rx) = std::sync::mpsc::sync_channel::<DeviceMatch>(256);

        // Background OS thread: runs async discovery without holding the GIL.
        let handle = std::thread::spawn(move || {
            runtime().block_on(async move {
                run_discovery(
                    ports,
                    subnets,
                    probe,
                    connect_timeout,
                    io_timeout,
                    max_concurrent,
                    preferred,
                    preferred_retry,
                    preferred_retry_delay,
                    use_arp,
                    use_mdns,
                    mdns_timeout,
                    cancel.as_ref(),
                    Some(&tx),
                )
                .await
            })
        });

        // Main thread (holds GIL): drain channel and call Python callback.
        let mut all_matches = Vec::new();
        for m in rx {
            callback.call1(py, (m.clone(),))?;
            all_matches.push(m);
        }

        handle.join().map_err(|_| {
            PyErr::new::<pyo3::exceptions::PyRuntimeError, _>("discovery thread panicked")
        })??;

        Ok(all_matches)
    }

    /// Watch for devices appearing or disappearing, calling `on_added` /
    /// `on_removed` as the device set changes.
    ///
    /// Polls `discover()` every `interval_ms` milliseconds. Each poll is
    /// compared against the previous result; new devices trigger `on_added`
    /// and vanished devices trigger `on_removed`. Runs until the
    /// `cancellation_token` is cancelled or a callback raises an exception.
    ///
    /// Args:
    ///   `on_added`: Called with each newly-found `DeviceMatch`.
    ///   `on_removed`: Called with each `DeviceMatch` that disappeared.
    ///   `interval_ms`: Poll interval in milliseconds (default 5000).
    ///
    /// # Errors
    ///
    /// Returns a [`pyo3::PyErr`] if no cancellation token is configured, no
    /// ports are set, or a callback raises an exception.
    #[allow(clippy::needless_pass_by_value)]
    pub fn watch(
        &self,
        py: Python<'_>,
        on_added: Py<PyAny>,
        on_removed: Py<PyAny>,
        interval_ms: Option<u64>,
    ) -> PyResult<()> {
        let Some(ref cancel) = self.cancellation_token else {
            return Err(PyValueError::new_err(
                "TcpDiscovery.watch() requires a cancellation_token to know when to stop",
            ));
        };

        let interval = Duration::from_millis(interval_ms.unwrap_or(5000));
        let mut prev: Vec<DeviceMatch> = Vec::new();

        loop {
            if cancel.is_cancelled() {
                break;
            }

            let current = self.discover(py)?;

            // Compare by address only — response bytes can vary across polls
            // for devices that include dynamic data in their probe response,
            // causing spurious add/remove events with response-based equality.
            let prev_addrs: HashSet<&str> = prev.iter().map(|m| m.address.as_str()).collect();
            let current_addrs: HashSet<&str> = current.iter().map(|m| m.address.as_str()).collect();

            for m in &current {
                if !prev_addrs.contains(m.address.as_str()) {
                    on_added.call1(py, (m.clone(),))?;
                }
            }
            for m in &prev {
                if !current_addrs.contains(m.address.as_str()) {
                    on_removed.call1(py, (m.clone(),))?;
                }
            }

            prev = current;

            let wake_at = std::time::Instant::now() + interval;
            while std::time::Instant::now() < wake_at {
                if cancel.is_cancelled() {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }

        Ok(())
    }
}

/// Core async discovery logic shared between `discover` and `discover_streaming`.
#[allow(clippy::too_many_arguments)]
async fn run_discovery(
    ports: Vec<u16>,
    subnets: Vec<String>,
    probe: Option<Vec<u8>>,
    connect_timeout: Duration,
    io_timeout: Duration,
    max_concurrent: usize,
    preferred: Option<String>,
    preferred_retry: u32,
    preferred_retry_delay: Duration,
    use_arp: bool,
    use_mdns: bool,
    mdns_timeout: Duration,
    cancel: Option<&CancellationToken>,
    tx: Option<&std::sync::mpsc::SyncSender<DeviceMatch>>,
) -> crate::error::Result<Vec<DeviceMatch>> {
    // Preferred host fast-path with configurable retry.
    if let Some(ref host) = preferred {
        for attempt in 0..=preferred_retry {
            if cancel.is_some_and(CancellationToken::is_cancelled) {
                return Ok(Vec::new());
            }
            let matches =
                scan::probe_host(host, &ports, probe.as_deref(), connect_timeout, io_timeout)
                    .await?;
            if !matches.is_empty() {
                if let Some(sender) = tx {
                    for m in &matches {
                        let _ = sender.try_send(m.clone());
                    }
                }
                return Ok(matches);
            }
            if attempt < preferred_retry {
                tokio::time::sleep(preferred_retry_delay).await;
            }
        }
    }

    // mDNS fast-path: listen for self-announcing devices before the subnet scan.
    let mut mdns_matches: Vec<DeviceMatch> = Vec::new();
    if use_mdns {
        let mdns_hosts = crate::net::mdns::passive_mdns_hosts(mdns_timeout).await;
        for ip in mdns_hosts {
            let host_str = ip.to_string();
            let found = scan::probe_host(
                &host_str,
                &ports,
                probe.as_deref(),
                connect_timeout,
                io_timeout,
            )
            .await?;
            for mut m in found {
                m.info.insert("source".to_owned(), "mdns".to_owned());
                if let Some(sender) = tx {
                    let _ = sender.try_send(m.clone());
                }
                mdns_matches.push(m);
            }
        }
    }

    // NDP cache: find IPv6 link-local neighbours and probe them.
    let ipv6_matches = probe_ndp_neighbours(
        &ports,
        probe.as_deref(),
        connect_timeout,
        io_timeout,
        max_concurrent,
    )
    .await
    .unwrap_or_default();

    let mdns_count = mdns_matches.len();
    let mut all_matches: Vec<DeviceMatch> = mdns_matches.into_iter().chain(ipv6_matches).collect();

    // Forward IPv6 NDP matches to the channel; mDNS were forwarded in the loop above.
    if let Some(sender) = tx {
        for m in &all_matches[mdns_count..] {
            let _ = sender.try_send(m.clone());
        }
    }

    // Subnet sweep (the main path for unknown device locations).
    let targets = if subnets.is_empty() {
        scan::local_subnets()
    } else {
        subnets
    };

    let sweep_matches = scan::scan_subnets(
        &targets,
        &ports,
        probe.as_deref(),
        connect_timeout,
        io_timeout,
        max_concurrent,
        use_arp,
        cancel,
        tx,
    )
    .await?;

    // Deduplicate by address: mDNS fast-path may have already found some hosts
    // that the subnet sweep also probed.
    let mut seen_addrs: HashSet<String> = all_matches.iter().map(|m| m.address.clone()).collect();
    for m in sweep_matches {
        if seen_addrs.insert(m.address.clone()) {
            all_matches.push(m);
        }
    }
    Ok(all_matches)
}

/// Probe IPv6 link-local neighbours from the NDP cache on all ports.
async fn probe_ndp_neighbours(
    ports: &[u16],
    probe: Option<&[u8]>,
    connect_timeout: Duration,
    io_timeout: Duration,
    max_concurrent: usize,
) -> crate::error::Result<Vec<DeviceMatch>> {
    use crate::tcp::scan::probe_addr;

    let ndp_hosts = crate::net::ndp::ndp_cache_hosts();
    if ndp_hosts.is_empty() {
        return Ok(Vec::new());
    }

    let probe_arc: Option<std::sync::Arc<[u8]>> = probe.map(std::sync::Arc::from);
    let sem = std::sync::Arc::new(Semaphore::new(max_concurrent));
    let mut set: JoinSet<crate::error::Result<Option<DeviceMatch>>> = JoinSet::new();

    for ip in ndp_hosts {
        for &port in ports {
            let addr = std::net::SocketAddr::new(std::net::IpAddr::V6(ip), port);
            let probe = probe_arc.clone();
            let sem = std::sync::Arc::clone(&sem);
            set.spawn(async move {
                let _permit = sem.acquire_owned().await;
                probe_addr(addr, probe.as_deref(), connect_timeout, io_timeout, None).await
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
