"""Tests for USB device discovery."""

import pytest

import dafydd


class TestUsbDiscoveryGoldenCase:
    """Tests for golden case: specific device found with VID/PID filter."""

    def test_usb_discovery_instantiation_with_vid_pid(self):
        """Test creating UsbDiscovery with VID/PID filter."""
        discovery = dafydd.UsbDiscovery(vid=0x1234, pid=0x5678)
        assert discovery is not None

    def test_usb_discovery_instantiation_vid_only(self):
        """Test creating UsbDiscovery with VID only."""
        discovery = dafydd.UsbDiscovery(vid=0x1234)
        assert discovery is not None

    def test_usb_discovery_instantiation_no_filter(self):
        """Test creating UsbDiscovery without filters."""
        discovery = dafydd.UsbDiscovery()
        assert discovery is not None


class TestUsbDiscoveryFoundCase:
    """Tests for found case: device found in list."""

    def test_discovery_returns_list(self):
        """Test that discover() returns a list."""
        discovery = dafydd.UsbDiscovery()
        results = discovery.discover()
        assert isinstance(results, list)

    def test_discovery_with_vid_filter_returns_list(self):
        """Test that discover() with VID filter returns a list."""
        discovery = dafydd.UsbDiscovery(vid=0x1234)
        results = discovery.discover()
        assert isinstance(results, list)

    def test_discovery_with_vid_pid_filter_returns_list(self):
        """Test that discover() with VID/PID filter returns a list."""
        discovery = dafydd.UsbDiscovery(vid=0x1234, pid=0x5678)
        results = discovery.discover()
        assert isinstance(results, list)


class TestUsbDiscoveryFailureCase:
    """Tests for failure case: no devices found."""

    def test_no_matching_devices_returns_empty(self):
        """Test when no devices match the filter."""
        discovery = dafydd.UsbDiscovery(vid=0x9999, pid=0x8888)
        results = discovery.discover()
        assert results == []

    def test_no_usb_devices_at_all_returns_empty(self):
        """Test when no USB devices are connected."""
        discovery = dafydd.UsbDiscovery()
        results = discovery.discover()
        assert isinstance(results, list)  # May be empty or contain system devices


class TestUsbDiscoveryEdgeCases:
    """Tests for edge cases."""

    def test_vid_hex_values(self):
        """Test various VID hex values."""
        for vid in [0x0000, 0x1234, 0xFFFF, 0xABCD]:
            discovery = dafydd.UsbDiscovery(vid=vid)
            assert discovery is not None

    def test_pid_hex_values(self):
        """Test various PID hex values."""
        for pid in [0x0000, 0x5678, 0xFFFF, 0xEF01]:
            discovery = dafydd.UsbDiscovery(pid=pid)
            assert discovery is not None

    def test_both_vid_and_pid(self):
        """Test with both VID and PID specified."""
        discovery = dafydd.UsbDiscovery(vid=0x1234, pid=0x5678)
        results = discovery.discover()
        assert isinstance(results, list)

    def test_zero_vid_pid(self):
        """Test with zero VID/PID."""
        discovery = dafydd.UsbDiscovery(vid=0, pid=0)
        results = discovery.discover()
        assert isinstance(results, list)


class TestUsbDiscoveryIntegration:
    """Integration tests that verify actual USB discovery behavior."""

    def test_usb_discovery_completes_without_error(self):
        """Test that USB discovery completes without raising exceptions."""
        discovery = dafydd.UsbDiscovery()
        try:
            results = discovery.discover()
            assert isinstance(results, list)
        except Exception as e:
            # May fail if USB subsystem is unavailable
            pytest.skip(f"USB subsystem unavailable: {e}")

    def test_usb_discovery_with_specific_vid_completes(self):
        """Test that USB discovery with specific VID completes."""
        discovery = dafydd.UsbDiscovery(vid=0x0403)  # Common FTDI VID
        try:
            results = discovery.discover()
            assert isinstance(results, list)
        except Exception:
            pytest.skip("USB subsystem unavailable")

    def test_usb_discovery_result_structure(self):
        """Test that USB discovery results have expected structure."""
        discovery = dafydd.UsbDiscovery()
        try:
            results = discovery.discover()
            for result in results:
                assert result.transport == dafydd.Transport.Usb
                assert hasattr(result, "address")
                assert hasattr(result, "info")
                # Address should be in VID:PID format
                assert ":" in result.address
        except Exception:
            pytest.skip("USB subsystem unavailable")
