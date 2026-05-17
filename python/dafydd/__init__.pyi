from typing import Callable, final

__all__ = [
    "CancellationToken",
    "CorrelatedDevice",
    "DeviceMatch",
    "SerialDiscovery",
    "TcpDiscovery",
    "Transport",
    "UsbDiscovery",
    "correlate_usb_serial",
    "partition_by_transport",
]

@final
class CancellationToken:
    """Token to cancel an in-progress discovery operation."""

    def __init__(self) -> None: ...
    def cancel(self) -> None: ...
    def is_cancelled(self) -> bool: ...
    def reset(self) -> None: ...

@final
class Transport:
    """A transport layer for device discovery (Serial, USB, or TCP)."""

    Serial: Transport
    Usb: Transport
    Tcp: Transport
    def __eq__(self, other: object) -> bool: ...
    def __hash__(self) -> int: ...

@final
class DeviceMatch:
    """A device discovered on a transport.

    Attributes:
      transport: The transport layer (Serial, USB, or TCP).
      address: The device address (port name, ``VID:PID[:serial]``, or IP:port).
      response: The response bytes from the probe command (if any).
      info: Metadata dict with transport-specific fields.
        Serial — ``baud_rate``; ``data_bits`` (if not 8), ``parity``
          (``"even"`` or ``"odd"`` if not ``"none"``), ``stop_bits`` (if not 1).
        TCP — ``hostname`` (when known), ``source`` (``"arp_cache"`` or ``"mdns"``
          when found via those paths).
        USB — ``vendor_id``, ``product_id``, ``device_class``, ``manufacturer``,
          ``product``, ``serial_number``.
    """

    transport: Transport
    address: str
    response: bytes | None
    info: dict[str, str]
    def __init__(
        self,
        transport: Transport,
        address: str,
        response: bytes | None = None,
        info: dict[str, str] | None = None,
    ) -> None: ...
    def __repr__(self) -> str: ...
    def __eq__(self, other: object) -> bool: ...
    def __hash__(self) -> int: ...
    def __lt__(self, other: DeviceMatch) -> bool: ...
    @property
    def baud_rate(self) -> int | None:
        """For Serial transport: baud rate confirmed during discovery."""
        ...
    @property
    def host(self) -> str | None:
        """For TCP transport: hostname or IP part of the address."""
        ...
    @property
    def port(self) -> int | None:
        """For TCP transport: port number part of the address."""
        ...
    @property
    def vendor_id(self) -> int | None:
        """For USB transport: vendor ID as an integer."""
        ...
    @property
    def product_id(self) -> int | None:
        """For USB transport: product ID as an integer."""
        ...
    @property
    def is_hid(self) -> bool:
        """For USB transport: True if the device reports HID class (0x03) at the device level."""
        ...
    def bus_params(self) -> dict[str, str | int]:
        """Return keyword arguments for the matching python-bus factory function."""
        ...

@final
class CorrelatedDevice:
    """A USB and Serial match that represent the same physical device.

    Attributes:
      usb: The USB enumeration result.
      serial: The Serial port result for the same physical device.
    """

    usb: DeviceMatch
    serial: DeviceMatch
    def __repr__(self) -> str: ...

def correlate_usb_serial(
    usb_matches: list[DeviceMatch],
    serial_matches: list[DeviceMatch],
) -> list[CorrelatedDevice]:
    """Correlate USB and Serial matches by USB serial number.

    Returns pairs where the same physical device appears in both lists.
    Matches without a ``serial_number`` in their info dict are skipped.
    """
    ...

def partition_by_transport(
    matches: list[DeviceMatch],
) -> tuple[list[DeviceMatch], list[DeviceMatch], list[DeviceMatch]]:
    """Partition a flat list of matches by transport.

    Returns ``(serial_matches, usb_matches, tcp_matches)``.
    """
    ...

@final
class SerialDiscovery:
    """Discover devices by probing serial ports with a command and watching for responses."""

    def __init__(
        self,
        probe_command: bytes,
        baud_rates: list[int],
        timeout_ms: int = 500,
        preferred_port: str | None = None,
        preferred_retry: int = 0,
        preferred_retry_delay_ms: int = 500,
        include_bluetooth: bool = False,
        data_bits: int | None = None,
        parity: str | None = None,
        stop_bits: int | None = None,
        flow_control: str | None = None,
        response_terminator: bytes | None = None,
        cancellation_token: CancellationToken | None = None,
    ) -> None:
        """Initialize a serial device discoverer.

        Args:
          probe_command: Bytes to send to each port (e.g., ``b'*IDN?\\r\\n'``).
          baud_rates: Baud rates to sweep in order (e.g., ``[9600, 115200]``).
          timeout_ms: Per-port read/write timeout in milliseconds.
          preferred_port: Port to try first before sweeping all ports.
          preferred_retry: Retry preferred_port this many times before fallback.
          preferred_retry_delay_ms: Delay between preferred port retries (ms).
          include_bluetooth: Include Bluetooth SPP ports (default False on Windows).
          data_bits: Character width: 5, 6, 7, or 8 (default 8).
          parity: ``'none'``, ``'even'``, or ``'odd'`` (default ``'none'``).
          stop_bits: 1 or 2 (default 1).
          flow_control: ``'none'``, ``'hardware'``, or ``'software'`` (default ``'none'``).
          response_terminator: Exit the read loop early when the response ends
            with these bytes (e.g., ``b'\\r\\n'``). Without this, every probe
            waits the full ``timeout_ms``.
          cancellation_token: Token to cancel an in-progress sweep or watch.
        """
        ...
    def discover(self) -> list[DeviceMatch]:
        """Probe all serial ports and return matching devices."""
        ...
    def discover_streaming(
        self, callback: Callable[[DeviceMatch], None]
    ) -> list[DeviceMatch]:
        """Probe all serial ports, calling ``callback`` as each device is found."""
        ...
    def watch(
        self,
        on_added: Callable[[DeviceMatch], None],
        on_removed: Callable[[DeviceMatch], None],
        interval_ms: int | None = None,
    ) -> None:
        """Poll for serial port changes, calling callbacks as devices appear or disappear.

        Requires a ``cancellation_token`` to stop. Compares devices by address
        only, so dynamic probe responses do not cause spurious events.
        """
        ...

@final
class UsbDiscovery:
    """Discover USB devices by vendor and product ID."""

    def __init__(
        self,
        vid: int | None = None,
        pid: int | None = None,
        manufacturer: str | None = None,
        product_string: str | None = None,
        serial_number: str | None = None,
        cancellation_token: CancellationToken | None = None,
    ) -> None:
        """Initialize a USB device discoverer.

        Args:
          vid: Vendor ID to filter by (None matches all vendors).
          pid: Product ID to filter by (None matches all products).
          manufacturer: Substring filter on manufacturer string.
          product_string: Substring filter on product string.
          serial_number: Substring filter on USB serial number.
          cancellation_token: Token to cancel an in-progress watch.
        """
        ...
    def discover(self) -> list[DeviceMatch]:
        """Enumerate USB devices and return matching devices.

        When two devices share the same VID and PID, they are distinguished
        by serial number in the ``address`` field (``VID:PID:serial``).
        """
        ...
    def discover_streaming(
        self, callback: Callable[[DeviceMatch], None]
    ) -> list[DeviceMatch]:
        """Enumerate USB devices, calling ``callback`` for each match found."""
        ...
    def watch(
        self,
        on_added: Callable[[DeviceMatch], None],
        on_removed: Callable[[DeviceMatch], None],
        interval_ms: int | None = None,
    ) -> None:
        """Poll for USB device changes, calling callbacks as devices plug in or out.

        Requires a ``cancellation_token`` to stop.
        """
        ...

@final
class TcpDiscovery:
    """Discover TCP devices by scanning subnets and probing ports."""

    def __init__(
        self,
        port: int | None = None,
        ports: list[int] = ...,
        subnets: list[str] = ...,
        probe_command: bytes | None = None,
        connect_timeout_ms: int = 200,
        io_timeout_ms: int = 500,
        max_concurrent: int = 500,
        preferred_host: str | None = None,
        preferred_retry: int = 0,
        preferred_retry_delay_ms: int = 500,
        use_arp_cache: bool = True,
        use_mdns: bool = False,
        mdns_timeout_ms: int = 1000,
        cancellation_token: CancellationToken | None = None,
    ) -> None:
        """Initialize a TCP device discoverer.

        Args:
          port: Single TCP port to probe on each host.
          ports: Multiple TCP ports to probe per host (e.g. ``[8080, 502]``).
            At least one of ``port`` or ``ports`` must be set. Duplicates are
            removed while preserving order.
          subnets: CIDR subnets to scan (default: auto-detect local subnets).
          probe_command: Bytes to send on connect; only hosts that respond are
            returned. Omit to match any host that accepts a TCP connection.
          connect_timeout_ms: Timeout for the TCP handshake in milliseconds.
          io_timeout_ms: Timeout for the probe write and response read (ms).
          max_concurrent: Max simultaneous open connections (default 500).
          preferred_host: Hostname or IP to probe before scanning subnets.
          preferred_retry: Retry ``preferred_host`` this many times before
            falling back to a full sweep.
          preferred_retry_delay_ms: Delay between preferred host retries (ms).
          use_arp_cache: Probe ARP-cached hosts first (default True). These
            matches include ``info["source"] = "arp_cache"``.
          use_mdns: Listen for mDNS announcements before scanning (default
            False). These matches include ``info["source"] = "mdns"``.
          mdns_timeout_ms: Duration to listen for mDNS (ms, default 1000).
          cancellation_token: Token to cancel an in-progress sweep or watch.
        """
        ...
    def discover(self) -> list[DeviceMatch]:
        """Scan subnets and return devices that respond."""
        ...
    def discover_streaming(
        self, callback: Callable[[DeviceMatch], None]
    ) -> list[DeviceMatch]:
        """Scan subnets, calling ``callback`` as each device is found."""
        ...
    def watch(
        self,
        on_added: Callable[[DeviceMatch], None],
        on_removed: Callable[[DeviceMatch], None],
        interval_ms: int | None = None,
    ) -> None:
        """Poll for TCP device changes, calling callbacks as devices appear or disappear.

        Requires a ``cancellation_token`` to stop. Compares devices by address
        only, so dynamic probe responses do not cause spurious events.
        """
        ...
