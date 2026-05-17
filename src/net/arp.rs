//! Kernel ARP cache reader — returns recently-seen IPv4 neighbours.

/// Return all IPv4 addresses currently in the kernel ARP cache with a complete
/// entry.
///
/// Incomplete entries (no MAC seen yet) and permanent stubs are excluded.
/// Loopback addresses are always excluded. Falls back gracefully to an empty
/// `Vec` on any error — permission denied, missing binary, parse failure, etc.
#[must_use]
pub fn arp_cache_hosts() -> Vec<std::net::Ipv4Addr> {
    os::arp_cache_hosts()
}

#[cfg(target_os = "linux")]
mod os {
    use std::net::Ipv4Addr;

    const fn is_loopback(ip: Ipv4Addr) -> bool {
        ip.octets()[0] == 127
    }

    pub(super) fn arp_cache_hosts() -> Vec<Ipv4Addr> {
        let Ok(content) = std::fs::read_to_string("/proc/net/arp") else {
            return Vec::new();
        };

        content
            .lines()
            .skip(1)
            .filter_map(|line| {
                let mut cols = line.split_ascii_whitespace();
                let ip_str = cols.next()?;
                let _hw_type = cols.next()?;
                let flags_str = cols.next()?;

                let ip: Ipv4Addr = ip_str.parse().ok()?;
                if is_loopback(ip) {
                    return None;
                }

                // 0x0 means the entry is incomplete — no response was received.
                let flags = u8::from_str_radix(flags_str.trim_start_matches("0x"), 16).ok()?;
                if flags == 0 {
                    return None;
                }

                Some(ip)
            })
            .collect()
    }
}

#[cfg(target_os = "macos")]
mod os {
    use std::net::Ipv4Addr;

    const fn is_loopback(ip: Ipv4Addr) -> bool {
        ip.octets()[0] == 127
    }

    pub(super) fn arp_cache_hosts() -> Vec<Ipv4Addr> {
        let Ok(output) = std::process::Command::new("arp").arg("-an").output() else {
            return Vec::new();
        };

        let Ok(stdout) = std::str::from_utf8(&output.stdout) else {
            return Vec::new();
        };

        stdout
            .lines()
            .filter(|line| !line.contains("(incomplete)"))
            .filter_map(|line| {
                // Format: ? (192.168.1.1) at aa:bb:cc:dd:ee:ff on en0 ...
                let start = line.find('(')? + 1;
                let end = line.find(')')?;
                let ip: Ipv4Addr = line[start..end].parse().ok()?;
                if is_loopback(ip) {
                    return None;
                }
                Some(ip)
            })
            .collect()
    }
}

#[cfg(target_os = "windows")]
mod os {
    use std::net::Ipv4Addr;

    pub(super) fn arp_cache_hosts() -> Vec<Ipv4Addr> {
        let Ok(output) = std::process::Command::new("arp").arg("-a").output() else {
            return Vec::new();
        };

        let Ok(stdout) = std::str::from_utf8(&output.stdout) else {
            return Vec::new();
        };

        stdout
            .lines()
            .filter_map(|line| {
                let mut cols = line.split_ascii_whitespace();
                let ip: Ipv4Addr = cols.next()?.parse().ok()?;
                let _physical = cols.next()?;
                let kind = cols.next()?;

                // Exclude broadcast and non-dynamic entries (static = manually configured stub).
                if kind != "dynamic" {
                    return None;
                }
                // Last-octet 255 is the subnet broadcast address.
                if ip.octets()[3] == 255 {
                    return None;
                }
                Some(ip)
            })
            .collect()
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod os {
    use std::net::Ipv4Addr;

    pub(super) fn arp_cache_hosts() -> Vec<Ipv4Addr> {
        Vec::new()
    }
}
