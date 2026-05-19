//! Shared polling-based watch loop for Serial and TCP transports.

use crate::types::{CancellationToken, DeviceMatch};
use pyo3::prelude::*;
use std::{collections::HashSet, sync::atomic::Ordering, time::Duration};

/// Compute which devices were added or removed between two consecutive polls.
///
/// Returns `(added, removed)` where each element is a reference into the
/// corresponding slice. Comparison is by address only — response bytes can
/// vary across polls for devices that include dynamic data.
pub(crate) fn compute_diff<'a>(
    prev: &'a [DeviceMatch],
    current: &'a [DeviceMatch],
) -> (Vec<&'a DeviceMatch>, Vec<&'a DeviceMatch>) {
    let prev_addrs: HashSet<&str> = prev.iter().map(|m| m.address.as_str()).collect();
    let current_addrs: HashSet<&str> = current.iter().map(|m| m.address.as_str()).collect();
    let added = current
        .iter()
        .filter(|m| !prev_addrs.contains(m.address.as_str()))
        .collect();
    let removed = prev
        .iter()
        .filter(|m| !current_addrs.contains(m.address.as_str()))
        .collect();
    (added, removed)
}

/// Run a poll-based watch loop, calling `on_added`/`on_removed` as the device
/// set changes between successive discovery passes.
///
/// The loop runs until the cancellation token is signalled or a callback raises
/// an exception. Between polls the GIL is released so other Python threads can
/// run.
///
/// Args:
///   py: The GIL token.
///   cancel: Token used to stop the loop.
///   interval: Time to wait between discovery passes.
///   `on_added`: Called with each newly-found `DeviceMatch`.
///   `on_removed`: Called with each `DeviceMatch` that disappeared.
///   tick: Closure that runs one discovery pass and returns the current set.
///
/// # Errors
///
/// Returns a [`pyo3::PyResult`] error if a callback raises an exception.
pub(crate) fn poll_watch<F>(
    py: Python<'_>,
    cancel: &CancellationToken,
    interval: Duration,
    on_added: &Py<PyAny>,
    on_removed: &Py<PyAny>,
    mut tick: F,
) -> PyResult<()>
where
    F: FnMut(Python<'_>) -> PyResult<Vec<DeviceMatch>>,
{
    let mut prev: Vec<DeviceMatch> = Vec::new();

    loop {
        if cancel.is_cancelled() {
            break;
        }

        let current = tick(py)?;

        let (added, removed) = compute_diff(&prev, &current);
        for m in added {
            on_added.call1(py, (m.clone(),))?;
        }
        for m in removed {
            on_removed.call1(py, (m.clone(),))?;
        }

        prev = current;

        let cancelled = py.detach(|| {
            let flag = cancel.inner();
            let wake_at = std::time::Instant::now() + interval;
            while std::time::Instant::now() < wake_at {
                if flag.load(Ordering::Relaxed) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            false
        });
        if cancelled {
            return Ok(());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Transport;
    use std::collections::HashMap;

    fn make_match(address: &str) -> DeviceMatch {
        DeviceMatch {
            transport: Transport::Tcp,
            address: address.to_string(),
            response: None,
            info: HashMap::new(),
        }
    }

    #[test]
    fn test_compute_diff_detects_addition() {
        let prev = vec![];
        let current = vec![make_match("10.0.0.1:502")];
        let (added, removed) = compute_diff(&prev, &current);
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].address, "10.0.0.1:502");
        assert!(removed.is_empty());
    }

    #[test]
    fn test_compute_diff_detects_removal() {
        let prev = vec![make_match("10.0.0.1:502")];
        let current = vec![];
        let (added, removed) = compute_diff(&prev, &current);
        assert!(added.is_empty());
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].address, "10.0.0.1:502");
    }

    #[test]
    fn test_compute_diff_no_spurious_events_for_unchanged() {
        let device = make_match("10.0.0.1:502");
        let prev = vec![device.clone()];
        let current = vec![device];
        let (added, removed) = compute_diff(&prev, &current);
        assert!(added.is_empty());
        assert!(removed.is_empty());
    }

    #[test]
    fn test_compute_diff_simultaneous_add_and_remove() {
        let prev = vec![make_match("10.0.0.1:502")];
        let current = vec![make_match("10.0.0.2:502")];
        let (added, removed) = compute_diff(&prev, &current);
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].address, "10.0.0.2:502");
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].address, "10.0.0.1:502");
    }
}
