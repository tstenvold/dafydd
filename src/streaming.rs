//! Shared streaming discovery helper for transports that collect all results
//! before firing callbacks (currently USB).

use crate::types::DeviceMatch;
use pyo3::{prelude::*, Py, PyAny, Python};

/// Run a single-shot discovery, then call `callback` once per result.
///
/// Calls `discover(py)` to obtain the full match list, iterates it, and
/// invokes `callback.call1(py, (m,))` for each item before returning.
///
/// Use this only for transports whose results are already fully collected
/// before any callback fires. For genuine streaming (matches reported as
/// found), keep a `sync_channel` + background thread instead.
///
/// # Errors
///
/// Returns a [`pyo3::PyResult`] error if `discover` fails or the callback
/// raises an exception.
pub(crate) fn run_streaming<F>(
    py: Python<'_>,
    callback: &Py<PyAny>,
    discover: F,
) -> PyResult<Vec<DeviceMatch>>
where
    F: FnOnce(Python<'_>) -> PyResult<Vec<DeviceMatch>>,
{
    let matches = discover(py)?;
    for m in &matches {
        callback.call1(py, (m.clone(),))?;
    }
    Ok(matches)
}

// Note: run_streaming requires a live Python interpreter for callback invocation.
// Callback-count behaviour is covered by tests/test_boundary.py (Python-level).
// These Rust tests verify the discover closure contract and the inner iteration
// logic via a thin wrapper that bypasses the PyO3 callback machinery.
