use arch_x86_64::pit;
use net::{htons, net_checksum, ntohs, Ipv4Addr, NetError, IP_PROTO_ICMP};

use crate::ipv4;
use crate::serial_println;

const ICMP_TYPE_ECHO_REPLY: u8 = 0;
const ICMP_TYPE_ECHO_REQUEST: u8 = 8;
const ICMP_TYPE_DEST_UNREACHABLE: u8 = 3;
const ICMP_TYPE_TIME_EXCEEDED: u8 = 11;

const ICMP_ECHO_HDR_LEN: usize = 8;
const ICMP_MAX_PACKET: usize = 1500;
const ICMP_ECHO_DATA_SIZE: usize = 56;
const ICMP_SOCKET_MAX: usize = 8;
const ICMP_SOCKET_RX_QUEUE: usize = 4;
const ICMP_SOCKET_RX_BUF: usize = 4096;

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

struct IcmpSocketRxEntry {
    data: [u8; ICMP_SOCKET_RX_BUF],
    len: usize,
    src_ip: Ipv4Addr,
    used: bool,
}

impl IcmpSocketRxEntry {
    const fn empty() -> Self {
        Self {
            data: [0u8; ICMP_SOCKET_RX_BUF],
            len: 0,
            src_ip: Ipv4Addr::ZERO,
            used: false,
        }
    }
}

struct IcmpSocketBinding {
    active: bool,
    rx_queue: [IcmpSocketRxEntry; ICMP_SOCKET_RX_QUEUE],
    rx_head: usize,
    rx_tail: usize,
    rx_count: usize,
}

impl IcmpSocketBinding {
    const fn empty() -> Self {
        Self {
            active: false,
            rx_queue: [
                IcmpSocketRxEntry::empty(),
                IcmpSocketRxEntry::empty(),
                IcmpSocketRxEntry::empty(),
                IcmpSocketRxEntry::empty(),
            ],
            rx_head: 0,
            rx_tail: 0,
            rx_count: 0,
        }
    }

    fn reset(&mut self) {
        self.active = false;
        self.rx_head = 0;
        self.rx_tail = 0;
        self.rx_count = 0;
        for slot in self.rx_queue.iter_mut() {
            slot.used = false;
            slot.len = 0;
            slot.src_ip = Ipv4Addr::ZERO;
        }
    }

    fn enqueue(&mut self, data: &[u8], src_ip: &Ipv4Addr) -> bool {
        if self.rx_count >= ICMP_SOCKET_RX_QUEUE {
            return false;
        }
        let slot = &mut self.rx_queue[self.rx_tail];
        let copy_len = data.len().min(ICMP_SOCKET_RX_BUF);
        slot.data[..copy_len].copy_from_slice(&data[..copy_len]);
        slot.len = copy_len;
        slot.src_ip = *src_ip;
        slot.used = true;
        self.rx_tail = (self.rx_tail + 1) % ICMP_SOCKET_RX_QUEUE;
        self.rx_count += 1;
        true
    }

    fn peek_data(&self) -> Option<&[u8]> {
        if self.rx_count == 0 {
            return None;
        }
        let slot = &self.rx_queue[self.rx_head];
        if !slot.used {
            return None;
        }
        Some(&slot.data[..slot.len])
    }

    fn dequeue(&mut self) -> Option<(usize, Ipv4Addr)> {
        if self.rx_count == 0 {
            return None;
        }
        let slot = &self.rx_queue[self.rx_head];
        if !slot.used {
            return None;
        }
        let len = slot.len;
        let src_ip = slot.src_ip;
        self.rx_head = (self.rx_head + 1) % ICMP_SOCKET_RX_QUEUE;
        self.rx_count -= 1;
        Some((len, src_ip))
    }
}

macro_rules! empty_icmp_sockets {
    ($n:expr) => {{
        const EMPTY: IcmpSocketBinding = IcmpSocketBinding::empty();
        [EMPTY; $n]
    }};
}

static mut ICMP_SOCKETS: [IcmpSocketBinding; ICMP_SOCKET_MAX] =
    empty_icmp_sockets!(ICMP_SOCKET_MAX);

fn enqueue_to_icmp_sockets(payload: &[u8], src: &Ipv4Addr) {
    unsafe {
        for binding in ICMP_SOCKETS.iter_mut() {
            if binding.active {
                let _ = binding.enqueue(payload, src);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ICMP datagram socket bridge
// ---------------------------------------------------------------------------

pub fn socket_open() -> Result<usize, NetError> {
    unsafe {
        for (i, binding) in ICMP_SOCKETS.iter_mut().enumerate() {
            if !binding.active {
                binding.reset();
                binding.active = true;
                return Ok(i);
            }
        }
    }
    Err(NetError::NoResources)
}

pub fn socket_close(handle: usize) {
    unsafe {
        if handle < ICMP_SOCKET_MAX {
            ICMP_SOCKETS[handle].reset();
        }
    }
}

pub fn socket_sendto(handle: usize, dst: &Ipv4Addr, payload: &[u8]) -> Result<(), NetError> {
    unsafe {
        if handle >= ICMP_SOCKET_MAX || !ICMP_SOCKETS[handle].active {
            return Err(NetError::NotInitialized);
        }
    }

    if payload.len() > ICMP_MAX_PACKET {
        return Err(NetError::BufferTooLarge);
    }

    // Toybox ping uses SOCK_DGRAM/IPPROTO_ICMP and does not provide a final
    // wire checksum. Normalize the packet so it goes out with a valid ICMP
    // checksum.
    let mut packet = [0u8; ICMP_MAX_PACKET];
    let total_len = payload.len();
    packet[..total_len].copy_from_slice(payload);
    if total_len >= 4 {
        packet[2] = 0;
        packet[3] = 0;
        let cksum = net_checksum(&packet[..total_len]);
        packet[2] = (cksum & 0xFF) as u8;
        packet[3] = ((cksum >> 8) & 0xFF) as u8;
    }

    ipv4::send(dst, IP_PROTO_ICMP, &packet[..total_len])
}

pub fn socket_recv(handle: usize, buf: &mut [u8]) -> Option<(usize, Ipv4Addr)> {
    unsafe {
        if handle >= ICMP_SOCKET_MAX || !ICMP_SOCKETS[handle].active {
            return None;
        }
        let binding = &mut ICMP_SOCKETS[handle];
        if let Some(data) = binding.peek_data() {
            let copy_len = data.len().min(buf.len());
            buf[..copy_len].copy_from_slice(&data[..copy_len]);
            if let Some((len, src_ip)) = binding.dequeue() {
                return Some((copy_len.min(len), src_ip));
            }
        }
    }
    None
}

pub fn socket_recv_timeout(
    handle: usize,
    buf: &mut [u8],
    timeout_ms: u64,
) -> Result<(usize, Ipv4Addr), NetError> {
    let start = pit::uptime_ms();
    loop {
        if let Some(result) = socket_recv(handle, buf) {
            return Ok(result);
        }
        if (pit::uptime_ms() - start) >= timeout_ms {
            return Err(NetError::Timeout);
        }
        crate::e1000::poll();
        pit::sleep_ms(1);
    }
}

pub fn socket_rx_pending(handle: usize) -> usize {
    unsafe {
        if handle >= ICMP_SOCKET_MAX || !ICMP_SOCKETS[handle].active {
            return 0;
        }
        ICMP_SOCKETS[handle].rx_count
    }
}

// ---------------------------------------------------------------------------
// Send
// ---------------------------------------------------------------------------

fn send_echo(
    dst: &Ipv4Addr,
    icmp_type: u8,
    identifier: u16,
    sequence: u16,
    data: &[u8],
) -> Result<(), net::NetError> {
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

pub fn send_echo_request(
    dst: &Ipv4Addr,
    identifier: u16,
    sequence: u16,
    data: &[u8],
) -> Result<(), net::NetError> {
    serial_println!(
        "[ICMP] Sending Echo Request to {}.{}.{}.{} seq={}",
        dst.0[0],
        dst.0[1],
        dst.0[2],
        dst.0[3],
        sequence
    );
    send_echo(dst, ICMP_TYPE_ECHO_REQUEST, identifier, sequence, data)
}

pub fn send_echo_reply(
    dst: &Ipv4Addr,
    identifier: u16,
    sequence: u16,
    data: &[u8],
) -> Result<(), net::NetError> {
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
        src.0[0],
        src.0[1],
        src.0[2],
        src.0[3],
        sequence
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
                src.0[0],
                src.0[1],
                src.0[2],
                src.0[3],
                sequence,
                PING_STATE.rtt
            );
        } else {
            serial_println!(
                "[ICMP] Echo Reply from {}.{}.{}.{} seq={}",
                src.0[0],
                src.0[1],
                src.0[2],
                src.0[3],
                sequence
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
        src.0[0],
        src.0[1],
        src.0[2],
        src.0[3],
        payload[1]
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
        src.0[0],
        src.0[1],
        src.0[2],
        src.0[3],
        payload[1]
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

    // Feed userspace ICMP datagram sockets (toybox ping path). Skip inbound
    // echo requests so user ping readers don't treat them as replies.
    if icmp_type != ICMP_TYPE_ECHO_REQUEST {
        enqueue_to_icmp_sockets(payload, src);
    }
}

// ---------------------------------------------------------------------------
// Ping
// ---------------------------------------------------------------------------

pub fn ping(target: &Ipv4Addr, timeout_ms: u64) -> Result<u64, net::NetError> {
    let identifier = unsafe { PING_IDENTIFIER };
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
        for binding in ICMP_SOCKETS.iter_mut() {
            binding.reset();
        }
    }

    // Register with IP layer
    ipv4::register_handler(IP_PROTO_ICMP, icmp_ip_receive);

    serial_println!("[ICMP] ICMP initialized");
}
