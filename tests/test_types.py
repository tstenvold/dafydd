"""Tests for device types and transport enum."""

import dafydd


class TestTransport:
    """Tests for the Transport enum."""

    def test_transport_values(self):
        """Test that all transport values exist."""
        assert dafydd.Transport.Serial is not None
        assert dafydd.Transport.Usb is not None
        assert dafydd.Transport.Tcp is not None

    def test_transport_equality(self):
        """Test transport enum equality."""
        assert dafydd.Transport.Serial == dafydd.Transport.Serial
        assert dafydd.Transport.Usb == dafydd.Transport.Usb
        assert dafydd.Transport.Tcp == dafydd.Transport.Tcp

        assert dafydd.Transport.Serial != dafydd.Transport.Usb
        assert dafydd.Transport.Usb != dafydd.Transport.Tcp


class TestDeviceMatch:
    """Tests for the DeviceMatch class."""

    def test_device_match_from_discovery(self):
        """Test DeviceMatch returned from discovery."""
        discovery = dafydd.TcpDiscovery(
            port=9999,
            subnets=["127.0.0.1/32"],
            connect_timeout_ms=100,
            io_timeout_ms=100,
        )
        results = discovery.discover()
        assert isinstance(results, list)

    def test_device_match_properties(self):
        """Test DeviceMatch has expected properties via TCP discovery."""
        import socket
        import threading
        import time

        server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        server.bind(("127.0.0.1", 0))
        server.listen(1)
        port = server.getsockname()[1]

        def serve():
            try:
                conn, _ = server.accept()
                conn.sendall(b"OK")
                time.sleep(0.1)
                conn.close()
            except OSError:
                pass

        thread = threading.Thread(target=serve, daemon=True)
        thread.start()
        time.sleep(0.1)

        try:
            discovery = dafydd.TcpDiscovery(
                port=port,
                subnets=["127.0.0.1/32"],
                connect_timeout_ms=1000,
                io_timeout_ms=1000,
            )
            results = discovery.discover()
            assert len(results) == 1
            match = results[0]
            assert hasattr(match, "transport")
            assert hasattr(match, "address")
            assert hasattr(match, "info")
            assert hasattr(match, "response")
            assert match.transport == dafydd.Transport.Tcp
            assert f"127.0.0.1:{port}" == match.address
        finally:
            server.close()

    def test_device_match_repr(self):
        """Test DeviceMatch repr via TCP discovery."""
        import socket
        import threading
        import time

        server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        server.bind(("127.0.0.1", 0))
        server.listen(1)
        port = server.getsockname()[1]

        def serve():
            try:
                conn, _ = server.accept()
                conn.sendall(b"OK")
                time.sleep(0.1)
                conn.close()
            except OSError:
                pass

        thread = threading.Thread(target=serve, daemon=True)
        thread.start()
        time.sleep(0.1)

        try:
            discovery = dafydd.TcpDiscovery(
                port=port,
                subnets=["127.0.0.1/32"],
                connect_timeout_ms=1000,
                io_timeout_ms=1000,
            )
            results = discovery.discover()
            assert len(results) == 1
            repr_str = repr(results[0])
            assert "Tcp" in repr_str
            assert f"127.0.0.1:{port}" in repr_str
        finally:
            server.close()

    def test_device_match_sorting(self):
        """Test DeviceMatch can be sorted via TCP discovery."""
        import socket
        import threading
        import time

        servers = []
        ports = []
        for i in range(2):
            server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            server.bind(("127.0.0.1", 0))
            server.listen(1)
            ports.append(server.getsockname()[1])
            servers.append(server)

        def serve(port):
            try:
                for s in servers:
                    if s.getsockname()[1] == port:
                        conn, _ = s.accept()
                        conn.sendall(b"OK")
                        time.sleep(0.1)
                        conn.close()
            except OSError:
                pass

        for port in ports:
            thread = threading.Thread(target=serve, args=(port,), daemon=True)
            thread.start()
        time.sleep(0.2)

        try:
            discovery = dafydd.TcpDiscovery(
                port=ports[0],
                subnets=["127.0.0.1/32"],
                connect_timeout_ms=1000,
                io_timeout_ms=1000,
            )
            results = discovery.discover()
            # Sort by address
            sorted_results = sorted(results)
            # Verify sorting works
            for i in range(len(sorted_results) - 1):
                assert sorted_results[i].address <= sorted_results[i + 1].address
        finally:
            for s in servers:
                s.close()

    def test_device_match_equality(self):
        """Test DeviceMatch equality based on all fields."""
        match1 = dafydd.DeviceMatch(
            transport=dafydd.Transport.Tcp,
            address="192.168.1.1:80",
            response=b"test",
            info={"key": "value"},
        )
        match2 = dafydd.DeviceMatch(
            transport=dafydd.Transport.Tcp,
            address="192.168.1.1:80",
            response=b"test",
            info={"key": "value"},
        )
        match3 = dafydd.DeviceMatch(
            transport=dafydd.Transport.Tcp,
            address="192.168.1.1:80",
            response=b"different",
            info={"key": "value"},
        )
        assert match1 == match2
        assert match1 != match3

    def test_device_match_hash(self):
        """Test DeviceMatch hash is consistent with equality."""
        match1 = dafydd.DeviceMatch(
            transport=dafydd.Transport.Tcp,
            address="192.168.1.1:80",
            response=b"test",
            info={"key": "value"},
        )
        match2 = dafydd.DeviceMatch(
            transport=dafydd.Transport.Tcp,
            address="192.168.1.1:80",
            response=b"test",
            info={"key": "value"},
        )
        assert hash(match1) == hash(match2)


class TestModuleImports:
    """Tests for module-level imports."""

    def test_all_exports(self):
        """Test that all expected exports are available."""
        assert hasattr(dafydd, "Transport")
        assert hasattr(dafydd, "DeviceMatch")
        assert hasattr(dafydd, "SerialDiscovery")
        assert hasattr(dafydd, "UsbDiscovery")
        assert hasattr(dafydd, "TcpDiscovery")

    def test_all_in_all(self):
        """Test __all__ contains all expected items."""
        expected = [
            "DeviceMatch",
            "SerialDiscovery",
            "TcpDiscovery",
            "Transport",
            "UsbDiscovery",
        ]
        for name in expected:
            assert name in dafydd.__all__


class TestDiscoveryClasses:
    """Tests that all discovery classes can be instantiated."""

    def test_serial_discovery_instantiation(self):
        """Test SerialDiscovery can be instantiated."""
        disc = dafydd.SerialDiscovery(b"test", [9600])
        assert disc is not None

    def test_usb_discovery_instantiation(self):
        """Test UsbDiscovery can be instantiated."""
        disc = dafydd.UsbDiscovery()
        assert disc is not None

        disc_with_filter = dafydd.UsbDiscovery(vid=0x1234, pid=0x5678)
        assert disc_with_filter is not None

    def test_tcp_discovery_instantiation(self):
        """Test TcpDiscovery can be instantiated."""
        disc = dafydd.TcpDiscovery(port=9000)
        assert disc is not None

        # Note: expected_response may not be exposed as Python param
        disc_with_options = dafydd.TcpDiscovery(
            port=9000,
            subnets=["192.168.1.0/24"],
            probe_command=b"STATUS",
            connect_timeout_ms=1000,
            io_timeout_ms=1000,
            max_concurrent=100,
            preferred_host="192.168.1.1",
        )
        assert disc_with_options is not None
