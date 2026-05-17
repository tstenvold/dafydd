//! Passive packet capture for zero-probe device discovery.
//!
//! Requires the `pcap-capture` feature and libpcap (system package).

#[cfg(feature = "pcap-capture")]
mod inner {
    use std::{
        collections::HashSet,
        net::{IpAddr, Ipv4Addr, Ipv6Addr},
        time::{Duration, Instant},
    };

    pub(super) fn capture(duration: Duration) -> Vec<IpAddr> {
        let device = match pcap::Device::lookup() {
            Ok(Some(d)) => d,
            _ => return Vec::new(),
        };

        let mut cap = match pcap::Capture::from_device(device)
            .map(|c| c.promisc(true).snaplen(96).timeout(50))
            .and_then(|c| c.open())
        {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        if cap.filter("ip or ip6", true).is_err() {
            return Vec::new();
        }

        let deadline = Instant::now() + duration;
        let mut seen: HashSet<IpAddr> = HashSet::new();

        loop {
            if Instant::now() >= deadline {
                break;
            }
            match cap.next_packet() {
                Ok(packet) => {
                    if let Some(ip) = extract_src_ip(packet.data) {
                        if !is_boring(ip) {
                            seen.insert(ip);
                        }
                    }
                }
                Err(pcap::Error::TimeoutExpired) => continue,
                Err(_) => break,
            }
        }

        seen.into_iter().collect()
    }

    fn extract_src_ip(data: &[u8]) -> Option<IpAddr> {
        if data.len() < 34 {
            return None;
        }

        let eth_type = u16::from_be_bytes([data[12], data[13]]);

        match eth_type {
            0x0800 => {
                if data.len() < 30 {
                    return None;
                }
                Some(IpAddr::V4(Ipv4Addr::new(
                    data[26], data[27], data[28], data[29],
                )))
            }
            0x86DD => {
                if data.len() < 38 {
                    return None;
                }
                let mut bytes = [0u8; 16];
                bytes.copy_from_slice(&data[22..38]);
                Some(IpAddr::V6(Ipv6Addr::from(bytes)))
            }
            _ => None,
        }
    }

    fn is_boring(ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => v4.is_loopback() || v4.is_broadcast() || v4.is_multicast(),
            IpAddr::V6(v6) => v6.is_loopback() || v6.is_multicast(),
        }
    }
}

/// Capture packets on the default interface and collect source IPs.
///
/// Requires the `pcap-capture` feature and libpcap installed on the system.
/// Returns an empty vec when the feature is disabled or capture fails (e.g.
/// insufficient privileges).
#[must_use]
#[allow(clippy::missing_const_for_fn)] // conditional cfg blocks prevent const fn
pub fn passive_capture_hosts(duration: std::time::Duration) -> Vec<std::net::IpAddr> {
    #[cfg(feature = "pcap-capture")]
    {
        inner::capture(duration)
    }
    #[cfg(not(feature = "pcap-capture"))]
    {
        let _ = duration;
        Vec::new()
    }
}
