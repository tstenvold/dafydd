//! Network utility modules for fast-path device pre-detection.

pub mod arp;
/// ICMP echo sweep for alive-host pre-filtering before TCP probing.
pub mod icmp;
/// Active mDNS DNS-SD querier for zero-probe device pre-discovery.
pub mod mdns;
/// SSDP/UPnP M-SEARCH querier for UPnP-capable device pre-discovery.
pub mod ssdp;
/// Raw TCP SYN scanner for open-port pre-filtering (Linux only).
pub mod syn_scan;
