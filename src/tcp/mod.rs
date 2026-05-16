//! TCP/IP device discovery.

pub mod scan;

use crate::{runtime::runtime, types::{DeviceMatch, CancellationToken}};
use pyo3::prelude::*;
use std::time::Duration;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Discovers devices reachable over TCP/IP.
///
/// When `probe_command` is set, it is written to each connection and the raw
/// response is returned in `DeviceMatch.info["response"]`. Any non-empty
/// response counts as a match. When `probe_command` is not set, a successful
/// TCP connection alone counts as a match (port-open check).
///
/// When `preferred_host` is set (hostname or IP address) it is resolved via DNS
/// and probed first. Only if no match is found does the library fall back to
/// sweeping the provided subnets.
///
/// When no subnets are provided, the library automatically discovers all
/// active network interfaces and sweeps their connected subnets.
#[pyclass]
pub struct TcpDiscovery {
    subnets: Vec<String>,
    port: u16,
    probe_command: Option<Vec<u8>>,
    timeout_ms: u64,
    max_concurrent: usize,
    preferred_host: Option<String>,
}

#[pymethods]
impl TcpDiscovery {
    /// Create a new [`TcpDiscovery`].
    ///
    /// Args:
    ///   `port`: TCP port to connect to on each host.
    ///   `subnets`: CIDR subnets to sweep (e.g. `["192.168.1.0/24"]`).
    ///     When empty, all connected network interface subnets are used.
    ///   `probe_command`: Optional bytes to send after connecting. When set,
    ///     only hosts that respond with any bytes are returned as matches.
    ///     When omitted, every host that accepts a TCP connection is a match.
    ///   `timeout_ms`: Per-host connect + I/O timeout in milliseconds.
    ///   `max_concurrent`: Maximum simultaneous open connections (default 500).
    ///   `preferred_host`: Hostname or IP to probe before sweeping subnets.
    ///     Hostnames are resolved via DNS; all returned addresses are tried.
    #[must_use]
    #[new]
    #[pyo3(signature = (
        port,
        subnets = vec![],
        probe_command = None,
        timeout_ms = 200,
        max_concurrent = 500,
        preferred_host = None,
    ))]
    pub const fn new(
        port: u16,
        subnets: Vec<String>,
        probe_command: Option<Vec<u8>>,
        timeout_ms: u64,
        max_concurrent: usize,
        preferred_host: Option<String>,
    ) -> Self {
        Self {
            subnets,
            port,
            probe_command,
            timeout_ms,
            max_concurrent,
            preferred_host,
        }
    }

    /// Run discovery and return a list of matching devices.
    ///
    /// Tries `preferred_host` first (with DNS resolution across all returned
    /// addresses). Falls back to sweeping all configured or auto-detected
    /// subnets only when necessary.
    ///
    /// # Errors
    ///
    /// Returns a [`pyo3::PyErr`] wrapping a [`crate::error::DafyddError`] if
    /// a subnet string is not valid CIDR notation.
    pub fn discover(&self, py: Python<'_>) -> PyResult<Vec<DeviceMatch>> {
        let subnets = self.subnets.clone();
        let port = self.port;
        let probe = self.probe_command.clone();
        let timeout = Duration::from_millis(self.timeout_ms);
        let max_concurrent = self.max_concurrent;
        let preferred = self.preferred_host.clone();

        py.allow_threads(|| {
            match runtime().block_on(async move {
                if let Some(host) = preferred {
                    if let Ok(Some(m)) =
                        scan::probe_host(&host, port, probe.as_deref(), timeout).await
                    {
                        return Ok(vec![m]);
                    }
                }

                let targets = if subnets.is_empty() {
                    scan::local_subnets()
                } else {
                    subnets
                };

                scan::scan_subnets(&targets, port, probe.as_deref(), timeout, max_concurrent).await
            }) {
                Ok(v) => Ok(v),
                Err(e) => Err(PyErr::from(e)),
            }
        })
    }
}
