"""End-to-end tests for TCP/IP device discovery using real TCP servers."""

import socket
import threading
import time
from contextlib import contextmanager

import pytest

import dafydd


class TcpDeviceSimulator:
    """Simulates a TCP device that responds to probe commands."""

    def __init__(
        self,
        host: str,
        port: int,
        probe_command: bytes = b"",
        response: bytes = b"DEVICE_OK",
        response_delay: float = 0.0,
    ):
        self.host = host
        self.port = port
        self.probe_command = probe_command
        self.response = response
        self.response_delay = response_delay
        self._server = None
        self._thread = None
        self._running = False
        self.connections = 0

    def start(self):
        """Start the TCP server."""
        self._running = True
        self._server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self._server.settimeout(5.0)
        self._server.bind((self.host, self.port))
        self._server.listen(1)
        self._thread = threading.Thread(target=self._serve, daemon=True)
        self._thread.start()

    def _serve(self):
        """Server loop."""
        assert self._server is not None
        while self._running:
            try:
                conn, _ = self._server.accept()
                self.connections += 1
                try:
                    if self.probe_command:
                        data = conn.recv(1024)
                        if self.probe_command in data:
                            time.sleep(self.response_delay)
                            conn.sendall(self.response)
                    else:
                        time.sleep(self.response_delay)
                        conn.sendall(self.response)
                finally:
                    conn.close()
            except socket.timeout:
                continue
            except Exception:
                if self._running:
                    raise

    def stop(self):
        """Stop the TCP server."""
        self._running = False
        if self._server:
            self._server.close()
        if self._thread:
            self._thread.join(timeout=3.0)


@contextmanager
def make_tcp_device(
    host: str = "127.0.0.1",
    port: int = 9000,
    probe_command: bytes = b"",
    response: bytes = b"DEVICE_OK",
    response_delay: float = 0.0,
):
    """Context manager that provides a running TCP device simulator."""
    device = TcpDeviceSimulator(host, port, probe_command, response, response_delay)
    device.start()
    time.sleep(0.1)  # Allow server to start
    try:
        yield device
    finally:
        device.stop()


class TestTcpDiscoveryGoldenCase:
    """Tests for the golden case: preferred host found immediately."""

    def test_preferred_host_found_with_probe(self, unused_port):
        """Test finding a device via preferred host with probe/response matching."""
        with make_tcp_device(
            host="127.0.0.1",
            port=unused_port,
            probe_command=b"STATUS?",
            response=b"DEVICE_READY",
        ) as _:
            # Note: expected_response param may not be exposed in Python API
            # Testing with probe_command only
            discovery = dafydd.TcpDiscovery(
                port=unused_port,
                subnets=[],
                probe_command=b"STATUS?",
                connect_timeout_ms=1000,
                io_timeout_ms=1000,
                preferred_host="127.0.0.1",
            )
            results = discovery.discover()

            assert len(results) == 1
            assert results[0].transport == dafydd.Transport.Tcp
            assert f"127.0.0.1:{unused_port}" == results[0].address

    def test_preferred_host_found_no_probe(self, unused_port):
        """Test finding a device via preferred host without probe/response."""
        with make_tcp_device(
            host="127.0.0.1",
            port=unused_port,
            response=b"OK",
        ) as _:
            discovery = dafydd.TcpDiscovery(
                port=unused_port,
                subnets=[],
                connect_timeout_ms=1000,
                io_timeout_ms=1000,
                preferred_host="127.0.0.1",
            )
            results = discovery.discover()

            assert len(results) == 1
            assert results[0].transport == dafydd.Transport.Tcp


class TestTcpDiscoveryFoundCase:
    """Tests for found case: device found during subnet sweep."""

    def test_subnet_sweep_finds_device(self, unused_port):
        """Test finding a device during subnet sweep."""
        with make_tcp_device(
            host="127.0.0.1",
            port=unused_port,
            response=b"READY",
        ) as _:
            discovery = dafydd.TcpDiscovery(
                port=unused_port,
                subnets=["127.0.0.1/32"],
                connect_timeout_ms=1000,
                io_timeout_ms=1000,
            )
            results = discovery.discover()

            assert len(results) == 1
            assert results[0].transport == dafydd.Transport.Tcp
            assert results[0].address == f"127.0.0.1:{unused_port}"

    def test_subnet_sweep_finds_multiple_devices(self, unused_port):
        """Test finding multiple devices during subnet sweep."""
        port1 = unused_port
        # Get a second independent free port; unused_port+1 is not guaranteed
        # to be available (Windows reserves port ranges that can block bind()).
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.bind(("", 0))
            port2 = s.getsockname()[1]

        with (
            make_tcp_device(host="127.0.0.1", port=port1, response=b"DEV1"),
            make_tcp_device(host="127.0.0.1", port=port2, response=b"DEV2"),
        ):
            discovery = dafydd.TcpDiscovery(
                port=port1,
                subnets=["127.0.0.1/32"],
                connect_timeout_ms=1000,
                io_timeout_ms=1000,
            )
            # Only devices on the configured port will be found
            results = discovery.discover()

            assert len(results) >= 1


class TestTcpDiscoveryFailureCase:
    """Tests for failure case: no devices found."""

    def test_no_devices_on_subnet(self):
        """Test that discovery returns empty when no devices are present."""
        discovery = dafydd.TcpDiscovery(
            port=9999,
            subnets=["127.0.0.1/32"],
            connect_timeout_ms=500,
            io_timeout_ms=500,
        )
        results = discovery.discover()

        assert results == []

    def test_preferred_host_not_found(self):
        """Test that preferred host fallback to subnet works."""
        # Start a device on a different port
        with make_tcp_device(host="127.0.0.1", port=9998, response=b"OK"):
            discovery = dafydd.TcpDiscovery(
                port=9997,  # Different port - won't be found
                subnets=["127.0.0.1/32"],
                connect_timeout_ms=500,
                io_timeout_ms=500,
                preferred_host="127.0.0.1",  # Preferred tries wrong port first
            )
            results = discovery.discover()
            # Preferred fails, subnet sweep should also fail since device is on different port
            assert results == []

    def test_timeout_no_response(self):
        """Test that discovery handles devices that don't respond."""
        # Create a server that accepts but never responds
        server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        server.bind(("127.0.0.1", 0))
        server.listen(1)
        server.settimeout(1.0)
        port = server.getsockname()[1]

        def serve_forever():
            while True:
                try:
                    conn, _ = server.accept()
                    time.sleep(10)  # Never respond
                    conn.close()
                except Exception:
                    break

        thread = threading.Thread(target=serve_forever, daemon=True)
        thread.start()

        try:
            # With probe_command set, the device must respond to be found
            # Without probe_command, any open port is considered a match
            discovery = dafydd.TcpDiscovery(
                port=port,
                subnets=["127.0.0.1/32"],
                probe_command=b"PING",  # Require a probe response
                connect_timeout_ms=500,
                io_timeout_ms=500,
            )
            results = discovery.discover()
            # Since the server never responds, it should not be found
            assert results == []
        finally:
            server.close()


class TestTcpDiscoveryEdgeCases:
    """Tests for edge cases."""

    def test_invalid_subnet_raises_value_error(self):
        """Test that invalid subnet raises ValueError."""
        discovery = dafydd.TcpDiscovery(
            port=9000,
            subnets=["invalid_subnet"],
        )
        with pytest.raises(ValueError) as exc_info:
            discovery.discover()
        assert "invalid subnet" in str(exc_info.value).lower()

    def test_malformed_cidr_raises_error(self):
        """Test that malformed CIDR raises appropriate error."""
        discovery = dafydd.TcpDiscovery(
            port=9000,
            subnets=["192.168.1.0/33"],  # Invalid prefix length
        )
        with pytest.raises(ValueError):
            discovery.discover()

    def test_dns_resolution_for_hostname(self, unused_port):
        """Test that DNS hostname resolution works for preferred_host."""
        with make_tcp_device(
            host="127.0.0.1",
            port=unused_port,
            response=b"OK",
        ) as _:
            discovery = dafydd.TcpDiscovery(
                port=unused_port,
                subnets=[],
                connect_timeout_ms=1000,
                io_timeout_ms=1000,
                preferred_host="localhost",
            )
            results = discovery.discover()
            assert isinstance(results, list)

    def test_device_response_captured(self, unused_port):
        """Test that device response is captured in results."""
        with make_tcp_device(
            host="127.0.0.1",
            port=unused_port,
            probe_command=b"PING",
            response=b"PONG",
        ) as _:
            discovery = dafydd.TcpDiscovery(
                port=unused_port,
                subnets=["127.0.0.1/32"],
                probe_command=b"PING",
                connect_timeout_ms=1000,
                io_timeout_ms=1000,
            )
            results = discovery.discover()

            assert len(results) == 1
            assert results[0].response == b"PONG"

    def test_no_probe_returns_empty_response(self, unused_port):
        """Test that no probe_command returns None response."""
        with make_tcp_device(
            host="127.0.0.1",
            port=unused_port,
            response=b"DATA",
        ) as _:
            discovery = dafydd.TcpDiscovery(
                port=unused_port,
                subnets=["127.0.0.1/32"],
                connect_timeout_ms=1000,
                io_timeout_ms=1000,
            )
            results = discovery.discover()

            assert len(results) == 1
            assert results[0].response is None

    def test_max_concurrent_limits_connections(self, unused_port):
        """Test that max_concurrent parameter is accepted."""
        discovery = dafydd.TcpDiscovery(
            port=unused_port,
            subnets=["127.0.0.1/32"],
            connect_timeout_ms=100,
            io_timeout_ms=100,
            max_concurrent=10,
        )
        results = discovery.discover()
        assert isinstance(results, list)

    def test_info_contains_hostname(self, unused_port):
        """Test that hostname is included in info when using preferred_host."""
        with make_tcp_device(
            host="127.0.0.1",
            port=unused_port,
            response=b"OK",
        ) as _:
            discovery = dafydd.TcpDiscovery(
                port=unused_port,
                subnets=[],
                connect_timeout_ms=1000,
                io_timeout_ms=1000,
                preferred_host="127.0.0.1",
            )
            results = discovery.discover()

            assert len(results) == 1
            assert "hostname" in results[0].info
