"""Tests for Serial device discovery with cross-platform support.

These tests verify serial device discovery functionality across different
platforms (Linux, macOS, Windows, ARM/QEMU).

Test categories:
- Platform-aware: Tests that automatically adapt to the current platform
- Marked tests: Tests explicitly gated by platform markers
- Slow tests: Tests that are slow (QEMU) and skipped in CI by default
"""

import os
import sys

# Add the tests directory to the path so fixtures can be imported
_tests_dir = os.path.dirname(os.path.abspath(__file__))
if _tests_dir not in sys.path:
    sys.path.insert(0, _tests_dir)

import pytest

import dafydd
from fixtures.platform import (
    get_platform_info,
    is_linux,
    is_macos,
    is_windows,
    supports_virtual_serial_ports,
    Platform,
)
from fixtures.serial_simulator import SerialDeviceConfig


# ============================================================================
# Platform-Aware Tests - Automatically adapt to current platform
# ============================================================================


class TestSerialDiscoveryPlatformAware:
    """Tests that automatically adapt to the current platform."""

    def test_discovery_returns_list(self):
        """Test that discover() returns a list (may be empty)."""
        discovery = dafydd.SerialDiscovery(b"", [9600], timeout_ms=100)
        results = discovery.discover()
        assert isinstance(results, list)

    def test_single_baud_rate(self):
        """Test with single baud rate."""
        discovery = dafydd.SerialDiscovery(b"STATUS", [115200], timeout_ms=500)
        assert discovery is not None

    def test_multiple_baud_rates_config(self):
        """Test configuration with multiple baud rates."""
        discovery = dafydd.SerialDiscovery(
            b"STATUS",
            [9600, 19200, 38400, 57600, 115200],
            timeout_ms=500,
        )
        assert discovery is not None


# ============================================================================
# Platform-Specific Tests - Only run on appropriate platforms
# ============================================================================


@pytest.mark.linux
class TestSerialDiscoveryLinux:
    """Linux-specific serial discovery tests."""

    def test_linux_port_paths(self):
        """Test Linux-specific port path formats."""
        discovery = dafydd.SerialDiscovery(
            b"",
            [9600],
            preferred_port="/dev/ttyUSB0",
        )
        assert discovery is not None

        discovery = dafydd.SerialDiscovery(
            b"",
            [9600],
            preferred_port="/dev/ttyS0",
        )
        assert discovery is not None


@pytest.mark.macos
class TestSerialDiscoveryMacOS:
    """macOS-specific serial discovery tests."""

    def test_macos_port_paths(self):
        """Test macOS-specific port path formats."""
        # macOS uses /dev/tty.* and /dev/cu.* paths
        discovery = dafydd.SerialDiscovery(
            b"",
            [9600],
            preferred_port="/dev/cu.usbserial",
        )
        assert discovery is not None


@pytest.mark.windows
class TestSerialDiscoveryWindows:
    """Windows-specific serial discovery tests."""

    def test_windows_port_paths(self):
        """Test Windows-specific port path formats."""
        discovery = dafydd.SerialDiscovery(
            b"",
            [9600],
            preferred_port="COM1",
        )
        assert discovery is not None


# ============================================================================
# Virtual Port Tests - Require virtual port support
# ============================================================================


@pytest.mark.requires_serial
@pytest.mark.skip(reason="Virtual serial ports require additional setup (socat)")
class TestSerialDiscoveryWithVirtualPorts:
    """Tests that require virtual serial port support.

    These tests will be skipped on Windows and other platforms
    without good virtual serial port support.

    TODO: Enable these tests once we have proper socat setup.
    """

    def test_discovery_with_mock_device(self):
        """Test discovery with a simulated device."""
        pytest.skip("Virtual serial port tests require socat installation")


# ============================================================================
# Cross-Platform Configuration Tests
# ============================================================================


class TestSerialDiscoveryConfiguration:
    """Test various configuration options across all platforms."""

    def test_timeout_configuration(self):
        """Test various timeout configurations."""
        for timeout in [100, 500, 1000, 5000]:
            discovery = dafydd.SerialDiscovery(b"", [9600], timeout_ms=timeout)
            assert discovery is not None

    def test_baud_rate_common_values(self):
        """Test common baud rate values."""
        common_bauds = [300, 1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200]
        for baud in common_bauds:
            discovery = dafydd.SerialDiscovery(b"", [baud], timeout_ms=100)
            assert discovery is not None

    def test_empty_probe_command(self):
        """Test with empty probe command (just looks for any response)."""
        discovery = dafydd.SerialDiscovery(b"", [9600], timeout_ms=100)
        assert discovery is not None

    def test_include_bluetooth_config(self):
        """Test configuration with bluetooth inclusion."""
        discovery = dafydd.SerialDiscovery(
            b"STATUS",
            [9600],
            timeout_ms=500,
            include_bluetooth=True,
        )
        assert discovery is not None


# ============================================================================
# Failure Case Tests
# ============================================================================


class TestSerialDiscoveryFailures:
    """Test failure cases for serial discovery."""

    def test_nonexistent_port_timeout(self):
        """Test timeout with non-existent port."""
        discovery = dafydd.SerialDiscovery(
            b"PING",
            [9600],
            timeout_ms=50,
            preferred_port="/dev/nonexistent_port_xyz",
        )
        # Should not raise, but may return empty or handle gracefully
        try:
            results = discovery.discover()
            assert isinstance(results, list)
        except Exception:
            # Some platforms raise on non-existent ports
            pass

    def test_no_ports_available(self):
        """Test discovery when no serial ports exist."""
        # This test verifies the API works
        discovery = dafydd.SerialDiscovery(b"", [9600], timeout_ms=100)
        results = discovery.discover()
        assert results == []


# ============================================================================
# Platform Information Tests
# ============================================================================


class TestPlatformDetection:
    """Test that platform detection works correctly."""

    def test_platform_info_available(self):
        """Test that platform info is available."""
        info = get_platform_info()
        assert info is not None
        assert info.platform in Platform
        assert info.architecture is not None

    def test_platform_detection_reflects_system(self):
        """Test that platform detection matches actual system."""
        info = get_platform_info()

        # Verify the platform matches what we'd expect from platform.system()
        import platform
        system = platform.system().lower()

        if system == "darwin":
            assert info.platform == Platform.MACOS
        elif system == "linux":
            # Could be Linux or ARM_QEMU depending on machine
            assert info.platform in (Platform.LINUX, Platform.ARM_QEMU)
        elif system == "windows":
            assert info.platform == Platform.WINDOWS

    def test_virtual_port_support(self):
        """Test virtual port support detection."""
        # This should be consistent with platform
        if is_linux() or is_macos():
            assert supports_virtual_serial_ports() is True
        elif is_windows():
            assert supports_virtual_serial_ports() is False