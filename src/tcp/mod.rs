//! TCP/IP device discovery.

pub mod scan;

use crate::{
    runtime::runtime,
    types::{CancellationToken, DeviceMatch},
};
use pyo3::{exceptions::PyValueError, prelude::*};
use std::{collections::HashSet, sync::Arc, time::Duration};

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
/// (optionally filtered by `response_filter`) counts as a match. When
/// `probe_command` is not set, a successful TCP connection alone counts as a
/// match (port-open check).
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
/// When running with root / `CAP_NET_RAW`, ICMP echo requests pre-filter the
/// remaining sweep to only alive hosts; on Linux a raw SYN scan then further
/// narrows to open ports — both happen automatically, no configuration needed.
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
    use_ssdp: bool,
    ssdp_timeout_ms: u64,
    response_filter: Option<Vec<u8>>,
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
    ///   `use_mdns`: Send an active DNS-SD query and probe responding hosts
    ///     before the subnet sweep (default `False`; adds latency equal to
    ///     `mdns_timeout_ms`).
    ///   `mdns_timeout_ms`: Duration to wait for mDNS responses in milliseconds
    ///     (default 1000). Only used when `use_mdns=True`.
    ///   `use_ssdp`: Send an SSDP M-SEARCH query and probe responding hosts
    ///     before the subnet sweep (default `False`; adds latency equal to
    ///     `ssdp_timeout_ms`). Finds UPnP-capable devices (routers, cameras,
    ///     smart home hardware) that do not advertise via mDNS.
    ///   `ssdp_timeout_ms`: Duration to wait for SSDP responses in milliseconds
    ///     (default 1000). Only used when `use_ssdp=True`.
    ///   `response_filter`: Optional bytes that must appear in the probe
    ///     response for a host to count as a match. When omitted, any non-empty
    ///     response is accepted. Has no effect when `probe_command` is not set.
    ///   `cancellation_token`: Optional token to cancel an in-progress
    ///     discovery. Call `.cancel()` from another thread to stop the sweep.
    #[must_use]
    #[new]
    #[pyo3(signature = (
        port = None,
        ports = vec![],
        subnets = vec![],
        probe_command = None,
        connect_timeout_ms = 50,
        io_timeout_ms = 500,
        max_concurrent = 500,
        preferred_host = None,
        preferred_retry = 0,
        preferred_retry_delay_ms = 500,
        use_arp_cache = true,
        use_mdns = false,
        mdns_timeout_ms = 1000,
        use_ssdp = false,
        ssdp_timeout_ms = 1000,
        response_filter = None,
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
        use_ssdp: bool,
        ssdp_timeout_ms: u64,
        response_filter: Option<Vec<u8>>,
        cancellation_token: Option<CancellationToken>,
    ) -> Self {
        // Merge `port` + `ports`, deduplicate while preserving order.
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
            use_ssdp,
            ssdp_timeout_ms,
            response_filter,
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

        let params = self.params();
        py.detach(
            || match runtime().block_on(async move { run_discovery(params, None).await }) {
                Ok(v) => Ok(v),
                Err(e) => Err(PyErr::from(e)),
            },
        )
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

        let params = self.params();

        // Bounded channel: backpressure prevents match accumulation when Python
        // callback is slow and the scan is fast.
        let (tx, rx) = std::sync::mpsc::sync_channel::<DeviceMatch>(256);

        // Background OS thread: runs async discovery without holding the GIL.
        let handle = std::thread::spawn(move || {
            runtime().block_on(async move { run_discovery(params, Some(&tx)).await })
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

    /// Run discovery as a Python coroutine, returning all matches when awaited.
    ///
    /// Equivalent to `discover()` but non-blocking — suitable for use in
    /// `asyncio` event loops without `run_in_executor`.
    ///
    /// # Errors
    ///
    /// Returns a [`pyo3::PyErr`] if no ports are configured or a subnet
    /// string is not valid CIDR notation.
    pub fn discover_async<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        if self.ports.is_empty() {
            return Err(PyValueError::new_err(
                "TcpDiscovery requires at least one port (use port= or ports=)",
            ));
        }
        let params = self.params();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            run_discovery(params, None).await.map_err(PyErr::from)
        })
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
    ///   `interval_ms`: Poll interval in milliseconds (default 30000).
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

        let interval = Duration::from_millis(interval_ms.unwrap_or(30_000));
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

/// Snapshot of `TcpDiscovery` config cloneable into async tasks.
struct DiscoveryParams {
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
    use_ssdp: bool,
    ssdp_timeout: Duration,
    response_filter: Option<Vec<u8>>,
    cancel: Option<CancellationToken>,
}

impl TcpDiscovery {
    fn params(&self) -> DiscoveryParams {
        DiscoveryParams {
            ports: self.ports.clone(),
            subnets: self.subnets.clone(),
            probe: self.probe_command.clone(),
            connect_timeout: Duration::from_millis(self.connect_timeout_ms),
            io_timeout: Duration::from_millis(self.io_timeout_ms),
            max_concurrent: self.max_concurrent,
            preferred: self.preferred_host.clone(),
            preferred_retry: self.preferred_retry,
            preferred_retry_delay: Duration::from_millis(self.preferred_retry_delay_ms),
            use_arp: self.use_arp_cache,
            use_mdns: self.use_mdns,
            mdns_timeout: Duration::from_millis(self.mdns_timeout_ms),
            use_ssdp: self.use_ssdp,
            ssdp_timeout: Duration::from_millis(self.ssdp_timeout_ms),
            response_filter: self.response_filter.clone(),
            cancel: self.cancellation_token.clone(),
        }
    }
}

/// Core async discovery logic shared between `discover`, `discover_streaming`,
/// and `discover_async`.
#[allow(clippy::too_many_lines)]
async fn run_discovery(
    p: DiscoveryParams,
    tx: Option<&std::sync::mpsc::SyncSender<DeviceMatch>>,
) -> crate::error::Result<Vec<DeviceMatch>> {
    let cancel = p.cancel.as_ref();
    let filter: Option<Arc<[u8]>> = p.response_filter.as_deref().map(Arc::from);

    // Preferred host fast-path with configurable retry.
    if let Some(ref host) = p.preferred {
        for attempt in 0..=p.preferred_retry {
            if cancel.is_some_and(CancellationToken::is_cancelled) {
                return Ok(Vec::new());
            }
            let matches = scan::probe_host(
                host,
                &p.ports,
                p.probe.as_deref(),
                p.connect_timeout,
                p.io_timeout,
                filter.clone(),
            )
            .await?;
            if !matches.is_empty() {
                if let Some(sender) = tx {
                    for m in &matches {
                        let _ = sender.try_send(m.clone());
                    }
                }
                return Ok(matches);
            }
            if attempt < p.preferred_retry {
                tokio::time::sleep(p.preferred_retry_delay).await;
            }
        }
    }

    // mDNS fast-path: send a DNS-SD query and probe responding devices.
    let mut fast_matches: Vec<DeviceMatch> = Vec::new();
    if p.use_mdns {
        let mdns_hosts = crate::net::mdns::active_mdns_hosts(p.mdns_timeout).await;
        for ip in mdns_hosts {
            let host_str = ip.to_string();
            let found = scan::probe_host(
                &host_str,
                &p.ports,
                p.probe.as_deref(),
                p.connect_timeout,
                p.io_timeout,
                filter.clone(),
            )
            .await?;
            for mut m in found {
                m.info.insert("source".to_owned(), "mdns".to_owned());
                if let Some(sender) = tx {
                    let _ = sender.try_send(m.clone());
                }
                fast_matches.push(m);
            }
        }
    }

    // SSDP fast-path: send an M-SEARCH query and probe UPnP-responding devices.
    if p.use_ssdp {
        let ssdp_hosts = crate::net::ssdp::active_ssdp_hosts(p.ssdp_timeout).await;
        for ip in ssdp_hosts {
            let host_str = ip.to_string();
            let found = scan::probe_host(
                &host_str,
                &p.ports,
                p.probe.as_deref(),
                p.connect_timeout,
                p.io_timeout,
                filter.clone(),
            )
            .await?;
            for mut m in found {
                m.info.insert("source".to_owned(), "ssdp".to_owned());
                if let Some(sender) = tx {
                    let _ = sender.try_send(m.clone());
                }
                fast_matches.push(m);
            }
        }
    }

    let mut all_matches = fast_matches;

    // Subnet sweep (the main path for unknown device locations).
    let targets = if p.subnets.is_empty() {
        scan::local_subnets()
    } else {
        p.subnets
    };

    let sweep_matches = scan::scan_subnets(
        &targets,
        &p.ports,
        p.probe.as_deref(),
        p.connect_timeout,
        p.io_timeout,
        p.max_concurrent,
        p.use_arp,
        cancel,
        tx,
        filter,
    )
    .await?;

    // Deduplicate by address: mDNS/SSDP fast-paths may have already found some
    // hosts that the subnet sweep also probed.
    let mut seen_addrs: HashSet<String> = all_matches.iter().map(|m| m.address.clone()).collect();
    for m in sweep_matches {
        if seen_addrs.insert(m.address.clone()) {
            all_matches.push(m);
        }
    }
    Ok(all_matches)
}
