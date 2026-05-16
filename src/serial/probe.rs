//! Blocking serial-port probe and parallel sweep logic.

use crate::{
    error::{DafyddError, Result},
    types::{DeviceMatch, Transport},
};
use std::{
    collections::HashMap,
    io::{ErrorKind, Read, Write},
    time::{Duration, Instant},
};
use tokio::task::JoinSet;

/// Attempt to open `port` at `baud`, send `probe`, and collect the response.
///
/// Reads in a loop until the port times out, the connection closes, or
/// `timeout` elapses — whichever comes first. A 4 KiB internal buffer prevents
/// silent truncation of long device responses. The full accumulated response is
/// stored verbatim in the returned [`DeviceMatch`]'s `info["response"]` field.
///
/// Returns `Ok(Some(_))` when any bytes are received, `Ok(None)` when the
/// device does not respond within `timeout`.
///
/// # Errors
///
/// Returns [`DafyddError::Serial`] if the port cannot be opened.
/// Returns [`DafyddError::Io`] if the write fails.
pub fn probe_port(
    port: &str,
    baud: u32,
    probe: &[u8],
    timeout: Duration,
) -> Result<Option<DeviceMatch>> {
    let mut serial = serialport::new(port, baud)
        .timeout(timeout)
        .open()
        .map_err(DafyddError::Serial)?;

    // Discard stale bytes buffered from a previous session before sending the
    // probe, so the response we read is always fresh.
    let _ = serial.clear(serialport::ClearBuffer::Input);
    serial.write_all(probe).map_err(DafyddError::Io)?;

    let deadline = Instant::now() + timeout;
    let mut response: Vec<u8> = Vec::with_capacity(4096);
    let mut buf = [0u8; 4096];

    loop {
        match serial.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break,
        }
        if Instant::now() >= deadline {
            break;
        }
    }

    if response.is_empty() {
        return Ok(None);
    }

    let mut info = HashMap::new();
    info.insert("baud_rate".to_owned(), baud.to_string());
    Ok(Some(DeviceMatch {
        transport: Transport::Serial,
        address: port.to_owned(),
        response: Some(response),
        info,
    }))
}

/// Probe `port` at each baud rate in `bauds` **sequentially** and return the
/// first match.
///
/// Baud rates are tried one at a time because serial ports are exclusive
/// resources — concurrent open attempts on the same port would fail with
/// "Access Denied" / "Device or resource busy" on every platform.
///
/// Returns `Ok(None)` if no baud rate produces a response.
///
/// # Errors
///
/// Returns [`DafyddError::Serial`] for unexpected port enumeration failures.
/// Simple open errors and I/O errors are silently skipped so every baud rate
/// gets a chance.
pub async fn probe_port_all_bauds(
    port: String,
    bauds: Vec<u32>,
    probe: Vec<u8>,
    timeout: Duration,
) -> Result<Option<DeviceMatch>> {
    // A single spawn_blocking call owns the port handle for the duration of
    // the sequential baud scan, avoiding repeated open/close overhead.
    let result = tokio::task::spawn_blocking(move || {
        for baud in bauds {
            if let Ok(Some(m)) = probe_port(&port, baud, &probe, timeout) {
                return Ok(Some(m));
            }
        }
        Ok(None)
    })
    .await;

    match result {
        Ok(inner) => inner,
        Err(join_err) => {
            // Task panicked - log and return None instead of silently swallowing
            tracing::warn!("serial probe task panicked: {}", join_err);
            Ok(None)
        }
    }
}

/// Probe every available serial port (at every baud rate) in parallel.
///
/// Different ports are swept concurrently; baud rates within each port are
/// tried sequentially (serial ports cannot be opened more than once at a time).
///
/// Returns all ports that respond with any payload. Silent-skips ports that
/// cannot be opened (busy, absent, permission-denied).
///
/// # Platform notes
///
/// **Windows**: Bluetooth SPP virtual COM ports (e.g. paired-but-off devices)
/// can stall for several seconds per port. They are excluded by default unless
/// `include_bluetooth` is `true`.
///
/// **macOS**: Each physical port appears as both `/dev/tty.XXX` (blocks until
/// DCD is asserted — hangs on most embedded devices) and `/dev/cu.XXX`
/// (non-blocking call-out port). The `tty.*` variant is automatically filtered
/// out when a matching `cu.*` entry exists.
///
/// # Errors
///
/// Returns [`DafyddError::Serial`] if the system port list cannot be read.
pub async fn sweep_all_ports(
    probe: &[u8],
    baud_rates: &[u32],
    timeout: Duration,
    include_bluetooth: bool,
) -> Result<Vec<DeviceMatch>> {
    let mut ports = serialport::available_ports().map_err(DafyddError::Serial)?;

    // Skip Bluetooth SPP virtual COM ports by default — they stall for several
    // seconds per open attempt when the paired device is off or out of range,
    // which can add minutes to a serial sweep on a machine with many paired
    // devices.
    if !include_bluetooth {
        ports.retain(|p| !matches!(p.port_type, serialport::SerialPortType::BluetoothPort));
    }

    // On macOS every physical port is exposed twice: as /dev/tty.XXX (blocks
    // until DCD is asserted — hangs on most embedded devices that never assert
    // DCD) and as /dev/cu.XXX (non-blocking call-out port — correct for device
    // discovery). Drop the tty.* entry whenever a cu.* counterpart exists.
    #[cfg(target_os = "macos")]
    {
        let cu_suffixes: std::collections::HashSet<String> = ports
            .iter()
            .filter_map(|p| p.port_name.strip_prefix("/dev/cu.").map(str::to_owned))
            .collect();
        ports.retain(|p| {
            p.port_name
                .strip_prefix("/dev/tty.")
                .map_or(true, |suffix| !cu_suffixes.contains(suffix))
        });
    }

    let mut set: JoinSet<Result<Option<DeviceMatch>>> = JoinSet::new();
    for port_info in ports {
        let port = port_info.port_name.clone();
        let bauds = baud_rates.to_vec();
        let probe = probe.to_vec();
        set.spawn(async move { probe_port_all_bauds(port, bauds, probe, timeout).await });
    }

    let mut matches = Vec::new();
    while let Some(result) = set.join_next().await {
        if let Ok(Ok(Some(m))) = result {
            matches.push(m);
        }
    }
    Ok(matches)
}
