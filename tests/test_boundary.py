"""Python boundary smoke tests for the dafydd module.

Verifies the Python-visible API hasn't drifted (no AttributeError / TypeError
on basic usage). Does NOT require a network or real devices.
"""

import pytest

dafydd = pytest.importorskip("dafydd")


class TestBoundarySignatures:
    def test_cancellation_token(self):
        tok = dafydd.CancellationToken()
        tok.cancel()
        assert tok.is_cancelled()
        tok.reset()
        assert not tok.is_cancelled()

    def test_transport_enum(self):
        assert dafydd.Transport.Serial is not None
        assert dafydd.Transport.Usb is not None
        assert dafydd.Transport.Tcp is not None

    def test_device_match_construction_and_getters(self):
        m = dafydd.DeviceMatch(
            transport=dafydd.Transport.Tcp,
            address="10.0.0.1:502",
            response=b"hello",
            info={"source": "test"},
        )
        assert m.transport == dafydd.Transport.Tcp
        assert m.address == "10.0.0.1:502"
        assert m.response == b"hello"
        assert m.info == {"source": "test"}
        assert m.host == "10.0.0.1"
        assert m.port == 502
        assert m.vendor_id is None
        assert m.product_id is None
        assert not m.is_hid
        assert isinstance(repr(m), str)
        assert m == m
        assert hash(m) == hash(m)

    def test_device_match_usb_getters(self):
        m = dafydd.DeviceMatch(
            transport=dafydd.Transport.Usb,
            address="0x04d8:0x00dd",
            info={"device_class": "3"},
        )
        assert m.vendor_id == 0x04D8
        assert m.product_id == 0x00DD
        assert m.is_hid
        assert m.host is None
        assert m.port is None

    def test_serial_discovery_instantiation(self):
        d = dafydd.SerialDiscovery()
        assert d is not None

    def test_serial_discovery_with_probe(self):
        tok = dafydd.CancellationToken()
        d = dafydd.SerialDiscovery(
            probe_command=b"*IDN?\r\n",
            baud_rates=[9600, 115200],
            timeout_ms=100,
            cancellation_token=tok,
        )
        assert d is not None

    def test_usb_discovery_instantiation(self):
        d = dafydd.UsbDiscovery()
        assert d is not None
        d2 = dafydd.UsbDiscovery(vid=0x1234, pid=0x5678)
        assert d2 is not None

    def test_tcp_discovery_instantiation(self):
        d = dafydd.TcpDiscovery(port=502)
        assert d is not None

    def test_tcp_discovery_with_all_args(self):
        tok = dafydd.CancellationToken()
        d = dafydd.TcpDiscovery(
            port=502,
            ports=[502, 8080],
            subnets=["192.168.1.0/24"],
            probe_command=b"\x00\x01",
            connect_timeout_ms=50,
            io_timeout_ms=500,
            max_concurrent=100,
            preferred_host="192.168.1.1",
            preferred_retry=1,
            preferred_retry_delay_ms=200,
            use_arp_cache=True,
            use_mdns=False,
            mdns_timeout_ms=500,
            use_ssdp=False,
            ssdp_timeout_ms=500,
            response_filter=b"OK",
            cancellation_token=tok,
            subnet_prefix=24,
            tcp_linger_seconds=None,
        )
        assert d is not None

    def test_local_subnets(self):
        subnets = dafydd.local_subnets()
        assert isinstance(subnets, list)
        for s in subnets:
            assert isinstance(s, str)
            assert "/" in s

    def test_local_subnets_max_prefix(self):
        subnets = dafydd.local_subnets(max_prefix=24)
        assert isinstance(subnets, list)
        with pytest.raises((ValueError, TypeError)):
            dafydd.local_subnets(max_prefix=15)

    def test_tcp_discovery_invalid_subnet_prefix(self):
        with pytest.raises(ValueError):
            dafydd.TcpDiscovery(port=502, subnet_prefix=10)

    def test_device_match_bus_params_tcp(self):
        m = dafydd.DeviceMatch(
            transport=dafydd.Transport.Tcp,
            address="10.0.0.1:502",
        )
        params = m.bus_params()
        assert params["host"] == "10.0.0.1"
        assert params["port"] == 502

    def test_device_match_bus_params_serial(self):
        m = dafydd.DeviceMatch(
            transport=dafydd.Transport.Serial,
            address="/dev/ttyUSB0",
            info={"baud_rate": "9600"},
        )
        params = m.bus_params()
        assert params["port"] == "/dev/ttyUSB0"
        assert params["baudrate"] == 9600
