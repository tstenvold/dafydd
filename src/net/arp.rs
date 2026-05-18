//! Kernel ARP cache reader — returns recently-seen IPv4 neighbours with MAC.
//!
//! All three platform paths are pure-Rust: Linux parses `/proc/net/arp`,
//! macOS walks the routing table via `sysctl(NET_RT_FLAGS)`, and Windows
//! calls `GetIpNetTable2`. No subprocesses are invoked.

use std::net::Ipv4Addr;

/// Link-layer hardware address (Ethernet MAC, 6 bytes).
pub type MacAddr = [u8; 6];

/// Return all (IPv4, MAC) pairs currently in the kernel ARP cache that are
/// usable as unicast scan targets.
///
/// Filtered out: incomplete entries (no MAC), loopback, multicast (`224/4`),
/// broadcast MAC (`ff:ff:ff:ff:ff:ff`), and the directed-broadcast IPv4 form
/// `*.*.*.255`. Falls back gracefully to an empty `Vec` on any error —
/// permission denied, missing table, parse failure, etc.
#[must_use]
pub fn arp_cache_hosts() -> Vec<(Ipv4Addr, MacAddr)> {
    os::arp_cache_hosts()
        .into_iter()
        .filter(|&(ip, mac)| is_unicast_target(ip, mac))
        .collect()
}

/// True iff `(ip, mac)` names a single physical device worth scanning.
const fn is_unicast_target(ip: Ipv4Addr, mac: MacAddr) -> bool {
    if is_loopback(ip) || ip.is_multicast() {
        return false;
    }
    if ip.octets()[3] == 255 {
        return false;
    }
    let mac_is_broadcast = mac[0] == 0xff
        && mac[1] == 0xff
        && mac[2] == 0xff
        && mac[3] == 0xff
        && mac[4] == 0xff
        && mac[5] == 0xff;
    if mac_is_broadcast {
        return false;
    }
    // First-byte LSB set = multicast MAC (covers IPv4 multicast 01:00:5e:*).
    if mac[0] & 0x01 != 0 {
        return false;
    }
    true
}

/// Format a MAC as `"aa:bb:cc:dd:ee:ff"` (lower-case, colon-separated).
#[must_use]
pub fn format_mac(mac: &MacAddr) -> String {
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

const fn is_loopback(ip: Ipv4Addr) -> bool {
    ip.octets()[0] == 127
}

// ── Linux: /proc/net/arp ─────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
mod os {
    use super::{is_loopback, parse_mac_colons, Ipv4Addr, MacAddr};

    pub(super) fn arp_cache_hosts() -> Vec<(Ipv4Addr, MacAddr)> {
        let Ok(content) = std::fs::read_to_string("/proc/net/arp") else {
            return Vec::new();
        };
        parse_proc_arp(&content)
    }

    pub(super) fn parse_proc_arp(content: &str) -> Vec<(Ipv4Addr, MacAddr)> {
        content
            .lines()
            .skip(1)
            .filter_map(|line| {
                let mut cols = line.split_ascii_whitespace();
                let ip_str = cols.next()?;
                let _hw_type = cols.next()?;
                let flags_str = cols.next()?;
                let mac_str = cols.next()?;

                let ip: Ipv4Addr = ip_str.parse().ok()?;
                if is_loopback(ip) {
                    return None;
                }

                // Flag 0x0 = incomplete entry (no response received).
                let flags = u8::from_str_radix(flags_str.trim_start_matches("0x"), 16).ok()?;
                if flags == 0 {
                    return None;
                }

                let mac = parse_mac_colons(mac_str)?;
                if mac == [0u8; 6] {
                    return None;
                }
                Some((ip, mac))
            })
            .collect()
    }
}

// ── macOS: sysctl(NET_RT_FLAGS, RTF_LLINFO) ──────────────────────────────────

#[cfg(target_os = "macos")]
mod os {
    use super::{is_loopback, Ipv4Addr, MacAddr};
    use libc::{
        c_int, c_void, sockaddr_dl, sockaddr_in, sysctl, AF_INET, AF_LINK, CTL_NET, NET_RT_FLAGS,
        PF_ROUTE, RTF_LLINFO,
    };
    use std::mem::size_of;

    /// Round a `sockaddr` `sa_len` up to the next pointer alignment, matching
    /// BSD's `SA_SIZE` / `ROUNDUP` macro. Used to step from one sockaddr to
    /// the next in routing-message payloads.
    const fn sa_size(sa_len: u8) -> usize {
        if sa_len == 0 {
            size_of::<u32>()
        } else {
            let len = sa_len as usize;
            1 + ((len - 1) | (size_of::<u32>() - 1))
        }
    }

    /// `rt_msghdr` prefix that we actually read — the message length and the
    /// fixed-offset fields. We only need `rtm_msglen` and `rtm_addrs`.
    #[repr(C)]
    struct RtMsgHdrHead {
        rtm_msglen: u16,
        rtm_version: u8,
        rtm_type: u8,
        rtm_index: u16,
        _pad: u16,
        rtm_flags: c_int,
        rtm_addrs: c_int,
        // ... more fields follow, we don't read them
    }

    pub(super) fn arp_cache_hosts() -> Vec<(Ipv4Addr, MacAddr)> {
        // sysctl args: {CTL_NET, PF_ROUTE, 0, AF_INET, NET_RT_FLAGS, RTF_LLINFO}
        let mut mib: [c_int; 6] = [CTL_NET, PF_ROUTE, 0, AF_INET, NET_RT_FLAGS, RTF_LLINFO];
        let mut needed: libc::size_t = 0;

        // First call: query the required buffer size.
        // SAFETY: sysctl is being called with a valid mib slice; oldp=NULL and
        // newp=NULL ask the kernel to fill `needed` with the buffer length.
        let rc = unsafe {
            sysctl(
                mib.as_mut_ptr(),
                6,
                std::ptr::null_mut(),
                &raw mut needed,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 || needed == 0 {
            return Vec::new();
        }

        let mut buf: Vec<u8> = vec![0; needed];
        // Second call: actually fetch the routing-message payload.
        // SAFETY: buffer is sized exactly to the kernel's reported `needed`.
        let rc = unsafe {
            sysctl(
                mib.as_mut_ptr(),
                6,
                buf.as_mut_ptr().cast::<c_void>(),
                &raw mut needed,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 {
            return Vec::new();
        }
        buf.truncate(needed);

        parse_rt_buffer(&buf)
    }

    /// Walk the `NET_RT_FLAGS` payload and extract every `(sockaddr_in,
    /// sockaddr_dl)` pair into `(Ipv4Addr, MacAddr)`.
    ///
    /// All struct reads use `ptr::read_unaligned` because the routing-message
    /// buffer is `Vec<u8>`-aligned (i.e. 1 byte), not the natural alignment of
    /// the C structs we are decoding.
    fn parse_rt_buffer(buf: &[u8]) -> Vec<(Ipv4Addr, MacAddr)> {
        let mut out: Vec<(Ipv4Addr, MacAddr)> = Vec::new();
        let mut offset: usize = 0;

        while offset + size_of::<RtMsgHdrHead>() <= buf.len() {
            // SAFETY: bounds-checked above; read_unaligned tolerates byte
            // alignment, and `RtMsgHdrHead` is `#[repr(C)]` with no padding
            // before rtm_msglen.
            let hdr: RtMsgHdrHead =
                unsafe { std::ptr::read_unaligned(buf.as_ptr().add(offset).cast()) };
            let msg_len = hdr.rtm_msglen as usize;
            if msg_len < size_of::<RtMsgHdrHead>() || offset + msg_len > buf.len() {
                break;
            }

            if let Some(entry) = parse_one_msg(&buf[offset..offset + msg_len]) {
                let (ip, mac) = entry;
                if !is_loopback(ip) && mac != [0u8; 6] {
                    out.push((ip, mac));
                }
            }
            offset += msg_len;
        }

        out
    }

    /// Parse a single routing message: header, then sockaddrs starting at the
    /// end of the full header. We read the first `sockaddr_in` (destination
    /// IPv4) and the *next* sockaddr after it, which is the `sockaddr_dl`
    /// gateway containing the MAC. Returns `None` if the format is unexpected.
    fn parse_one_msg(msg: &[u8]) -> Option<(Ipv4Addr, MacAddr)> {
        // Step past the *full* header. libc::rt_msghdr is authoritative for
        // size on the current platform/architecture.
        let hdr_size = size_of::<libc::rt_msghdr>();
        if msg.len() <= hdr_size {
            return None;
        }
        let mut p = hdr_size;

        // First sockaddr: AF_INET destination (sockaddr_in or sockaddr_inarp).
        if p + size_of::<sockaddr_in>() > msg.len() {
            return None;
        }
        // SAFETY: bounds-checked; read_unaligned tolerates byte alignment.
        let sin: sockaddr_in = unsafe { std::ptr::read_unaligned(msg.as_ptr().add(p).cast()) };
        if c_int::from(sin.sin_family) != AF_INET {
            return None;
        }
        // sin_addr is in network byte order; preserve the bytes.
        let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));

        // `sockaddr_in` is fixed-width 16 bytes; truncation of the constant
        // size to u8 is exact.
        #[allow(clippy::cast_possible_truncation)]
        let sin_len = if sin.sin_len == 0 {
            size_of::<sockaddr_in>() as u8
        } else {
            sin.sin_len
        };
        p += sa_size(sin_len);

        // Second sockaddr: AF_LINK gateway (sockaddr_dl) containing MAC.
        if p + size_of::<sockaddr_dl>() > msg.len() {
            return None;
        }
        // SAFETY: bounds-checked; read_unaligned tolerates byte alignment.
        let sdl: sockaddr_dl = unsafe { std::ptr::read_unaligned(msg.as_ptr().add(p).cast()) };
        if c_int::from(sdl.sdl_family) != AF_LINK {
            return None;
        }
        if sdl.sdl_alen != 6 {
            return None;
        }

        // MAC bytes start at sdl_data[sdl_nlen..sdl_nlen+6]. sdl_data is a
        // flexible array; we index back into the raw `msg` buffer rather than
        // into the (truncated) struct copy.
        let sdl_data_offset = p + size_of::<sockaddr_dl>() - sdl.sdl_data.len();
        let mac_start = sdl_data_offset + sdl.sdl_nlen as usize;
        let mac_end = mac_start + 6;
        if mac_end > msg.len() {
            return None;
        }
        let mut mac = [0u8; 6];
        // c_char on macOS is signed; reinterpret as u8 byte-for-byte.
        for (i, b) in msg[mac_start..mac_end].iter().enumerate() {
            mac[i] = *b;
        }
        Some((ip, mac))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_sa_size_alignment() {
            // BSD SA_SIZE rounds up to sizeof(long) = 4 on 32-bit, 8 on 64-bit;
            // sockaddr structs in routing messages are always aligned to 4.
            assert_eq!(sa_size(0), 4);
            assert_eq!(sa_size(1), 4);
            assert_eq!(sa_size(4), 4);
            assert_eq!(sa_size(5), 8);
            assert_eq!(sa_size(16), 16);
            assert_eq!(sa_size(20), 20);
        }

        #[test]
        fn test_arp_cache_hosts_returns_without_panic() {
            // Smoke: just confirm the sysctl path doesn't crash. The cache
            // may be empty on a CI runner without recent traffic — that's OK.
            let _entries = arp_cache_hosts();
        }
    }
}

// ── Windows: GetIpNetTable2 ───────────────────────────────────────────────────

#[cfg(target_os = "windows")]
mod os {
    use super::{is_loopback, Ipv4Addr, MacAddr};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        FreeMibTable, GetIpNetTable2, MIB_IPNET_TABLE2,
    };
    use windows_sys::Win32::Networking::WinSock::AF_INET;

    // Neighbor states from netioapi.h that mean "MAC is valid" — anything
    // other than Unreachable / Incomplete is usable for priority probing.
    const NL_NS_INCOMPLETE: i32 = 1;
    const NL_NS_UNREACHABLE: i32 = 6;

    pub(super) fn arp_cache_hosts() -> Vec<(Ipv4Addr, MacAddr)> {
        let mut table: *mut MIB_IPNET_TABLE2 = std::ptr::null_mut();
        // SAFETY: GetIpNetTable2 allocates a table the caller frees with
        // FreeMibTable. AF_INET filters to IPv4 only.
        let rc = unsafe { GetIpNetTable2(AF_INET, &raw mut table) };
        if rc != 0 || table.is_null() {
            return Vec::new();
        }

        // SAFETY: GetIpNetTable2 succeeded → table points to a valid
        // MIB_IPNET_TABLE2 with NumEntries valid rows.
        let entries = unsafe { parse_table(table) };

        // SAFETY: pairs with the successful allocation above.
        unsafe { FreeMibTable(table.cast()) };

        entries
    }

    /// Walk the table rows and extract reachable IPv4 + 6-byte MAC pairs.
    ///
    /// # Safety
    /// `table` must point to a valid `MIB_IPNET_TABLE2` returned by
    /// `GetIpNetTable2` with at least `NumEntries` rows.
    unsafe fn parse_table(table: *const MIB_IPNET_TABLE2) -> Vec<(Ipv4Addr, MacAddr)> {
        let count = (*table).NumEntries as usize;
        let mut out = Vec::with_capacity(count);

        let rows = std::ptr::addr_of!((*table).Table)
            .cast::<windows_sys::Win32::NetworkManagement::IpHelper::MIB_IPNET_ROW2>();

        for i in 0..count {
            let row = &*rows.add(i);
            if row.State == NL_NS_INCOMPLETE || row.State == NL_NS_UNREACHABLE {
                continue;
            }
            if row.PhysicalAddressLength != 6 {
                continue;
            }
            // row.Address is SOCKADDR_INET (union of v4/v6). Filter by family.
            let family = row.Address.si_family;
            if family != AF_INET {
                continue;
            }
            // SAFETY: si_family == AF_INET, so the Ipv4 variant is the active one.
            let octets = row.Address.Ipv4.sin_addr.S_un.S_un_b;
            let ip = Ipv4Addr::new(octets.s_b1, octets.s_b2, octets.s_b3, octets.s_b4);
            if is_loopback(ip) {
                continue;
            }
            let mut mac = [0u8; 6];
            mac.copy_from_slice(&row.PhysicalAddress[..6]);
            if mac == [0u8; 6] {
                continue;
            }
            out.push((ip, mac));
        }
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_arp_cache_hosts_returns_without_panic() {
            let _entries = arp_cache_hosts();
        }
    }
}

// ── Other OS stub ────────────────────────────────────────────────────────────

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod os {
    use super::{Ipv4Addr, MacAddr};

    pub(super) fn arp_cache_hosts() -> Vec<(Ipv4Addr, MacAddr)> {
        Vec::new()
    }
}

// ── Shared helpers / tests ───────────────────────────────────────────────────

/// Parse `"aa:bb:cc:dd:ee:ff"` (case-insensitive) into 6 bytes.
#[cfg(any(target_os = "linux", test))]
fn parse_mac_colons(s: &str) -> Option<MacAddr> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut out = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        out[i] = u8::from_str_radix(p, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_unicast_target_filters() {
        let valid_mac = [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0x01];
        assert!(is_unicast_target("192.168.1.1".parse().unwrap(), valid_mac));
        // Multicast MAC (01:00:5e:*)
        assert!(!is_unicast_target(
            "192.168.1.1".parse().unwrap(),
            [0x01, 0x00, 0x5e, 0x00, 0x00, 0xfb]
        ));
        // Broadcast MAC
        assert!(!is_unicast_target(
            "192.168.1.255".parse().unwrap(),
            [0xff; 6]
        ));
        // Multicast IP (224.0.0.0/4)
        assert!(!is_unicast_target(
            "224.0.0.251".parse().unwrap(),
            valid_mac
        ));
        // Loopback IP
        assert!(!is_unicast_target("127.0.0.1".parse().unwrap(), valid_mac));
        // Directed broadcast (.255 last octet)
        assert!(!is_unicast_target("10.0.0.255".parse().unwrap(), valid_mac));
    }

    #[test]
    fn test_format_mac_roundtrip() {
        let mac = [0xaa, 0xbb, 0xcc, 0x01, 0x02, 0x03];
        assert_eq!(format_mac(&mac), "aa:bb:cc:01:02:03");
        assert_eq!(parse_mac_colons("aa:bb:cc:01:02:03"), Some(mac));
    }

    #[test]
    fn test_parse_mac_colons_rejects_bad() {
        assert!(parse_mac_colons("aa:bb:cc:01:02").is_none());
        assert!(parse_mac_colons("aa:bb:cc:01:02:03:04").is_none());
        assert!(parse_mac_colons("zz:bb:cc:01:02:03").is_none());
        assert!(parse_mac_colons("aabbcc010203").is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_parse_proc_arp_complete_and_incomplete() {
        let fixture = "\
IP address       HW type     Flags       HW address            Mask     Device
192.168.1.1      0x1         0x2         aa:bb:cc:dd:ee:01     *        eth0
192.168.1.50     0x1         0x0         00:00:00:00:00:00     *        eth0
127.0.0.1        0x1         0x2         de:ad:be:ef:00:01     *        lo
192.168.1.100    0x1         0x2         11:22:33:44:55:66     *        eth0
";
        let entries = super::os::parse_proc_arp(fixture);
        // Loopback and incomplete entries are filtered out, leaving 2.
        assert_eq!(entries.len(), 2);
        assert!(entries.contains(&(
            "192.168.1.1".parse().unwrap(),
            [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0x01]
        )));
        assert!(entries.contains(&(
            "192.168.1.100".parse().unwrap(),
            [0x11, 0x22, 0x33, 0x44, 0x55, 0x66]
        )));
    }
}
