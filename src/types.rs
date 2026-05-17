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
    fn host(&self) -> Option<String> {
        match self.transport {
            Transport::Tcp => self.address.rsplit_once(':').map(|(h, _)| h.to_string()),
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
            Transport::Usb => self
                .address
                .split_once(':')
                .and_then(|(_, p)| u16::from_str_radix(p.trim_start_matches("0x"), 16).ok()),
            _ => None,
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
}
