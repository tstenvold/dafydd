//! USB device discovery via `nusb`.

use crate::{
    error::DafyddError,
    runtime::runtime,
    types::{CancellationToken, DeviceMatch, Transport},
};
use pyo3::prelude::*;
use std::{collections::HashMap, time::Duration};

/// Discovers USB devices, optionally filtered by Vendor ID and/or Product ID.
#[pyclass]
pub struct UsbDiscovery {
    vid: Option<u16>,
    pid: Option<u16>,
    manufacturer: Option<String>,
    product_string: Option<String>,
    serial_number: Option<String>,
    cancellation_token: Option<CancellationToken>,
}

#[pymethods]
impl UsbDiscovery {
    /// Create a new [`UsbDiscovery`].
    ///
    /// Args:
    ///   `vid`: Optional USB Vendor ID filter (e.g. `0x1234`).
    ///   `pid`: Optional USB Product ID filter (e.g. `0x5678`).
    ///   `manufacturer`: Optional substring filter on manufacturer string.
    ///   `product_string`: Optional substring filter on product string.
    ///   `serial_number`: Optional substring filter on USB serial number.
    ///   `cancellation_token`: Optional token to cancel an in-progress watch.
    #[must_use]
    #[new]
    #[pyo3(signature = (
        vid = None,
        pid = None,
        manufacturer = None,
        product_string = None,
        serial_number = None,
        cancellation_token = None,
    ))]
    pub const fn new(
        vid: Option<u16>,
        pid: Option<u16>,
        manufacturer: Option<String>,
        product_string: Option<String>,
        serial_number: Option<String>,
        cancellation_token: Option<CancellationToken>,
    ) -> Self {
        Self {
            vid,
            pid,
            manufacturer,
            product_string,
            serial_number,
            cancellation_token,
        }
    }

    /// Return all connected USB devices matching the configured filters.
    ///
    /// # Errors
    ///
    /// Returns a [`pyo3::PyErr`] wrapping a [`DafyddError::Usb`] if the OS
    /// USB device list cannot be read.
    pub fn discover(&self, py: Python<'_>) -> PyResult<Vec<DeviceMatch>> {
        let vid = self.vid;
        let pid = self.pid;
        let mfg_filter = self.manufacturer.clone();
        let prod_filter = self.product_string.clone();
        let sn_filter = self.serial_number.clone();

        py.detach(|| {
            let inner = || -> crate::error::Result<Vec<DeviceMatch>> {
                let devices = runtime()
                    .block_on(async { nusb::list_devices().await })
                    .map_err(|e| DafyddError::Usb(e.to_string()))?;

                let mut matches = Vec::new();
                for device in devices {
                    if vid.is_some_and(|v| device.vendor_id() != v) {
                        continue;
                    }
                    if pid.is_some_and(|p| device.product_id() != p) {
                        continue;
                    }
                    if let Some(m_filter) = &mfg_filter {
                        if !device
                            .manufacturer_string()
                            .is_some_and(|m| m.contains(m_filter))
                        {
                            continue;
                        }
                    }
                    if let Some(p_filter) = &prod_filter {
                        if !device
                            .product_string()
                            .is_some_and(|p| p.contains(p_filter))
                        {
                            continue;
                        }
                    }
                    if let Some(sn_filter) = &sn_filter {
                        if !device
                            .serial_number()
                            .is_some_and(|sn| sn.contains(sn_filter))
                        {
                            continue;
                        }
                    }

                    let mut info: HashMap<String, String> = HashMap::with_capacity(6);
                    info.insert(
                        "vendor_id".to_owned(),
                        format!("{:#06x}", device.vendor_id()),
                    );
                    info.insert(
                        "product_id".to_owned(),
                        format!("{:#06x}", device.product_id()),
                    );
                    if let Some(m) = device.manufacturer_string() {
                        info.insert("manufacturer".to_owned(), m.to_owned());
                    }
                    if let Some(p) = device.product_string() {
                        info.insert("product".to_owned(), p.to_owned());
                    }
                    if let Some(s) = device.serial_number() {
                        info.insert("serial_number".to_owned(), s.to_owned());
                    }

                    // Include serial number in the address so two devices with
                    // the same VID:PID but different serial numbers are distinct.
                    let address = device.serial_number().map_or_else(
                        || format!("{:#06x}:{:#06x}", device.vendor_id(), device.product_id()),
                        |sn| {
                            format!(
                                "{:#06x}:{:#06x}:{}",
                                device.vendor_id(),
                                device.product_id(),
                                sn
                            )
                        },
                    );

                    matches.push(DeviceMatch {
                        transport: Transport::Usb,
                        address,
                        response: None,
                        info,
                    });
                }
                Ok(matches)
            };

            match inner() {
                Ok(v) => Ok(v),
                Err(e) => Err(PyErr::from(e)),
            }
        })
    }

    /// Run discovery and call `callback(match)` for each device found.
    ///
    /// # Errors
    ///
    /// Returns a [`pyo3::PyErr`] if device enumeration fails or the callback
    /// raises an exception.
    #[allow(clippy::needless_pass_by_value)]
    pub fn discover_streaming(
        &self,
        py: Python<'_>,
        callback: Py<PyAny>,
    ) -> PyResult<Vec<DeviceMatch>> {
        let matches = self.discover(py)?;
        for m in &matches {
            callback.call1(py, (m.clone(),))?;
        }
        Ok(matches)
    }

    /// Watch for USB devices appearing or disappearing.
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
                "UsbDiscovery.watch() requires a cancellation_token",
            ));
        };

        let interval = Duration::from_millis(interval_ms.unwrap_or(2000));
        let mut prev: Vec<DeviceMatch> = Vec::new();

        loop {
            if cancel.is_cancelled() {
                break;
            }

            let current = self.discover(py)?;

            let prev_addrs: std::collections::HashSet<&str> =
                prev.iter().map(|m| m.address.as_str()).collect();
            let current_addrs: std::collections::HashSet<&str> =
                current.iter().map(|m| m.address.as_str()).collect();

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
