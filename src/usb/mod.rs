//! USB device discovery via `nusb`.

use crate::{
    error::DafyddError,
    runtime::runtime,
    types::{DeviceMatch, Transport},
};
use pyo3::prelude::*;
use std::collections::HashMap;

/// Discovers USB devices, optionally filtered by Vendor ID and/or Product ID.
#[pyclass]
pub struct UsbDiscovery {
    vid: Option<u16>,
    pid: Option<u16>,
}

#[pymethods]
impl UsbDiscovery {
    /// Create a new [`UsbDiscovery`].
    ///
    /// Args:
    ///   `vid`: Optional USB Vendor ID filter (e.g. `0x1234`).
    ///   `pid`: Optional USB Product ID filter (e.g. `0x5678`).
    #[must_use]
    #[new]
    #[pyo3(signature = (vid = None, pid = None))]
    pub const fn new(vid: Option<u16>, pid: Option<u16>) -> Self {
        Self { vid, pid }
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

                    let mut info: HashMap<String, String> = HashMap::new();
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

                    matches.push(DeviceMatch {
                        transport: Transport::Usb,
                        address: format!(
                            "{:#06x}:{:#06x}",
                            device.vendor_id(),
                            device.product_id()
                        ),
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
}
