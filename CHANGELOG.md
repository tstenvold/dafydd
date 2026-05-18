# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-05-16

### Added

- **Serial Discovery**: Probe serial ports with custom commands, sweep baud rates, with preferred port fast path
- **USB Discovery**: Enumerate USB devices by vendor/product ID with nusb (no libusb dependency)
- **TCP Discovery**: Multi-threaded subnet scanning with configurable concurrency limits and preferred host fast path
- **Python Bindings**: Full PyO3 integration with type stubs and integrated Tokio runtime
- **Cross-platform Support**: Linux, macOS (Intel & Apple Silicon), Windows
- **Benchmarks**: Criterion-based performance baselines for all three transports
- **Comprehensive Tests**: Platform-aware test suite with fixtures for serial/TCP/USB simulation

[0.1.0]: https://github.com/tstenvold/dafydd/releases/tag/v0.1.0
