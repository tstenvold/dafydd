//! Serial port device discovery.

pub mod probe;

use crate::{
    runtime::runtime,
    types::{CancellationToken, DeviceMatch},
};
use pyo3::prelude::*;
use std::{sync::Arc, time::Duration};

/// Discovers devices connected over serial ports.
///
/// When `probe_command` is `None`, `discover()` returns all available ports
/// without opening them — useful for enumerating what is connected. When
/// `probe_command` is supplied, it is sent to each port and only ports that
/// respond (optionally matching `response_filter`) are returned.
///
/// When `preferred_port` is supplied, that port is probed first at each
/// configured baud rate. Only if it produces no response does the library
/// sweep every available port. Use `preferred_retry` to retry the preferred
/// port before falling back.
///
/// A `cancellation_token` can be used to abort an in-progress sweep from
/// another thread by calling `.cancel()` on the token.
///
/// When `response_terminator` is set, the read loop exits as soon as the
/// accumulated response ends with those bytes, instead of waiting for the
/// full `timeout_ms`. This is useful for line-delimited protocols such as
/// SCPI (`\r\n`) or Modbus ASCII (`\r\n`).
///
/// # Platform notes
///
/// **Windows**: Bluetooth SPP virtual COM ports can stall for several seconds
/// per port when the paired device is off. They are excluded from the sweep
/// by default; set `include_bluetooth = True` to include them.
///
/// **macOS**: Each physical port appears as both `/dev/tty.XXX` and
/// `/dev/cu.XXX`. The `tty.*` variant is automatically filtered from sweeps
/// to avoid DCD-assertion stalls. If you supply a `preferred_port`, you may
/// pass either form — both are tried as-is.
#[pyclass]
pub struct SerialDiscovery {
    probe_command: Option<Vec<u8>>,
    baud_rates: Vec<u32>,
    timeout_ms: u64,
    preferred_port: Option<String>,
    preferred_retry: u32,
    preferred_retry_delay_ms: u64,
    include_bluetooth: bool,
    data_bits: Option<u8>,
    parity: Option<String>,
    stop_bits: Option<u8>,
    flow_control: Option<String>,
    port_filter: Option<String>,
    response_terminator: Option<Vec<u8>>,
    response_filter: Option<Vec<u8>>,
    cancellation_token: Option<CancellationToken>,
}

#[pymethods]
impl SerialDiscovery {
    /// Create a new [`SerialDiscovery`].
    ///
    /// Args:
    ///   `probe_command`: Bytes sent to the device to elicit a response.
    ///     When `None` (default), ports are enumerated without opening them.
    ///   `baud_rates`: Baud rates to attempt on each port, tried in order.
    ///     Required when `probe_command` is set; ignored otherwise.
    ///   `timeout_ms`: Per-port read/write timeout in milliseconds.
    ///   `preferred_port`: Optional port path to try first (e.g. `/dev/ttyUSB0`
    ///     or `COM3`). Falls back to a full sweep on no response. Ignored in
    ///     list-only mode (`probe_command=None`).
    ///   `preferred_retry`: Number of times to retry `preferred_port` before
    ///     falling back to a full sweep (default 0).
    ///   `preferred_retry_delay_ms`: Delay between preferred port retries in
    ///     milliseconds (default 500).
    ///   `include_bluetooth`: When `True`, Bluetooth SPP virtual COM ports are
    ///     included in the sweep.
    ///   `data_bits`: Number of data bits per character: 5, 6, 7, or 8 (default 8).
    ///   `parity`: Parity mode: `'none'`, `'even'`, or `'odd'` (default `'none'`).
    ///   `stop_bits`: Number of stop bits: 1 or 2 (default 1).
    ///   `flow_control`: Flow control: `'none'`, `'hardware'`, or `'software'`
    ///     (default `'none'`).
    ///   `port_filter`: Optional substring filter applied to port names during
    ///     the sweep (e.g. `'/dev/ttyUSB'` matches `/dev/ttyUSB0`, `/dev/ttyUSB1`).
    ///     Does not affect `preferred_port`.
    ///   `response_terminator`: If set, the read loop exits as soon as the
    ///     response ends with these bytes (e.g. `b'\r\n'`). Without this, every
    ///     probe waits the full `timeout_ms`.
    ///   `response_filter`: Optional bytes that must appear in the probe response
    ///     for a port to count as a match. When omitted, any non-empty response
    ///     is accepted. Has no effect when `probe_command` is not set.
    ///   `cancellation_token`: Optional token to cancel an in-progress sweep.
    #[must_use]
    #[new]
    #[pyo3(signature = (
        probe_command = None,
        baud_rates = vec![],
        timeout_ms = 500,
        preferred_port = None,
        preferred_retry = 0,
        preferred_retry_delay_ms = 500,
        include_bluetooth = false,
        data_bits = None,
        parity = None,
        stop_bits = None,
        flow_control = None,
        port_filter = None,
        response_terminator = None,
        response_filter = None,
        cancellation_token = None,
    ))]
    // PyO3's #[new] maps directly to Python's __init__; a builder pattern would
    // require a separate Python class, breaking the documented API contract.
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        probe_command: Option<Vec<u8>>,
        baud_rates: Vec<u32>,
        timeout_ms: u64,
        preferred_port: Option<String>,
        preferred_retry: u32,
        preferred_retry_delay_ms: u64,
        include_bluetooth: bool,
        data_bits: Option<u8>,
        parity: Option<String>,
        stop_bits: Option<u8>,
        flow_control: Option<String>,
        port_filter: Option<String>,
        response_terminator: Option<Vec<u8>>,
        response_filter: Option<Vec<u8>>,
        cancellation_token: Option<CancellationToken>,
    ) -> Self {
        Self {
            probe_command,
            baud_rates,
            timeout_ms,
            preferred_port,
            preferred_retry,
            preferred_retry_delay_ms,
            include_bluetooth,
            data_bits,
            parity,
            stop_bits,
            flow_control,
            port_filter,
            response_terminator,
            response_filter,
            cancellation_token,
        }
    }

    /// Run discovery and return a list of matching devices.
    ///
    /// When `probe_command` is `None`, returns all available ports without
    /// opening them (list-only mode). When `probe_command` is set, probes
    /// each port and returns only those that respond.
    ///
    /// # Errors
    ///
    /// Returns a [`pyo3::PyErr`] if `probe_command` is set but `baud_rates`
    /// is empty, or if the system port list cannot be read.
    pub fn discover(&self, py: Python<'_>) -> PyResult<Vec<DeviceMatch>> {
        self.validate()?;
        let config = self.config();
        py.detach(|| match runtime().block_on(run_discovery(config, None)) {
            Ok(v) => Ok(v),
            Err(e) => Err(PyErr::from(e)),
        })
    }

    /// Run discovery and call `callback(match)` for each device found.
    ///
    /// Calls the callback immediately as each port responds rather than
    /// waiting for all ports to complete. Useful for sweeps across many
    /// ports where early results matter.
    ///
    /// # Errors
    ///
    /// Returns a [`pyo3::PyErr`] if the system port list cannot be read or
    /// the callback raises an exception.
    #[allow(clippy::needless_pass_by_value)]
    pub fn discover_streaming(
        &self,
        py: Python<'_>,
        callback: Py<PyAny>,
    ) -> PyResult<Vec<DeviceMatch>> {
        self.validate()?;
        let config = self.config();
        let (tx, rx) = std::sync::mpsc::sync_channel::<DeviceMatch>(64);

        let handle =
            std::thread::spawn(move || runtime().block_on(run_discovery(config, Some(&tx))));

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
    /// Returns a [`pyo3::PyErr`] if `probe_command` is set but `baud_rates`
    /// is empty, or the system port list cannot be read.
    pub fn discover_async<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.validate()?;
        let config = self.config();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            run_discovery(config, None).await.map_err(PyErr::from)
        })
    }

    /// Watch for serial ports appearing or disappearing.
    ///
    /// Polls `discover()` every `interval_ms` milliseconds. Requires a
    /// `cancellation_token` to know when to stop.
    ///
    /// Args:
    ///   `on_added`: Called with each newly-found `DeviceMatch`.
    ///   `on_removed`: Called with each `DeviceMatch` that disappeared.
    ///   `interval_ms`: Poll interval in milliseconds (default 2000).
    ///
    /// # Errors
    ///
    /// Returns a [`pyo3::PyErr`] if no cancellation token is configured or a
    /// callback raises an exception.
    #[allow(clippy::needless_pass_by_value)]
    pub fn watch(
        &self,
        py: Python<'_>,
        on_added: Py<PyAny>,
        on_removed: Py<PyAny>,
        interval_ms: Option<u64>,
    ) -> PyResult<()> {
        let Some(ref cancel) = self.cancellation_token else {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "SerialDiscovery.watch() requires a cancellation_token",
            ));
        };

        let interval = Duration::from_millis(interval_ms.unwrap_or(2000));
        crate::watch::poll_watch(py, cancel, interval, &on_added, &on_removed, |py| {
            self.discover(py)
        })
    }
}

impl SerialDiscovery {
    /// Validate parameters before starting any I/O.
    fn validate(&self) -> PyResult<()> {
        if self.probe_command.is_some() && self.baud_rates.is_empty() {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "baud_rates is required when probe_command is set",
            ));
        }
        Ok(())
    }

    fn config(&self) -> DiscoveryConfig {
        DiscoveryConfig {
            probe: self.probe_command.as_deref().map(Arc::from),
            bauds: Arc::from(self.baud_rates.as_slice()),
            timeout: Duration::from_millis(self.timeout_ms),
            preferred_port: self.preferred_port.clone(),
            preferred_retry: self.preferred_retry,
            preferred_retry_delay: Duration::from_millis(self.preferred_retry_delay_ms),
            include_bluetooth: self.include_bluetooth,
            data_bits: self.data_bits,
            parity: self.parity.as_deref().map(Arc::from),
            stop_bits: self.stop_bits,
            flow_control: self.flow_control.as_deref().map(Arc::from),
            port_filter: self.port_filter.as_deref().map(Arc::from),
            response_terminator: self.response_terminator.as_deref().map(Arc::from),
            response_filter: self.response_filter.as_deref().map(Arc::from),
            cancel: self.cancellation_token.clone(),
        }
    }
}

/// Snapshot of `SerialDiscovery` config for passing into async tasks.
struct DiscoveryConfig {
    probe: Option<Arc<[u8]>>,
    bauds: Arc<[u32]>,
    timeout: Duration,
    preferred_port: Option<String>,
    preferred_retry: u32,
    preferred_retry_delay: Duration,
    include_bluetooth: bool,
    data_bits: Option<u8>,
    parity: Option<Arc<str>>,
    stop_bits: Option<u8>,
    flow_control: Option<Arc<str>>,
    port_filter: Option<Arc<str>>,
    response_terminator: Option<Arc<[u8]>>,
    response_filter: Option<Arc<[u8]>>,
    cancel: Option<CancellationToken>,
}

async fn run_discovery(
    cfg: DiscoveryConfig,
    tx: Option<&std::sync::mpsc::SyncSender<DeviceMatch>>,
) -> crate::error::Result<Vec<DeviceMatch>> {
    // List-only mode: no probe command, just enumerate available ports.
    let Some(probe) = cfg.probe else {
        return probe::list_all_ports(cfg.include_bluetooth, cfg.port_filter.as_deref());
    };

    // Preferred port fast-path with configurable retry.
    if let Some(ref port) = cfg.preferred_port {
        for attempt in 0..=cfg.preferred_retry {
            if cfg
                .cancel
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
            {
                return Ok(Vec::new());
            }

            if let Ok(Some(m)) = probe::probe_port_all_bauds(
                port.clone(),
                Arc::clone(&cfg.bauds),
                Arc::clone(&probe),
                cfg.timeout,
                cfg.data_bits,
                cfg.parity.clone(),
                cfg.stop_bits,
                cfg.flow_control.clone(),
                cfg.cancel.as_ref().map(|c| Arc::clone(&c.inner())),
                cfg.response_terminator.clone(),
                cfg.response_filter.clone(),
            )
            .await
            {
                if let Some(sender) = tx {
                    let _ = sender.try_send(m.clone());
                }
                return Ok(vec![m]);
            }

            if attempt < cfg.preferred_retry {
                tokio::time::sleep(cfg.preferred_retry_delay).await;
            }
        }
    }

    probe::sweep_all_ports(
        probe.as_ref(),
        cfg.bauds.as_ref(),
        cfg.timeout,
        cfg.include_bluetooth,
        cfg.data_bits,
        cfg.parity,
        cfg.stop_bits,
        cfg.flow_control,
        cfg.port_filter.as_deref(),
        cfg.cancel.as_ref(),
        tx,
        cfg.response_terminator,
        cfg.response_filter,
    )
    .await
}
