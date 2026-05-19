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
///
/// `max_prefix` is the broadest subnet width auto-detection will widen *to*.
/// Defaults to /24 (≤254 hosts). Must be in `[16, 32]`; values outside that
/// range raise `ValueError` to prevent accidental /15-or-wider sweeps.
///
/// # Errors
///
/// Raises `ValueError` when `max_prefix` is outside `16..=32`.
#[pyfunction]
#[pyo3(signature = (max_prefix = 24))]
pub fn local_subnets(max_prefix: u8) -> PyResult<Vec<String>> {
    scan::local_subnets(max_prefix).map_err(PyErr::from)
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
    subnet_prefix: u8,
    tcp_linger_seconds: Option<u32>,
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
    ///   `subnet_prefix`: Broadest subnet width to widen auto-detected
    ///     interfaces to (default 24, ≤254 hosts). Must be in `[16, 32]`.
    ///     Only used when `subnets` is empty.
    ///   `tcp_linger_seconds`: TCP `SO_LINGER` value. `None` (default) =
    ///     graceful FIN close (OS default); `Some(0)` = RST close, no
    ///     `TIME_WAIT` — fast but antisocial; `Some(n)` = block close for
    ///     up to `n` seconds.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when `subnet_prefix` is outside `16..=32`.
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
        subnet_prefix = 24,
        tcp_linger_seconds = None,
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
        subnet_prefix: u8,
        tcp_linger_seconds: Option<u32>,
    ) -> PyResult<Self> {
        if !(16..=32).contains(&subnet_prefix) {
            return Err(PyValueError::new_err(format!(
                "subnet_prefix /{subnet_prefix} out of range; must be in [16, 32]"
            )));
        }

        // Merge `port` + `ports`, deduplicate while preserving order.
        let mut seen = std::collections::HashSet::new();
        let all_ports: Vec<u16> = port
            .into_iter()
            .chain(ports)
            .filter(|p| seen.insert(*p))
            .collect();
        Ok(Self {
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
            subnet_prefix,
            tcp_linger_seconds,
        })
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
        crate::watch::poll_watch(py, cancel, interval, &on_added, &on_removed, |py| {
            self.discover(py)
        })
    }
}

/// Snapshot of `TcpDiscovery` config cloneable into async tasks.
struct DiscoveryParams {
    ports: Arc<[u16]>,
    subnets: Vec<String>,
    probe: Option<Arc<[u8]>>,
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
    subnet_prefix: u8,
    linger: Option<Duration>,
}

impl TcpDiscovery {
    fn params(&self) -> DiscoveryParams {
        DiscoveryParams {
            ports: Arc::from(self.ports.as_slice()),
            subnets: self.subnets.clone(),
            probe: self.probe_command.as_deref().map(Arc::from),
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
            subnet_prefix: self.subnet_prefix,
            linger: self
                .tcp_linger_seconds
                .map(|s| Duration::from_secs(u64::from(s))),
        }
    }
}

/// Handle the preferred-host fast-path with configurable retry.
///
/// Returns `Some(matches)` when the preferred host responds (caller should
/// return early). Returns `None` to fall through to the subnet sweep.
async fn try_preferred(
    p: &DiscoveryParams,
    filter: Option<Arc<[u8]>>,
    tx: Option<&std::sync::mpsc::SyncSender<DeviceMatch>>,
) -> crate::error::Result<Option<Vec<DeviceMatch>>> {
    let Some(ref host) = p.preferred else {
        return Ok(None);
    };
    let cancel = p.cancel.as_ref();
    for attempt in 0..=p.preferred_retry {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return Ok(Some(Vec::new()));
        }
        let matches = scan::probe_host(
            host,
            p.ports.as_ref(),
            p.probe.as_deref(),
            p.connect_timeout,
            p.io_timeout,
            filter.clone(),
            p.linger,
        )
        .await?;
        if !matches.is_empty() {
            if let Some(sender) = tx {
                for m in &matches {
                    let _ = sender.try_send(m.clone());
                }
            }
            return Ok(Some(matches));
        }
        if attempt < p.preferred_retry {
            tokio::time::sleep(p.preferred_retry_delay).await;
        }
    }
    Ok(None)
}

/// Probe a list of IPs, tag each match with `source`, forward to `tx`, and
/// return all matches.
async fn probe_named_hosts(
    ips: Vec<std::net::Ipv4Addr>,
    source: &'static str,
    p: &DiscoveryParams,
    filter: Option<Arc<[u8]>>,
    tx: Option<&std::sync::mpsc::SyncSender<DeviceMatch>>,
) -> crate::error::Result<Vec<DeviceMatch>> {
    let mut matches = Vec::new();
    for ip in ips {
        let host_str = ip.to_string();
        let found = scan::probe_host(
            &host_str,
            p.ports.as_ref(),
            p.probe.as_deref(),
            p.connect_timeout,
            p.io_timeout,
            filter.clone(),
            p.linger,
        )
        .await?;
        for mut m in found {
            m.info.insert("source".to_owned(), source.to_owned());
            if let Some(sender) = tx {
                let _ = sender.try_send(m.clone());
            }
            matches.push(m);
        }
    }
    Ok(matches)
}

/// Core async discovery logic shared between `discover`, `discover_streaming`,
/// and `discover_async`.
async fn run_discovery(
    p: DiscoveryParams,
    tx: Option<&std::sync::mpsc::SyncSender<DeviceMatch>>,
) -> crate::error::Result<Vec<DeviceMatch>> {
    let cancel = p.cancel.as_ref();
    let filter: Option<Arc<[u8]>> = p.response_filter.as_deref().map(Arc::from);

    if let Some(matches) = try_preferred(&p, filter.clone(), tx).await? {
        return Ok(matches);
    }

    let mut all_matches = Vec::new();

    if p.use_mdns {
        let h = crate::net::mdns::active_mdns_hosts(p.mdns_timeout).await;
        all_matches.extend(probe_named_hosts(h, "mdns", &p, filter.clone(), tx).await?);
    }

    if p.use_ssdp {
        let h = crate::net::ssdp::active_ssdp_hosts(p.ssdp_timeout).await;
        all_matches.extend(probe_named_hosts(h, "ssdp", &p, filter.clone(), tx).await?);
    }

    // Subnet sweep (the main path for unknown device locations).
    let targets = if p.subnets.is_empty() {
        scan::local_subnets(p.subnet_prefix)?
    } else {
        p.subnets
    };

    let sweep_matches = scan::scan_subnets(
        &targets,
        p.ports.as_ref(),
        p.probe.as_deref(),
        p.connect_timeout,
        p.io_timeout,
        p.max_concurrent,
        p.use_arp,
        cancel,
        tx,
        filter,
        p.linger,
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
