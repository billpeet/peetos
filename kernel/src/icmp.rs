use arch_x86_64::pit;
use net::{htons, ntohs, net_checksum, Ipv4Addr, IP_PROTO_ICMP};

use crate::ipv4;
use crate::serial_println;

const ICMP_TYPE_ECHO_REPLY: u8 = 0;
const ICMP_TYPE_ECHO_REQUEST: u8 = 8;
const ICMP_TYPE_DEST_UNREACHABLE: u8 = 3;
const ICMP_TYPE_TIME_EXCEEDED: u8 = 11;

const ICMP_ECHO_HDR_LEN: usize = 8;
const ICMP_MAX_PACKET: usize = 1500;
const ICMP_ECHO_DATA_SIZE: usize = 56;

struct PingState {
    target: Ipv4Addr,
    identifier: u16,
    sequence: u16,
    sent_time: u64,
    pending: bool,
    received: bool,
    rtt: u64,
}

static mut PING_STATE: PingState = PingState {
    target: Ipv4Addr::ZERO,
    identifier: 0,
    sequence: 0,
    sent_time: 0,
    pending: false,
    received: false,
    rtt: 0,
};

static mut PING_IDENTIFIER: u16 = 1;
static mut PING_SEQUENCE: u16 = 0;

// ---------------------------------------------------------------------------
// Send
// ---------------------------------------------------------------------------

fn send_echo(dst: &Ipv4Addr, icmp_type: u8, identifier: u16, sequence: u16, data: &[u8]) -> Result<(), net::NetError> {
    let total_len = ICMP_ECHO_HDR_LEN + data.len();
    if total_len > ICMP_MAX_PACKET {
        return Err(net::NetError::BufferTooLarge);
    }

    let mut packet = [0u8; ICMP_MAX_PACKET];
    packet[0] = icmp_type;
    packet[1] = 0; // code
    // checksum [2..4] = 0 for now
    let id = htons(identifier);
    packet[4] = (id & 0xFF) as u8;
    packet[5] = ((id >> 8) & 0xFF) as u8;
    let seq = htons(sequence);
    packet[6] = (seq & 0xFF) as u8;
    packet[7] = ((seq >> 8) & 0xFF) as u8;

    if !data.is_empty() {
        packet[ICMP_ECHO_HDR_LEN..ICMP_ECHO_HDR_LEN + data.len()].copy_from_slice(data);
    }

    // Calculate checksum
    let cksum = net_checksum(&packet[..total_len]);
    packet[2] = (cksum & 0xFF) as u8;
    packet[3] = ((cksum >> 8) & 0xFF) as u8;

    ipv4::send(dst, IP_PROTO_ICMP, &packet[..total_len])
}

pub fn send_echo_request(dst: &Ipv4Addr, identifier: u16, sequence: u16, data: &[u8]) -> Result<(), net::NetError> {
    serial_println!(
        "[ICMP] Sending Echo Request to {}.{}.{}.{} seq={}",
        dst.0[0], dst.0[1], dst.0[2], dst.0[3], sequence
    );
    send_echo(dst, ICMP_TYPE_ECHO_REQUEST, identifier, sequence, data)
}

pub fn send_echo_reply(dst: &Ipv4Addr, identifier: u16, sequence: u16, data: &[u8]) -> Result<(), net::NetError> {
    send_echo(dst, ICMP_TYPE_ECHO_REPLY, identifier, sequence, data)
}

// ---------------------------------------------------------------------------
// Receive
// ---------------------------------------------------------------------------

fn handle_echo_request(payload: &[u8], src: &Ipv4Addr) {
    if payload.len() < ICMP_ECHO_HDR_LEN {
        return;
    }

    let identifier = ntohs((payload[4] as u16) | ((payload[5] as u16) << 8));
    let sequence = ntohs((payload[6] as u16) | ((payload[7] as u16) << 8));
    let data = &payload[ICMP_ECHO_HDR_LEN..];

    serial_println!(
        "[ICMP] Echo Request from {}.{}.{}.{} seq={}",
        src.0[0], src.0[1], src.0[2], src.0[3], sequence
    );

    let _ = send_echo_reply(src, identifier, sequence, data);
}

fn handle_echo_reply(payload: &[u8], src: &Ipv4Addr) {
    if payload.len() < ICMP_ECHO_HDR_LEN {
        return;
    }

    let identifier = ntohs((payload[4] as u16) | ((payload[5] as u16) << 8));
    let sequence = ntohs((payload[6] as u16) | ((payload[7] as u16) << 8));

    unsafe {
        if PING_STATE.pending
            && src.eq(&PING_STATE.target)
            && identifier == PING_STATE.identifier
            && sequence == PING_STATE.sequence
        {
            PING_STATE.rtt = pit::uptime_ms() - PING_STATE.sent_time;
            PING_STATE.received = true;
            PING_STATE.pending = false;

            serial_println!(
                "[ICMP] Echo Reply from {}.{}.{}.{} seq={} time={} ms",
                src.0[0], src.0[1], src.0[2], src.0[3],
                sequence, PING_STATE.rtt
            );
        } else {
            serial_println!(
                "[ICMP] Echo Reply from {}.{}.{}.{} seq={}",
                src.0[0], src.0[1], src.0[2], src.0[3], sequence
            );
        }
    }
}

fn handle_dest_unreachable(payload: &[u8], src: &Ipv4Addr) {
    if payload.len() < 8 {
        return;
    }
    serial_println!(
        "[ICMP] Destination Unreachable from {}.{}.{}.{} code={}",
        src.0[0], src.0[1], src.0[2], src.0[3], payload[1]
    );

    unsafe {
        if PING_STATE.pending {
            PING_STATE.pending = false;
            PING_STATE.received = false;
        }
    }
}

fn handle_time_exceeded(payload: &[u8], src: &Ipv4Addr) {
    if payload.len() < 8 {
        return;
    }
    serial_println!(
        "[ICMP] Time Exceeded from {}.{}.{}.{} code={}",
        src.0[0], src.0[1], src.0[2], src.0[3], payload[1]
    );
}

/// IPv4 handler for ICMP packets.
pub fn icmp_ip_receive(payload: &[u8], src: &Ipv4Addr, _dst: &Ipv4Addr) {
    if payload.len() < 8 {
        return;
    }

    // Verify checksum
    if net_checksum(payload) != 0 {
        serial_println!("[ICMP] Bad checksum");
        return;
    }

    let icmp_type = payload[0];
    match icmp_type {
        ICMP_TYPE_ECHO_REQUEST => handle_echo_request(payload, src),
        ICMP_TYPE_ECHO_REPLY => handle_echo_reply(payload, src),
        ICMP_TYPE_DEST_UNREACHABLE => handle_dest_unreachable(payload, src),
        ICMP_TYPE_TIME_EXCEEDED => handle_time_exceeded(payload, src),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Ping
// ---------------------------------------------------------------------------

pub fn ping(target: &Ipv4Addr, timeout_ms: u64) -> Result<u64, net::NetError> {
    let identifier = unsafe {
        PING_IDENTIFIER
    };
    let sequence = unsafe {
        PING_SEQUENCE = PING_SEQUENCE.wrapping_add(1);
        PING_SEQUENCE
    };

    // Fill data with pattern
    let mut data = [0u8; ICMP_ECHO_DATA_SIZE];
    for i in 0..ICMP_ECHO_DATA_SIZE {
        data[i] = (i & 0xFF) as u8;
    }

    // Set up ping state
    unsafe {
        PING_STATE.target.copy_from(target);
        PING_STATE.identifier = identifier;
        PING_STATE.sequence = sequence;
        PING_STATE.sent_time = pit::uptime_ms();
        PING_STATE.pending = true;
        PING_STATE.received = false;
        PING_STATE.rtt = 0;
    }

    // Send echo request
    send_echo_request(target, identifier, sequence, &data)?;

    // Wait for reply
    let start = pit::uptime_ms();
    loop {
        crate::e1000::poll();

        let received = unsafe { PING_STATE.received };
        if received {
            let rtt = unsafe { PING_STATE.rtt };
            return Ok(rtt);
        }

        if (pit::uptime_ms() - start) >= timeout_ms {
            unsafe {
                PING_STATE.pending = false;
            }
            return Err(net::NetError::Timeout);
        }

        pit::sleep_ms(10);
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("[ICMP] Initializing ICMP subsystem...");

    unsafe {
        PING_STATE.pending = false;
        PING_STATE.received = false;
    }

    // Register with IP layer
    ipv4::register_handler(IP_PROTO_ICMP, icmp_ip_receive);

    serial_println!("[ICMP] ICMP initialized");
}
