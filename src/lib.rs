//! Dafydd — fast device discovery over Serial, USB, and TCP/IP.
//!
//! Exposes [`serial::SerialDiscovery`], [`usb::UsbDiscovery`], and [`tcp::TcpDiscovery`] to
//! Python via [`pyo3`]. Call `.discover()` on any of them to receive a list
//! of [`types::DeviceMatch`] objects. Use `.discover_streaming(callback)` to
//! receive results as they are found. Use `.watch(on_added, on_removed)` for
//! continuous hotplug monitoring.

pub mod error;
pub mod net;
pub mod runtime;
pub mod types;

pub mod serial;
pub mod tcp;
pub mod usb;

use pyo3::prelude::*;

/// Python extension module entry point.
#[pymodule]
fn dafydd(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Eagerly spin up the Tokio runtime at import time so the first
    // `.discover()` call incurs no startup latency.
    let _ = runtime::runtime();

    m.add_class::<types::Transport>()?;
    m.add_class::<types::DeviceMatch>()?;
    m.add_class::<types::CancellationToken>()?;
    m.add_class::<serial::SerialDiscovery>()?;
    m.add_class::<usb::UsbDiscovery>()?;
    m.add_class::<tcp::TcpDiscovery>()?;
    m.add_function(wrap_pyfunction!(tcp::local_subnets, m)?)?;

    Ok(())
}
