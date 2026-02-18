use net::{htons, net_checksum_pseudo, ntohs, Ipv4Addr, NetError, IP_PROTO_UDP, MTU};

use crate::ipv4;
use crate::serial_println;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const UDP_HEADER_LEN: usize = 8;
const UDP_MAX_PAYLOAD: usize = MTU - 20 - UDP_HEADER_LEN; // 1500 - IP hdr - UDP hdr
const MAX_BINDINGS: usize = 16;
const RX_QUEUE_SIZE: usize = 8;
const RX_BUF_SIZE: usize = 1472; // max UDP payload in single IP datagram

// ---------------------------------------------------------------------------
// Receive queue entry
// ---------------------------------------------------------------------------

struct RxEntry {
    data: [u8; RX_BUF_SIZE],
    len: usize,
    src_ip: Ipv4Addr,
    src_port: u16,
    used: bool,
}

impl RxEntry {
    const fn empty() -> Self {
        Self {
            data: [0u8; RX_BUF_SIZE],
            len: 0,
            src_ip: Ipv4Addr::ZERO,
            src_port: 0,
            used: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Binding table
// ---------------------------------------------------------------------------

struct UdpBinding {
    local_port: u16,
    active: bool,
    rx_queue: [RxEntry; RX_QUEUE_SIZE],
    rx_head: usize,
    rx_tail: usize,
    rx_count: usize,
}

impl UdpBinding {
    const fn empty() -> Self {
        Self {
            local_port: 0,
            active: false,
            rx_queue: [
                RxEntry::empty(),
                RxEntry::empty(),
                RxEntry::empty(),
                RxEntry::empty(),
                RxEntry::empty(),
                RxEntry::empty(),
                RxEntry::empty(),
                RxEntry::empty(),
            ],
            rx_head: 0,
            rx_tail: 0,
            rx_count: 0,
        }
    }

    fn enqueue(&mut self, data: &[u8], src_ip: &Ipv4Addr, src_port: u16) -> bool {
        if self.rx_count >= RX_QUEUE_SIZE {
            return false; // queue full, drop
        }
        let slot = &mut self.rx_queue[self.rx_tail];
        let copy_len = data.len().min(RX_BUF_SIZE);
        slot.data[..copy_len].copy_from_slice(&data[..copy_len]);
        slot.len = copy_len;
        slot.src_ip.copy_from(src_ip);
        slot.src_port = src_port;
        slot.used = true;
        self.rx_tail = (self.rx_tail + 1) % RX_QUEUE_SIZE;
        self.rx_count += 1;
        true
    }

    fn dequeue(&mut self) -> Option<(usize, Ipv4Addr, u16)> {
        if self.rx_count == 0 {
            return None;
        }
        let slot = &self.rx_queue[self.rx_head];
        if !slot.used {
            return None;
        }
        let len = slot.len;
        let src_ip = slot.src_ip;
        let src_port = slot.src_port;
        // Mark consumed — caller must copy data before next dequeue
        self.rx_head = (self.rx_head + 1) % RX_QUEUE_SIZE;
        self.rx_count -= 1;
        Some((len, src_ip, src_port))
    }

    /// Peek at the head entry's data without consuming. Returns slice length.
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
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

// Binding table uses const-init array to avoid alloc.
macro_rules! empty_bindings {
    ($n:expr) => {{
        const EMPTY: UdpBinding = UdpBinding::empty();
        [EMPTY; $n]
    }};
}

static mut BINDINGS: [UdpBinding; MAX_BINDINGS] = empty_bindings!(MAX_BINDINGS);
static mut NEXT_EPHEMERAL: u16 = 49152;

// ---------------------------------------------------------------------------
// Bind / Unbind
// ---------------------------------------------------------------------------

/// Bind a local UDP port. Returns a binding handle (index) or error.
pub fn bind(port: u16) -> Result<usize, NetError> {
    unsafe {
        // Check for duplicate
        for b in BINDINGS.iter() {
            if b.active && b.local_port == port {
                serial_println!("[UDP] Port {} already bound", port);
                return Err(NetError::PortInUse);
            }
        }
        // Find free slot
        for (i, b) in BINDINGS.iter_mut().enumerate() {
            if !b.active {
                b.local_port = port;
                b.active = true;
                b.rx_head = 0;
                b.rx_tail = 0;
                b.rx_count = 0;
                for entry in b.rx_queue.iter_mut() {
                    entry.used = false;
                }
                serial_println!("[UDP] Bound port {}", port);
                return Ok(i);
            }
        }
        serial_println!("[UDP] No free binding slots");
        Err(NetError::NoResources)
    }
}

/// Allocate an ephemeral port and bind it.
pub fn bind_ephemeral() -> Result<(usize, u16), NetError> {
    unsafe {
        let start = NEXT_EPHEMERAL;
        loop {
            let port = NEXT_EPHEMERAL;
            NEXT_EPHEMERAL = if NEXT_EPHEMERAL >= 65534 {
                49152
            } else {
                NEXT_EPHEMERAL + 1
            };

            // Check availability
            let in_use = BINDINGS.iter().any(|b| b.active && b.local_port == port);
            if !in_use {
                let handle = bind(port)?;
                return Ok((handle, port));
            }

            if NEXT_EPHEMERAL == start {
                return Err(NetError::NoResources); // wrapped around, all in use
            }
        }
    }
}

/// Release a binding.
pub fn unbind(handle: usize) {
    unsafe {
        if handle < MAX_BINDINGS && BINDINGS[handle].active {
            serial_println!("[UDP] Unbound port {}", BINDINGS[handle].local_port);
            BINDINGS[handle].active = false;
        }
    }
}

// ---------------------------------------------------------------------------
// Metadata accessors (used by socket layer)
// ---------------------------------------------------------------------------

/// Returns the local port for a binding, or None if invalid/inactive.
pub fn binding_port(handle: usize) -> Option<u16> {
    unsafe {
        if handle >= MAX_BINDINGS || !BINDINGS[handle].active {
            return None;
        }
        Some(BINDINGS[handle].local_port)
    }
}

// ---------------------------------------------------------------------------
// Send
// ---------------------------------------------------------------------------

/// Send a UDP datagram to `dst_ip:dst_port` from `src_port`.
pub fn send(
    dst_ip: &Ipv4Addr,
    dst_port: u16,
    src_port: u16,
    payload: &[u8],
) -> Result<(), NetError> {
    if payload.len() > UDP_MAX_PAYLOAD {
        return Err(NetError::BufferTooLarge);
    }

    let total_len = (UDP_HEADER_LEN + payload.len()) as u16;
    let mut packet = [0u8; MTU];

    // Build UDP header
    let sp = htons(src_port);
    packet[0] = (sp & 0xFF) as u8;
    packet[1] = ((sp >> 8) & 0xFF) as u8;
    let dp = htons(dst_port);
    packet[2] = (dp & 0xFF) as u8;
    packet[3] = ((dp >> 8) & 0xFF) as u8;
    let tl = htons(total_len);
    packet[4] = (tl & 0xFF) as u8;
    packet[5] = ((tl >> 8) & 0xFF) as u8;
    // checksum [6..8] = 0 for now

    // Copy payload
    packet[UDP_HEADER_LEN..UDP_HEADER_LEN + payload.len()].copy_from_slice(payload);

    // Compute UDP checksum (with pseudo-header)
    let cfg = crate::net::get_config();
    let cksum = net_checksum_pseudo(&packet[..total_len as usize], &cfg.ip, dst_ip, IP_PROTO_UDP);
    // RFC 768: if computed checksum is 0, transmit as 0xFFFF
    let cksum = if cksum == 0 { 0xFFFF } else { cksum };
    packet[6] = (cksum & 0xFF) as u8;
    packet[7] = ((cksum >> 8) & 0xFF) as u8;

    ipv4::send(dst_ip, IP_PROTO_UDP, &packet[..total_len as usize])
}

/// Send from a bound handle, using the binding's local port as source.
pub fn send_from(
    handle: usize,
    dst_ip: &Ipv4Addr,
    dst_port: u16,
    payload: &[u8],
) -> Result<(), NetError> {
    let src_port = unsafe {
        if handle >= MAX_BINDINGS || !BINDINGS[handle].active {
            return Err(NetError::NotInitialized);
        }
        BINDINGS[handle].local_port
    };
    send(dst_ip, dst_port, src_port, payload)
}

// ---------------------------------------------------------------------------
// Receive (from binding queue)
// ---------------------------------------------------------------------------

/// Non-blocking receive. Copies data into `buf`, returns (bytes_copied, src_ip, src_port)
/// or None if no data available.
pub fn recv(handle: usize, buf: &mut [u8]) -> Option<(usize, Ipv4Addr, u16)> {
    unsafe {
        if handle >= MAX_BINDINGS || !BINDINGS[handle].active {
            return None;
        }
        let binding = &mut BINDINGS[handle];
        // Peek data first so we can copy it
        if let Some(data) = binding.peek_data() {
            let copy_len = data.len().min(buf.len());
            buf[..copy_len].copy_from_slice(&data[..copy_len]);
            // Now consume
            if let Some((len, src_ip, src_port)) = binding.dequeue() {
                return Some((copy_len.min(len), src_ip, src_port));
            }
        }
    }
    None
}

/// Blocking receive with timeout (ms). Polls the NIC while waiting.
pub fn recv_timeout(
    handle: usize,
    buf: &mut [u8],
    timeout_ms: u64,
) -> Result<(usize, Ipv4Addr, u16), NetError> {
    let start = arch_x86_64::pit::uptime_ms();
    loop {
        if let Some(result) = recv(handle, buf) {
            return Ok(result);
        }

        if (arch_x86_64::pit::uptime_ms() - start) >= timeout_ms {
            return Err(NetError::Timeout);
        }

        // Poll NIC to drive packet reception
        crate::e1000::poll();
        arch_x86_64::pit::sleep_ms(1);
    }
}

/// Return the number of queued datagrams for a binding.
pub fn rx_pending(handle: usize) -> usize {
    unsafe {
        if handle >= MAX_BINDINGS || !BINDINGS[handle].active {
            return 0;
        }
        BINDINGS[handle].rx_count
    }
}

// ---------------------------------------------------------------------------
// Incoming packet handler (called from IPv4 dispatch)
// ---------------------------------------------------------------------------

pub fn udp_ip_receive(payload: &[u8], src: &Ipv4Addr, dst: &Ipv4Addr) {
    if payload.len() < UDP_HEADER_LEN {
        return;
    }

    let src_port = ntohs((payload[0] as u16) | ((payload[1] as u16) << 8));
    let dst_port = ntohs((payload[2] as u16) | ((payload[3] as u16) << 8));
    let udp_len = ntohs((payload[4] as u16) | ((payload[5] as u16) << 8)) as usize;

    if udp_len < UDP_HEADER_LEN || udp_len > payload.len() {
        return;
    }

    // Verify checksum if non-zero
    let cksum_field = (payload[6] as u16) | ((payload[7] as u16) << 8);
    if cksum_field != 0 {
        let computed = net_checksum_pseudo(&payload[..udp_len], src, dst, IP_PROTO_UDP);
        if computed != 0 {
            serial_println!(
                "[UDP] Bad checksum from {}.{}.{}.{}:{} -> port {}",
                src.0[0],
                src.0[1],
                src.0[2],
                src.0[3],
                src_port,
                dst_port
            );
            return;
        }
    }

    let data = &payload[UDP_HEADER_LEN..udp_len];

    // Find binding for this destination port
    unsafe {
        for b in BINDINGS.iter_mut() {
            if b.active && b.local_port == dst_port {
                if !b.enqueue(data, src, src_port) {
                    serial_println!("[UDP] Receive queue full on port {}, dropping", dst_port);
                }
                return;
            }
        }
    }

    // No binding — silently drop (per RFC 768 / common practice; could send
    // ICMP port unreachable here in a future iteration).
}

// ---------------------------------------------------------------------------
// Smoke test
// ---------------------------------------------------------------------------

#[cfg(feature = "net-udp-smoke")]
pub fn smoke_test() {
    serial_println!("[UDP] Running UDP smoke test...");

    // Bind a local port, send a packet to the QEMU gateway (will be dropped
    // but exercises the full TX path), then unbind.
    match bind_ephemeral() {
        Ok((handle, port)) => {
            serial_println!("[UDP] Smoke: bound ephemeral port {}", port);
            let msg = b"PeetOS UDP smoke";
            let gw = Ipv4Addr::new(10, 0, 2, 2);
            match send_from(handle, &gw, 7, msg) {
                // port 7 = echo
                Ok(()) => serial_println!("[UDP] Smoke: sent {} bytes to 10.0.2.2:7", msg.len()),
                Err(e) => serial_println!("[UDP] Smoke: send failed: {:?}", e),
            }
            unbind(handle);
            serial_println!("[UDP] Smoke: unbound port {}", port);
        }
        Err(e) => serial_println!("[UDP] Smoke: bind failed: {:?}", e),
    }

    serial_println!("[UDP] Smoke test done");
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("[UDP] Initializing UDP subsystem...");

    unsafe {
        for b in BINDINGS.iter_mut() {
            b.active = false;
        }
        NEXT_EPHEMERAL = 49152;
    }

    // Register with IP layer
    ipv4::register_handler(IP_PROTO_UDP, udp_ip_receive);

    serial_println!("[UDP] UDP initialized");
}
