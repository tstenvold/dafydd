# Contributing to Dafydd

Thank you for your interest in contributing! This guide covers building, testing, and submitting changes.

## Setup

### Requirements

- **Rust 1.95+** ([install](https://rustup.rs/))
- **Python 3.11+**
- **uv** ([install](https://docs.astral.sh/uv/getting-started/installation/))
- **Platform-specific**:
  - **Linux/macOS**: `socat` (for serial port simulation in tests)
  - **Linux**: `libudev-dev` (for `serialport` crate)
  - **Windows**: No extra dependencies

### Clone & Build

```bash
git clone https://github.com/tstenvold/dafydd
cd dafydd

# Sync dependencies and create venv
uv sync

# Build and install the native module
uv run maturin develop --release
```

## Development Workflow

### Linting & Formatting

```bash
# Rust formatting
cargo fmt

# Rust lints (must pass with -D warnings)
cargo clippy --lib --bins --tests -- -D warnings

# Python formatting and linting
uv run ruff format tests/ python/
uv run ruff check tests/ python/ --fix

# Type checking
uv run pyright tests/
```

### Testing

```bash
# Run all Python tests (platform-aware)
uv run pytest tests/ -v

# Run Rust tests
cargo test --lib --tests

# Run a specific test
uv run pytest tests/test_tcp.py::test_tcp_discovery_golden -v

# Run with coverage
cargo install cargo-llvm-cov  # one-time
cargo llvm-cov --lib --tests --html
```

### Benchmarks

```bash
# Full benchmarks (slow)
cargo bench

# Smoke test benches (fast, for CI)
cargo bench --bench tcp_scan -- --test
cargo bench --bench serial_scan -- --test
cargo bench --bench usb_scan -- --test
```

### Documentation

```bash
# Build rustdoc (catches broken links)
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items

# Check Python stub files
uv run pyright python/dafydd/
```

## Making Changes

### Code Style

- Follow Rust conventions (clippy enforces it)
- Python: PEP 8 via `ruff`
- Type hints on all Python functions
- Doc comments on all public Rust items (`missing_docs = "warn"`)
- Docstrings on all public Python classes and methods

### Commit Messages

- Start with a verb: "Add", "Fix", "Refactor", "Test", "Docs"
- Keep subject to ~50 characters
- Reference issues: "Fixes #123"

### Branches

- Create a feature branch: `git checkout -b feature/your-feature`
- Push and open a pull request against `main`
- Ensure all CI checks pass

## Submitting a Pull Request

1. Fork and create a branch
2. Make your changes and commit
3. Run `cargo fmt`, `cargo clippy`, `uv run pytest` locally
4. Push to your fork and open a PR
5. Address feedback from CI and reviewers
6. Merge when approved

## Bug Reports & Feature Requests

Open an issue on GitHub describing:

- **Bug**: Steps to reproduce, expected vs. actual behavior
- **Feature**: Use case and proposed API
- **Improve docs**: What was unclear?

## Questions?

- Check [README.md](README.md) for usage examples
- Read the rustdoc: `cargo doc --open`
- Review `tests/` for more examples
