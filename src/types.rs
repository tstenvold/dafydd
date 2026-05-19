//! Shared Python-visible types returned by all discovery transports.

use pyo3::{prelude::*, types::PyDict};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::atomic::{AtomicBool, Ordering},
    sync::Arc,
};

/// Token that can be used to cancel ongoing discovery operations.
#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct CancellationToken {
    inner: Arc<AtomicBool>,
}

impl CancellationToken {
    /// Create a new cancellation token.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Check if cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.inner.load(Ordering::Relaxed)
    }

    /// Get a reference to the inner atomic bool.
    #[must_use]
    pub fn inner(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.inner)
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

#[pymethods]
impl CancellationToken {
    /// Create a new cancellation token.
    #[new]
    fn py_new() -> Self {
        Self::new()
    }

    /// Cancel the operation.
    fn cancel(&self) {
        self.inner.store(true, Ordering::Relaxed);
    }

    /// Check if cancellation has been requested.
    #[pyo3(name = "is_cancelled")]
    fn py_is_cancelled(&self) -> bool {
        self.is_cancelled()
    }

    /// Reset the token for reuse.
    fn reset(&self) {
        self.inner.store(false, Ordering::Relaxed);
    }
}

/// Which physical transport was used to discover the device.
#[pyclass(eq, eq_int, from_py_object)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport {
    /// RS-232 / RS-485 serial port.
    Serial,
    /// Universal Serial Bus.
    Usb,
    /// TCP socket over Ethernet or Wi-Fi.
    Tcp,
}

/// A device found during discovery.
#[pyclass(get_all, from_py_object)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceMatch {
    /// Transport layer that found this device.
    pub transport: Transport,
    /// Human-readable address: port path, `VID:PID`, or `IP:port`.
    pub address: String,
    /// Raw bytes returned by the device.
    pub response: Option<Vec<u8>>,
    /// Transport-specific metadata (baud rate, firmware response, hostname, …).
    pub info: HashMap<String, String>,
}

// HashMap doesn't implement Hash, so we implement it manually — mirroring __hash__
// by hashing transport + address + response and skipping info.
impl Hash for DeviceMatch {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(&self.transport).hash(state);
        self.address.hash(state);
        self.response.hash(state);
    }
}

#[pymethods]
impl DeviceMatch {
    /// Create a new [`DeviceMatch`].
    ///
    /// Args:
    ///   `transport`: Transport layer that found this device.
    ///   `address`: Human-readable address.
    ///   `response`: Raw bytes returned by the device.
    ///   `info`: Transport-specific metadata.
    #[must_use]
    #[new]
    #[pyo3(signature = (transport, address, response = None, info = None))]
    pub fn new(
        transport: Transport,
        address: String,
        response: Option<Vec<u8>>,
        info: Option<HashMap<String, String>>,
    ) -> Self {
        Self {
            transport,
            address,
            response,
            info: info.unwrap_or_default(),
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "DeviceMatch(transport={:?}, address={:?}, response={:?}, info={:?})",
            self.transport, self.address, self.response, self.info
        )
    }

    fn __eq__(&self, other: &Self) -> bool {
        // Include all fields in equality check to be consistent with hash
        self.transport == other.transport
            && self.address == other.address
            && self.response == other.response
            && self.info == other.info
    }

    #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
    fn __hash__(&self) -> isize {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::mem::discriminant(&self.transport).hash(&mut h);
        self.address.hash(&mut h);
        self.response.hash(&mut h);
        // Exclude `info` from hashing because `std::collections::HashMap` does not implement `Hash`.
        // This is perfectly safe: the Hash contract only requires `a == b -> hash(a) == hash(b)`.
        h.finish() as isize
    }

    fn __lt__(&self, other: &Self) -> bool {
        self.address < other.address
    }

    /// For Serial transport: baud rate confirmed during discovery.
    #[getter]
    fn baud_rate(&self) -> Option<u32> {
        self.info.get("baud_rate").and_then(|s| s.parse().ok())
    }

    /// For TCP transport: hostname or IP address part of the connection address.
    #[getter]
    fn host(&self) -> Option<&str> {
        match self.transport {
            Transport::Tcp => self.address.rsplit_once(':').map(|(h, _)| h),
            _ => None,
        }
    }

    /// For TCP transport: port number part of the connection address.
    #[getter]
    fn port(&self) -> Option<u16> {
        match self.transport {
            Transport::Tcp => self
                .address
                .rsplit_once(':')
                .and_then(|(_, p)| p.parse().ok()),
            _ => None,
        }
    }

    /// For USB transport: vendor ID as an integer.
    #[getter]
    fn vendor_id(&self) -> Option<u16> {
        match self.transport {
            Transport::Usb => self
                .address
                .split_once(':')
                .and_then(|(v, _)| u16::from_str_radix(v.trim_start_matches("0x"), 16).ok()),
            _ => None,
        }
    }

    /// For USB transport: product ID as an integer.
    #[getter]
    fn product_id(&self) -> Option<u16> {
        match self.transport {
            Transport::Usb => {
                // Address format is "VID:PID" or "VID:PID:serial_number".
                let mut parts = self.address.splitn(3, ':');
                parts.next(); // skip vendor
                let pid_str = parts.next()?;
                u16::from_str_radix(pid_str.trim_start_matches("0x"), 16).ok()
            }
            _ => None,
        }
    }

    /// For USB transport: True if the device reports HID class (0x03) at the device level.
    ///
    /// Note: devices that report HID only at the interface level will return False.
    #[getter]
    fn is_hid(&self) -> bool {
        match self.transport {
            Transport::Usb => self
                .info
                .get("device_class")
                .and_then(|s| s.parse::<u8>().ok())
                .is_some_and(|c| c == 3),
            _ => false,
        }
    }

    /// Return keyword arguments for the matching python-bus factory function.
    fn bus_params<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let d = PyDict::new(py);
        match self.transport {
            Transport::Serial => {
                d.set_item("port", &self.address)?;
                if let Some(br) = self.baud_rate() {
                    d.set_item("baudrate", br)?;
                }
                if let Some(db) = self
                    .info
                    .get("data_bits")
                    .and_then(|s| s.parse::<u8>().ok())
                {
                    d.set_item("bytesize", db)?;
                }
                if let Some(p) = self.info.get("parity") {
                    let parity_char = match p.as_str() {
                        "even" => "E",
                        "odd" => "O",
                        _ => "N",
                    };
                    d.set_item("parity", parity_char)?;
                }
                if let Some(sb) = self
                    .info
                    .get("stop_bits")
                    .and_then(|s| s.parse::<u8>().ok())
                {
                    d.set_item("stopbits", sb)?;
                }
            }
            Transport::Tcp => {
                if let Some(h) = self.host() {
                    d.set_item("host", h)?;
                }
                if let Some(p) = self.port() {
                    d.set_item("port", p)?;
                }
            }
            Transport::Usb => {
                if let Some(v) = self.vendor_id() {
                    d.set_item("vendor", v)?;
                }
                if let Some(p) = self.product_id() {
                    d.set_item("product", p)?;
                }
                if let Some(sn) = self.info.get("serial_number") {
                    d.set_item("serial_number", sn)?;
                }
            }
        }
        Ok(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_device_match_eq() {
        let dm1 = DeviceMatch {
            transport: Transport::Tcp,
            address: "192.168.1.1:80".to_string(),
            response: Some(vec![1, 2, 3]),
            info: HashMap::new(),
        };
        let dm2 = DeviceMatch {
            transport: Transport::Tcp,
            address: "192.168.1.1:80".to_string(),
            response: Some(vec![1, 2, 3]),
            info: HashMap::new(),
        };
        assert!(dm1.__eq__(&dm2));
    }

    #[test]
    fn test_device_match_neq_response() {
        let dm1 = DeviceMatch {
            transport: Transport::Tcp,
            address: "192.168.1.1:80".to_string(),
            response: Some(vec![1, 2, 3]),
            info: HashMap::new(),
        };
        let dm2 = DeviceMatch {
            transport: Transport::Tcp,
            address: "192.168.1.1:80".to_string(),
            response: None,
            info: HashMap::new(),
        };
        assert!(!dm1.__eq__(&dm2));
    }

    #[test]
    fn test_device_match_neq() {
        let dm1 = DeviceMatch {
            transport: Transport::Tcp,
            address: "192.168.1.1:80".to_string(),
            response: None,
            info: HashMap::new(),
        };
        let dm2 = DeviceMatch {
            transport: Transport::Tcp,
            address: "192.168.1.2:80".to_string(),
            response: None,
            info: HashMap::new(),
        };
        let dm3 = DeviceMatch {
            transport: Transport::Serial,
            address: "192.168.1.1:80".to_string(),
            response: None,
            info: HashMap::new(),
        };
        assert!(!dm1.__eq__(&dm2));
        assert!(!dm1.__eq__(&dm3));
    }

    #[test]
    fn test_device_match_hash() {
        let dm1 = DeviceMatch {
            transport: Transport::Usb,
            address: "1234:5678".to_string(),
            response: Some(vec![1, 2, 3]),
            info: HashMap::new(),
        };
        let dm2 = DeviceMatch {
            transport: Transport::Usb,
            address: "1234:5678".to_string(),
            response: Some(vec![1, 2, 3]),
            info: HashMap::new(),
        };
        assert_eq!(dm1.__hash__(), dm2.__hash__());
    }

    #[test]
    fn test_device_match_hash_different_responses() {
        let dm1 = DeviceMatch {
            transport: Transport::Usb,
            address: "1234:5678".to_string(),
            response: None,
            info: HashMap::new(),
        };
        let dm2 = DeviceMatch {
            transport: Transport::Usb,
            address: "1234:5678".to_string(),
            response: Some(vec![0xFF]),
            info: HashMap::new(),
        };
        // Different responses should produce different hashes
        assert_ne!(dm1.__hash__(), dm2.__hash__());
    }

    #[test]
    fn test_hash_eq_invariant() {
        // Equal objects must have identical hashes.
        let mut info = HashMap::new();
        info.insert("key".to_string(), "value".to_string());
        let a = DeviceMatch {
            transport: Transport::Tcp,
            address: "10.0.0.1:502".to_string(),
            response: Some(vec![1, 2, 3]),
            info: info.clone(),
        };
        let b = DeviceMatch {
            transport: Transport::Tcp,
            address: "10.0.0.1:502".to_string(),
            response: Some(vec![1, 2, 3]),
            info: info.clone(),
        };
        assert!(a.__eq__(&b), "identical DeviceMatch must be equal");
        assert_eq!(
            a.__hash__(),
            b.__hash__(),
            "equal DeviceMatch must have equal hashes"
        );

        // Same transport/address/response but different info → NOT equal.
        let mut other_info = HashMap::new();
        other_info.insert("key".to_string(), "different".to_string());
        let c = DeviceMatch {
            transport: Transport::Tcp,
            address: "10.0.0.1:502".to_string(),
            response: Some(vec![1, 2, 3]),
            info: other_info,
        };
        assert!(
            !a.__eq__(&c),
            "DeviceMatch with different info must not be equal"
        );

        // Hash intentionally skips `info`, so objects with different info still share a hash.
        assert_eq!(
            a.__hash__(),
            c.__hash__(),
            "DeviceMatch with different info must still have equal hashes (info excluded from hash)"
        );
    }

    #[test]
    fn test_device_match_sort() {
        let dm1 = DeviceMatch {
            transport: Transport::Serial,
            address: "COM1".to_string(),
            response: None,
            info: HashMap::new(),
        };
        let dm2 = DeviceMatch {
            transport: Transport::Serial,
            address: "COM2".to_string(),
            response: None,
            info: HashMap::new(),
        };
        assert!(dm1.__lt__(&dm2));
        assert!(!dm2.__lt__(&dm1));
    }

    #[test]
    fn test_tcp_host_and_port_getters() {
        let tcp = DeviceMatch {
            transport: Transport::Tcp,
            address: "10.0.0.1:502".to_string(),
            response: None,
            info: HashMap::new(),
        };
        assert_eq!(tcp.host(), Some("10.0.0.1"));
        assert_eq!(tcp.port(), Some(502));

        let serial = DeviceMatch {
            transport: Transport::Serial,
            address: "/dev/ttyUSB0".to_string(),
            response: None,
            info: HashMap::new(),
        };
        assert_eq!(serial.host(), None);
        assert_eq!(serial.port(), None);

        let usb = DeviceMatch {
            transport: Transport::Usb,
            address: "0x04d8:0x00dd".to_string(),
            response: None,
            info: HashMap::new(),
        };
        assert_eq!(usb.host(), None);
        assert_eq!(usb.port(), None);
    }

    #[test]
    fn test_usb_vendor_product_getters() {
        let usb = DeviceMatch {
            transport: Transport::Usb,
            address: "0x04d8:0x00dd".to_string(),
            response: None,
            info: HashMap::new(),
        };
        assert_eq!(usb.vendor_id(), Some(0x04d8));
        assert_eq!(usb.product_id(), Some(0x00dd));

        let tcp = DeviceMatch {
            transport: Transport::Tcp,
            address: "10.0.0.1:502".to_string(),
            response: None,
            info: HashMap::new(),
        };
        assert_eq!(tcp.vendor_id(), None);
        assert_eq!(tcp.product_id(), None);
    }

    #[test]
    fn test_usb_is_hid() {
        let mut info_hid = HashMap::new();
        info_hid.insert("device_class".to_string(), "3".to_string());
        let hid = DeviceMatch {
            transport: Transport::Usb,
            address: "0x04d8:0x00dd".to_string(),
            response: None,
            info: info_hid,
        };
        assert!(hid.is_hid());

        let mut info_other = HashMap::new();
        info_other.insert("device_class".to_string(), "0".to_string());
        let not_hid = DeviceMatch {
            transport: Transport::Usb,
            address: "0x04d8:0x00dd".to_string(),
            response: None,
            info: info_other,
        };
        assert!(!not_hid.is_hid());

        let tcp = DeviceMatch {
            transport: Transport::Tcp,
            address: "10.0.0.1:502".to_string(),
            response: None,
            info: HashMap::new(),
        };
        assert!(!tcp.is_hid());
    }

    #[test]
    fn test_serial_baud_rate_getter() {
        let mut info = HashMap::new();
        info.insert("baud_rate".to_string(), "115200".to_string());
        let with_baud = DeviceMatch {
            transport: Transport::Serial,
            address: "/dev/ttyUSB0".to_string(),
            response: None,
            info,
        };
        assert_eq!(with_baud.baud_rate(), Some(115_200));

        let without_baud = DeviceMatch {
            transport: Transport::Serial,
            address: "/dev/ttyUSB0".to_string(),
            response: None,
            info: HashMap::new(),
        };
        assert_eq!(without_baud.baud_rate(), None);
    }
}
