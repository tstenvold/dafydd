from typing import final

__all__ = [
    "DeviceMatch",
    "SerialDiscovery",
    "TcpDiscovery",
    "Transport",
    "UsbDiscovery",
]

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
      address: The device address (port name, VID:PID, or IP:port).
      response: The response bytes from the probe command (if any).
      info: Metadata dict with transport-specific fields (e.g., baud_rate, manufacturer).
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

@final
class SerialDiscovery:
    """Discover devices by probing serial ports with a command and watching for responses."""

    def __init__(
        self,
        probe_command: bytes,
        baud_rates: list[int],
        timeout_ms: int = 500,
        preferred_port: str | None = None,
        include_bluetooth: bool = False,
    ) -> None:
        """Initialize a serial device discoverer.

        Args:
          probe_command: Bytes to send to each port (e.g., b'*IDN?\r\n').
          baud_rates: List of baud rates to sweep (e.g., [9600, 115200]).
          timeout_ms: Timeout per port and baud rate in milliseconds.
          preferred_port: Port to try first before sweeping all ports.
          include_bluetooth: Include Bluetooth SPP ports (default: False on Windows).
        """
        ...
    def discover(self) -> list[DeviceMatch]:
        """Probe all serial ports and return matching devices."""
        ...

@final
class UsbDiscovery:
    """Discover USB devices by vendor and product ID."""

    def __init__(
        self,
        vid: int | None = None,
        pid: int | None = None,
    ) -> None:
        """Initialize a USB device discoverer.

        Args:
          vid: Vendor ID to filter by (optional; None matches all vendors).
          pid: Product ID to filter by (optional; None matches all products).
        """
        ...
    def discover(self) -> list[DeviceMatch]:
        """Enumerate USB devices and return matching devices."""
        ...

@final
class TcpDiscovery:
    """Discover TCP devices by scanning subnets and probing ports."""

    def __init__(
        self,
        port: int,
        subnets: list[str] = ...,
        probe_command: bytes | None = None,
        timeout_ms: int = 200,
        max_concurrent: int = 500,
        preferred_host: str | None = None,
    ) -> None:
        """Initialize a TCP device discoverer.

        Args:
          port: Port number to scan on all hosts.
          subnets: CIDR subnets to scan (default: auto-detect local subnets).
          probe_command: Bytes to send on connect; match response if provided.
          timeout_ms: Timeout per connection attempt.
          max_concurrent: Max concurrent connections (semaphore limit).
          preferred_host: IP to try first before scanning subnets.
        """
        ...
    def discover(self) -> list[DeviceMatch]:
        """Scan subnets and return devices that respond."""
        ...
