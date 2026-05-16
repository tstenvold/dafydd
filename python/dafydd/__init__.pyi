from typing import final

@final
class Transport:
    Serial: Transport
    Usb: Transport
    Tcp: Transport
    def __eq__(self, other: object) -> bool: ...
    def __hash__(self) -> int: ...

@final
class DeviceMatch:
    transport: Transport
    address: str
    response: bytes | None
    info: dict[str, str]
    def __repr__(self) -> str: ...
    def __eq__(self, other: object) -> bool: ...
    def __hash__(self) -> int: ...
    def __lt__(self, other: DeviceMatch) -> bool: ...

@final
class SerialDiscovery:
    def __init__(
        self,
        probe_command: bytes,
        baud_rates: list[int],
        timeout_ms: int = 500,
        preferred_port: str | None = None,
        include_bluetooth: bool = False,
    ) -> None: ...
    def discover(self) -> list[DeviceMatch]: ...

@final
class UsbDiscovery:
    def __init__(
        self,
        vid: int | None = None,
        pid: int | None = None,
    ) -> None: ...
    def discover(self) -> list[DeviceMatch]: ...

@final
class TcpDiscovery:
    def __init__(
        self,
        port: int,
        subnets: list[str] = ...,
        probe_command: bytes | None = None,
        timeout_ms: int = 200,
        max_concurrent: int = 500,
        preferred_host: str | None = None,
    ) -> None: ...
    def discover(self) -> list[DeviceMatch]: ...
