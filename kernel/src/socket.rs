use net::{Ipv4Addr, NetError};

use crate::icmp;
use crate::serial_println;
use crate::{tcp, udp};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_SOCKETS: usize = 32;
const TCP_TIMEOUT_MS: u64 = 30_000;

// Socket protocol types
const PROTO_TCP: u8 = 1;
const PROTO_UDP: u8 = 2;
const PROTO_ICMP: u8 = 3;

const IPPROTO_ICMP: u32 = 1;
const IPPROTO_TCP: u32 = 6;
const IPPROTO_UDP: u32 = 17;

// Socket states
const STATE_CREATED: u8 = 0;
const STATE_BOUND: u8 = 1;
const STATE_LISTENING: u8 = 2;
const STATE_CONNECTED: u8 = 3;
const STATE_SHUTDOWN: u8 = 4;

// Errno values (matching Linux ABI)
const EBADF: i64 = 9;
const EAGAIN: i64 = 11;
const ENOMEM: i64 = 12;
const EINVAL: i64 = 22;
const ENOTSOCK: i64 = 88;
const EPROTONOSUPPORT: i64 = 93;
const EOPNOTSUPP: i64 = 95;
const EADDRINUSE: i64 = 98;
const ENOTCONN: i64 = 107;
const ETIMEDOUT: i64 = 110;
const ECONNREFUSED: i64 = 111;
const EINPROGRESS: i64 = 115;

// ---------------------------------------------------------------------------
// Socket table entry
// ---------------------------------------------------------------------------

struct SocketEntry {
    used: bool,
    protocol: u8,
    state: u8,
    handle: u16, // tcp/udp connection/binding handle
    local_port: u16,
    remote_port: u16,
    remote_ip: Ipv4Addr,
    nonblocking: bool,
}

impl SocketEntry {
    const fn empty() -> Self {
        Self {
            used: false,
            protocol: 0,
            state: STATE_CREATED,
            handle: 0,
            local_port: 0,
            remote_port: 0,
            remote_ip: Ipv4Addr::ZERO,
            nonblocking: false,
        }
    }

    fn reset(&mut self) {
        self.used = false;
        self.protocol = 0;
        self.state = STATE_CREATED;
        self.handle = 0;
        self.local_port = 0;
        self.remote_port = 0;
        self.remote_ip = Ipv4Addr::ZERO;
        self.nonblocking = false;
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

macro_rules! empty_sockets {
    ($n:expr) => {{
        const EMPTY: SocketEntry = SocketEntry::empty();
        [EMPTY; $n]
    }};
}

static mut SOCKETS: [SocketEntry; MAX_SOCKETS] = empty_sockets!(MAX_SOCKETS);

// ---------------------------------------------------------------------------
// NetError → errno mapping
// ---------------------------------------------------------------------------

fn net_to_errno(err: NetError) -> i64 {
    match err {
        NetError::NotInitialized => EBADF,
        NetError::BufferTooLarge => EINVAL,
        NetError::TransmitFailed => 5, // EIO
        NetError::NoRoute => 101,      // ENETUNREACH
        NetError::ArpFailed => 101,    // ENETUNREACH
        NetError::Timeout => ETIMEDOUT,
        NetError::PortInUse => EADDRINUSE,
        NetError::NoResources => ENOMEM,
        NetError::ConnectionRefused => ECONNREFUSED,
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn alloc_socket() -> Option<u16> {
    unsafe {
        for (i, s) in SOCKETS.iter().enumerate() {
            if !s.used {
                return Some(i as u16);
            }
        }
    }
    None
}

fn get_socket(id: u16) -> Result<&'static mut SocketEntry, i64> {
    let idx = id as usize;
    if idx >= MAX_SOCKETS {
        return Err(-ENOTSOCK);
    }
    unsafe {
        if !SOCKETS[idx].used {
            return Err(-ENOTSOCK);
        }
        Ok(&mut SOCKETS[idx])
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new socket. `sock_type`: 1=SOCK_STREAM, 2=SOCK_DGRAM.
pub fn socket_create(sock_type: u32, protocol: u32) -> Result<u16, i64> {
    let proto = match sock_type {
        1 => {
            if protocol == 0 || protocol == IPPROTO_TCP {
                PROTO_TCP
            } else {
                return Err(-EPROTONOSUPPORT);
            }
        }
        2 => {
            if protocol == 0 || protocol == IPPROTO_UDP {
                PROTO_UDP
            } else if protocol == IPPROTO_ICMP {
                PROTO_ICMP
            } else {
                return Err(-EPROTONOSUPPORT);
            }
        }
        _ => return Err(-EPROTONOSUPPORT),
    };

    let id = alloc_socket().ok_or(-ENOMEM)?;
    let icmp_handle = if proto == PROTO_ICMP {
        Some(icmp::socket_open().map_err(|e| -net_to_errno(e))? as u16)
    } else {
        None
    };
    unsafe {
        let s = &mut SOCKETS[id as usize];
        s.reset();
        s.used = true;
        s.protocol = proto;
        s.state = STATE_CREATED;
        s.handle = icmp_handle.unwrap_or(0);
    }
    serial_println!(
        "[SOCK] Created socket {} ({})",
        id,
        match proto {
            PROTO_TCP => "TCP",
            PROTO_UDP => "UDP",
            PROTO_ICMP => "ICMP",
            _ => "?",
        }
    );
    Ok(id)
}

/// Bind a socket to a local address/port.
pub fn socket_bind(id: u16, _ip: &Ipv4Addr, port: u16) -> Result<(), i64> {
    let s = get_socket(id)?;
    if s.state != STATE_CREATED {
        return Err(-EINVAL);
    }

    if s.protocol == PROTO_UDP {
        let handle = udp::bind(port).map_err(|e| -net_to_errno(e))?;
        s.handle = handle as u16;
        s.local_port = port;
        s.state = STATE_BOUND;
    } else if s.protocol == PROTO_ICMP {
        // ICMP datagram sockets use protocol dispatch, not ports.
        if port != 0 {
            return Err(-EINVAL);
        }
        s.state = STATE_BOUND;
    } else {
        // TCP: just record the port, actual binding happens at listen/connect
        s.local_port = port;
        s.state = STATE_BOUND;
    }

    serial_println!("[SOCK] Bound socket {} to port {}", id, port);
    Ok(())
}

/// Start listening on a TCP socket.
pub fn socket_listen(id: u16, _backlog: u32) -> Result<(), i64> {
    let s = get_socket(id)?;
    if s.protocol != PROTO_TCP {
        return Err(-EOPNOTSUPP);
    }
    if s.state != STATE_BOUND {
        return Err(-EINVAL);
    }

    let port = s.local_port;
    let handle = tcp::listen(port).map_err(|e| -net_to_errno(e))?;
    s.handle = handle as u16;
    s.state = STATE_LISTENING;

    serial_println!("[SOCK] Socket {} listening on port {}", id, port);
    Ok(())
}

/// Connect a socket to a remote address.
pub fn socket_connect(id: u16, ip: &Ipv4Addr, port: u16, nonblocking: bool) -> Result<(), i64> {
    let s = get_socket(id)?;

    if s.protocol == PROTO_UDP || s.protocol == PROTO_ICMP {
        // UDP "connect" just stores the default destination
        s.remote_ip.copy_from(ip);
        s.remote_port = port;
        s.state = STATE_CONNECTED;
        return Ok(());
    }

    // TCP connect
    if s.state != STATE_CREATED && s.state != STATE_BOUND {
        return Err(-EINVAL);
    }

    if nonblocking {
        return Err(-EINPROGRESS);
    }

    let handle = tcp::connect(ip, port, TCP_TIMEOUT_MS).map_err(|e| -net_to_errno(e))?;
    s.handle = handle as u16;
    s.state = STATE_CONNECTED;

    // Fill in connection info from TCP layer
    if let Some((lp, rip, rp)) = tcp::connection_info(handle) {
        s.local_port = lp;
        s.remote_ip.copy_from(&rip);
        s.remote_port = rp;
    }

    serial_println!(
        "[SOCK] Socket {} connected to {}.{}.{}.{}:{}",
        id,
        ip.0[0],
        ip.0[1],
        ip.0[2],
        ip.0[3],
        port
    );
    Ok(())
}

/// Accept a connection on a listening TCP socket.
/// Returns (new_socket_id, remote_ip, remote_port).
pub fn socket_accept(id: u16, nonblocking: bool) -> Result<(u16, Ipv4Addr, u16), i64> {
    let (listen_handle, listen_port) = {
        let s = get_socket(id)?;
        if s.protocol != PROTO_TCP || s.state != STATE_LISTENING {
            return Err(-EINVAL);
        }
        (s.handle as usize, s.local_port)
    };

    if nonblocking {
        return Err(-EAGAIN);
    }

    let child_handle = tcp::accept(listen_handle, TCP_TIMEOUT_MS).map_err(|e| -net_to_errno(e))?;

    let (remote_ip, remote_port) = match tcp::connection_info(child_handle) {
        Some((_, rip, rp)) => (rip, rp),
        None => return Err(-EINVAL),
    };

    // Allocate a new socket for the accepted connection
    let new_id = alloc_socket().ok_or(-ENOMEM)?;
    unsafe {
        let ns = &mut SOCKETS[new_id as usize];
        ns.reset();
        ns.used = true;
        ns.protocol = PROTO_TCP;
        ns.state = STATE_CONNECTED;
        ns.handle = child_handle as u16;
        ns.local_port = listen_port;
        ns.remote_ip.copy_from(&remote_ip);
        ns.remote_port = remote_port;
    }

    serial_println!(
        "[SOCK] Socket {} accepted connection -> socket {} from {}.{}.{}.{}:{}",
        id,
        new_id,
        remote_ip.0[0],
        remote_ip.0[1],
        remote_ip.0[2],
        remote_ip.0[3],
        remote_port
    );
    Ok((new_id, remote_ip, remote_port))
}

/// Send data on a connected socket.
pub fn socket_send(id: u16, data: &[u8], nonblocking: bool) -> Result<usize, i64> {
    let s = get_socket(id)?;

    if s.protocol == PROTO_TCP {
        if s.state != STATE_CONNECTED {
            return Err(-ENOTCONN);
        }
        let handle = s.handle as usize;
        if nonblocking && !tcp::is_connected(handle) {
            return Err(-EAGAIN);
        }
        tcp::send(handle, data).map_err(|e| -net_to_errno(e))
    } else if s.protocol == PROTO_UDP {
        // UDP send to connected destination
        if s.remote_ip.is_zero() && s.remote_port == 0 {
            return Err(-ENOTCONN);
        }
        let rip = s.remote_ip;
        let rport = s.remote_port;
        let handle = s.handle as usize;

        // Auto-bind if not yet bound
        if s.state == STATE_CREATED {
            let (h, port) = udp::bind_ephemeral().map_err(|e| -net_to_errno(e))?;
            // Re-borrow after bind
            let s = get_socket(id)?;
            s.handle = h as u16;
            s.local_port = port;
            s.state = STATE_BOUND;
            udp::send_from(h, &rip, rport, data).map_err(|e| -net_to_errno(e))?;
        } else {
            udp::send_from(handle, &rip, rport, data).map_err(|e| -net_to_errno(e))?;
        }
        Ok(data.len())
    } else {
        // ICMP datagram send to connected destination.
        if s.remote_ip.is_zero() {
            return Err(-ENOTCONN);
        }
        let rip = s.remote_ip;
        let handle = s.handle as usize;
        icmp::socket_sendto(handle, &rip, data).map_err(|e| -net_to_errno(e))?;
        Ok(data.len())
    }
}

/// Send data to a specific destination (UDP sendto).
pub fn socket_sendto(id: u16, data: &[u8], ip: &Ipv4Addr, port: u16) -> Result<usize, i64> {
    let s = get_socket(id)?;
    if s.protocol == PROTO_ICMP {
        let handle = s.handle as usize;
        icmp::socket_sendto(handle, ip, data).map_err(|e| -net_to_errno(e))?;
        return Ok(data.len());
    }

    if s.protocol != PROTO_UDP {
        // TCP ignores dest addr in sendto, just use send
        return socket_send(id, data, false);
    }

    // Auto-bind if not yet bound
    if s.state == STATE_CREATED {
        let (h, lport) = udp::bind_ephemeral().map_err(|e| -net_to_errno(e))?;
        let s = get_socket(id)?;
        s.handle = h as u16;
        s.local_port = lport;
        s.state = STATE_BOUND;
    }

    let handle = {
        let s = get_socket(id)?;
        s.handle as usize
    };
    udp::send_from(handle, ip, port, data).map_err(|e| -net_to_errno(e))?;
    Ok(data.len())
}

/// Receive data from a connected socket.
pub fn socket_recv(id: u16, buf: &mut [u8], nonblocking: bool) -> Result<usize, i64> {
    let s = get_socket(id)?;

    if s.protocol == PROTO_TCP {
        if s.state != STATE_CONNECTED && s.state != STATE_SHUTDOWN {
            return Err(-ENOTCONN);
        }
        let handle = s.handle as usize;

        if nonblocking {
            match tcp::recv(handle, buf) {
                Some((n, _, _)) => Ok(n),
                None => {
                    // Check if connection is still alive
                    if !tcp::is_connected(handle) {
                        return Ok(0); // EOF
                    }
                    Err(-EAGAIN)
                }
            }
        } else {
            match tcp::recv_timeout(handle, buf, TCP_TIMEOUT_MS) {
                Ok((n, _, _)) => Ok(n),
                Err(NetError::Timeout) => Err(-EAGAIN),
                Err(NetError::NotInitialized) => Ok(0), // connection closed = EOF
                Err(e) => Err(-net_to_errno(e)),
            }
        }
    } else if s.protocol == PROTO_UDP {
        // UDP recv
        let handle = s.handle as usize;
        if s.state == STATE_CREATED {
            return Err(-EINVAL); // not bound
        }

        if nonblocking {
            match udp::recv(handle, buf) {
                Some((n, _, _)) => Ok(n),
                None => Err(-EAGAIN),
            }
        } else {
            match udp::recv_timeout(handle, buf, TCP_TIMEOUT_MS) {
                Ok((n, _, _)) => Ok(n),
                Err(NetError::Timeout) => Err(-EAGAIN),
                Err(e) => Err(-net_to_errno(e)),
            }
        }
    } else {
        // ICMP recv
        let handle = s.handle as usize;
        if nonblocking {
            match icmp::socket_recv(handle, buf) {
                Some((n, _)) => Ok(n),
                None => Err(-EAGAIN),
            }
        } else {
            match icmp::socket_recv_timeout(handle, buf, TCP_TIMEOUT_MS) {
                Ok((n, _)) => Ok(n),
                Err(NetError::Timeout) => Err(-EAGAIN),
                Err(e) => Err(-net_to_errno(e)),
            }
        }
    }
}

/// Receive data from a socket, also returning source address.
pub fn socket_recvfrom(
    id: u16,
    buf: &mut [u8],
    nonblocking: bool,
) -> Result<(usize, Ipv4Addr, u16), i64> {
    let s = get_socket(id)?;

    if s.protocol == PROTO_TCP {
        let handle = s.handle as usize;
        let rip = s.remote_ip;
        let rport = s.remote_port;

        if s.state != STATE_CONNECTED && s.state != STATE_SHUTDOWN {
            return Err(-ENOTCONN);
        }

        if nonblocking {
            match tcp::recv(handle, buf) {
                Some((n, _, _)) => Ok((n, rip, rport)),
                None => {
                    if !tcp::is_connected(handle) {
                        return Ok((0, rip, rport));
                    }
                    Err(-EAGAIN)
                }
            }
        } else {
            match tcp::recv_timeout(handle, buf, TCP_TIMEOUT_MS) {
                Ok((n, _, _)) => Ok((n, rip, rport)),
                Err(NetError::Timeout) => Err(-EAGAIN),
                Err(NetError::NotInitialized) => Ok((0, rip, rport)),
                Err(e) => Err(-net_to_errno(e)),
            }
        }
    } else if s.protocol == PROTO_UDP {
        let handle = s.handle as usize;
        if s.state == STATE_CREATED {
            return Err(-EINVAL);
        }

        if nonblocking {
            match udp::recv(handle, buf) {
                Some((n, ip, port)) => Ok((n, ip, port)),
                None => Err(-EAGAIN),
            }
        } else {
            match udp::recv_timeout(handle, buf, TCP_TIMEOUT_MS) {
                Ok((n, ip, port)) => Ok((n, ip, port)),
                Err(NetError::Timeout) => Err(-EAGAIN),
                Err(e) => Err(-net_to_errno(e)),
            }
        }
    } else {
        let handle = s.handle as usize;
        if nonblocking {
            match icmp::socket_recv(handle, buf) {
                Some((n, ip)) => Ok((n, ip, 0)),
                None => Err(-EAGAIN),
            }
        } else {
            match icmp::socket_recv_timeout(handle, buf, TCP_TIMEOUT_MS) {
                Ok((n, ip)) => Ok((n, ip, 0)),
                Err(NetError::Timeout) => Err(-EAGAIN),
                Err(e) => Err(-net_to_errno(e)),
            }
        }
    }
}

// Poll event bits (match Linux ABI)
const POLLIN: u16 = 0x0001;
const POLLOUT: u16 = 0x0004;
const POLLHUP: u16 = 0x0010;

/// Return a bitmask of POLLIN/POLLOUT/POLLHUP for socket `id`.
pub fn socket_poll_events(id: u16) -> u16 {
    let s = match get_socket(id) {
        Ok(s) => s,
        Err(_) => return 0,
    };

    let mut events: u16 = 0;

    if s.protocol == PROTO_TCP {
        let handle = s.handle as usize;
        match s.state {
            STATE_CONNECTED => {
                if tcp::has_rx_data(handle) || !tcp::is_connected(handle) {
                    events |= POLLIN;
                }
                if tcp::can_send(handle) {
                    events |= POLLOUT;
                }
                if !tcp::is_connected(handle) {
                    events |= POLLHUP;
                }
            }
            STATE_LISTENING => {
                if tcp::has_pending_accept(s.local_port) {
                    events |= POLLIN;
                }
            }
            STATE_SHUTDOWN => {
                events |= POLLHUP;
                if tcp::has_rx_data(handle) {
                    events |= POLLIN;
                }
            }
            _ => {}
        }
    } else if s.protocol == PROTO_UDP {
        // UDP
        let handle = s.handle as usize;
        if s.state == STATE_BOUND || s.state == STATE_CONNECTED {
            if udp::rx_pending(handle) > 0 {
                events |= POLLIN;
            }
            events |= POLLOUT; // UDP can always send
        }
    } else if s.protocol == PROTO_ICMP {
        let handle = s.handle as usize;
        if icmp::socket_rx_pending(handle) > 0 {
            events |= POLLIN;
        }
        events |= POLLOUT;
    }

    events
}

/// Shutdown a socket (partial close).
pub fn socket_shutdown(id: u16, _how: i32) -> Result<(), i64> {
    let s = get_socket(id)?;
    if s.protocol == PROTO_TCP && s.state == STATE_CONNECTED {
        tcp::close(s.handle as usize);
        s.state = STATE_SHUTDOWN;
    }
    Ok(())
}

/// Close and free a socket.
pub fn socket_close(id: u16) -> Result<(), i64> {
    let idx = id as usize;
    if idx >= MAX_SOCKETS {
        return Err(-ENOTSOCK);
    }
    unsafe {
        if !SOCKETS[idx].used {
            return Ok(());
        }

        let s = &mut SOCKETS[idx];
        if s.protocol == PROTO_TCP {
            if s.state == STATE_CONNECTED || s.state == STATE_LISTENING {
                tcp::close(s.handle as usize);
            }
        } else if s.protocol == PROTO_UDP {
            if s.state == STATE_BOUND || s.state == STATE_CONNECTED {
                udp::unbind(s.handle as usize);
            }
        } else if s.protocol == PROTO_ICMP {
            icmp::socket_close(s.handle as usize);
        }

        serial_println!("[SOCK] Closed socket {}", id);
        s.reset();
    }
    Ok(())
}

/// Get local address/port for a socket.
pub fn socket_getsockname(id: u16) -> Result<(Ipv4Addr, u16), i64> {
    let s = get_socket(id)?;
    let cfg = crate::net::get_config();
    Ok((cfg.ip, s.local_port))
}

/// Get remote address/port for a connected socket.
pub fn socket_getpeername(id: u16) -> Result<(Ipv4Addr, u16), i64> {
    let s = get_socket(id)?;
    if s.state != STATE_CONNECTED {
        return Err(-ENOTCONN);
    }
    Ok((s.remote_ip, s.remote_port))
}

// ---------------------------------------------------------------------------
// Smoke test
// ---------------------------------------------------------------------------

#[cfg(feature = "net-socket-smoke")]
pub fn smoke_test() {
    serial_println!("[SOCK] Running socket smoke test...");

    // Test 1: TCP socket → bind → listen → close
    match socket_create(1, IPPROTO_TCP) {
        Ok(id) => {
            serial_println!("[SOCK] Smoke: created TCP socket {}", id);
            match socket_bind(id, &Ipv4Addr::ZERO, 8080) {
                Ok(()) => serial_println!("[SOCK] Smoke: bound to port 8080"),
                Err(e) => serial_println!("[SOCK] Smoke: bind failed: {}", e),
            }
            match socket_listen(id, 5) {
                Ok(()) => serial_println!("[SOCK] Smoke: listening"),
                Err(e) => serial_println!("[SOCK] Smoke: listen failed: {}", e),
            }
            let _ = socket_close(id);
            serial_println!("[SOCK] Smoke: TCP socket closed");
        }
        Err(e) => serial_println!("[SOCK] Smoke: TCP create failed: {}", e),
    }

    // Test 2: UDP socket → bind → close
    match socket_create(2, IPPROTO_UDP) {
        Ok(id) => {
            serial_println!("[SOCK] Smoke: created UDP socket {}", id);
            match socket_bind(id, &Ipv4Addr::ZERO, 9090) {
                Ok(()) => serial_println!("[SOCK] Smoke: bound to port 9090"),
                Err(e) => serial_println!("[SOCK] Smoke: bind failed: {}", e),
            }
            let _ = socket_close(id);
            serial_println!("[SOCK] Smoke: UDP socket closed");
        }
        Err(e) => serial_println!("[SOCK] Smoke: UDP create failed: {}", e),
    }

    // Test 3: TCP connect to gateway (exercises active open path)
    match socket_create(1, IPPROTO_TCP) {
        Ok(id) => {
            serial_println!("[SOCK] Smoke: attempting connect to 10.0.2.2:80");
            let target = Ipv4Addr::new(10, 0, 2, 2);
            match socket_connect(id, &target, 80, false) {
                Ok(()) => {
                    serial_println!("[SOCK] Smoke: connected to gateway");
                    let _ = socket_close(id);
                }
                Err(e) => {
                    serial_println!("[SOCK] Smoke: connect result: {}", e);
                    let _ = socket_close(id);
                }
            }
        }
        Err(e) => serial_println!("[SOCK] Smoke: TCP create failed: {}", e),
    }

    serial_println!("[SOCK] Socket smoke test done");
}
