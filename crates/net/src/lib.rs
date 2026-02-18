#![no_std]

#[cfg(test)]
extern crate std;

/// Swap bytes in a 16-bit value (host <-> network byte order on little-endian x86).
#[inline]
pub const fn htons(x: u16) -> u16 {
    ((x >> 8) & 0xFF) | ((x & 0xFF) << 8)
}

/// Swap bytes in a 32-bit value (host <-> network byte order on little-endian x86).
#[inline]
pub const fn htonl(x: u32) -> u32 {
    ((x >> 24) & 0x0000_00FF)
        | ((x >> 8) & 0x0000_FF00)
        | ((x << 8) & 0x00FF_0000)
        | ((x << 24) & 0xFF00_0000)
}

/// Network to host byte order (16-bit). Same operation as htons on x86.
#[inline]
pub const fn ntohs(x: u16) -> u16 {
    htons(x)
}

/// Network to host byte order (32-bit). Same operation as htonl on x86.
#[inline]
pub const fn ntohl(x: u32) -> u32 {
    htonl(x)
}

/// Maximum Transmission Unit (Ethernet payload).
pub const MTU: usize = 1500;

/// Maximum Ethernet frame size (with header, without FCS).
pub const MAX_PACKET: usize = 1518;

/// IP protocol numbers.
pub const IP_PROTO_ICMP: u8 = 1;
pub const IP_PROTO_TCP: u8 = 6;
pub const IP_PROTO_UDP: u8 = 17;

/// MAC (Ethernet) address — 48 bits.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct MacAddr(pub [u8; 6]);

impl MacAddr {
    pub const BROADCAST: MacAddr = MacAddr([0xFF; 6]);
    pub const ZERO: MacAddr = MacAddr([0; 6]);

    pub const fn new(bytes: [u8; 6]) -> Self {
        Self(bytes)
    }

    pub fn eq(&self, other: &MacAddr) -> bool {
        self.0[0] == other.0[0]
            && self.0[1] == other.0[1]
            && self.0[2] == other.0[2]
            && self.0[3] == other.0[3]
            && self.0[4] == other.0[4]
            && self.0[5] == other.0[5]
    }

    pub fn is_broadcast(&self) -> bool {
        self.eq(&Self::BROADCAST)
    }

    pub fn is_zero(&self) -> bool {
        self.eq(&Self::ZERO)
    }

    pub fn copy_from(&mut self, src: &MacAddr) {
        self.0 = src.0;
    }
}

/// IPv4 address — 32 bits, stored in network byte order.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Ipv4Addr(pub [u8; 4]);

impl Ipv4Addr {
    pub const BROADCAST: Ipv4Addr = Ipv4Addr([255, 255, 255, 255]);
    pub const ZERO: Ipv4Addr = Ipv4Addr([0, 0, 0, 0]);

    pub const fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Self([a, b, c, d])
    }

    pub fn eq(&self, other: &Ipv4Addr) -> bool {
        self.0[0] == other.0[0]
            && self.0[1] == other.0[1]
            && self.0[2] == other.0[2]
            && self.0[3] == other.0[3]
    }

    pub fn is_broadcast(&self) -> bool {
        self.eq(&Self::BROADCAST)
    }

    pub fn is_zero(&self) -> bool {
        self.eq(&Self::ZERO)
    }

    pub fn is_loopback(&self) -> bool {
        self.0[0] == 127
    }

    /// Return the address as a u32 in network byte order.
    pub fn to_u32(&self) -> u32 {
        u32::from_ne_bytes(self.0)
    }

    /// Create from a u32 in network byte order.
    pub fn from_u32(val: u32) -> Self {
        Self(val.to_ne_bytes())
    }

    pub fn copy_from(&mut self, src: &Ipv4Addr) {
        self.0 = src.0;
    }
}

/// Network interface configuration.
pub struct NetConfig {
    pub mac: MacAddr,
    pub ip: Ipv4Addr,
    pub netmask: Ipv4Addr,
    pub gateway: Ipv4Addr,
    pub configured: bool,
}

impl NetConfig {
    pub const fn new() -> Self {
        Self {
            mac: MacAddr::ZERO,
            ip: Ipv4Addr::ZERO,
            netmask: Ipv4Addr::ZERO,
            gateway: Ipv4Addr::ZERO,
            configured: false,
        }
    }
}

/// Calculate Internet checksum (RFC 1071).
///
/// The checksum is the 16-bit one's complement of the one's complement
/// sum of all 16-bit words in the data.
pub fn net_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    let len = data.len();

    // Sum all 16-bit words
    while i + 1 < len {
        let word = (data[i] as u32) | ((data[i + 1] as u32) << 8);
        sum += word;
        i += 2;
    }

    // Add left-over byte if any
    if i < len {
        sum += data[i] as u32;
    }

    // Fold 32-bit sum to 16 bits
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    // Return one's complement
    !(sum as u16)
}

/// Calculate checksum with pseudo-header for TCP/UDP.
pub fn net_checksum_pseudo(data: &[u8], src: &Ipv4Addr, dst: &Ipv4Addr, protocol: u8) -> u16 {
    let mut sum: u32 = 0;

    // Add pseudo-header: src IP (4 bytes as two 16-bit words)
    sum += (src.0[0] as u32) | ((src.0[1] as u32) << 8);
    sum += (src.0[2] as u32) | ((src.0[3] as u32) << 8);

    // dst IP
    sum += (dst.0[0] as u32) | ((dst.0[1] as u32) << 8);
    sum += (dst.0[2] as u32) | ((dst.0[3] as u32) << 8);

    // Protocol and length (in network byte order)
    sum += htons(protocol as u16) as u32;
    sum += htons(data.len() as u16) as u32;

    // Add data
    let mut i = 0;
    let len = data.len();
    while i + 1 < len {
        let word = (data[i] as u32) | ((data[i + 1] as u32) << 8);
        sum += word;
        i += 2;
    }

    if i < len {
        sum += data[i] as u32;
    }

    // Fold and complement
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    !(sum as u16)
}

/// Error type for network operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetError {
    NotInitialized,
    BufferTooLarge,
    TransmitFailed,
    NoRoute,
    ArpFailed,
    Timeout,
    PortInUse,
    NoResources,
    ConnectionRefused,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_order_roundtrip_16() {
        assert_eq!(ntohs(htons(0x1234)), 0x1234);
        assert_eq!(htons(0x0100), 0x0001);
    }

    #[test]
    fn byte_order_roundtrip_32() {
        assert_eq!(ntohl(htonl(0x12345678)), 0x12345678);
    }

    #[test]
    fn mac_addr_broadcast() {
        assert!(MacAddr::BROADCAST.is_broadcast());
        assert!(!MacAddr::BROADCAST.is_zero());
        assert!(!MacAddr::ZERO.is_broadcast());
        assert!(MacAddr::ZERO.is_zero());
    }

    #[test]
    fn ipv4_addr_basics() {
        let addr = Ipv4Addr::new(10, 0, 2, 15);
        assert!(!addr.is_broadcast());
        assert!(!addr.is_zero());
        assert!(!addr.is_loopback());

        let loopback = Ipv4Addr::new(127, 0, 0, 1);
        assert!(loopback.is_loopback());

        assert!(Ipv4Addr::BROADCAST.is_broadcast());
        assert!(Ipv4Addr::ZERO.is_zero());
    }

    #[test]
    fn ipv4_u32_roundtrip() {
        let addr = Ipv4Addr::new(10, 0, 2, 15);
        let val = addr.to_u32();
        let back = Ipv4Addr::from_u32(val);
        assert!(addr.eq(&back));
    }

    #[test]
    fn checksum_zeros() {
        // Checksum of all zeros should be 0xFFFF
        let data = [0u8; 20];
        assert_eq!(net_checksum(&data), 0xFFFF);
    }

    #[test]
    fn checksum_known_value() {
        // Verify checksum with a known IP header
        // Version=4, IHL=5, TOS=0, Total=40, ID=1, Flags=DF, TTL=64, Proto=6 (TCP)
        // Src=10.0.2.15, Dst=10.0.2.2, Checksum field=0
        let mut hdr = [0u8; 20];
        hdr[0] = 0x45; // version + IHL
        hdr[2] = 0x00;
        hdr[3] = 0x28; // total length = 40
        hdr[4] = 0x00;
        hdr[5] = 0x01; // identification
        hdr[6] = 0x40;
        hdr[7] = 0x00; // flags + frag offset (DF)
        hdr[8] = 0x40; // TTL = 64
        hdr[9] = 0x06; // protocol = TCP
                       // checksum bytes [10..12] = 0
        hdr[12] = 10;
        hdr[13] = 0;
        hdr[14] = 2;
        hdr[15] = 15; // src = 10.0.2.15
        hdr[16] = 10;
        hdr[17] = 0;
        hdr[18] = 2;
        hdr[19] = 2; // dst = 10.0.2.2

        let cksum = net_checksum(&hdr);
        // Verify that inserting the checksum and re-computing yields 0
        hdr[10] = (cksum & 0xFF) as u8;
        hdr[11] = ((cksum >> 8) & 0xFF) as u8;
        assert_eq!(net_checksum(&hdr), 0);
    }
}
