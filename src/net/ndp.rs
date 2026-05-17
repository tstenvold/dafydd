//! Kernel NDP neighbour cache reader — returns recently-seen IPv6 neighbours.

/// Return all IPv6 addresses currently in the kernel neighbour table with a
/// complete entry.
///
/// Only entries in states REACHABLE, STALE, DELAY, or PROBE are included.
/// FAILED and INCOMPLETE entries are excluded. The loopback address (`::1`) is
/// always excluded. Falls back gracefully to an empty `Vec` on any error.
#[must_use]
pub fn ndp_cache_hosts() -> Vec<std::net::Ipv6Addr> {
    os::ndp_cache_hosts()
}

#[cfg(target_os = "linux")]
mod os {
    use std::net::Ipv6Addr;

    const REACHABLE_STATES: &[&str] = &["REACHABLE", "STALE", "DELAY", "PROBE"];

    pub(super) fn ndp_cache_hosts() -> Vec<Ipv6Addr> {
        let Ok(output) = std::process::Command::new("ip")
            .args(["-6", "neigh", "show"])
            .output()
        else {
            return Vec::new();
        };

        let Ok(stdout) = std::str::from_utf8(&output.stdout) else {
            return Vec::new();
        };

        stdout
            .lines()
            .filter_map(|line| {
                let ip: Ipv6Addr = line.split_ascii_whitespace().next()?.parse().ok()?;
                if ip == Ipv6Addr::LOCALHOST {
                    return None;
                }
                // State is the last whitespace-delimited token on the line.
                let state = line.split_ascii_whitespace().next_back()?;
                if !REACHABLE_STATES.contains(&state) {
                    return None;
                }
                Some(ip)
            })
            .collect()
    }
}

#[cfg(target_os = "macos")]
mod os {
    use std::net::Ipv6Addr;

    pub(super) fn ndp_cache_hosts() -> Vec<Ipv6Addr> {
        let Ok(output) = std::process::Command::new("ndp").arg("-an").output() else {
            return Vec::new();
        };

        let Ok(stdout) = std::str::from_utf8(&output.stdout) else {
            return Vec::new();
        };

        stdout
            .lines()
            .skip(1) // header row
            .filter(|line| !line.contains("(incomplete)"))
            .filter_map(|line| {
                // First column may carry a scope suffix: fe80::1%en0
                let raw = line.split_ascii_whitespace().next()?;
                let addr_str = raw.split('%').next()?;
                let ip: Ipv6Addr = addr_str.parse().ok()?;
                if ip == Ipv6Addr::LOCALHOST {
                    return None;
                }
                Some(ip)
            })
            .collect()
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod os {
    use std::net::Ipv6Addr;

    pub(super) fn ndp_cache_hosts() -> Vec<Ipv6Addr> {
        Vec::new()
    }
}
