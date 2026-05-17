//! Cross-transport device correlation.
//!
//! USB-CDC devices show up in both [`crate::usb::UsbDiscovery`] and
//! [`crate::serial::SerialDiscovery`] results. This module provides utilities
//! to correlate matches across transports so callers can identify when a
//! Serial port and a USB device represent the same physical hardware.

use crate::types::{DeviceMatch, Transport};
use pyo3::prelude::*;
use std::collections::HashMap;

/// A pair of matches that likely represent the same physical device.
#[pyclass(get_all, from_py_object)]
#[derive(Debug, Clone)]
pub struct CorrelatedDevice {
    /// The USB enumeration result.
    pub usb: DeviceMatch,
    /// The Serial port result for the same physical device.
    pub serial: DeviceMatch,
}

#[pymethods]
impl CorrelatedDevice {
    fn __repr__(&self) -> String {
        format!(
            "CorrelatedDevice(usb=DeviceMatch(address={:?}), serial=DeviceMatch(address={:?}))",
            self.usb.address, self.serial.address
        )
    }
}

/// Correlate USB and Serial matches by USB serial number.
///
/// On platforms that expose USB serial numbers through the serial port
/// enumeration API, a USB device's `serial_number` in its USB match will
/// also appear in the serial port's `info["serial_number"]`. This function
/// finds those pairs and returns them.
///
/// Any USB matches without a serial number, or serial matches without
/// `info["serial_number"]`, are skipped — they cannot be correlated.
#[must_use]
pub fn correlate_usb_serial(
    usb_matches: &[DeviceMatch],
    serial_matches: &[DeviceMatch],
) -> Vec<CorrelatedDevice> {
    let serial_by_sn: HashMap<&str, &DeviceMatch> = serial_matches
        .iter()
        .filter(|m| m.transport == Transport::Serial)
        .filter_map(|m| m.info.get("serial_number").map(|sn| (sn.as_str(), m)))
        .collect();

    usb_matches
        .iter()
        .filter(|m| m.transport == Transport::Usb)
        .filter_map(|usb| {
            let sn = usb.info.get("serial_number")?;
            let serial = *serial_by_sn.get(sn.as_str())?;
            Some(CorrelatedDevice {
                usb: usb.clone(),
                serial: serial.clone(),
            })
        })
        .collect()
}

/// Partition a flat list of matches by transport type.
///
/// Returns `(serial_matches, usb_matches, tcp_matches)`.
#[must_use]
pub fn partition_by_transport(
    matches: &[DeviceMatch],
) -> (Vec<&DeviceMatch>, Vec<&DeviceMatch>, Vec<&DeviceMatch>) {
    let mut serial = Vec::new();
    let mut usb = Vec::new();
    let mut tcp = Vec::new();

    for m in matches {
        match m.transport {
            Transport::Serial => serial.push(m),
            Transport::Usb => usb.push(m),
            Transport::Tcp => tcp.push(m),
        }
    }

    (serial, usb, tcp)
}

/// Python wrapper for [`correlate_usb_serial`].
#[must_use]
#[pyfunction(name = "correlate_usb_serial")]
#[allow(clippy::needless_pass_by_value)]
pub fn correlate_usb_serial_py(
    usb_matches: Vec<DeviceMatch>,
    serial_matches: Vec<DeviceMatch>,
) -> Vec<CorrelatedDevice> {
    correlate_usb_serial(&usb_matches, &serial_matches)
}

/// Python wrapper for [`partition_by_transport`].
///
/// Returns `(serial_matches, usb_matches, tcp_matches)`.
#[must_use]
#[pyfunction(name = "partition_by_transport")]
#[allow(clippy::needless_pass_by_value)]
pub fn partition_by_transport_py(
    matches: Vec<DeviceMatch>,
) -> (Vec<DeviceMatch>, Vec<DeviceMatch>, Vec<DeviceMatch>) {
    let (s, u, t) = partition_by_transport(&matches);
    (
        s.into_iter().cloned().collect(),
        u.into_iter().cloned().collect(),
        t.into_iter().cloned().collect(),
    )
}
