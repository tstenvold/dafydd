//! Serial port device discovery.

pub mod probe;

use crate::{
    runtime::runtime,
    types::{CancellationToken, DeviceMatch},
};
use pyo3::prelude::*;
use std::time::Duration;

/// Discovers devices connected over serial ports.
///
/// Sends `probe_command` to each port and returns every port that responds
/// with any bytes. The raw response is available in `DeviceMatch.response`.
///
/// When `preferred_port` is supplied, that port is probed first at each
/// configured baud rate. Only if it produces no response does the library
/// sweep every available port. Use `preferred_retry` to retry the preferred
/// port before falling back.
///
/// A `cancellation_token` can be used to abort an in-progress sweep from
/// another thread by calling `.cancel()` on the token.
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
    probe_command: Vec<u8>,
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
    cancellation_token: Option<CancellationToken>,
}

#[pymethods]
impl SerialDiscovery {
    /// Create a new [`SerialDiscovery`].
    ///
    /// Args:
    ///   `probe_command`: Bytes sent to the device to elicit a response.
    ///   `baud_rates`: Baud rates to attempt on each port, tried in order.
    ///   `timeout_ms`: Per-port read/write timeout in milliseconds.
    ///   `preferred_port`: Optional port path to try first (e.g. `/dev/ttyUSB0`
    ///     or `COM3`). Falls back to a full sweep on no response.
    ///   `preferred_retry`: Number of times to retry `preferred_port` before
    ///     falling back to a full sweep (default 0).
    ///   `preferred_retry_delay_ms`: Delay between preferred port retries in
    ///     milliseconds (default 500).
    ///   `include_bluetooth`: When `True`, Bluetooth SPP virtual COM ports are
    ///     included in the sweep.
    ///   `cancellation_token`: Optional token to cancel an in-progress sweep.
    #[must_use]
    #[new]
    #[pyo3(signature = (
        probe_command,
        baud_rates,
        timeout_ms = 500,
        preferred_port = None,
        preferred_retry = 0,
        preferred_retry_delay_ms = 500,
        include_bluetooth = false,
        data_bits = None,
        parity = None,
        stop_bits = None,
        flow_control = None,
        cancellation_token = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        probe_command: Vec<u8>,
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
            cancellation_token,
        }
    }

    /// Run discovery and return a list of matching devices.
    ///
    /// Tries `preferred_port` (at all configured baud rates, sequentially)
    /// first when set, with configurable retry. Falls back to sweeping all
    /// available ports only if the preferred port produces no match.
    ///
    /// # Errors
    ///
    /// Returns a [`pyo3::PyErr`] wrapping a [`crate::error::DafyddError`] if
    /// the system port list cannot be read.
    pub fn discover(&self, py: Python<'_>) -> PyResult<Vec<DeviceMatch>> {
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
        let mut prev: Vec<DeviceMatch> = Vec::new();

        loop {
            if cancel.is_cancelled() {
                break;
            }

            let current = self.discover(py)?;

            for m in &current {
                if !prev.iter().any(|p| p == m) {
                    on_added.call1(py, (m.clone(),))?;
                }
            }
            for m in &prev {
                if !current.iter().any(|c| c == m) {
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

/// Snapshot of `SerialDiscovery` config for passing into async tasks.
struct DiscoveryConfig {
    probe: std::sync::Arc<[u8]>,
    bauds: std::sync::Arc<[u32]>,
    timeout: Duration,
    preferred_port: Option<String>,
    preferred_retry: u32,
    preferred_retry_delay: Duration,
    include_bluetooth: bool,
    data_bits: Option<u8>,
    parity: Option<String>,
    stop_bits: Option<u8>,
    flow_control: Option<String>,
    cancel: Option<CancellationToken>,
}

impl SerialDiscovery {
    fn config(&self) -> DiscoveryConfig {
        DiscoveryConfig {
            probe: std::sync::Arc::from(self.probe_command.as_slice()),
            bauds: std::sync::Arc::from(self.baud_rates.as_slice()),
            timeout: Duration::from_millis(self.timeout_ms),
            preferred_port: self.preferred_port.clone(),
            preferred_retry: self.preferred_retry,
            preferred_retry_delay: Duration::from_millis(self.preferred_retry_delay_ms),
            include_bluetooth: self.include_bluetooth,
            data_bits: self.data_bits,
            parity: self.parity.clone(),
            stop_bits: self.stop_bits,
            flow_control: self.flow_control.clone(),
            cancel: self.cancellation_token.clone(),
        }
    }
}

async fn run_discovery(
    cfg: DiscoveryConfig,
    tx: Option<&std::sync::mpsc::SyncSender<DeviceMatch>>,
) -> crate::error::Result<Vec<DeviceMatch>> {
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
                std::sync::Arc::clone(&cfg.bauds),
                std::sync::Arc::clone(&cfg.probe),
                cfg.timeout,
                cfg.data_bits,
                cfg.parity.clone(),
                cfg.stop_bits,
                cfg.flow_control.clone(),
                cfg.cancel
                    .as_ref()
                    .map(|c| std::sync::Arc::clone(&c.inner())),
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
        cfg.probe.as_ref(),
        cfg.bauds.as_ref(),
        cfg.timeout,
        cfg.include_bluetooth,
        cfg.data_bits,
        cfg.parity,
        cfg.stop_bits,
        cfg.flow_control,
        cfg.cancel.as_ref(),
        tx,
    )
    .await
}
