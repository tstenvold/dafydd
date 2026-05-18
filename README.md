# Dafydd

[![CI](https://img.shields.io/github/actions/workflow/status/tstenvold/dafydd/ci.yml?branch=main&label=CI)](https://github.com/tstenvold/dafydd/actions/workflows/ci.yml)
[![PyPI](https://img.shields.io/pypi/v/dafydd)](https://pypi.org/project/dafydd/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Python](https://img.shields.io/badge/python-3.11+-blue.svg)](https://www.python.org/downloads/)
[![Rust](https://img.shields.io/badge/rust-1.95%2B-orange.svg)](https://www.rust-lang.org/)

**Fast device discovery over Serial, USB, and TCP/IP.**

Dafydd is a Rust library with Python bindings (via PyO3) for probing and enumerating physical hardware. It sweeps serial ports, USB buses, and TCP subnets concurrently — returning structured results with transport-specific metadata.

## Features

- **Three transports, one API**: Serial, USB, and TCP discovery share the same `DeviceMatch` result type and discovery interface
- **Preferred-address fast path**: Try a known port/host first; fall back to a full sweep only if it fails
- **Streaming and watch modes**: Get results as they arrive via callbacks, or poll for hot-plug events
- **Probe & response matching**: Send custom bytes and filter devices by their response
- **ARP cache and mDNS acceleration**: On TCP, prioritise hosts already in the ARP table or announcing via mDNS
- **No extra system dependencies**: Pure Rust (no libpcap, no libusb), PyO3 for Python interop
- **Tokio-powered**: Shared async runtime — no per-call startup cost

## Installation

```bash
pip install dafydd
```

To build from source (requires Rust 1.95+):

```bash
uv sync
uv run maturin develop --release
```

## Quick Start

### TCP

```python
import dafydd

# Scan local subnets for hosts accepting connections on port 502 (Modbus)
devices = dafydd.TcpDiscovery(port=502).discover()

# With a probe command: only return hosts that respond
devices = dafydd.TcpDiscovery(
    port=8080,
    subnets=["192.168.1.0/24"],
    probe_command=b"PING",
    connect_timeout_ms=200,
    io_timeout_ms=500,
).discover()

for d in devices:
    print(f"{d.host}:{d.port}  {d.response}")
```

### Serial

```python
import dafydd

# Sweep all ports at common baud rates
devices = dafydd.SerialDiscovery(
    probe_command=b"*IDN?\r\n",
    baud_rates=[9600, 115200],
    timeout_ms=500,
    response_terminator=b"\r\n",   # stop reading early once terminator arrives
).discover()

for d in devices:
    print(f"{d.address} @ {d.baud_rate} baud: {d.response}")
```

### USB

```python
import dafydd

# Filter by vendor ID, product ID, or device class
devices = dafydd.UsbDiscovery(vid=0x04D8).discover()

for d in devices:
    print(f"{d.address}  {d.info.get('product', '')}")
```

## Streaming and Watch

All three discovery classes expose `discover_streaming` (callback per result) and `watch` (hot-plug polling):

```python
import dafydd

token = dafydd.CancellationToken()

# Streaming — callback fires as each device is found
dafydd.SerialDiscovery(
    probe_command=b"*IDN?\r\n",
    baud_rates=[9600],
    timeout_ms=500,
    cancellation_token=token,
).discover_streaming(lambda d: print("found", d.address))

# Watch — polls for plug/unplug events until cancelled
import threading
threading.Thread(
    target=dafydd.UsbDiscovery(vid=0x04D8).watch,
    kwargs={
        "on_added":   lambda d: print("connected",    d.address),
        "on_removed": lambda d: print("disconnected", d.address),
        "interval_ms": 1000,
    },
    daemon=True,
).start()

# Stop from any thread
token.cancel()
```

## Correlating USB and Serial

When the same physical device appears on both a USB bus and a serial port, `correlate_usb_serial` pairs them by USB serial number:

```python
import dafydd

usb     = dafydd.UsbDiscovery().discover()
serial  = dafydd.SerialDiscovery(probe_command=b"", baud_rates=[9600], timeout_ms=100).discover()

for pair in dafydd.correlate_usb_serial(usb, serial):
    print(f"USB {pair.usb.address}  ↔  Serial {pair.serial.address}")
```

## Utilities

```python
# Auto-detect local subnets (what TcpDiscovery uses when subnets is not set)
print(dafydd.local_subnets())   # e.g. ['192.168.1.0/24', '10.0.0.0/24']

# Split a mixed list by transport
serial_ms, usb_ms, tcp_ms = dafydd.partition_by_transport(all_matches)
```

## How It Works

1. **Preferred-address path**: If `preferred_host` / `preferred_port` is set, try it first with a short timeout. On success, return immediately without sweeping.
2. **Full sweep**: Enumerate available endpoints (ports, USB devices, or subnet hosts) and probe them concurrently via a Tokio `JoinSet`.
3. **ARP / mDNS acceleration** (TCP): Hosts already in the system ARP cache or announcing via mDNS are probed before the raw subnet sweep, reducing latency for recently-seen devices.
4. **macOS tty dedup** (Serial): Each physical port appears as both `/dev/tty.*` and `/dev/cu.*`; the blocking `tty.*` variant is filtered out automatically.

## Building from Source

```bash
# Development build (editable install)
uv sync
uv run maturin develop

# Release build
uv run maturin develop --release

# Rust-only tests
cargo test
cargo bench
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for build instructions, test workflows, and PR guidelines.

## License

Licensed under MIT. See [LICENSE](LICENSE) for details.
