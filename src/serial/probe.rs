//! Async serial-port probe and parallel sweep logic.
//!
//! Uses `tokio-serial` for non-blocking I/O: no `spawn_blocking` thread per
//! port, no 1 ms busy-wait sleep between reads. Each port probe is a native
//! Tokio task. Baud rates are tried sequentially on the same open port handle
//! via `set_baud_rate()` to avoid repeated open/close overhead.

use crate::{
    error::{DafyddError, Result},
    types::{CancellationToken, DeviceMatch, Transport},
};
use smallvec::SmallVec;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    task::JoinSet,
    time::timeout,
};
use tokio_serial::{SerialPort, SerialPortBuilderExt};

/// Response accumulation buffer: stays on the stack for responses ≤ 64 bytes,
/// which covers the majority of embedded device ID strings and status replies.
type ResponseBuf = SmallVec<[u8; 64]>;

/// Build a `serialport::SerialPortBuilder` with the common parameters.
fn build_builder(
    port: &str,
    baud: u32,
    data_bits: Option<u8>,
    parity: Option<&str>,
    stop_bits: Option<u8>,
    flow_control: Option<&str>,
) -> Result<serialport::SerialPortBuilder> {
    let db = match data_bits {
        Some(5) => serialport::DataBits::Five,
        Some(6) => serialport::DataBits::Six,
        Some(7) => serialport::DataBits::Seven,
        Some(8) | None => serialport::DataBits::Eight,
        _ => {
            return Err(DafyddError::Serial(serialport::Error::new(
                serialport::ErrorKind::InvalidInput,
                "invalid data bits: must be 5, 6, 7, or 8",
            )))
        }
    };

    let par = match parity.unwrap_or("none") {
        "even" => serialport::Parity::Even,
        "odd" => serialport::Parity::Odd,
        "none" => serialport::Parity::None,
        _ => {
            return Err(DafyddError::Serial(serialport::Error::new(
                serialport::ErrorKind::InvalidInput,
                "invalid parity: must be 'none', 'even', or 'odd'",
            )))
        }
    };

    let sb = match stop_bits {
        Some(1) | None => serialport::StopBits::One,
        Some(2) => serialport::StopBits::Two,
        _ => {
            return Err(DafyddError::Serial(serialport::Error::new(
                serialport::ErrorKind::InvalidInput,
                "invalid stop bits: must be 1 or 2",
            )))
        }
    };

    let fc = match flow_control.unwrap_or("none") {
        "hardware" => serialport::FlowControl::Hardware,
        "software" => serialport::FlowControl::Software,
        "none" => serialport::FlowControl::None,
        _ => {
            return Err(DafyddError::Serial(serialport::Error::new(
                serialport::ErrorKind::InvalidInput,
                "invalid flow control: must be 'none', 'hardware', or 'software'",
            )))
        }
    };

    Ok(serialport::new(port, baud)
        .data_bits(db)
        .parity(par)
        .stop_bits(sb)
        .flow_control(fc))
}

/// Read available bytes from `serial` until the connection closes, `read_timeout`
/// elapses, or `response_terminator` is seen at the end of the accumulated data.
async fn read_response(
    serial: &mut tokio_serial::SerialStream,
    read_timeout: Duration,
    response_terminator: Option<&[u8]>,
) -> ResponseBuf {
    let mut response = ResponseBuf::new();
    let mut buf = [0u8; 64];

    let _ = timeout(read_timeout, async {
        loop {
            match serial.read(&mut buf).await {
                Ok(0) | Err(_) => {
                    core::hint::cold_path();
                    break;
                }
                Ok(n) => {
                    response.extend_from_slice(&buf[..n]);
                    if let Some(term) = response_terminator {
                        if response.ends_with(term) {
                            break;
                        }
                    }
                }
            }
        }
    })
    .await;

    response
}

/// Probe `port` at `baud`, send `probe`, and collect the response.
///
/// Returns `Ok(Some(_))` when any bytes are received, `Ok(None)` when the
/// device does not respond within `timeout`. A `DafyddError::Serial` is
/// returned only when the port cannot be opened — simple I/O errors are
/// treated as no-response so every baud rate still gets a chance.
///
/// # Errors
///
/// Returns [`DafyddError::Serial`] if the port cannot be opened.
#[allow(clippy::too_many_arguments)]
pub async fn probe_port(
    port: &str,
    baud: u32,
    probe: &[u8],
    read_timeout: Duration,
    data_bits: Option<u8>,
    parity: Option<&str>,
    stop_bits: Option<u8>,
    flow_control: Option<&str>,
    response_terminator: Option<&[u8]>,
) -> Result<Option<DeviceMatch>> {
    let mut serial = build_builder(port, baud, data_bits, parity, stop_bits, flow_control)?
        .open_native_async()
        .map_err(DafyddError::Serial)?;

    let _ = serial.clear(serialport::ClearBuffer::Input);
    if serial.write_all(probe).await.is_err() {
        return Ok(None);
    }

    let response = read_response(&mut serial, read_timeout, response_terminator).await;
    if response.is_empty() {
        return Ok(None);
    }

    let mut info = HashMap::with_capacity(4);
    info.insert("baud_rate".to_owned(), baud.to_string());
    if let Some(db) = data_bits {
        if db != 8 {
            info.insert("data_bits".to_owned(), db.to_string());
        }
    }
    if let Some(p) = parity {
        if p != "none" {
            info.insert("parity".to_owned(), p.to_owned());
        }
    }
    if let Some(sb) = stop_bits {
        if sb != 1 {
            info.insert("stop_bits".to_owned(), sb.to_string());
        }
    }
    Ok(Some(DeviceMatch {
        transport: Transport::Serial,
        address: port.to_owned(),
        response: Some(response.into_vec()),
        info,
    }))
}

/// Probe `port` sequentially at each baud rate, reusing the open port handle.
///
/// Baud rates are tried one at a time because a serial port is an exclusive
/// resource. The port is opened once at the first baud rate; subsequent rates
/// are applied via `set_baud_rate()` to avoid the overhead of repeated
/// open/close pairs. The input buffer is flushed before each attempt.
///
/// Returns `Ok(None)` if no baud rate produces a response. Cancellation is
/// checked between baud rates.
///
/// # Errors
///
/// Returns [`DafyddError::Serial`] if the port cannot be opened at the first
/// baud rate.
#[allow(clippy::too_many_arguments)]
pub async fn probe_port_all_bauds(
    port: String,
    bauds: Arc<[u32]>,
    probe: Arc<[u8]>,
    read_timeout: Duration,
    data_bits: Option<u8>,
    parity: Option<String>,
    stop_bits: Option<u8>,
    flow_control: Option<String>,
    cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    response_terminator: Option<Arc<[u8]>>,
) -> Result<Option<DeviceMatch>> {
    if bauds.is_empty() {
        return Ok(None);
    }

    let first_baud = bauds[0];
    let mut serial = build_builder(
        &port,
        first_baud,
        data_bits,
        parity.as_deref(),
        stop_bits,
        flow_control.as_deref(),
    )?
    .open_native_async()
    .map_err(DafyddError::Serial)?;

    for &baud in bauds.iter() {
        if cancel
            .as_ref()
            .is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
        {
            return Ok(None);
        }

        // Change baud rate on the already-open port (free — no syscall overhead).
        if baud != first_baud && serial.set_baud_rate(baud).is_err() {
            continue;
        }

        let _ = serial.clear(serialport::ClearBuffer::Input);

        if serial.write_all(probe.as_ref()).await.is_err() {
            continue;
        }

        let response =
            read_response(&mut serial, read_timeout, response_terminator.as_deref()).await;
        if response.is_empty() {
            continue;
        }

        let mut info = HashMap::with_capacity(4);
        info.insert("baud_rate".to_owned(), baud.to_string());
        if let Some(db) = data_bits {
            if db != 8 {
                info.insert("data_bits".to_owned(), db.to_string());
            }
        }
        if let Some(ref p) = parity {
            if p != "none" {
                info.insert("parity".to_owned(), p.clone());
            }
        }
        if let Some(sb) = stop_bits {
            if sb != 1 {
                info.insert("stop_bits".to_owned(), sb.to_string());
            }
        }
        return Ok(Some(DeviceMatch {
            transport: Transport::Serial,
            address: port,
            response: Some(response.into_vec()),
            info,
        }));
    }

    Ok(None)
}

/// Probe every available serial port (at every baud rate) in parallel.
///
/// Different ports are swept concurrently via a `JoinSet`; baud rates within
/// each port are tried sequentially on the same open handle. The probe command
/// and baud rate list are shared across tasks as `Arc<[_]>` to avoid O(ports)
/// heap copies.
///
/// Cancellation is checked between spawning port tasks and between draining
/// results, allowing an in-progress sweep to terminate early.
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
#[allow(clippy::too_many_arguments)]
pub async fn sweep_all_ports(
    probe: &[u8],
    baud_rates: &[u32],
    read_timeout: Duration,
    include_bluetooth: bool,
    data_bits: Option<u8>,
    parity: Option<String>,
    stop_bits: Option<u8>,
    flow_control: Option<String>,
    cancel: Option<&CancellationToken>,
    tx: Option<&std::sync::mpsc::SyncSender<DeviceMatch>>,
    response_terminator: Option<Arc<[u8]>>,
) -> Result<Vec<DeviceMatch>> {
    let mut ports = serialport::available_ports().map_err(DafyddError::Serial)?;

    if !include_bluetooth {
        ports.retain(|p| !matches!(p.port_type, serialport::SerialPortType::BluetoothPort));
    }

    #[cfg(target_os = "macos")]
    {
        let cu_suffixes: std::collections::HashSet<String> = ports
            .iter()
            .filter_map(|p| p.port_name.strip_prefix("/dev/cu.").map(str::to_owned))
            .collect();
        ports.retain(|p| {
            p.port_name
                .strip_prefix("/dev/tty.")
                .is_none_or(|suffix| !cu_suffixes.contains(suffix))
        });
    }

    // Wrap in Arc so each task gets a pointer bump instead of a Vec clone.
    let probe_arc: Arc<[u8]> = Arc::from(probe);
    let bauds_arc: Arc<[u32]> = Arc::from(baud_rates);

    let mut set: JoinSet<Result<Option<DeviceMatch>>> = JoinSet::new();
    for port_info in ports {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            break;
        }

        let port = port_info.port_name.clone();
        let bauds = Arc::clone(&bauds_arc);
        let probe = Arc::clone(&probe_arc);
        let parity = parity.clone();
        let flow_control = flow_control.clone();
        let cancel_arc = cancel.map(|c| Arc::clone(&c.inner()));
        let terminator = response_terminator.clone();

        set.spawn(async move {
            probe_port_all_bauds(
                port,
                bauds,
                probe,
                read_timeout,
                data_bits,
                parity,
                stop_bits,
                flow_control,
                cancel_arc,
                terminator,
            )
            .await
        });
    }

    let mut matches = Vec::new();
    while let Some(result) = set.join_next().await {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            set.abort_all();
            break;
        }
        if let Ok(Ok(Some(m))) = result {
            if let Some(sender) = tx {
                let _ = sender.try_send(m.clone());
            }
            matches.push(m);
        }
    }
    Ok(matches)
}
