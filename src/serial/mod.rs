//! Serial port device discovery.

pub mod probe;

use crate::{runtime::runtime, types::DeviceMatch};
use pyo3::prelude::*;
use std::time::Duration;

/// Discovers devices connected over serial ports.
///
/// Sends `probe_command` to each port and returns every port that responds
/// with any bytes. The raw response is available in `DeviceMatch.info["response"]`.
///
/// When `preferred_port` is supplied, that port is probed first at each
/// configured baud rate. Only if it produces no response does the library
/// sweep every available port.
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
    include_bluetooth: bool,
    data_bits: Option<u8>,
    parity: Option<String>,
    stop_bits: Option<u8>,
    flow_control: Option<String>,
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
    ///   `include_bluetooth`: When `True`, Bluetooth SPP virtual COM ports are
    ///     included in the sweep. Defaults to `False` because they stall for
    ///     several seconds per port when the paired device is unreachable.
    #[must_use]
    #[new]
    #[pyo3(signature = (
        probe_command,
        baud_rates,
        timeout_ms = 500,
        preferred_port = None,
        include_bluetooth = false,
        data_bits = None,
        parity = None,
        stop_bits = None,
        flow_control = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        probe_command: Vec<u8>,
        baud_rates: Vec<u32>,
        timeout_ms: u64,
        preferred_port: Option<String>,
        include_bluetooth: bool,
        data_bits: Option<u8>,
        parity: Option<String>,
        stop_bits: Option<u8>,
        flow_control: Option<String>,
    ) -> Self {
        Self {
            probe_command,
            baud_rates,
            timeout_ms,
            preferred_port,
            include_bluetooth,
            data_bits,
            parity,
            stop_bits,
            flow_control,
        }
    }

    /// Run discovery and return a list of matching devices.
    ///
    /// Tries `preferred_port` (at all configured baud rates, sequentially)
    /// first when set. Falls back to sweeping all available ports only if
    /// the preferred port produces no match.
    ///
    /// # Errors
    ///
    /// Returns a [`pyo3::PyErr`] wrapping a [`crate::error::DafyddError`] if
    /// the system port list cannot be read.
    pub fn discover(&self, py: Python<'_>) -> PyResult<Vec<DeviceMatch>> {
        let probe = self.probe_command.clone();
        let bauds = self.baud_rates.clone();
        let timeout = Duration::from_millis(self.timeout_ms);
        let preferred = self.preferred_port.clone();
        let include_bluetooth = self.include_bluetooth;
        let data_bits = self.data_bits;
        let parity = self.parity.clone();
        let stop_bits = self.stop_bits;
        let flow_control = self.flow_control.clone();

        py.detach(|| {
            match runtime().block_on(async move {
                if let Some(port) = preferred {
                    if let Ok(Some(m)) = probe::probe_port_all_bauds(
                        port,
                        bauds.clone(),
                        probe.clone(),
                        timeout,
                        data_bits,
                        parity.clone(),
                        stop_bits,
                        flow_control.clone(),
                    )
                    .await
                    {
                        return Ok(vec![m]);
                    }
                }

                probe::sweep_all_ports(
                    &probe,
                    &bauds,
                    timeout,
                    include_bluetooth,
                    data_bits,
                    parity,
                    stop_bits,
                    flow_control,
                )
                .await
            }) {
                Ok(v) => Ok(v),
                Err(e) => Err(PyErr::from(e)),
            }
        })
    }
}
