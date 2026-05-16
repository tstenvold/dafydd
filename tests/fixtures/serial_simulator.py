"""Serial port simulator implementations for cross-platform testing.

This module provides virtual serial port capabilities for testing serial device
discovery across different platforms (Linux, macOS, Windows, ARM/QEMU).
"""

import asyncio
import os
import subprocess
import threading
import time
from abc import ABC, abstractmethod
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional


@dataclass
class SerialDeviceConfig:
    """Configuration for a simulated serial device."""
    port_path: str
    baud_rate: int = 9600
    probe_command: bytes = b"STATUS"
    expected_response: bytes = b"DEVICE_OK"
    response_delay: float = 0.0


class SerialSimulator(ABC):
    """Abstract base class for serial port simulation."""

    @abstractmethod
    async def start(self) -> None:
        """Start the serial simulator."""
        pass

    @abstractmethod
    async def stop(self) -> None:
        """Stop the serial simulator."""
        pass

    @abstractmethod
    def get_device_port(self) -> str:
        """Get the path to the device port for connection."""
        pass

    @abstractmethod
    def get_client_port(self) -> str:
        """Get the path to the client port for connecting the test."""
        pass

    @property
    @abstractmethod
    def is_running(self) -> bool:
        """Check if the simulator is running."""
        pass


class UnixPTYSimulator(SerialSimulator):
    """Serial simulator using PTY (pseudo-terminal) pairs on Unix systems.

    Uses socat to create a pair of connected PTYs, or falls back to
    Python's pty module if socat is not available.
    """

    def __init__(
        self,
        config: SerialDeviceConfig,
        use_socat: bool = True,
    ):
        self.config = config
        self.use_socat = use_socat and self._check_socat()
        self._socat_process: Optional[subprocess.Popen] = None
        self._device_port: Optional[str] = None
        self._client_port: Optional[str] = None
        self._running = False

    def _check_socat(self) -> bool:
        """Check if socat is available."""
        import shutil
        return shutil.which("socat") is not None

    async def start(self) -> None:
        """Start the PTY pair using socat."""
        if self.use_socat:
            await self._start_socat()
        else:
            await self._start_pty_module()

    async def _start_socat(self) -> None:
        """Start using socat to create PTY pair."""
        # Create symbolic links for easier access
        device_link = f"/tmp/dafydd_test_device_{os.getpid()}"
        client_link = f"/tmp/dafydd_test_client_{os.getpid()}"

        # Remove existing links
        for link in [device_link, client_link]:
            if os.path.exists(link):
                os.unlink(link)

        # Start socat to create a bidirectional pipe between two PTYs
        # One side will be our "device", the other is for the client
        cmd = [
            "socat",
            "-d", "-d",  # Verbose debugging
            f"PTY,raw,echo=0,link={device_link}",
            f"PTY,raw,echo=0,link={client_link}",
        ]

        try:
            self._socat_process = subprocess.Popen(
                cmd,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            # Wait for socat to create the links
            time.sleep(0.5)

            # Verify links were created
            if os.path.exists(device_link) and os.path.exists(client_link):
                self._device_port = device_link
                self._client_port = client_link
                self._running = True
            else:
                raise RuntimeError("Failed to create PTY links")

        except FileNotFoundError:
            self.use_socat = False
            await self._start_pty_module()
        except Exception as e:
            raise RuntimeError(f"Failed to start socat: {e}")

    async def _start_pty_module(self) -> None:
        """Start using Python's pty module as fallback."""
        import pty
        import tty

        # Create a PTY pair
        master_fd, slave_fd = pty.openpty()
        self._master_fd = master_fd
        self._slave_fd = slave_fd

        # Get the device names
        self._device_port = os.ttyname(slave_fd)
        self._client_port = os.ttyname(master_fd)

        # Start a thread to handle device responses
        self._device_thread = threading.Thread(
            target=self._device_loop,
            args=(slave_fd,),
            daemon=True,
        )
        self._device_thread.start()
        self._running = True

    def _device_loop(self, fd: int) -> None:
        """Device response loop running in a thread."""
        import select

        while self._running:
            try:
                ready, _, _ = select.select([fd], [], [], 0.1)
                if ready:
                    data = os.read(fd, 1024)
                    if self.config.probe_command in data:
                        time.sleep(self.config.response_delay)
                        os.write(fd, self.config.expected_response)
            except Exception:
                break

    async def stop(self) -> None:
        """Stop the simulator."""
        self._running = False

        if self._socat_process:
            self._socat_process.terminate()
            try:
                self._socat_process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                self._socat_process.kill()

        if hasattr(self, '_master_fd'):
            try:
                os.close(self._master_fd)
                os.close(self._slave_fd)
            except Exception:
                pass

        # Clean up symlinks
        for link in [self._device_port, self._client_port]:
            if link and os.path.exists(link):
                try:
                    os.unlink(link)
                except Exception:
                    pass

    def get_device_port(self) -> str:
        """Get the device port path."""
        if not self._device_port:
            raise RuntimeError("Simulator not started")
        return self._device_port

    def get_client_port(self) -> str:
        """Get the client port path."""
        if not self._client_port:
            raise RuntimeError("Simulator not started")
        return self._client_port

    @property
    def is_running(self) -> bool:
        return self._running


class QEMUARMSerialSimulator(SerialSimulator):
    """Serial simulator using QEMU emulated ARM board.

    This runs a QEMU virtual machine with a serial port redirected
    to a TCP socket, allowing tests to connect as if to a real device.
    """

    def __init__(
        self,
        config: SerialDeviceConfig,
        qemu_machine: str = "virt",
        qemu_cpu: str = "cortex-a7",
        kernel_image: Optional[str] = None,
    ):
        self.config = config
        self.qemu_machine = qemu_machine
        self.qemu_cpu = qemu_cpu
        self.kernel_image = kernel_image
        self._qemu_process: Optional[subprocess.Popen] = None
        self._tcp_port: Optional[int] = None
        self._running = False

    async def start(self) -> None:
        """Start QEMU with serial port redirected to TCP."""
        import socket

        # Find an available port
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.bind(("", 0))
            self._tcp_port = s.getsockname()[1]

        # Build QEMU command
        cmd = [
            "qemu-system-arm",
            "-M", self.qemu_machine,
            "-cpu", self.qemu_cpu,
            "-nographic",
            "-serial", f"tcp::{self._tcp_port},server,nowait",
        ]

        if self.kernel_image:
            cmd.extend(["-kernel", self.kernel_image])

        try:
            self._qemu_process = subprocess.Popen(
                cmd,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            # Wait for QEMU to start
            time.sleep(2)
            self._running = True
        except FileNotFoundError:
            raise RuntimeError("qemu-system-arm not found. Install with: brew install qemu")
        except Exception as e:
            raise RuntimeError(f"Failed to start QEMU: {e}")

    async def stop(self) -> None:
        """Stop QEMU."""
        self._running = False

        if self._qemu_process:
            self._qemu_process.terminate()
            try:
                self._qemu_process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self._qemu_process.kill()

    def get_device_port(self) -> str:
        """Get the TCP host:port for connecting."""
        if not self._tcp_port:
            raise RuntimeError("Simulator not started")
        return f"127.0.0.1:{self._tcp_port}"

    def get_client_port(self) -> str:
        """Same as device port for TCP."""
        return self.get_device_port()

    @property
    def is_running(self) -> bool:
        return self._running


class MockSerialSimulator(SerialSimulator):
    """Mock serial simulator for platforms without virtual port support.

    This provides an API-compatible mock that verifies the test infrastructure
    works but doesn't perform real serial I/O. Useful for Windows or when
    permissions prevent port creation.
    """

    def __init__(self, config: SerialDeviceConfig):
        self.config = config
        self._running = False

    async def start(self) -> None:
        """Start the mock simulator."""
        self._running = True

    async def stop(self) -> None:
        """Stop the mock simulator."""
        self._running = False

    def get_device_port(self) -> str:
        """Return a mock port path."""
        return "/dev/ttyUSB0"

    def get_client_port(self) -> str:
        """Return a mock port path."""
        return "/dev/ttyUSB1"

    @property
    def is_running(self) -> bool:
        return self._running


class SerialSimulatorFactory:
    """Factory for creating appropriate serial simulators based on platform."""

    @staticmethod
    def create(config: SerialDeviceConfig) -> SerialSimulator:
        """Create a serial simulator appropriate for the current platform."""
        from .platform import get_platform_info, Platform, is_arm

        info = get_platform_info()

        if info.platform == Platform.WINDOWS:
            # Windows doesn't have good virtual port support
            # Use mock for API testing, or suggest Docker/WSL
            return MockSerialSimulator(config)

        elif info.platform == Platform.ARM_QEMU:
            # ARM platform - could be real hardware or QEMU
            # For testing, use QEMU simulator
            return QEMUARMSerialSimulator(config)

        elif info.platform in (Platform.LINUX, Platform.MACOS):
            # Unix-like systems - use PTY pair
            return UnixPTYSimulator(config)

        else:
            # Unknown platform - use mock
            return MockSerialSimulator(config)


@contextmanager
def serial_device(config: SerialDeviceConfig):
    """Context manager for serial device simulation.

    Usage:
        config = SerialDeviceConfig(
            port_path="/dev/ttyUSB0",
            probe_command=b"STATUS",
            expected_response=b"DEVICE_OK",
        )
        with serial_device(config) as sim:
            # sim.get_client_port() is available
            discovery = dafydd.SerialDiscovery(...)
            discovery.discover()
    """
    simulator = SerialSimulatorFactory.create(config)
    loop = asyncio.new_event_loop()
    asyncio.set_event_loop(loop)

    try:
        loop.run_until_complete(simulator.start())
        yield simulator
    finally:
        loop.run_until_complete(simulator.stop())
        loop.close()