"""Test fixtures for cross-platform serial device simulation."""

from .platform import (
    Platform,
    Architecture,
    PlatformInfo,
    get_platform_info,
    is_linux,
    is_macos,
    is_windows,
    is_arm,
    is_ci,
    supports_virtual_serial_ports,
)
from .serial_simulator import (
    SerialDeviceConfig,
    SerialSimulator,
    UnixPTYSimulator,
    QEMUARMSerialSimulator,
    MockSerialSimulator,
    SerialSimulatorFactory,
    serial_device,
)

__all__ = [
    # Platform
    "Platform",
    "Architecture", 
    "PlatformInfo",
    "get_platform_info",
    "is_linux",
    "is_macos",
    "is_windows",
    "is_arm",
    "is_ci",
    "supports_virtual_serial_ports",
    # Serial Simulator
    "SerialDeviceConfig",
    "SerialSimulator",
    "UnixPTYSimulator",
    "QEMUARMSerialSimulator",
    "MockSerialSimulator",
    "SerialSimulatorFactory",
    "serial_device",
]