use net::{htons, ntohs, MacAddr, NetError, MAX_PACKET, MTU};

use crate::e1000;
use crate::serial_println;

// Ethernet header size (6 + 6 + 2 = 14 bytes)
const ETH_HEADER_LEN: usize = 14;
const ETH_MIN_PAYLOAD: usize = 46;
const ETH_MAX_PAYLOAD: usize = MTU;
const ETH_MAX_FRAME: usize = ETH_HEADER_LEN + ETH_MAX_PAYLOAD;

pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const ETHERTYPE_ARP: u16 = 0x0806;

const MAX_HANDLERS: usize = 4;

/// Ethernet frame header (packed, 14 bytes).
#[repr(C, packed)]
struct EthHeader {
    dst: [u8; 6],
    src: [u8; 6],
    ethertype: u16,
}

type EthHandlerFn = fn(payload: &[u8], src_mac: &MacAddr);

struct HandlerEntry {
    ethertype: u16,
    handler: EthHandlerFn,
}

static mut HANDLERS: [Option<HandlerEntry>; MAX_HANDLERS] = [None, None, None, None];
static mut HANDLER_COUNT: usize = 0;
static mut LOCAL_MAC: MacAddr = MacAddr::ZERO;
static mut INITIALIZED: bool = false;

pub fn init() {
    serial_println!("[ETH] Initializing Ethernet layer...");

    unsafe {
        LOCAL_MAC = e1000::get_mac();
        // Copy MAC to net config
        crate::net::config_set_mac(&LOCAL_MAC);
        HANDLER_COUNT = 0;
        for h in HANDLERS.iter_mut() {
            *h = None;
        }
        INITIALIZED = true;
    }

    let mac = unsafe { &LOCAL_MAC };
    serial_println!(
        "[ETH] Local MAC: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac.0[0],
        mac.0[1],
        mac.0[2],
        mac.0[3],
        mac.0[4],
        mac.0[5]
    );
    serial_println!("[ETH] Ethernet layer initialized");
}

pub fn register_handler(ethertype: u16, handler: EthHandlerFn) {
    unsafe {
        if HANDLER_COUNT >= MAX_HANDLERS {
            serial_println!("[ETH] ERROR: Too many handlers");
            return;
        }
        HANDLERS[HANDLER_COUNT] = Some(HandlerEntry { ethertype, handler });
        HANDLER_COUNT += 1;
    }
    serial_println!("[ETH] Registered handler for EtherType 0x{:04X}", ethertype);
}

fn find_handler(ethertype: u16) -> Option<EthHandlerFn> {
    unsafe {
        for i in 0..HANDLER_COUNT {
            if let Some(ref entry) = HANDLERS[i] {
                if entry.ethertype == ethertype {
                    return Some(entry.handler);
                }
            }
        }
    }
    None
}

pub fn send(dst: &MacAddr, ethertype: u16, payload: &[u8]) -> Result<(), NetError> {
    if payload.is_empty() || payload.len() > ETH_MAX_PAYLOAD {
        return Err(NetError::BufferTooLarge);
    }

    // Build frame into a stack buffer
    let mut frame = [0u8; MAX_PACKET];

    // Destination MAC
    frame[0..6].copy_from_slice(&dst.0);
    // Source MAC
    let src = unsafe { &LOCAL_MAC };
    frame[6..12].copy_from_slice(&src.0);
    // EtherType (network byte order)
    let et = htons(ethertype);
    frame[12] = (et & 0xFF) as u8;
    frame[13] = ((et >> 8) & 0xFF) as u8;

    // Copy payload
    frame[ETH_HEADER_LEN..ETH_HEADER_LEN + payload.len()].copy_from_slice(payload);

    // Pad to minimum Ethernet frame size
    let mut total_len = ETH_HEADER_LEN + payload.len();
    let min_frame = ETH_HEADER_LEN + ETH_MIN_PAYLOAD;
    if total_len < min_frame {
        // Zero-pad (already zeroed from initialization)
        total_len = min_frame;
    }

    e1000::send_packet(&frame[..total_len])
}

pub fn receive(frame: &[u8]) {
    if frame.len() < ETH_HEADER_LEN {
        return;
    }

    let dst_mac = MacAddr::new([frame[0], frame[1], frame[2], frame[3], frame[4], frame[5]]);
    let src_mac = MacAddr::new([frame[6], frame[7], frame[8], frame[9], frame[10], frame[11]]);
    let ethertype = ntohs((frame[12] as u16) | ((frame[13] as u16) << 8));

    // Check destination — must be for us or broadcast
    let local = unsafe { &LOCAL_MAC };
    if !dst_mac.eq(local) && !dst_mac.is_broadcast() {
        return;
    }

    let payload = &frame[ETH_HEADER_LEN..];

    if let Some(handler) = find_handler(ethertype) {
        handler(payload, &src_mac);
    }
}

pub fn get_mac() -> MacAddr {
    unsafe { LOCAL_MAC }
}
