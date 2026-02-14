use arch_x86_64::pit;
use net::{htons, ntohs, Ipv4Addr, MacAddr};

use crate::ethernet::{self, ETHERTYPE_ARP};
use crate::serial_println;

// ARP constants
const ARP_HW_ETHERNET: u16 = 1;
const ARP_PROTO_IPV4: u16 = 0x0800;
const ARP_OP_REQUEST: u16 = 1;
const ARP_OP_REPLY: u16 = 2;
const ARP_PACKET_LEN: usize = 28;
const ARP_CACHE_SIZE: usize = 32;

#[derive(Clone, Copy, PartialEq)]
enum ArpState {
    Free,
    Pending,
    Resolved,
}

struct ArpCacheEntry {
    ip: Ipv4Addr,
    mac: MacAddr,
    state: ArpState,
    timestamp: u64,
    retries: u8,
}

impl ArpCacheEntry {
    const fn empty() -> Self {
        Self {
            ip: Ipv4Addr::ZERO,
            mac: MacAddr::ZERO,
            state: ArpState::Free,
            timestamp: 0,
            retries: 0,
        }
    }
}

const EMPTY_ENTRY: ArpCacheEntry = ArpCacheEntry::empty();
static mut ARP_CACHE: [ArpCacheEntry; ARP_CACHE_SIZE] = [EMPTY_ENTRY; ARP_CACHE_SIZE];

// ---------------------------------------------------------------------------
// ARP packet layout (28 bytes for Ethernet/IPv4)
// ---------------------------------------------------------------------------

fn build_arp_packet(buf: &mut [u8; ARP_PACKET_LEN], operation: u16, sender_mac: &MacAddr, sender_ip: &Ipv4Addr, target_mac: &MacAddr, target_ip: &Ipv4Addr) {
    let hw_type = htons(ARP_HW_ETHERNET);
    let proto_type = htons(ARP_PROTO_IPV4);
    let op = htons(operation);

    buf[0] = (hw_type & 0xFF) as u8;
    buf[1] = ((hw_type >> 8) & 0xFF) as u8;
    buf[2] = (proto_type & 0xFF) as u8;
    buf[3] = ((proto_type >> 8) & 0xFF) as u8;
    buf[4] = 6; // hw_len
    buf[5] = 4; // proto_len
    buf[6] = (op & 0xFF) as u8;
    buf[7] = ((op >> 8) & 0xFF) as u8;
    buf[8..14].copy_from_slice(&sender_mac.0);
    buf[14..18].copy_from_slice(&sender_ip.0);
    buf[18..24].copy_from_slice(&target_mac.0);
    buf[24..28].copy_from_slice(&target_ip.0);
}

fn parse_operation(data: &[u8]) -> u16 {
    ntohs((data[6] as u16) | ((data[7] as u16) << 8))
}

fn parse_sender_mac(data: &[u8]) -> MacAddr {
    MacAddr::new([data[8], data[9], data[10], data[11], data[12], data[13]])
}

fn parse_sender_ip(data: &[u8]) -> Ipv4Addr {
    Ipv4Addr::new(data[14], data[15], data[16], data[17])
}

fn parse_target_ip(data: &[u8]) -> Ipv4Addr {
    Ipv4Addr::new(data[24], data[25], data[26], data[27])
}

// ---------------------------------------------------------------------------
// Cache management
// ---------------------------------------------------------------------------

fn cache_find(ip: &Ipv4Addr) -> Option<usize> {
    unsafe {
        for i in 0..ARP_CACHE_SIZE {
            if ARP_CACHE[i].state != ArpState::Free && ARP_CACHE[i].ip.eq(ip) {
                return Some(i);
            }
        }
    }
    None
}

fn cache_alloc() -> Option<usize> {
    unsafe {
        // Find free entry
        for i in 0..ARP_CACHE_SIZE {
            if ARP_CACHE[i].state == ArpState::Free {
                return Some(i);
            }
        }
        // Evict oldest
        let mut oldest_idx = 0;
        let mut oldest_time = u64::MAX;
        for i in 0..ARP_CACHE_SIZE {
            if ARP_CACHE[i].timestamp < oldest_time {
                oldest_time = ARP_CACHE[i].timestamp;
                oldest_idx = i;
            }
        }
        ARP_CACHE[oldest_idx].state = ArpState::Free;
        Some(oldest_idx)
    }
}

fn cache_add(ip: &Ipv4Addr, mac: &MacAddr) {
    if ip.is_broadcast() || ip.is_zero() {
        return;
    }

    let now = pit::uptime_ms();

    if let Some(idx) = cache_find(ip) {
        unsafe {
            ARP_CACHE[idx].mac.copy_from(mac);
            ARP_CACHE[idx].state = ArpState::Resolved;
            ARP_CACHE[idx].timestamp = now;
        }
        return;
    }

    if let Some(idx) = cache_alloc() {
        unsafe {
            ARP_CACHE[idx].ip.copy_from(ip);
            ARP_CACHE[idx].mac.copy_from(mac);
            ARP_CACHE[idx].state = ArpState::Resolved;
            ARP_CACHE[idx].timestamp = now;
            ARP_CACHE[idx].retries = 0;
        }
    }
}

// ---------------------------------------------------------------------------
// ARP lookup
// ---------------------------------------------------------------------------

pub fn lookup(ip: &Ipv4Addr) -> Option<MacAddr> {
    // Broadcast IP -> broadcast MAC
    if ip.is_broadcast() {
        return Some(MacAddr::BROADCAST);
    }

    if let Some(idx) = cache_find(ip) {
        let entry = unsafe { &ARP_CACHE[idx] };
        if entry.state == ArpState::Resolved {
            return Some(entry.mac);
        }
        if entry.state == ArpState::Pending {
            return None;
        }
    }

    // Cache miss — send ARP request and mark as pending
    if let Some(idx) = cache_alloc() {
        unsafe {
            ARP_CACHE[idx].ip.copy_from(ip);
            ARP_CACHE[idx].state = ArpState::Pending;
            ARP_CACHE[idx].timestamp = pit::uptime_ms();
            ARP_CACHE[idx].retries = 0;
        }
    }

    send_request(ip);
    None
}

/// Blocking ARP resolution with timeout.
pub fn resolve(ip: &Ipv4Addr, timeout_ms: u64) -> Option<MacAddr> {
    let start = pit::uptime_ms();

    loop {
        if let Some(mac) = lookup(ip) {
            return Some(mac);
        }

        // Poll for incoming packets
        crate::e1000::poll();

        if (pit::uptime_ms() - start) >= timeout_ms {
            return None;
        }

        // Brief spin
        for _ in 0..100_000u32 {
            core::hint::spin_loop();
        }
    }
}

// ---------------------------------------------------------------------------
// Sending ARP
// ---------------------------------------------------------------------------

fn send_request(target_ip: &Ipv4Addr) {
    serial_println!(
        "[ARP] Sending request for {}.{}.{}.{}",
        target_ip.0[0],
        target_ip.0[1],
        target_ip.0[2],
        target_ip.0[3]
    );

    let cfg = crate::net::get_config();
    let our_mac = ethernet::get_mac();
    let mut pkt = [0u8; ARP_PACKET_LEN];
    build_arp_packet(&mut pkt, ARP_OP_REQUEST, &our_mac, &cfg.ip, &MacAddr::ZERO, target_ip);
    let _ = ethernet::send(&MacAddr::BROADCAST, ETHERTYPE_ARP, &pkt);
}

pub fn send_gratuitous() {
    let cfg = crate::net::get_config();
    if !cfg.configured {
        return;
    }
    serial_println!("[ARP] Sending gratuitous ARP");
    let our_mac = ethernet::get_mac();
    let mut pkt = [0u8; ARP_PACKET_LEN];
    build_arp_packet(&mut pkt, ARP_OP_REQUEST, &our_mac, &cfg.ip, &MacAddr::ZERO, &cfg.ip);
    let _ = ethernet::send(&MacAddr::BROADCAST, ETHERTYPE_ARP, &pkt);
}

// ---------------------------------------------------------------------------
// Receiving ARP
// ---------------------------------------------------------------------------

fn handle_request(data: &[u8]) {
    let cfg = crate::net::get_config();
    let target_ip = parse_target_ip(data);

    if !cfg.configured || !target_ip.eq(&cfg.ip) {
        return;
    }

    let sender_ip = parse_sender_ip(data);
    let sender_mac = parse_sender_mac(data);

    serial_println!(
        "[ARP] Request from {}.{}.{}.{}",
        sender_ip.0[0],
        sender_ip.0[1],
        sender_ip.0[2],
        sender_ip.0[3]
    );

    // Cache the sender
    cache_add(&sender_ip, &sender_mac);

    // Send reply
    serial_println!("[ARP] Sending reply");
    let our_mac = ethernet::get_mac();
    let mut pkt = [0u8; ARP_PACKET_LEN];
    build_arp_packet(&mut pkt, ARP_OP_REPLY, &our_mac, &cfg.ip, &sender_mac, &sender_ip);
    let _ = ethernet::send(&sender_mac, ETHERTYPE_ARP, &pkt);
}

fn handle_reply(data: &[u8]) {
    let sender_ip = parse_sender_ip(data);
    let sender_mac = parse_sender_mac(data);

    serial_println!(
        "[ARP] Reply: {}.{}.{}.{} is at {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        sender_ip.0[0],
        sender_ip.0[1],
        sender_ip.0[2],
        sender_ip.0[3],
        sender_mac.0[0],
        sender_mac.0[1],
        sender_mac.0[2],
        sender_mac.0[3],
        sender_mac.0[4],
        sender_mac.0[5]
    );

    cache_add(&sender_ip, &sender_mac);
}

/// Ethernet handler for ARP frames.
pub fn arp_receive(payload: &[u8], _src_mac: &MacAddr) {
    if payload.len() < ARP_PACKET_LEN {
        return;
    }

    // Validate hw/proto types
    let hw_type = ntohs((payload[0] as u16) | ((payload[1] as u16) << 8));
    let proto_type = ntohs((payload[2] as u16) | ((payload[3] as u16) << 8));
    if hw_type != ARP_HW_ETHERNET || proto_type != ARP_PROTO_IPV4 {
        return;
    }
    if payload[4] != 6 || payload[5] != 4 {
        return;
    }

    let operation = parse_operation(payload);
    match operation {
        ARP_OP_REQUEST => handle_request(payload),
        ARP_OP_REPLY => handle_reply(payload),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("[ARP] Initializing ARP subsystem...");

    // Clear cache
    unsafe {
        for i in 0..ARP_CACHE_SIZE {
            ARP_CACHE[i].state = ArpState::Free;
        }
    }

    // Register with Ethernet layer
    ethernet::register_handler(ETHERTYPE_ARP, arp_receive);

    serial_println!("[ARP] ARP initialized");
}
