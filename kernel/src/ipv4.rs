use net::{htons, net_checksum, ntohs, Ipv4Addr, MacAddr, NetError, MTU};

use crate::arp;
use crate::ethernet::{self, ETHERTYPE_IPV4};
use crate::serial_println;

// IPv4 constants
const IP_VERSION_4: u8 = 4;
const IP_HEADER_LEN: usize = 20;
const IP_DEFAULT_TTL: u8 = 64;
const IP_FLAG_DF: u16 = 0x4000;
const IP_FLAG_MF: u16 = 0x2000;
const IP_FRAG_OFFSET_MASK: u16 = 0x1FFF;
const IP_MAX_PACKET: usize = MTU;

const MAX_HANDLERS: usize = 4;

type Ipv4HandlerFn = fn(payload: &[u8], src: &Ipv4Addr, dst: &Ipv4Addr);

struct HandlerEntry {
    protocol: u8,
    handler: Ipv4HandlerFn,
}

static mut HANDLERS: [Option<HandlerEntry>; MAX_HANDLERS] = [None, None, None, None];
static mut HANDLER_COUNT: usize = 0;
static mut IP_IDENTIFICATION: u16 = 1;

// ---------------------------------------------------------------------------
// Header helpers
// ---------------------------------------------------------------------------

fn get_version(hdr: &[u8]) -> u8 {
    (hdr[0] >> 4) & 0x0F
}

fn get_header_len(hdr: &[u8]) -> usize {
    ((hdr[0] & 0x0F) as usize) * 4
}

fn get_total_len(hdr: &[u8]) -> u16 {
    ntohs((hdr[2] as u16) | ((hdr[3] as u16) << 8))
}

fn get_flags_fragment(hdr: &[u8]) -> u16 {
    ntohs((hdr[6] as u16) | ((hdr[7] as u16) << 8))
}

fn is_fragment(hdr: &[u8]) -> bool {
    let ff = get_flags_fragment(hdr);
    (ff & IP_FLAG_MF) != 0 || (ff & IP_FRAG_OFFSET_MASK) != 0
}

fn get_protocol(hdr: &[u8]) -> u8 {
    hdr[9]
}

fn get_src(hdr: &[u8]) -> Ipv4Addr {
    Ipv4Addr::new(hdr[12], hdr[13], hdr[14], hdr[15])
}

fn get_dst(hdr: &[u8]) -> Ipv4Addr {
    Ipv4Addr::new(hdr[16], hdr[17], hdr[18], hdr[19])
}

// ---------------------------------------------------------------------------
// Checksum
// ---------------------------------------------------------------------------

fn ip_verify_checksum(hdr: &[u8]) -> bool {
    let hlen = get_header_len(hdr);
    if hlen > hdr.len() {
        return false;
    }
    net_checksum(&hdr[..hlen]) == 0
}

// ---------------------------------------------------------------------------
// Routing
// ---------------------------------------------------------------------------

pub fn is_local(dst: &Ipv4Addr) -> bool {
    let cfg = crate::net::get_config();
    if !cfg.configured {
        return false;
    }

    let dst_net = dst.to_u32();
    let local_net = cfg.ip.to_u32();
    let mask = cfg.netmask.to_u32();

    (dst_net & mask) == (local_net & mask)
}

fn route_lookup(dst: &Ipv4Addr) -> Option<Ipv4Addr> {
    let cfg = crate::net::get_config();
    if !cfg.configured {
        return None;
    }

    if dst.is_broadcast() {
        return Some(*dst);
    }

    if is_local(dst) {
        return Some(*dst);
    }

    if !cfg.gateway.is_zero() {
        return Some(cfg.gateway);
    }

    None
}

// ---------------------------------------------------------------------------
// Handler registration
// ---------------------------------------------------------------------------

pub fn register_handler(protocol: u8, handler: Ipv4HandlerFn) {
    unsafe {
        if HANDLER_COUNT >= MAX_HANDLERS {
            serial_println!("[IP] ERROR: Too many handlers");
            return;
        }
        HANDLERS[HANDLER_COUNT] = Some(HandlerEntry { protocol, handler });
        HANDLER_COUNT += 1;
    }
    serial_println!("[IP] Registered handler for protocol {}", protocol);
}

fn find_handler(protocol: u8) -> Option<Ipv4HandlerFn> {
    unsafe {
        for i in 0..HANDLER_COUNT {
            if let Some(ref entry) = HANDLERS[i] {
                if entry.protocol == protocol {
                    return Some(entry.handler);
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Send
// ---------------------------------------------------------------------------

pub fn send(dst: &Ipv4Addr, protocol: u8, payload: &[u8]) -> Result<(), NetError> {
    if payload.len() > IP_MAX_PACKET - IP_HEADER_LEN {
        return Err(NetError::BufferTooLarge);
    }

    let cfg = crate::net::get_config();
    if !cfg.configured {
        return Err(NetError::NotInitialized);
    }

    let total_len = (IP_HEADER_LEN + payload.len()) as u16;
    let identification = unsafe {
        let id = IP_IDENTIFICATION;
        IP_IDENTIFICATION = IP_IDENTIFICATION.wrapping_add(1);
        id
    };

    // Build IP header
    let mut packet = [0u8; IP_MAX_PACKET];
    packet[0] = (IP_VERSION_4 << 4) | ((IP_HEADER_LEN as u8) / 4);
    packet[1] = 0; // TOS
    let tl = htons(total_len);
    packet[2] = (tl & 0xFF) as u8;
    packet[3] = ((tl >> 8) & 0xFF) as u8;
    let id = htons(identification);
    packet[4] = (id & 0xFF) as u8;
    packet[5] = ((id >> 8) & 0xFF) as u8;
    let ff = htons(IP_FLAG_DF);
    packet[6] = (ff & 0xFF) as u8;
    packet[7] = ((ff >> 8) & 0xFF) as u8;
    packet[8] = IP_DEFAULT_TTL;
    packet[9] = protocol;
    // checksum [10..12] = 0 for now
    packet[12..16].copy_from_slice(&cfg.ip.0);
    packet[16..20].copy_from_slice(&dst.0);

    // Calculate header checksum
    let cksum = net_checksum(&packet[..IP_HEADER_LEN]);
    packet[10] = (cksum & 0xFF) as u8;
    packet[11] = ((cksum >> 8) & 0xFF) as u8;

    // Copy payload
    packet[IP_HEADER_LEN..IP_HEADER_LEN + payload.len()].copy_from_slice(payload);

    // Loopback check
    if dst.eq(&cfg.ip) || dst.is_loopback() {
        ip_receive(&packet[..total_len as usize], &MacAddr::ZERO);
        return Ok(());
    }

    // Route lookup
    let next_hop = route_lookup(dst).ok_or(NetError::NoRoute)?;

    // ARP resolve
    let dst_mac = if dst.is_broadcast() {
        MacAddr::BROADCAST
    } else {
        arp::resolve(&next_hop, 1000).ok_or(NetError::ArpFailed)?
    };

    // Send via Ethernet
    ethernet::send(&dst_mac, ETHERTYPE_IPV4, &packet[..total_len as usize])
}

// ---------------------------------------------------------------------------
// Receive
// ---------------------------------------------------------------------------

fn ip_receive(data: &[u8], _src_mac: &MacAddr) {
    if data.len() < IP_HEADER_LEN {
        return;
    }

    if get_version(data) != IP_VERSION_4 {
        return;
    }

    let header_len = get_header_len(data);
    if header_len < IP_HEADER_LEN || header_len > data.len() {
        return;
    }

    let total_len = get_total_len(data) as usize;
    if total_len < header_len || total_len > data.len() {
        return;
    }

    if !ip_verify_checksum(data) {
        return;
    }

    // Reject fragments
    if is_fragment(data) {
        return;
    }

    // Destination check
    let dst = get_dst(data);
    let cfg = crate::net::get_config();
    if !dst.eq(&cfg.ip) && !dst.is_broadcast() && !dst.is_loopback() {
        return;
    }

    let protocol = get_protocol(data);
    let payload = &data[header_len..total_len];
    let src = get_src(data);

    if let Some(handler) = find_handler(protocol) {
        handler(payload, &src, &dst);
    }
}

/// Ethernet handler for IPv4 frames.
pub fn ipv4_eth_receive(payload: &[u8], src_mac: &MacAddr) {
    ip_receive(payload, src_mac);
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("[IP] Initializing IP layer...");

    unsafe {
        HANDLER_COUNT = 0;
        for h in HANDLERS.iter_mut() {
            *h = None;
        }
        IP_IDENTIFICATION = 1;
    }

    // Register with Ethernet layer
    ethernet::register_handler(ETHERTYPE_IPV4, ipv4_eth_receive);

    serial_println!("[IP] IP layer initialized");
}
