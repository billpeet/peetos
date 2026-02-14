use net::{htons, ntohs, htonl, ntohl, net_checksum_pseudo, Ipv4Addr, IP_PROTO_TCP, NetError, MTU};

use crate::ipv4;
use crate::serial_println;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const TCP_HEADER_LEN: usize = 20;
const TCP_MSS: usize = MTU - 20 - TCP_HEADER_LEN; // 1460
const MAX_CONNECTIONS: usize = 16;
const RX_BUF_SIZE: usize = 4096;
const TX_BUF_SIZE: usize = 4096;
const DEFAULT_WINDOW: u16 = 4096;
const RETRANSMIT_MS: u64 = 1000;
const MAX_RETRANSMITS: u8 = 5;
const TIME_WAIT_MS: u64 = 2000;

// TCP flag bits
const TCP_FIN: u8 = 0x01;
const TCP_SYN: u8 = 0x02;
const TCP_RST: u8 = 0x04;
const TCP_PSH: u8 = 0x08;
const TCP_ACK: u8 = 0x10;

// ---------------------------------------------------------------------------
// TCP state machine
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum TcpState {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
}

// ---------------------------------------------------------------------------
// Sequence number helpers (modular 32-bit arithmetic)
// ---------------------------------------------------------------------------

fn seq_lt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

fn seq_le(a: u32, b: u32) -> bool {
    a == b || seq_lt(a, b)
}

// ---------------------------------------------------------------------------
// TCP connection block
// ---------------------------------------------------------------------------

struct TcpConnection {
    state: TcpState,
    active: bool,
    listener: bool,

    local_port: u16,
    remote_port: u16,
    remote_ip: Ipv4Addr,

    // Send sequence space
    snd_una: u32, // oldest unacknowledged
    snd_nxt: u32, // next to send
    snd_wnd: u16, // peer's receive window
    iss: u32,     // initial send sequence

    // Receive sequence space
    rcv_nxt: u32, // next expected
    rcv_wnd: u16, // our receive window
    irs: u32,     // initial receive sequence

    // Receive buffer
    rx_buf: [u8; RX_BUF_SIZE],
    rx_len: usize,

    // Retransmit buffer (unacked sent data)
    tx_buf: [u8; TX_BUF_SIZE],
    tx_len: usize,

    // Retransmit timer
    retransmit_time: u64, // next retransmit time (0 = none pending)
    retransmit_count: u8,

    // TIME_WAIT expiry
    time_wait_expiry: u64,

    // FIN sequence tracking
    fin_seq: u32,
}

impl TcpConnection {
    const fn empty() -> Self {
        Self {
            state: TcpState::Closed,
            active: false,
            listener: false,
            local_port: 0,
            remote_port: 0,
            remote_ip: Ipv4Addr::ZERO,
            snd_una: 0,
            snd_nxt: 0,
            snd_wnd: 0,
            iss: 0,
            rcv_nxt: 0,
            rcv_wnd: DEFAULT_WINDOW,
            irs: 0,
            rx_buf: [0u8; RX_BUF_SIZE],
            rx_len: 0,
            tx_buf: [0u8; TX_BUF_SIZE],
            tx_len: 0,
            retransmit_time: 0,
            retransmit_count: 0,
            time_wait_expiry: 0,
            fin_seq: 0,
        }
    }

    fn rx_space(&self) -> usize {
        RX_BUF_SIZE - self.rx_len
    }

    fn rx_push(&mut self, data: &[u8]) -> usize {
        let avail = self.rx_space();
        let copy_len = data.len().min(avail);
        self.rx_buf[self.rx_len..self.rx_len + copy_len].copy_from_slice(&data[..copy_len]);
        self.rx_len += copy_len;
        copy_len
    }

    fn rx_pop(&mut self, buf: &mut [u8]) -> usize {
        let copy_len = self.rx_len.min(buf.len());
        buf[..copy_len].copy_from_slice(&self.rx_buf[..copy_len]);
        if copy_len < self.rx_len {
            let remaining = self.rx_len - copy_len;
            for i in 0..remaining {
                self.rx_buf[i] = self.rx_buf[copy_len + i];
            }
        }
        self.rx_len -= copy_len;
        copy_len
    }

    fn reset(&mut self) {
        self.state = TcpState::Closed;
        self.active = false;
        self.listener = false;
        self.local_port = 0;
        self.remote_port = 0;
        self.remote_ip = Ipv4Addr::ZERO;
        self.snd_una = 0;
        self.snd_nxt = 0;
        self.snd_wnd = 0;
        self.iss = 0;
        self.rcv_nxt = 0;
        self.rcv_wnd = DEFAULT_WINDOW;
        self.irs = 0;
        self.rx_len = 0;
        self.tx_len = 0;
        self.retransmit_time = 0;
        self.retransmit_count = 0;
        self.time_wait_expiry = 0;
        self.fin_seq = 0;
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

macro_rules! empty_connections {
    ($n:expr) => {{
        const EMPTY: TcpConnection = TcpConnection::empty();
        [EMPTY; $n]
    }};
}

static mut CONNECTIONS: [TcpConnection; MAX_CONNECTIONS] = empty_connections!(MAX_CONNECTIONS);
static mut NEXT_EPHEMERAL: u16 = 49152;
static mut ISS_COUNTER: u32 = 0;

/// Generate a pseudo-random initial sequence number.
fn generate_iss() -> u32 {
    unsafe {
        let tick = arch_x86_64::pit::uptime_ms() as u32;
        ISS_COUNTER = ISS_COUNTER.wrapping_add(64000);
        tick.wrapping_add(ISS_COUNTER)
    }
}

// ---------------------------------------------------------------------------
// Header helpers
// ---------------------------------------------------------------------------

fn build_header(
    packet: &mut [u8],
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
) {
    let sp = htons(src_port);
    packet[0] = (sp & 0xFF) as u8;
    packet[1] = ((sp >> 8) & 0xFF) as u8;
    let dp = htons(dst_port);
    packet[2] = (dp & 0xFF) as u8;
    packet[3] = ((dp >> 8) & 0xFF) as u8;
    let sq = htonl(seq);
    packet[4] = (sq & 0xFF) as u8;
    packet[5] = ((sq >> 8) & 0xFF) as u8;
    packet[6] = ((sq >> 16) & 0xFF) as u8;
    packet[7] = ((sq >> 24) & 0xFF) as u8;
    let ak = htonl(ack);
    packet[8] = (ak & 0xFF) as u8;
    packet[9] = ((ak >> 8) & 0xFF) as u8;
    packet[10] = ((ak >> 16) & 0xFF) as u8;
    packet[11] = ((ak >> 24) & 0xFF) as u8;
    // Data offset: 5 words (20 bytes), no options
    packet[12] = 5 << 4;
    packet[13] = flags;
    let wn = htons(window);
    packet[14] = (wn & 0xFF) as u8;
    packet[15] = ((wn >> 8) & 0xFF) as u8;
    // Checksum [16..18] filled by send_segment
    packet[16] = 0;
    packet[17] = 0;
    // Urgent pointer
    packet[18] = 0;
    packet[19] = 0;
}

fn read_u16(data: &[u8], offset: usize) -> u16 {
    ntohs((data[offset] as u16) | ((data[offset + 1] as u16) << 8))
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    ntohl(
        (data[offset] as u32)
            | ((data[offset + 1] as u32) << 8)
            | ((data[offset + 2] as u32) << 16)
            | ((data[offset + 3] as u32) << 24),
    )
}

/// Send a TCP segment with optional payload.
fn send_segment(
    dst_ip: &Ipv4Addr,
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    payload: &[u8],
) -> Result<(), NetError> {
    let total_len = TCP_HEADER_LEN + payload.len();
    let mut packet = [0u8; MTU];

    build_header(&mut packet, src_port, dst_port, seq, ack, flags, window);

    if !payload.is_empty() {
        packet[TCP_HEADER_LEN..TCP_HEADER_LEN + payload.len()].copy_from_slice(payload);
    }

    // Compute TCP checksum (with pseudo-header)
    let cfg = crate::net::get_config();
    let cksum = net_checksum_pseudo(&packet[..total_len], &cfg.ip, dst_ip, IP_PROTO_TCP);
    packet[16] = (cksum & 0xFF) as u8;
    packet[17] = ((cksum >> 8) & 0xFF) as u8;

    ipv4::send(dst_ip, IP_PROTO_TCP, &packet[..total_len])
}

// ---------------------------------------------------------------------------
// Send helpers (extract connection fields to locals before calling
// send_segment, avoiding aliasing issues with static-mut access)
// ---------------------------------------------------------------------------

fn send_ack(idx: usize) {
    unsafe {
        let rip = CONNECTIONS[idx].remote_ip;
        let lp = CONNECTIONS[idx].local_port;
        let rp = CONNECTIONS[idx].remote_port;
        let seq = CONNECTIONS[idx].snd_nxt;
        let ack = CONNECTIONS[idx].rcv_nxt;
        let wnd = CONNECTIONS[idx].rcv_wnd;
        let _ = send_segment(&rip, lp, rp, seq, ack, TCP_ACK, wnd, &[]);
    }
}

fn send_ctrl(idx: usize, seq: u32, ack: u32, flags: u8) {
    unsafe {
        let rip = CONNECTIONS[idx].remote_ip;
        let lp = CONNECTIONS[idx].local_port;
        let rp = CONNECTIONS[idx].remote_port;
        let wnd = CONNECTIONS[idx].rcv_wnd;
        let _ = send_segment(&rip, lp, rp, seq, ack, flags, wnd, &[]);
    }
}

// ---------------------------------------------------------------------------
// Connection lookup
// ---------------------------------------------------------------------------

fn find_connection(local_port: u16, remote_ip: &Ipv4Addr, remote_port: u16) -> Option<usize> {
    unsafe {
        for (i, c) in CONNECTIONS.iter().enumerate() {
            if c.active
                && !c.listener
                && c.local_port == local_port
                && c.remote_port == remote_port
                && c.remote_ip.eq(remote_ip)
            {
                return Some(i);
            }
        }
    }
    None
}

fn find_listener(local_port: u16) -> Option<usize> {
    unsafe {
        for (i, c) in CONNECTIONS.iter().enumerate() {
            if c.active && c.listener && c.local_port == local_port && c.state == TcpState::Listen {
                return Some(i);
            }
        }
    }
    None
}

fn alloc_connection() -> Option<usize> {
    unsafe {
        for (i, c) in CONNECTIONS.iter().enumerate() {
            if !c.active {
                return Some(i);
            }
        }
    }
    None
}

fn alloc_ephemeral_port() -> Option<u16> {
    unsafe {
        let start = NEXT_EPHEMERAL;
        loop {
            let port = NEXT_EPHEMERAL;
            NEXT_EPHEMERAL = if NEXT_EPHEMERAL >= 65534 {
                49152
            } else {
                NEXT_EPHEMERAL + 1
            };

            let in_use = CONNECTIONS.iter().any(|c| c.active && c.local_port == port);
            if !in_use {
                return Some(port);
            }

            if NEXT_EPHEMERAL == start {
                return None;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Public API: listen
// ---------------------------------------------------------------------------

/// Create a passive (listening) socket on `port`. Returns a handle.
pub fn listen(port: u16) -> Result<usize, NetError> {
    unsafe {
        for c in CONNECTIONS.iter() {
            if c.active && c.listener && c.local_port == port {
                serial_println!("[TCP] Port {} already has a listener", port);
                return Err(NetError::PortInUse);
            }
        }

        let idx = alloc_connection().ok_or(NetError::NoResources)?;
        let c = &mut CONNECTIONS[idx];
        c.reset();
        c.active = true;
        c.listener = true;
        c.local_port = port;
        c.state = TcpState::Listen;
        serial_println!("[TCP] Listening on port {}", port);
        Ok(idx)
    }
}

// ---------------------------------------------------------------------------
// Public API: connect (active open)
// ---------------------------------------------------------------------------

/// Initiate a TCP connection to `dst_ip:dst_port`. Blocks until established
/// or timeout. Returns a handle on success.
pub fn connect(dst_ip: &Ipv4Addr, dst_port: u16, timeout_ms: u64) -> Result<usize, NetError> {
    let local_port = alloc_ephemeral_port().ok_or(NetError::NoResources)?;
    let idx = alloc_connection().ok_or(NetError::NoResources)?;
    let iss = generate_iss();

    unsafe {
        let c = &mut CONNECTIONS[idx];
        c.reset();
        c.active = true;
        c.local_port = local_port;
        c.remote_port = dst_port;
        c.remote_ip.copy_from(dst_ip);
        c.iss = iss;
        c.snd_una = iss;
        c.snd_nxt = iss.wrapping_add(1); // SYN consumes one sequence number
        c.state = TcpState::SynSent;
        c.retransmit_time = arch_x86_64::pit::uptime_ms() + RETRANSMIT_MS;
        c.retransmit_count = 0;
    }

    // Send SYN
    send_segment(dst_ip, local_port, dst_port, iss, 0, TCP_SYN, DEFAULT_WINDOW, &[])?;

    serial_println!(
        "[TCP] SYN sent to {}.{}.{}.{}:{}",
        dst_ip.0[0],
        dst_ip.0[1],
        dst_ip.0[2],
        dst_ip.0[3],
        dst_port
    );

    // Wait for ESTABLISHED
    let start = arch_x86_64::pit::uptime_ms();
    loop {
        let (active, state) = unsafe { (CONNECTIONS[idx].active, CONNECTIONS[idx].state) };

        if state == TcpState::Established {
            serial_println!("[TCP] Connection established (handle {})", idx);
            return Ok(idx);
        }
        if !active || state == TcpState::Closed {
            serial_println!("[TCP] Connection refused");
            return Err(NetError::ConnectionRefused);
        }

        if (arch_x86_64::pit::uptime_ms() - start) >= timeout_ms {
            unsafe {
                CONNECTIONS[idx].reset();
            }
            serial_println!("[TCP] Connect timeout");
            return Err(NetError::Timeout);
        }

        crate::e1000::poll();
        check_retransmit(idx);
        arch_x86_64::pit::sleep_ms(1);
    }
}

// ---------------------------------------------------------------------------
// Public API: accept (from a listening socket)
// ---------------------------------------------------------------------------

/// Block until a new connection is established on the listening socket, or
/// timeout. Returns the handle of the accepted connection.
pub fn accept(listen_handle: usize, timeout_ms: u64) -> Result<usize, NetError> {
    unsafe {
        if listen_handle >= MAX_CONNECTIONS
            || !CONNECTIONS[listen_handle].active
            || !CONNECTIONS[listen_handle].listener
        {
            return Err(NetError::NotInitialized);
        }
    }

    let listen_port = unsafe { CONNECTIONS[listen_handle].local_port };
    let start = arch_x86_64::pit::uptime_ms();

    loop {
        // Look for an established child connection on this port
        unsafe {
            for (i, c) in CONNECTIONS.iter().enumerate() {
                if c.active
                    && !c.listener
                    && c.local_port == listen_port
                    && c.state == TcpState::Established
                {
                    serial_println!("[TCP] Accepted connection (handle {})", i);
                    return Ok(i);
                }
            }
        }

        if (arch_x86_64::pit::uptime_ms() - start) >= timeout_ms {
            return Err(NetError::Timeout);
        }

        crate::e1000::poll();
        tick();
        arch_x86_64::pit::sleep_ms(1);
    }
}

// ---------------------------------------------------------------------------
// Public API: send
// ---------------------------------------------------------------------------

/// Send data on an established connection. Returns the number of bytes
/// accepted (may be less than `data.len()` due to window/buffer limits).
pub fn send(handle: usize, data: &[u8]) -> Result<usize, NetError> {
    if data.is_empty() {
        return Ok(0);
    }

    unsafe {
        if handle >= MAX_CONNECTIONS || !CONNECTIONS[handle].active {
            return Err(NetError::NotInitialized);
        }
        let c = &mut CONNECTIONS[handle];
        if c.state != TcpState::Established && c.state != TcpState::CloseWait {
            return Err(NetError::NotInitialized);
        }

        let unacked = c.snd_nxt.wrapping_sub(c.snd_una) as usize;
        let window = c.snd_wnd as usize;
        let avail = if window > unacked { window - unacked } else { 0 };
        let send_len = data.len().min(avail).min(TCP_MSS);
        if send_len == 0 {
            return Ok(0);
        }

        // Store in retransmit buffer
        let tx_copy = send_len.min(TX_BUF_SIZE - c.tx_len);
        if tx_copy > 0 {
            c.tx_buf[c.tx_len..c.tx_len + tx_copy].copy_from_slice(&data[..tx_copy]);
            c.tx_len += tx_copy;
        }

        let seq = c.snd_nxt;
        c.snd_nxt = c.snd_nxt.wrapping_add(send_len as u32);
        c.retransmit_time = arch_x86_64::pit::uptime_ms() + RETRANSMIT_MS;
        c.retransmit_count = 0;

        let remote_ip = c.remote_ip;
        let local_port = c.local_port;
        let remote_port = c.remote_port;
        let rcv_nxt = c.rcv_nxt;
        let rcv_wnd = c.rcv_wnd;

        send_segment(
            &remote_ip,
            local_port,
            remote_port,
            seq,
            rcv_nxt,
            TCP_ACK | TCP_PSH,
            rcv_wnd,
            &data[..send_len],
        )?;

        Ok(send_len)
    }
}

// ---------------------------------------------------------------------------
// Public API: recv
// ---------------------------------------------------------------------------

/// Non-blocking receive. Copies data into `buf`, returns
/// `(bytes_copied, remote_ip, remote_port)` or `None`.
pub fn recv(handle: usize, buf: &mut [u8]) -> Option<(usize, Ipv4Addr, u16)> {
    unsafe {
        if handle >= MAX_CONNECTIONS || !CONNECTIONS[handle].active {
            return None;
        }
        let c = &mut CONNECTIONS[handle];
        if c.rx_len == 0 {
            return None;
        }
        let n = c.rx_pop(buf);
        let ip = c.remote_ip;
        let port = c.remote_port;
        c.rcv_wnd = c.rx_space() as u16;
        Some((n, ip, port))
    }
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

        unsafe {
            if handle >= MAX_CONNECTIONS || !CONNECTIONS[handle].active {
                return Err(NetError::NotInitialized);
            }
            let st = CONNECTIONS[handle].state;
            if st == TcpState::Closed || st == TcpState::TimeWait {
                return Err(NetError::NotInitialized);
            }
        }

        if (arch_x86_64::pit::uptime_ms() - start) >= timeout_ms {
            return Err(NetError::Timeout);
        }

        crate::e1000::poll();
        tick();
        arch_x86_64::pit::sleep_ms(1);
    }
}

// ---------------------------------------------------------------------------
// Public API: close
// ---------------------------------------------------------------------------

/// Initiate a graceful close. Non-blocking — the connection will proceed
/// through the FIN handshake and be cleaned up by `tick()`.
pub fn close(handle: usize) {
    unsafe {
        if handle >= MAX_CONNECTIONS || !CONNECTIONS[handle].active {
            return;
        }
        match CONNECTIONS[handle].state {
            TcpState::Established | TcpState::SynReceived => {
                let seq = CONNECTIONS[handle].snd_nxt;
                let ack = CONNECTIONS[handle].rcv_nxt;
                CONNECTIONS[handle].fin_seq = seq;
                CONNECTIONS[handle].snd_nxt = seq.wrapping_add(1);
                CONNECTIONS[handle].state = TcpState::FinWait1;
                CONNECTIONS[handle].retransmit_time =
                    arch_x86_64::pit::uptime_ms() + RETRANSMIT_MS;
                CONNECTIONS[handle].retransmit_count = 0;
                send_ctrl(handle, seq, ack, TCP_FIN | TCP_ACK);
                serial_println!("[TCP] FIN sent (handle {})", handle);
            }
            TcpState::CloseWait => {
                let seq = CONNECTIONS[handle].snd_nxt;
                let ack = CONNECTIONS[handle].rcv_nxt;
                CONNECTIONS[handle].fin_seq = seq;
                CONNECTIONS[handle].snd_nxt = seq.wrapping_add(1);
                CONNECTIONS[handle].state = TcpState::LastAck;
                CONNECTIONS[handle].retransmit_time =
                    arch_x86_64::pit::uptime_ms() + RETRANSMIT_MS;
                CONNECTIONS[handle].retransmit_count = 0;
                send_ctrl(handle, seq, ack, TCP_FIN | TCP_ACK);
                serial_println!("[TCP] FIN sent (CloseWait, handle {})", handle);
            }
            TcpState::Listen | TcpState::SynSent => {
                CONNECTIONS[handle].reset();
                serial_println!("[TCP] Closed (handle {})", handle);
            }
            _ => { /* already closing or closed */ }
        }
    }
}

// ---------------------------------------------------------------------------
// Incoming packet handler (called from IPv4 dispatch)
// ---------------------------------------------------------------------------

pub fn tcp_ip_receive(payload: &[u8], src: &Ipv4Addr, dst: &Ipv4Addr) {
    if payload.len() < TCP_HEADER_LEN {
        return;
    }

    let src_port = read_u16(payload, 0);
    let dst_port = read_u16(payload, 2);
    let seq = read_u32(payload, 4);
    let ack = read_u32(payload, 8);
    let data_offset = ((payload[12] >> 4) & 0x0F) as usize * 4;
    let flags = payload[13];
    let window = read_u16(payload, 14);

    if data_offset < TCP_HEADER_LEN || data_offset > payload.len() {
        return;
    }

    // Verify checksum
    let computed = net_checksum_pseudo(payload, src, dst, IP_PROTO_TCP);
    if computed != 0 {
        serial_println!(
            "[TCP] Bad checksum from {}.{}.{}.{}:{}",
            src.0[0],
            src.0[1],
            src.0[2],
            src.0[3],
            src_port
        );
        return;
    }

    let data = &payload[data_offset..];

    // Try existing connection first (4-tuple match)
    if let Some(idx) = find_connection(dst_port, src, src_port) {
        handle_segment(idx, seq, ack, flags, window, data);
        return;
    }

    // Check for a listener on this port
    if find_listener(dst_port).is_some() {
        if flags & TCP_SYN != 0 && flags & TCP_ACK == 0 {
            // Incoming SYN — create child connection
            if let Some(child_idx) = alloc_connection() {
                let iss = generate_iss();
                unsafe {
                    let c = &mut CONNECTIONS[child_idx];
                    c.reset();
                    c.active = true;
                    c.local_port = dst_port;
                    c.remote_port = src_port;
                    c.remote_ip.copy_from(src);
                    c.iss = iss;
                    c.irs = seq;
                    c.snd_una = iss;
                    c.snd_nxt = iss.wrapping_add(1); // SYN-ACK consumes 1
                    c.rcv_nxt = seq.wrapping_add(1); // SYN consumes 1
                    c.snd_wnd = window;
                    c.state = TcpState::SynReceived;
                    c.retransmit_time = arch_x86_64::pit::uptime_ms() + RETRANSMIT_MS;
                    c.retransmit_count = 0;
                }

                // Send SYN-ACK
                let rcv_nxt = unsafe { CONNECTIONS[child_idx].rcv_nxt };
                let _ = send_segment(
                    src, dst_port, src_port, iss, rcv_nxt, TCP_SYN | TCP_ACK, DEFAULT_WINDOW, &[],
                );
                serial_println!(
                    "[TCP] SYN-ACK sent to {}.{}.{}.{}:{}",
                    src.0[0],
                    src.0[1],
                    src.0[2],
                    src.0[3],
                    src_port
                );
            } else {
                serial_println!("[TCP] No free connection slots, dropping SYN");
            }
        }
        return;
    }

    // No connection or listener — send RST
    if flags & TCP_RST == 0 {
        let (rst_seq, rst_ack, rst_flags) = if flags & TCP_ACK != 0 {
            (ack, 0u32, TCP_RST)
        } else {
            let data_len = data.len() as u32;
            let syn_fin =
                ((flags & TCP_SYN) >> 1) as u32 + (flags & TCP_FIN) as u32;
            (
                0u32,
                seq.wrapping_add(data_len).wrapping_add(syn_fin),
                TCP_RST | TCP_ACK,
            )
        };
        let _ = send_segment(src, dst_port, src_port, rst_seq, rst_ack, rst_flags, 0, &[]);
    }
}

// ---------------------------------------------------------------------------
// Segment processing per connection state
// ---------------------------------------------------------------------------

fn handle_segment(idx: usize, seg_seq: u32, seg_ack: u32, flags: u8, window: u16, data: &[u8]) {
    unsafe {
        // RST handling (all states)
        if flags & TCP_RST != 0 {
            serial_println!("[TCP] RST received (handle {})", idx);
            CONNECTIONS[idx].reset();
            return;
        }

        let state = CONNECTIONS[idx].state;

        match state {
            TcpState::SynSent => {
                if flags & TCP_SYN != 0 && flags & TCP_ACK != 0 {
                    if seg_ack == CONNECTIONS[idx].snd_nxt {
                        CONNECTIONS[idx].irs = seg_seq;
                        CONNECTIONS[idx].rcv_nxt = seg_seq.wrapping_add(1);
                        CONNECTIONS[idx].snd_una = seg_ack;
                        CONNECTIONS[idx].snd_wnd = window;
                        CONNECTIONS[idx].state = TcpState::Established;
                        CONNECTIONS[idx].retransmit_time = 0;
                        CONNECTIONS[idx].retransmit_count = 0;
                        send_ack(idx);
                        serial_println!("[TCP] Established (handle {})", idx);
                    }
                } else if flags & TCP_SYN != 0 {
                    // Simultaneous open
                    CONNECTIONS[idx].irs = seg_seq;
                    CONNECTIONS[idx].rcv_nxt = seg_seq.wrapping_add(1);
                    CONNECTIONS[idx].snd_wnd = window;
                    CONNECTIONS[idx].state = TcpState::SynReceived;
                    let iss = CONNECTIONS[idx].iss;
                    let rcv_nxt = CONNECTIONS[idx].rcv_nxt;
                    send_ctrl(idx, iss, rcv_nxt, TCP_SYN | TCP_ACK);
                }
            }

            TcpState::SynReceived => {
                if flags & TCP_ACK != 0 && seg_ack == CONNECTIONS[idx].snd_nxt {
                    CONNECTIONS[idx].snd_una = seg_ack;
                    CONNECTIONS[idx].snd_wnd = window;
                    CONNECTIONS[idx].state = TcpState::Established;
                    CONNECTIONS[idx].retransmit_time = 0;
                    CONNECTIONS[idx].retransmit_count = 0;
                    serial_println!("[TCP] Established (handle {} via SYN_RECEIVED)", idx);
                    if !data.is_empty() {
                        process_data(idx, seg_seq, data);
                    }
                }
            }

            TcpState::Established => {
                if flags & TCP_ACK != 0 {
                    process_ack(idx, seg_ack, window);
                }
                if !data.is_empty() {
                    process_data(idx, seg_seq, data);
                }
                if flags & TCP_FIN != 0 {
                    let fin_seq = seg_seq.wrapping_add(data.len() as u32);
                    if fin_seq == CONNECTIONS[idx].rcv_nxt {
                        CONNECTIONS[idx].rcv_nxt =
                            CONNECTIONS[idx].rcv_nxt.wrapping_add(1);
                        CONNECTIONS[idx].state = TcpState::CloseWait;
                        send_ack(idx);
                        serial_println!(
                            "[TCP] FIN received, entering CloseWait (handle {})",
                            idx
                        );
                    }
                }
            }

            TcpState::FinWait1 => {
                if flags & TCP_ACK != 0 {
                    process_ack(idx, seg_ack, window);
                }
                let fin_acked = CONNECTIONS[idx].snd_una == CONNECTIONS[idx].snd_nxt;
                if !data.is_empty() {
                    process_data(idx, seg_seq, data);
                }
                if flags & TCP_FIN != 0 {
                    let fin_seq = seg_seq.wrapping_add(data.len() as u32);
                    if fin_seq == CONNECTIONS[idx].rcv_nxt {
                        CONNECTIONS[idx].rcv_nxt =
                            CONNECTIONS[idx].rcv_nxt.wrapping_add(1);
                        if fin_acked {
                            CONNECTIONS[idx].state = TcpState::TimeWait;
                            CONNECTIONS[idx].time_wait_expiry =
                                arch_x86_64::pit::uptime_ms() + TIME_WAIT_MS;
                        } else {
                            CONNECTIONS[idx].state = TcpState::Closing;
                        }
                        send_ack(idx);
                    }
                } else if fin_acked {
                    CONNECTIONS[idx].state = TcpState::FinWait2;
                }
            }

            TcpState::FinWait2 => {
                if flags & TCP_ACK != 0 {
                    process_ack(idx, seg_ack, window);
                }
                if !data.is_empty() {
                    process_data(idx, seg_seq, data);
                }
                if flags & TCP_FIN != 0 {
                    let fin_seq = seg_seq.wrapping_add(data.len() as u32);
                    if fin_seq == CONNECTIONS[idx].rcv_nxt {
                        CONNECTIONS[idx].rcv_nxt =
                            CONNECTIONS[idx].rcv_nxt.wrapping_add(1);
                        CONNECTIONS[idx].state = TcpState::TimeWait;
                        CONNECTIONS[idx].time_wait_expiry =
                            arch_x86_64::pit::uptime_ms() + TIME_WAIT_MS;
                        send_ack(idx);
                        serial_println!(
                            "[TCP] FIN received, entering TimeWait (handle {})",
                            idx
                        );
                    }
                }
            }

            TcpState::Closing => {
                if flags & TCP_ACK != 0 {
                    process_ack(idx, seg_ack, window);
                    if CONNECTIONS[idx].snd_una == CONNECTIONS[idx].snd_nxt {
                        CONNECTIONS[idx].state = TcpState::TimeWait;
                        CONNECTIONS[idx].time_wait_expiry =
                            arch_x86_64::pit::uptime_ms() + TIME_WAIT_MS;
                    }
                }
            }

            TcpState::LastAck => {
                if flags & TCP_ACK != 0 {
                    process_ack(idx, seg_ack, window);
                    if CONNECTIONS[idx].snd_una == CONNECTIONS[idx].snd_nxt {
                        serial_println!("[TCP] LastAck -> Closed (handle {})", idx);
                        CONNECTIONS[idx].reset();
                    }
                }
            }

            TcpState::TimeWait => {
                // Re-ACK any FIN retransmits
                if flags & TCP_FIN != 0 {
                    send_ack(idx);
                    CONNECTIONS[idx].time_wait_expiry =
                        arch_x86_64::pit::uptime_ms() + TIME_WAIT_MS;
                }
            }

            TcpState::CloseWait => {
                if flags & TCP_ACK != 0 {
                    process_ack(idx, seg_ack, window);
                }
            }

            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Data and ACK processing
// ---------------------------------------------------------------------------

unsafe fn process_data(idx: usize, seg_seq: u32, data: &[u8]) {
    // Only accept in-order data
    if seg_seq != CONNECTIONS[idx].rcv_nxt {
        send_ack(idx); // duplicate ACK for out-of-order
        return;
    }

    let accepted = CONNECTIONS[idx].rx_push(data);
    CONNECTIONS[idx].rcv_nxt = CONNECTIONS[idx].rcv_nxt.wrapping_add(accepted as u32);
    CONNECTIONS[idx].rcv_wnd = CONNECTIONS[idx].rx_space() as u16;
    send_ack(idx);
}

unsafe fn process_ack(idx: usize, seg_ack: u32, window: u16) {
    let snd_una = CONNECTIONS[idx].snd_una;
    let snd_nxt = CONNECTIONS[idx].snd_nxt;

    if seq_lt(snd_una, seg_ack) && seq_le(seg_ack, snd_nxt) {
        let acked_bytes = seg_ack.wrapping_sub(snd_una) as usize;

        // Remove acknowledged data from retransmit buffer
        let tx_len = CONNECTIONS[idx].tx_len;
        if acked_bytes <= tx_len {
            let remaining = tx_len - acked_bytes;
            for i in 0..remaining {
                CONNECTIONS[idx].tx_buf[i] = CONNECTIONS[idx].tx_buf[acked_bytes + i];
            }
            CONNECTIONS[idx].tx_len = remaining;
        } else {
            CONNECTIONS[idx].tx_len = 0;
        }

        CONNECTIONS[idx].snd_una = seg_ack;

        // Reset retransmit timer if everything acked
        if seg_ack == snd_nxt {
            CONNECTIONS[idx].retransmit_time = 0;
            CONNECTIONS[idx].retransmit_count = 0;
        } else {
            CONNECTIONS[idx].retransmit_time = arch_x86_64::pit::uptime_ms() + RETRANSMIT_MS;
            CONNECTIONS[idx].retransmit_count = 0;
        }
    }

    CONNECTIONS[idx].snd_wnd = window;
}

// ---------------------------------------------------------------------------
// Retransmit timer
// ---------------------------------------------------------------------------

fn check_retransmit(idx: usize) {
    unsafe {
        if !CONNECTIONS[idx].active || CONNECTIONS[idx].retransmit_time == 0 {
            return;
        }

        let now = arch_x86_64::pit::uptime_ms();
        if now < CONNECTIONS[idx].retransmit_time {
            return;
        }

        CONNECTIONS[idx].retransmit_count += 1;
        if CONNECTIONS[idx].retransmit_count > MAX_RETRANSMITS {
            serial_println!("[TCP] Max retransmits exceeded (handle {})", idx);
            let snd_nxt = CONNECTIONS[idx].snd_nxt;
            let rcv_nxt = CONNECTIONS[idx].rcv_nxt;
            send_ctrl(idx, snd_nxt, rcv_nxt, TCP_RST);
            CONNECTIONS[idx].reset();
            return;
        }

        let count = CONNECTIONS[idx].retransmit_count;

        match CONNECTIONS[idx].state {
            TcpState::SynSent => {
                let iss = CONNECTIONS[idx].iss;
                let rip = CONNECTIONS[idx].remote_ip;
                let lp = CONNECTIONS[idx].local_port;
                let rp = CONNECTIONS[idx].remote_port;
                let _ = send_segment(&rip, lp, rp, iss, 0, TCP_SYN, DEFAULT_WINDOW, &[]);
                serial_println!("[TCP] SYN retransmit #{}", count);
            }
            TcpState::SynReceived => {
                let iss = CONNECTIONS[idx].iss;
                let rcv_nxt = CONNECTIONS[idx].rcv_nxt;
                send_ctrl(idx, iss, rcv_nxt, TCP_SYN | TCP_ACK);
                serial_println!("[TCP] SYN-ACK retransmit #{}", count);
            }
            TcpState::Established | TcpState::CloseWait => {
                let tx_len = CONNECTIONS[idx].tx_len;
                if tx_len > 0 {
                    let send_len = tx_len.min(TCP_MSS);
                    let snd_una = CONNECTIONS[idx].snd_una;
                    let rcv_nxt = CONNECTIONS[idx].rcv_nxt;
                    let rip = CONNECTIONS[idx].remote_ip;
                    let lp = CONNECTIONS[idx].local_port;
                    let rp = CONNECTIONS[idx].remote_port;
                    let wnd = CONNECTIONS[idx].rcv_wnd;
                    // Copy data out to avoid aliasing
                    let mut buf = [0u8; TCP_MSS];
                    buf[..send_len].copy_from_slice(&CONNECTIONS[idx].tx_buf[..send_len]);
                    let _ =
                        send_segment(&rip, lp, rp, snd_una, rcv_nxt, TCP_ACK | TCP_PSH, wnd, &buf[..send_len]);
                    serial_println!("[TCP] Data retransmit #{} ({} bytes)", count, send_len);
                }
            }
            TcpState::FinWait1 | TcpState::LastAck | TcpState::Closing => {
                let fin_seq = CONNECTIONS[idx].fin_seq;
                let rcv_nxt = CONNECTIONS[idx].rcv_nxt;
                send_ctrl(idx, fin_seq, rcv_nxt, TCP_FIN | TCP_ACK);
                serial_println!("[TCP] FIN retransmit #{}", count);
            }
            _ => {
                CONNECTIONS[idx].retransmit_time = 0;
                return;
            }
        }

        CONNECTIONS[idx].retransmit_time = now + RETRANSMIT_MS;
    }
}

// ---------------------------------------------------------------------------
// Periodic tick — call from net::poll()
// ---------------------------------------------------------------------------

pub fn tick() {
    unsafe {
        let now = arch_x86_64::pit::uptime_ms();
        for i in 0..MAX_CONNECTIONS {
            if !CONNECTIONS[i].active {
                continue;
            }

            // TIME_WAIT expiry
            if CONNECTIONS[i].state == TcpState::TimeWait && now >= CONNECTIONS[i].time_wait_expiry
            {
                serial_println!("[TCP] TimeWait expired (handle {})", i);
                CONNECTIONS[i].reset();
                continue;
            }

            // Retransmit check
            check_retransmit(i);
        }
    }
}

// ---------------------------------------------------------------------------
// Smoke test
// ---------------------------------------------------------------------------

#[cfg(feature = "net-tcp-smoke")]
pub fn smoke_test() {
    serial_println!("[TCP] Running TCP smoke test...");

    // Test 1: Listen and close
    match listen(12345) {
        Ok(h) => {
            serial_println!("[TCP] Smoke: listening on port 12345");
            close(h);
            serial_println!("[TCP] Smoke: listen socket closed");
        }
        Err(e) => serial_println!("[TCP] Smoke: listen failed: {:?}", e),
    }

    // Test 2: Attempt connection to gateway (exercises SYN TX path and
    // handles SYN-ACK or RST from QEMU user-mode NAT)
    let target = Ipv4Addr::new(10, 0, 2, 2);
    match connect(&target, 80, 3000) {
        Ok(handle) => {
            serial_println!("[TCP] Smoke: connected to 10.0.2.2:80");
            // Send a minimal HTTP request
            let req = b"GET / HTTP/1.0\r\nHost: 10.0.2.2\r\n\r\n";
            match send(handle, req) {
                Ok(n) => serial_println!("[TCP] Smoke: sent {} bytes", n),
                Err(e) => serial_println!("[TCP] Smoke: send failed: {:?}", e),
            }
            // Try to receive a response
            let mut buf = [0u8; 512];
            match recv_timeout(handle, &mut buf, 3000) {
                Ok((n, _, _)) => serial_println!("[TCP] Smoke: received {} bytes", n),
                Err(e) => serial_println!("[TCP] Smoke: recv: {:?}", e),
            }
            close(handle);
            serial_println!("[TCP] Smoke: connection closed");
        }
        Err(e) => serial_println!("[TCP] Smoke: connect result: {:?}", e),
    }

    serial_println!("[TCP] Smoke test done");
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("[TCP] Initializing TCP subsystem...");

    unsafe {
        for c in CONNECTIONS.iter_mut() {
            c.reset();
        }
        NEXT_EPHEMERAL = 49152;
        ISS_COUNTER = 0;
    }

    // Register with IP layer
    ipv4::register_handler(IP_PROTO_TCP, tcp_ip_receive);

    serial_println!("[TCP] TCP initialized");
}
