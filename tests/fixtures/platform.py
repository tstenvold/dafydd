"""Platform detection and identification utilities."""

import os
import platform
from dataclasses import dataclass
from enum import Enum


class Platform(Enum):
    """Supported platforms for testing."""

    LINUX = "linux"
    MACOS = "macos"
    WINDOWS = "windows"
    ARM_QEMU = "arm_qemu"
    UNKNOWN = "unknown"


class Architecture(Enum):
    """CPU architectures."""

    X86_64 = "x86_64"
    AARCH64 = "aarch64"
    ARM = "arm"
    X86 = "x86"
    UNKNOWN = "unknown"


@dataclass
class PlatformInfo:
    """Complete platform information."""

    platform: Platform
    architecture: Architecture
    os_name: str
    os_version: str
    machine: str
    is_ci: bool
    has_socat: bool
    has_virtual_ports: bool


def detect_platform() -> PlatformInfo:
    """Detect the current platform and its capabilities."""
    system = platform.system().lower()
    machine = platform.machine().lower()
    is_ci = os.environ.get("CI", "").lower() == "true"

    # Detect architecture
    if machine in ("x86_64", "amd64"):
        arch = Architecture.X86_64
    elif machine in ("aarch64", "arm64"):
        # Could be real ARM (Apple Silicon, Raspberry Pi) or QEMU
        # Check if we're in a QEMU environment
        if _is_qemu_environment():
            arch = Architecture.ARM
        else:
            arch = Architecture.AARCH64
    elif machine in ("i386", "i686", "x86"):
        arch = Architecture.X86
    elif machine.startswith("arm"):
        arch = Architecture.ARM
    else:
        arch = Architecture.UNKNOWN

    # Detect OS
    if system == "darwin":
        os_platform = Platform.MACOS
    elif system == "linux":
        # Check if it's ARM QEMU
        if arch == Architecture.ARM or _is_qemu_environment():
            os_platform = Platform.ARM_QEMU
        else:
            os_platform = Platform.LINUX
    elif system == "windows":
        os_platform = Platform.WINDOWS
    else:
        os_platform = Platform.UNKNOWN

    # Check for available tools
    has_socat = _check_command_exists("socat")
    has_virtual_ports = _check_virtual_port_support(os_platform)

    # Get OS version
    if system == "darwin":
        os_version = platform.mac_ver()[0] or "unknown"
    elif system == "linux":
        os_version = platform.release() or "unknown"
    else:
        os_version = "unknown"

    return PlatformInfo(
        platform=os_platform,
        architecture=arch,
        os_name=system,
        os_version=os_version,
        machine=machine,
        is_ci=is_ci,
        has_socat=has_socat,
        has_virtual_ports=has_virtual_ports,
    )


def _is_qemu_environment() -> bool:
    """Check if running in a QEMU environment."""
    # Check CPU info for QEMU signatures
    try:
        with open("/proc/cpuinfo", "r") as f:
            cpuinfo = f.read().lower()
            if "qemu" in cpuinfo or "kvm" in cpuinfo:
                return True
    except (FileNotFoundError, PermissionError):
        pass

    # Check for QEMU environment variables
    if os.environ.get("QEMU_ROOT") or os.environ.get("RUNNING_IN_QEMU"):
        return True

    return False


def _check_command_exists(cmd: str) -> bool:
    """Check if a command exists in PATH."""
    import shutil

    return shutil.which(cmd) is not None


def _check_virtual_port_support(platform: Platform) -> bool:
    """Check if virtual serial ports are supported."""
    if platform == Platform.WINDOWS:
        return False  # Requires additional software
    elif platform in (Platform.LINUX, Platform.MACOS, Platform.ARM_QEMU):
        return True  # Can use PTY/socat
    return False


# Global singleton for platform info
_platform_info: PlatformInfo | None = None


def get_platform_info() -> PlatformInfo:
    """Get cached platform information."""
    global _platform_info
    if _platform_info is None:
        _platform_info = detect_platform()
    return _platform_info


def is_linux() -> bool:
    """Check if running on Linux."""
    return get_platform_info().platform == Platform.LINUX


def is_macos() -> bool:
    """Check if running on macOS."""
    return get_platform_info().platform == Platform.MACOS


def is_windows() -> bool:
    """Check if running on Windows."""
    return get_platform_info().platform == Platform.WINDOWS


def is_arm() -> bool:
    """Check if running on ARM (real or QEMU)."""
    return get_platform_info().platform == Platform.ARM_QEMU


def is_ci() -> bool:
    """Check if running in CI environment."""
    return get_platform_info().is_ci


def supports_virtual_serial_ports() -> bool:
    """Check if virtual serial ports are supported on this platform."""
    return get_platform_info().has_virtual_ports
