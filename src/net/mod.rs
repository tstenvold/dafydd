//! Network utility modules for fast-path device pre-detection.

pub mod arp;
/// Passive mDNS multicast listener for zero-probe device pre-discovery.
pub mod mdns;
pub mod ndp;
pub mod pcap;
