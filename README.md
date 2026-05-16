# Dafydd

**Fast Device Discovery over Serial, USB, and TCP/IP.**

Dafydd is a native Rust library offering highly concurrent and lightning-fast physical device probing with convenient Python bindings via PyO3. 

## Features
* **TCP Iteration**: Multi-threaded Tokio backend bounded by semaphores for high-speed subnet sweeping.
* **USB & Serial Probing**: Intercept direct byte responses mapping into Python natively for complete hardware configurability.
* **Open Source**: Licensed under `MIT OR Apache-2.0`.

## Quick Start (Python)
```python
from dafydd import tcp

discoverer = tcp.TcpDiscovery()
# High speed probe looking for a `\x01` byte response on 8080
devices = discoverer.discover(subnets=["192.168.1.0/24"], port=8080, probe=b"ping")

for device in devices:
    print(device.address)
    print(device.response)
```

## Contributing & Testing
* **Python Tests**: `uv run pytest tests/ -v`
* **Rust Tests & Coverage**: Utilizes `cargo-llvm-cov` to bind Rust+Python combined code coverage.
* **Benchmarks**: We use `criterion`. Run `cargo bench` to assert speed scaling.
