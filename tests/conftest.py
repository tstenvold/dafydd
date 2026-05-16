"""Pytest configuration and fixtures for dafydd tests.

This module provides:
- Platform detection and awareness
- Platform-specific serial device simulation
- Fixtures for TCP, USB, and Serial testing
- Test markers for platform gating
"""

import asyncio
import os
import socket
import sys
import threading
import time
from contextlib import contextmanager
from dataclasses import dataclass
from typing import Any, Generator, Optional

import pytest

# Add the tests directory to the path so fixtures can be imported
_tests_dir = os.path.dirname(os.path.abspath(__file__))
if _tests_dir not in sys.path:
    sys.path.insert(0, _tests_dir)

from fixtures.platform import (
    Platform,
    get_platform_info,
    is_linux,
    is_macos,
    is_windows,
    is_arm,
    is_ci,
    supports_virtual_serial_ports,
)
from fixtures.serial_simulator import (
    SerialDeviceConfig,
    SerialSimulatorFactory,
    serial_device,
)


def pytest_configure(config):
    """Register custom markers."""
    config.addinivalue_line(
        "markers",
        "linux: Tests that run on Linux only",
    )
    config.addinivalue_line(
        "markers",
        "macos: Tests that run on macOS only",
    )
    config.addinivalue_line(
        "markers",
        "windows: Tests that run on Windows only",
    )
    config.addinivalue_line(
        "markers",
        "arm: Tests that run on ARM platforms (real or QEMU)",
    )
    config.addinivalue_line(
        "markers",
        "slow: Tests that are slow (QEMU, full integration)",
    )
    config.addinivalue_line(
        "markers",
        "requires_serial: Tests that require serial port support",
    )


# ============================================================================
# Pytest Hooks for Platform Gating
# ============================================================================


def pytest_collection_modifyitems(config, items):
    """Modify test collection based on platform."""
    info = get_platform_info()

    # Add platform markers based on current platform
    for item in items:
        # Add automatic platform markers
        if info.platform == Platform.LINUX and "linux" not in item.keywords:
            item.add_marker(pytest.mark.linux)
        elif info.platform == Platform.MACOS and "macos" not in item.keywords:
            item.add_marker(pytest.mark.macos)
        elif info.platform == Platform.WINDOWS and "windows" not in item.keywords:
            item.add_marker(pytest.mark.windows)
        elif info.platform == Platform.ARM_QEMU and "arm" not in item.keywords:
            item.add_marker(pytest.mark.arm)


def pytest_runtest_setup(item):
    """Skip tests based on platform and markers."""
    info = get_platform_info()

    # Skip tests that require serial ports on platforms that don't support them
    if "requires_serial" in item.keywords:
        if not supports_virtual_serial_ports():
            pytest.skip("Virtual serial ports not supported on this platform")

    # Skip ARM-specific tests on non-ARM platforms
    if "arm" in item.keywords and info.platform != Platform.ARM_QEMU:
        # Allow running if explicitly requested
        if "arm" not in [m.name for m in item.iter_markers()]:
            pass  # Don't auto-skip, allow to run on any platform for testing


# ============================================================================
# Fixtures
# ============================================================================


@pytest.fixture
def platform_info():
    """Get platform information."""
    return get_platform_info()


@pytest.fixture
def current_platform():
    """Get current platform enum."""
    return get_platform_info().platform


@pytest.fixture
def is_ci_environment():
    """Check if running in CI environment."""
    return is_ci()


@dataclass
class MockTcpDevice:
    """Configuration for a mock TCP device."""
    host: str
    port: int
    probe_command: bytes = b""
    expected_response: bytes = b"DEVICE_OK"
    response_delay: float = 0.0


class MockTcpServer:
    """TCP server that simulates a device responding to probes."""

    def __init__(self, device: MockTcpDevice):
        self.device = device
        self._server: Optional[socket.socket] = None
        self._thread: Optional[threading.Thread] = None
        self._running = False
        self._connection_count = 0
        self._connection_event = threading.Event()

    def start(self) -> None:
        """Start the mock TCP server."""
        self._running = True
        self._server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self._server.settimeout(5.0)
        self._server.bind((self.device.host, self.device.port))
        self._server.listen(1)

        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def _run(self) -> None:
        """Server loop running in a thread."""
        while self._running:
            try:
                conn, _ = self._server.accept()
                self._connection_count += 1
                self._connection_event.set()

                try:
                    if self.device.probe_command:
                        data = conn.recv(4096)
                        if self.device.probe_command in data:
                            time.sleep(self.device.response_delay)
                            conn.sendall(self.device.expected_response)
                    else:
                        time.sleep(self.device.response_delay)
                        conn.sendall(self.device.expected_response)

                    time.sleep(0.1)
                except Exception:
                    pass
                finally:
                    conn.close()
            except socket.timeout:
                continue
            except Exception:
                if self._running:
                    raise

    def stop(self) -> None:
        """Stop the mock TCP server."""
        self._running = False
        if self._server:
            try:
                self._server.close()
            except Exception:
                pass
        if self._thread:
            self._thread.join(timeout=3.0)

    @property
    def connection_count(self) -> int:
        return self._connection_count

    def wait_for_connection(self, timeout: float = 5.0) -> bool:
        """Wait for a client to connect."""
        return self._connection_event.wait(timeout)


@dataclass
class MockSerialDevice:
    """Configuration for a mock serial device."""
    port_path: str
    baud_rate: int = 9600
    probe_command: bytes = b""
    expected_response: bytes = b"OK"
    response_delay: float = 0.0


@dataclass
class MockUsbDevice:
    """Configuration for a mock USB device."""
    vid: int
    pid: int
    manufacturer: str = "TestCorp"
    product: str = "Test Device"
    serial_number: str = "SN12345"


@pytest.fixture
def unused_port() -> int:
    """Get an unused port for TCP testing."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("", 0))
        s.listen(1)
        return s.getsockname()[1]


@pytest.fixture(scope="function")
def event_loop():
    """Create an instance of the default event loop for each test case."""
    loop = asyncio.new_event_loop()
    asyncio.set_event_loop(loop)
    yield loop
    loop.run_until_complete(loop.shutdown_asyncgens())
    asyncio.set_event_loop(None)


@pytest.fixture
def serial_simulator():
    """Provide a serial device simulator based on the current platform.

    This fixture automatically selects the appropriate simulator:
    - Unix (Linux/macOS): PTY pair via socat or Python pty module
    - ARM QEMU: QEMU emulated serial port
    - Windows: Mock simulator (virtual ports not well supported)

    Returns a context manager that provides a running simulator.
    """
    config = SerialDeviceConfig(
        port_path="/tmp/test_device",
        baud_rate=9600,
        probe_command=b"STATUS",
        expected_response=b"DEVICE_OK",
    )
    return serial_device(config)


@pytest.fixture
def serial_simulator_with_config():
    """Provide a factory for creating custom serial simulators.

    Returns a function that accepts SerialDeviceConfig and returns
    a context manager with the running simulator.
    """
    def _create(config: SerialDeviceConfig):
        return serial_device(config)
    return _create