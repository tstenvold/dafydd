//! USB device discovery via `nusb`.

use crate::{
    error::DafyddError,
    runtime::runtime,
    types::{CancellationToken, DeviceMatch, Transport},
};
use pyo3::prelude::*;
use std::collections::HashMap;

/// Apply VID/PID/string/class filters to a USB device info struct.
// Each argument is an independent optional filter; collapsing them into a struct
// would force callers to construct a filter object for every call site.
#[allow(clippy::too_many_arguments)]
fn apply_filters(
    device: &nusb::DeviceInfo,
    vid: Option<u16>,
    pid: Option<u16>,
    mfg_filter: Option<&str>,
    prod_filter: Option<&str>,
    sn_filter: Option<&str>,
    class_filter: Option<u8>,
) -> bool {
    if vid.is_some_and(|v| device.vendor_id() != v) {
        return false;
    }
    if pid.is_some_and(|p| device.product_id() != p) {
        return false;
    }
    if mfg_filter.is_some_and(|f| !device.manufacturer_string().is_some_and(|m| m.contains(f))) {
        return false;
    }
    if prod_filter.is_some_and(|f| !device.product_string().is_some_and(|p| p.contains(f))) {
        return false;
    }
    if sn_filter.is_some_and(|f| !device.serial_number().is_some_and(|s| s.contains(f))) {
        return false;
    }
    if class_filter.is_some_and(|c| device.class() != c) {
        return false;
    }
    true
}

/// Build a [`DeviceMatch`] from a `nusb::DeviceInfo`.
fn build_device_match(device: &nusb::DeviceInfo) -> DeviceMatch {
    let mut info: HashMap<String, String> = HashMap::with_capacity(7);
    info.insert(
        "vendor_id".to_owned(),
        format!("{:#06x}", device.vendor_id()),
    );
    info.insert(
        "product_id".to_owned(),
        format!("{:#06x}", device.product_id()),
    );
    info.insert("device_class".to_owned(), device.class().to_string());
    if let Some(m) = device.manufacturer_string() {
        info.insert("manufacturer".to_owned(), m.to_owned());
    }
    if let Some(p) = device.product_string() {
        info.insert("product".to_owned(), p.to_owned());
    }
    if let Some(s) = device.serial_number() {
        info.insert("serial_number".to_owned(), s.to_owned());
    }

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

    DeviceMatch {
        transport: Transport::Usb,
        address,
        response: None,
        info,
    }
}

/// Discovers USB devices, optionally filtered by Vendor ID and/or Product ID.
#[pyclass]
pub struct UsbDiscovery {
    vid: Option<u16>,
    pid: Option<u16>,
    manufacturer: Option<String>,
    product_string: Option<String>,
    serial_number: Option<String>,
    device_class: Option<u8>,
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
    ///   `device_class`: Optional USB device class code filter (e.g. `0x03` for HID).
    ///   `cancellation_token`: Optional token to cancel an in-progress watch.
    #[must_use]
    #[new]
    #[pyo3(signature = (
        vid = None,
        pid = None,
        manufacturer = None,
        product_string = None,
        serial_number = None,
        device_class = None,
        cancellation_token = None,
    ))]
    pub const fn new(
        vid: Option<u16>,
        pid: Option<u16>,
        manufacturer: Option<String>,
        product_string: Option<String>,
        serial_number: Option<String>,
        device_class: Option<u8>,
        cancellation_token: Option<CancellationToken>,
    ) -> Self {
        Self {
            vid,
            pid,
            manufacturer,
            product_string,
            serial_number,
            device_class,
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
        let class_filter = self.device_class;

        py.detach(|| {
            let inner = || -> crate::error::Result<Vec<DeviceMatch>> {
                let devices = runtime()
                    .block_on(async { nusb::list_devices().await })
                    .map_err(DafyddError::from)?;
                Ok(devices
                    .filter(|d| {
                        apply_filters(
                            d,
                            vid,
                            pid,
                            mfg_filter.as_deref(),
                            prod_filter.as_deref(),
                            sn_filter.as_deref(),
                            class_filter,
                        )
                    })
                    .map(|d| build_device_match(&d))
                    .collect())
            };

            match inner() {
                Ok(v) => Ok(v),
                Err(e) => Err(PyErr::from(e)),
            }
        })
    }

    /// Run discovery as a Python coroutine, returning all matches when awaited.
    ///
    /// Equivalent to `discover()` but non-blocking — suitable for use in
    /// `asyncio` event loops without `run_in_executor`.
    ///
    /// # Errors
    ///
    /// Returns a [`pyo3::PyErr`] if device enumeration fails.
    pub fn discover_async<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let vid = self.vid;
        let pid = self.pid;
        let mfg_filter = self.manufacturer.clone();
        let prod_filter = self.product_string.clone();
        let sn_filter = self.serial_number.clone();
        let class_filter = self.device_class;

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let devices = nusb::list_devices()
                .await
                .map_err(|e| PyErr::from(DafyddError::from(e)))?;
            Ok(devices
                .filter(|d| {
                    apply_filters(
                        d,
                        vid,
                        pid,
                        mfg_filter.as_deref(),
                        prod_filter.as_deref(),
                        sn_filter.as_deref(),
                        class_filter,
                    )
                })
                .map(|d| build_device_match(&d))
                .collect::<Vec<_>>())
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
        crate::streaming::run_streaming(py, &callback, |py| self.discover(py))
    }

    /// Watch for USB devices appearing or disappearing using OS-level hotplug events.
    ///
    /// Calls `on_added` immediately for all currently-connected matching devices,
    /// then blocks until the cancellation token is cancelled, calling `on_added`
    /// or `on_removed` as devices connect or disconnect.
    ///
    /// Args:
    ///   `on_added`: Called with each newly-found `DeviceMatch`.
    ///   `on_removed`: Called with each `DeviceMatch` that disappeared.
    ///   `interval_ms`: Ignored — retained for API compatibility. Events are
    ///     delivered immediately via OS hotplug notifications.
    ///
    /// # Errors
    ///
    /// Returns a [`pyo3::PyErr`] if no cancellation token is configured, the OS
    /// hotplug watcher cannot be started, or a callback raises an exception.
    #[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
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
        let _ = interval_ms; // superseded by OS hotplug events

        let vid = self.vid;
        let pid = self.pid;
        let mfg_filter = self.manufacturer.clone();
        let prod_filter = self.product_string.clone();
        let sn_filter = self.serial_number.clone();
        let class_filter = self.device_class;

        let watcher = nusb::watch_devices().map_err(|e| PyErr::from(DafyddError::from(e)))?;

        // Channel: background OS-event thread → Python main thread.
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel::<nusb::hotplug::HotplugEvent>(64);
        let cancel_inner = cancel.inner();

        // Background thread: drain the OS hotplug stream into the channel.
        // Uses a 100 ms timeout per poll so cancellation is checked promptly.
        std::thread::spawn(move || {
            use futures_util::StreamExt;
            runtime().block_on(async move {
                let mut watch = std::pin::pin!(watcher);
                loop {
                    if cancel_inner.load(std::sync::atomic::Ordering::Relaxed) {
                        break;
                    }
                    match tokio::time::timeout(std::time::Duration::from_millis(100), watch.next())
                        .await
                    {
                        Ok(Some(event)) => {
                            if event_tx.send(event).is_err() {
                                break;
                            }
                        }
                        Ok(None) => break, // stream closed
                        Err(_timeout) => {}
                    }
                }
            });
        });

        // Build initial DeviceId → DeviceMatch map (used to correlate disconnect events).
        let mut tracked: HashMap<nusb::DeviceId, DeviceMatch> = py.detach(|| {
            runtime().block_on(async {
                let Ok(devices) = nusb::list_devices().await else {
                    return HashMap::new();
                };
                devices
                    .filter(|d| {
                        apply_filters(
                            d,
                            vid,
                            pid,
                            mfg_filter.as_deref(),
                            prod_filter.as_deref(),
                            sn_filter.as_deref(),
                            class_filter,
                        )
                    })
                    .map(|d| (d.id(), build_device_match(&d)))
                    .collect()
            })
        });

        // Emit on_added for all initially-present devices.
        for m in tracked.values() {
            on_added.call1(py, (m.clone(),))?;
        }

        // Process hotplug events until cancelled.
        loop {
            if cancel.is_cancelled() {
                break;
            }
            match event_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(nusb::hotplug::HotplugEvent::Connected(info)) => {
                    if !apply_filters(
                        &info,
                        vid,
                        pid,
                        mfg_filter.as_deref(),
                        prod_filter.as_deref(),
                        sn_filter.as_deref(),
                        class_filter,
                    ) {
                        continue;
                    }
                    // Guard against devices enumerated in the initial pass that also
                    // fire a Connected event (narrow race between watcher start and list).
                    if let std::collections::hash_map::Entry::Vacant(e) = tracked.entry(info.id()) {
                        let m = build_device_match(&info);
                        e.insert(m.clone());
                        on_added.call1(py, (m,))?;
                    }
                }
                Ok(nusb::hotplug::HotplugEvent::Disconnected(id)) => {
                    if let Some(m) = tracked.remove(&id) {
                        on_removed.call1(py, (m,))?;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        Ok(())
    }
}
