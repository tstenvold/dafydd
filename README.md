# Dafydd

[![CI](https://img.shields.io/github/actions/workflow/status/tstenvold/dafydd/ci.yml?branch=main&label=CI)](https://github.com/tstenvold/dafydd/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Python](https://img.shields.io/badge/python-3.11+-blue.svg)](https://www.python.org/downloads/)
[![Rust](https://img.shields.io/badge/rust-1.95%2B-orange.svg)](https://www.rust-lang.org/)

**Fast Device Discovery over Serial, USB, and TCP/IP.**

Dafydd is a native Rust library offering highly concurrent and lightning-fast physical device probing with convenient Python bindings via PyO3. Perfect for hardware automation, device enumeration, and network scanning.

## Features

- **Parallel Transport Scanning**: Sweeps Serial, USB, and TCP/IP endpoints concurrently with configurable rate limits
- **Probe & Response Matching**: Send custom command bytes and match device responses to identify hardware
- **Preferred Address Fast Path**: Try a known device address first, sweep as fallback
- **No System Dependencies**: Pure Rust (no libusb, no extra drivers), PyO3 for Python interop
- **Tokio-powered**: Efficient async I/O with a global shared runtime — no per-call startup cost

## Installation

```bash
# From source (requires Rust 1.95+, Python 3.11+)
pip install -e .
uv run maturin develop --release

# From PyPI (once published)
pip install dafydd
```

## Quick Start

### TCP Discovery

```python
from dafydd import TcpDiscovery

discoverer = TcpDiscovery(
    port=8080,
    subnets=["192.168.1.0/24"],
    probe_command=b"PING",
    timeout_ms=200,
)
devices = discoverer.discover()

for device in devices:
    print(f"{device.address}: {device.response}")
```

### Serial Discovery

```python
from dafydd import SerialDiscovery

discoverer = SerialDiscovery(
    probe_command=b"*IDN?\r\n",
    baud_rates=[9600, 115200],
    timeout_ms=500,
)
devices = discoverer.discover()

for device in devices:
    print(f"{device.address} @ {device.info.get('baud_rate')} baud")
```

### USB Discovery

```python
from dafydd import UsbDiscovery

# Find all USB devices with VID 0x1234
discoverer = UsbDiscovery(vid=0x1234)
devices = discoverer.discover()

for device in devices:
    print(f"{device.address}: {device.info}")
```

## How It Works

Dafydd sweeps physical transports in parallel:

1. **Preferred Address Path**: If you know a device's likely address (port, host, or device path), try it first with a short timeout
2. **Fallback Sweep**: If the preferred address fails, enumerate all available endpoints and probe them concurrently
3. **Async I/O**: Uses Tokio with semaphore-bounded concurrency to prevent resource exhaustion on large scans
4. **Response Matching**: Filters results by probe command and expected response bytes

## Performance

Run benchmarks locally:

```bash
cargo bench
```

See `benches/` for benchmark configurations (TCP subnet scans, serial baud sweeps, USB enumeration).

## Building

```bash
# Development build
uv sync
uv run maturin develop

# Release build
uv run maturin develop --release

# Rust-only
cargo build --release
```

## Testing

```bash
# Python tests (platform-aware, with fixtures for serial/TCP/USB simulation)
uv run pytest tests/ -v

# Rust tests and benchmarks
cargo test
cargo bench --bench tcp_scan -- --test  # Smoke test
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for build instructions, test workflows, and PR guidelines.

## License

Licensed under MIT. See [LICENSE](LICENSE) for details.
