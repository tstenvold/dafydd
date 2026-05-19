# Dafydd

[![CI](https://img.shields.io/github/actions/workflow/status/tstenvold/dafydd/ci.yml?branch=main&label=CI)](https://github.com/tstenvold/dafydd/actions/workflows/ci.yml)
[![PyPI](https://img.shields.io/pypi/v/dafydd)](https://pypi.org/project/dafydd/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Python](https://img.shields.io/badge/python-3.11+-blue.svg)](https://www.python.org/downloads/)
[![Rust](https://img.shields.io/badge/rust-1.95%2B-orange.svg)](https://www.rust-lang.org/)

**Find your device, even when it isn't where it's supposed to be.**

Dafydd is a Rust library with Python bindings (via PyO3) for two questions
every embedded / instrument-control codebase asks:

1. **"I have a device of type X with serial number Y — where is it?"**
   Sweep the relevant bus (USB / Serial / TCP), probe each endpoint, and
   return the one whose response matches your identifier.

2. **"My device was supposed to be at `COM3` / VID:PID `1234:5678` /
   `192.168.1.50:5025`. It's not. Find it anywhere else on the same bus."**
   Try the expected location first with a short timeout. On failure, fall
   back to a probe-and-identify sweep — and return the *actual* location.

Same `DeviceMatch` shape comes back regardless of transport, so that it stays uniform across USB, Serial, and TCP devices.

## Installation

```bash
pip install dafydd
```

Build from source (requires Rust 1.95+):

```bash
uv sync
uv run maturin develop --release
```

## Use case 1 — "find this device, I don't know where it is"

You know the type (USB VID/PID, or how it responds to `*IDN?`). dafydd sweeps
and returns the match.

### Serial

```python
import dafydd

# Find a device that responds to *IDN? with "MyDevice" anywhere on the bus.
matches = dafydd.SerialDiscovery(
    probe_command=b"*IDN?\r\n",
    baud_rates=[9600, 115200],
    timeout_ms=500,
    response_filter=b"MyDevice",
    response_terminator=b"\r\n",
).discover()

for m in matches:
    print(f"{m.address} @ {m.baud_rate} baud: {m.response}")
```

### USB

```python
# Filter by vendor/product ID — descriptor enumeration only, no probing.
matches = dafydd.UsbDiscovery(vid=0x04D8, pid=0x000A).discover()
for m in matches:
    print(f"{m.address}  {m.info.get('product', '')}")
```

### TCP

```python
# Sweep the LAN for a device that answers to your probe with the right ID.
matches = dafydd.TcpDiscovery(
    port=5025,
    probe_command=b"*IDN?\n",
    response_filter=b"SN:ES123DFD3",
).discover()

for m in matches:
    print(m.address, m.info.get("mac"))   # info["mac"] populated when from ARP
```

## Use case 2 — "it should be at X but it isn't; find it"

You have an *expected* location. Try it first; on failure, fall back to a
full bus sweep filtered by probe response.

### Serial — "should be `COM3`, but `COM3` is empty"

```python
m = dafydd.SerialDiscovery(
    probe_command=b"C0AMSF\r\n",
    response_filter=b"sn:ES123DFD3",
    preferred_port="COM3",          # try this first with a fast timeout
    preferred_retry=2,              # retry a couple of times
    baud_rates=[9600],
    timeout_ms=200,
).discover()
# m[0].address now points to wherever the device actually is — COM4, /dev/ttyUSB1, …
```

### TCP — "should be 192.168.1.50:5025, but it's not"

```python
m = dafydd.TcpDiscovery(
    port=5025,
    preferred_host="192.168.1.50",  # try first; if no response, sweep
    probe_command=b"*IDN?\n",
    response_filter=b"SN:ES123DFD3",
    subnet_prefix=24,               # how broad the fallback sweep goes
).discover()
```

### Passing the result to your transport library

Every `DeviceMatch` produces kwargs ready for `python-bus` (or any
serial/socket library) via `.bus_params()`:

```python
import bus  # python-bus

match = matches[0]
device = bus.Device(**match.bus_params())
device.write(b"some-command")
```

## Streaming and Watch

```python
import dafydd

token = dafydd.CancellationToken()

# Streaming — callback fires as each device is found.
dafydd.SerialDiscovery(
    probe_command=b"*IDN?\r\n",
    baud_rates=[9600],
    timeout_ms=500,
    cancellation_token=token,
).discover_streaming(lambda d: print("found", d.address))

# Watch — fires on plug/unplug. USB uses real OS hotplug; Serial and TCP poll.
import threading
threading.Thread(
    target=dafydd.UsbDiscovery(vid=0x04D8).watch,
    kwargs={
        "on_added":   lambda d: print("connected",    d.address),
        "on_removed": lambda d: print("disconnected", d.address),
    },
    daemon=True,
).start()

# Stop from any thread.
token.cancel()
```

## Configuration knobs

### `TcpDiscovery`

| Knob | Default | Purpose |
|---|---|---|
| `subnet_prefix` | `24` | Broadest auto-detected subnet to sweep. Must be in `[16, 32]`. `16` allows /16 sweeps (65 k hosts, slow); `24` is the polite default. |
| `tcp_linger_seconds` | `None` | TCP `SO_LINGER`. `None` = OS default (graceful FIN close). `0` = RST close, no TIME_WAIT — fast but antisocial; use only on networks you own. `n>0` = block for `n` seconds. |
| `use_arp_cache` | `True` | Probe IPs from the kernel ARP cache before the linear sweep. Pure-Rust on all platforms (no `arp` subprocess). When a match comes from ARP, its `info["mac"]` is populated. |
| `use_mdns` / `use_ssdp` | `False` | Active DNS-SD or SSDP M-SEARCH before the sweep. Useful for routers, cameras, printers. |
| `preferred_host`, `preferred_retry` | `None` / `0` | "Try here first" fast-path. |

### `SerialDiscovery`

Key knobs: `preferred_port`, `probe_command`, `response_filter`,
`response_terminator`, `baud_rates`, `port_filter` (callback to exclude
ports), `include_bluetooth` (Windows). See the docstring on the class for
the full list.

### `UsbDiscovery`

Filters: `vid`, `pid`, `manufacturer`, `product`, `serial`, `class`. All
optional. Real OS hotplug via `nusb::watch_devices()`.

## Utilities

```python
# Auto-detect local subnets — same list TcpDiscovery uses when `subnets=[]`.
print(dafydd.local_subnets())              # default /24 ceiling
print(dafydd.local_subnets(max_prefix=20)) # allow up to /20 (4096 hosts)
```

## How it works

1. **Preferred-address path** (when configured): try the named port / host
   / VID:PID with a short timeout. On success, return immediately.
2. **TCP only — priority probes**: ARP cache (with MAC stamped on the match),
   then common last-octet heuristics (`.1`, `.100`, `.254`, …), then optional
   mDNS/SSDP responders.
3. **Full sweep**: enumerate endpoints, probe concurrently via Tokio
   `JoinSet` + semaphore. With raw-socket privilege on Linux, a single-RTT
   SYN scan further narrows TCP targets to open ports.
4. **macOS tty dedup** (Serial): the blocking `/dev/tty.*` variant of each
   port is filtered out so probes use `/dev/cu.*`.

## Building from source

```bash
# Editable install
uv sync
uv run maturin develop

# Release build
uv run maturin develop --release

# Rust-only tests + benches
cargo test
cargo bench --bench tcp_scan       # see benches/README.md
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT — see [LICENSE](LICENSE).
