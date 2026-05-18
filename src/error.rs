//! Unified error type for all Dafydd discovery operations.

use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    PyErr,
};
use thiserror::Error;

/// All errors that can occur during device discovery.
#[derive(Debug, Error)]
pub enum DafyddError {
    /// Serial port enumeration or I/O failure.
    #[error("serial: {0}")]
    Serial(#[from] serialport::Error),

    /// USB enumeration failure.
    #[error("usb: {0}")]
    Usb(String),

    /// Generic I/O error (TCP connect, read/write).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// Unparseable subnet/CIDR string supplied by the caller.
    #[error("invalid subnet: {0}")]
    InvalidSubnet(String),
}

impl From<DafyddError> for PyErr {
    fn from(e: DafyddError) -> Self {
        match e {
            // Bad user input → ValueError so callers can write `except ValueError`.
            DafyddError::InvalidSubnet(_) => PyValueError::new_err(e.to_string()),
            _ => PyRuntimeError::new_err(e.to_string()),
        }
    }
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, DafyddError>;
