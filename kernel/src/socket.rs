use net::{Ipv4Addr, NetError};

use crate::icmp;
use crate::serial_println;
use crate::task;
use crate::{tcp, udp};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Socket table size. A libwayland compositor holds one listening socket
/// per display socket (`wl_display_add_socket_auto` tries up to 33) plus
/// one per client.
const MAX_SOCKETS: usize = 256;
const TCP_TIMEOUT_MS: u64 = 30_000;
const UNIX_STREAM_BUF_SIZE: usize = 2048;
const UNIX_NO_PEER: u16 = u16::MAX;
const UNIX_PATH_MAX: usize = 108;
const UNIX_BINDINGS_MAX: usize = 128;
const UNIX_ACCEPT_QUEUE_SIZE: usize = 8;
const UNIX_RIGHTS_QUEUE_SIZE: usize = 8;
pub const UNIX_RIGHTS_MAX_FDS: usize = 8;

// Socket protocol types
const PROTO_TCP: u8 = 1;
const PROTO_UDP: u8 = 2;
const PROTO_ICMP: u8 = 3;
const PROTO_UNIX: u8 = 4;

const AF_UNIX: u32 = 1;
const AF_INET: u32 = 2;

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
const ENOENT: i64 = 2;
const ENOTDIR: i64 = 20;
const EINVAL: i64 = 22;
/// Kernel-internal: wait interrupted by a signal (see `signal::ERESTARTSYS`).
const ERESTARTSYS: i64 = crate::signal::ERESTARTSYS;
const EROFS: i64 = 30;
const ENOTSOCK: i64 = 88;
const EPROTONOSUPPORT: i64 = 93;
const EOPNOTSUPP: i64 = 95;
const EADDRINUSE: i64 = 98;
const ENOTCONN: i64 = 107;
const ETIMEDOUT: i64 = 110;
const ECONNREFUSED: i64 = 111;
const EINPROGRESS: i64 = 115;
const EPIPE: i64 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnixRightsFd {
    pub kind: u8,
    pub inode: u16,
    pub flags: u32,
    pub offset: u64,
}

impl UnixRightsFd {
    pub const EMPTY: Self = Self {
        kind: task::USER_FD_KIND_EMPTY,
        inode: task::USER_FD_NONE,
        flags: 0,
        offset: 0,
    };
}

#[derive(Clone, Copy)]
pub struct UnixRightsMessage {
    pub count: usize,
    pub fds: [UnixRightsFd; UNIX_RIGHTS_MAX_FDS],
}

// ---------------------------------------------------------------------------
// Socket table entry
// ---------------------------------------------------------------------------

/// Process credentials recorded on an `AF_UNIX` endpoint for `SO_PEERCRED`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnixCred {
    pub pid: i32,
    pub uid: u32,
    pub gid: u32,
}

impl UnixCred {
    pub const NONE: Self = Self {
        pid: 0,
        uid: u32::MAX,
        gid: u32::MAX,
    };
}

struct SocketEntry {
    /// Credentials of the peer process, captured when the connection formed.
    unix_peer_cred: UnixCred,
    unix_peer_cred_valid: bool,
    /// Credentials of the process that called `listen()` (handed to clients).
    unix_listen_cred: UnixCred,
    used: bool,
    refs: u16,
    protocol: u8,
    state: u8,
    handle: u16, // tcp/udp connection/binding handle
    local_port: u16,
    remote_port: u16,
    remote_ip: Ipv4Addr,
    nonblocking: bool,
    unix_peer: u16,
    unix_rx_buf: [u8; UNIX_STREAM_BUF_SIZE],
    unix_rx_head: u16,
    unix_rx_tail: u16,
    unix_rx_len: u16,
    unix_peer_open: bool,
    unix_bound_path_len: u8,
    unix_bound_path: [u8; UNIX_PATH_MAX],
    unix_accept_q: [u16; UNIX_ACCEPT_QUEUE_SIZE],
    unix_accept_q_head: u8,
    unix_accept_q_tail: u8,
    unix_accept_q_len: u8,
    unix_rights_q: [UnixRightsQueueEntry; UNIX_RIGHTS_QUEUE_SIZE],
    unix_rights_q_head: u8,
    unix_rights_q_tail: u8,
    unix_rights_q_len: u8,
}

impl SocketEntry {
    const fn empty() -> Self {
        Self {
            unix_peer_cred: UnixCred::NONE,
            unix_peer_cred_valid: false,
            unix_listen_cred: UnixCred::NONE,
            used: false,
            refs: 0,
            protocol: 0,
            state: STATE_CREATED,
            handle: 0,
            local_port: 0,
            remote_port: 0,
            remote_ip: Ipv4Addr::ZERO,
            nonblocking: false,
            unix_peer: UNIX_NO_PEER,
            unix_rx_buf: [0; UNIX_STREAM_BUF_SIZE],
            unix_rx_head: 0,
            unix_rx_tail: 0,
            unix_rx_len: 0,
            unix_peer_open: false,
            unix_bound_path_len: 0,
            unix_bound_path: [0; UNIX_PATH_MAX],
            unix_accept_q: [UNIX_NO_PEER; UNIX_ACCEPT_QUEUE_SIZE],
            unix_accept_q_head: 0,
            unix_accept_q_tail: 0,
            unix_accept_q_len: 0,
            unix_rights_q: [UnixRightsQueueEntry::empty(); UNIX_RIGHTS_QUEUE_SIZE],
            unix_rights_q_head: 0,
            unix_rights_q_tail: 0,
            unix_rights_q_len: 0,
        }
    }

    fn reset(&mut self) {
        self.unix_peer_cred = UnixCred::NONE;
        self.unix_peer_cred_valid = false;
        self.unix_listen_cred = UnixCred::NONE;
        self.used = false;
        self.refs = 0;
        self.protocol = 0;
        self.state = STATE_CREATED;
        self.handle = 0;
        self.local_port = 0;
        self.remote_port = 0;
        self.remote_ip = Ipv4Addr::ZERO;
        self.nonblocking = false;
        self.unix_peer = UNIX_NO_PEER;
        self.unix_rx_head = 0;
        self.unix_rx_tail = 0;
        self.unix_rx_len = 0;
        self.unix_peer_open = false;
        self.unix_bound_path_len = 0;
        self.unix_accept_q_head = 0;
        self.unix_accept_q_tail = 0;
        self.unix_accept_q_len = 0;
        self.unix_rights_q_head = 0;
        self.unix_rights_q_tail = 0;
        self.unix_rights_q_len = 0;
        self.unix_rx_buf.fill(0);
        self.unix_bound_path.fill(0);
        self.unix_accept_q.fill(UNIX_NO_PEER);
        self.unix_rights_q.fill(UnixRightsQueueEntry::empty());
    }
}

#[derive(Clone, Copy)]
struct UnixRightsQueueEntry {
    count: u8,
    fds: [UnixRightsFd; UNIX_RIGHTS_MAX_FDS],
}

impl UnixRightsQueueEntry {
    const fn empty() -> Self {
        Self {
            count: 0,
            fds: [UnixRightsFd::EMPTY; UNIX_RIGHTS_MAX_FDS],
        }
    }
}

#[derive(Clone, Copy)]
struct UnixBinding {
    used: bool,
    listener: u16,
    /// VFS node created by `bind`; the binding is only reachable while the
    /// path still names this node (so `unlink` + re-`bind` works).
    inode: u16,
    path_len: u8,
    path: [u8; UNIX_PATH_MAX],
}

impl UnixBinding {
    const fn empty() -> Self {
        Self {
            used: false,
            listener: UNIX_NO_PEER,
            inode: 0,
            path_len: 0,
            path: [0; UNIX_PATH_MAX],
        }
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
static mut UNIX_BINDINGS: [UnixBinding; UNIX_BINDINGS_MAX] =
    [const { UnixBinding::empty() }; UNIX_BINDINGS_MAX];

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
        NetError::Interrupted => ERESTARTSYS,
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

#[inline]
fn unix_accept_queue_push(s: &mut SocketEntry, sock_id: u16) -> bool {
    if s.unix_accept_q_len as usize >= UNIX_ACCEPT_QUEUE_SIZE {
        return false;
    }
    s.unix_accept_q[s.unix_accept_q_head as usize] = sock_id;
    s.unix_accept_q_head = ((s.unix_accept_q_head as usize + 1) % UNIX_ACCEPT_QUEUE_SIZE) as u8;
    s.unix_accept_q_len += 1;
    true
}

#[inline]
fn unix_accept_queue_pop(s: &mut SocketEntry) -> Option<u16> {
    if s.unix_accept_q_len == 0 {
        return None;
    }
    let out = s.unix_accept_q[s.unix_accept_q_tail as usize];
    s.unix_accept_q[s.unix_accept_q_tail as usize] = UNIX_NO_PEER;
    s.unix_accept_q_tail = ((s.unix_accept_q_tail as usize + 1) % UNIX_ACCEPT_QUEUE_SIZE) as u8;
    s.unix_accept_q_len -= 1;
    if out == UNIX_NO_PEER {
        None
    } else {
        Some(out)
    }
}

#[inline]
fn unix_rights_queue_push(s: &mut SocketEntry, msg: UnixRightsQueueEntry) -> bool {
    if s.unix_rights_q_len as usize >= UNIX_RIGHTS_QUEUE_SIZE {
        return false;
    }
    s.unix_rights_q[s.unix_rights_q_head as usize] = msg;
    s.unix_rights_q_head = ((s.unix_rights_q_head as usize + 1) % UNIX_RIGHTS_QUEUE_SIZE) as u8;
    s.unix_rights_q_len += 1;
    true
}

#[inline]
fn unix_rights_queue_pop(s: &mut SocketEntry) -> Option<UnixRightsQueueEntry> {
    if s.unix_rights_q_len == 0 {
        return None;
    }
    let out = s.unix_rights_q[s.unix_rights_q_tail as usize];
    s.unix_rights_q[s.unix_rights_q_tail as usize] = UnixRightsQueueEntry::empty();
    s.unix_rights_q_tail = ((s.unix_rights_q_tail as usize + 1) % UNIX_RIGHTS_QUEUE_SIZE) as u8;
    s.unix_rights_q_len -= 1;
    if out.count == 0 {
        None
    } else {
        Some(out)
    }
}

fn unix_rights_retain_one(fd: &UnixRightsFd) -> Result<(), i64> {
    if fd.inode == task::USER_FD_NONE {
        return Err(-EINVAL);
    }
    match fd.kind {
        task::USER_FD_KIND_EMPTY | task::USER_FD_KIND_CONSOLE => Err(-EOPNOTSUPP),
        kind => crate::syscall::fd_object_retain(kind, fd.inode, fd.flags),
    }
}

fn unix_rights_release_one(fd: &UnixRightsFd) {
    if fd.inode == task::USER_FD_NONE {
        return;
    }
    match fd.kind {
        task::USER_FD_KIND_EMPTY | task::USER_FD_KIND_CONSOLE => {}
        kind => crate::syscall::fd_object_release(kind, fd.inode, fd.flags),
    }
}

fn unix_rights_drain_socket(s: &mut SocketEntry) {
    while let Some(msg) = unix_rights_queue_pop(s) {
        let mut idx = 0usize;
        while idx < msg.count as usize {
            unix_rights_release_one(&msg.fds[idx]);
            idx += 1;
        }
    }
}

#[inline]
fn unix_path_eq(binding: &UnixBinding, path: &[u8]) -> bool {
    binding.path_len as usize == path.len() && binding.path[..path.len()] == *path
}

fn unix_binding_lookup(path: &[u8]) -> Option<usize> {
    unsafe {
        let mut idx = 0usize;
        while idx < UNIX_BINDINGS_MAX {
            let b = &UNIX_BINDINGS[idx];
            if b.used && unix_path_eq(b, path) && unix_binding_vfs_current(path, b.inode) {
                return Some(idx);
            }
            idx += 1;
        }
    }
    None
}

fn unix_binding_alloc(listener: u16, path: &[u8], inode: u16) -> Result<(), i64> {
    if path.is_empty() || path.len() > UNIX_PATH_MAX {
        return Err(-EINVAL);
    }
    if unix_binding_lookup(path).is_some() {
        return Err(-EADDRINUSE);
    }
    unsafe {
        // A listener whose path was unlinked is unreachable; drop its stale
        // entry so the path can be bound again.
        let mut idx = 0usize;
        while idx < UNIX_BINDINGS_MAX {
            let b = &mut UNIX_BINDINGS[idx];
            if b.used && unix_path_eq(b, path) {
                b.path_len = 0;
                b.path.fill(0);
            }
            idx += 1;
        }
        let mut idx = 0usize;
        while idx < UNIX_BINDINGS_MAX {
            if !UNIX_BINDINGS[idx].used {
                let b = &mut UNIX_BINDINGS[idx];
                b.used = true;
                b.listener = listener;
                b.inode = inode;
                b.path_len = path.len() as u8;
                b.path.fill(0);
                b.path[..path.len()].copy_from_slice(path);
                return Ok(());
            }
            idx += 1;
        }
    }
    Err(-ENOMEM)
}

/// Create the socket's filesystem node (`bind` fails with `EADDRINUSE` if
/// the path exists, as on Linux). Returns its inode.
#[cfg(not(test))]
fn unix_binding_vfs_claim(path: &[u8]) -> Result<u16, i64> {
    let path_str = core::str::from_utf8(path).map_err(|_| -EINVAL)?;
    match fs::open(path_str, fs::O_CREAT | fs::O_EXCL | fs::O_RDWR, 0o600) {
        Ok(opened) => Ok(opened.inode),
        Err(fs::EEXIST) => Err(-EADDRINUSE),
        Err(fs::ENOENT) => Err(-ENOENT),
        Err(fs::ENOTDIR) => Err(-ENOTDIR),
        Err(fs::EROFS) => Err(-EROFS),
        Err(_) => Err(-EINVAL),
    }
}

#[cfg(test)]
fn unix_binding_vfs_claim(_path: &[u8]) -> Result<u16, i64> {
    Ok(0)
}

/// True while `path` still names the node created when it was bound.
#[cfg(not(test))]
fn unix_binding_vfs_current(path: &[u8], inode: u16) -> bool {
    match core::str::from_utf8(path) {
        Ok(path_str) => fs::lookup(path_str) == Ok(inode),
        Err(_) => false,
    }
}

#[cfg(test)]
fn unix_binding_vfs_current(_path: &[u8], _inode: u16) -> bool {
    true
}

#[cfg(not(test))]
fn unix_binding_vfs_release(path: &[u8]) {
    let path_str = match core::str::from_utf8(path) {
        Ok(v) => v,
        Err(_) => return,
    };
    let _ = fs::unlink(path_str);
}

#[cfg(test)]
fn unix_binding_vfs_release(_path: &[u8]) {}

fn unix_binding_remove_listener(listener: u16) {
    unsafe {
        let mut idx = 0usize;
        while idx < UNIX_BINDINGS_MAX {
            let b = &mut UNIX_BINDINGS[idx];
            if b.used && b.listener == listener {
                *b = UnixBinding::empty();
            }
            idx += 1;
        }
    }
}

#[inline]
fn unix_rx_space(s: &SocketEntry) -> usize {
    UNIX_STREAM_BUF_SIZE.saturating_sub(s.unix_rx_len as usize)
}

fn unix_rx_push(s: &mut SocketEntry, byte: u8) -> bool {
    if s.unix_rx_len as usize >= UNIX_STREAM_BUF_SIZE {
        return false;
    }
    s.unix_rx_buf[s.unix_rx_head as usize] = byte;
    s.unix_rx_head = ((s.unix_rx_head as usize + 1) % UNIX_STREAM_BUF_SIZE) as u16;
    s.unix_rx_len += 1;
    true
}

fn unix_rx_pop(s: &mut SocketEntry, out: &mut [u8]) -> usize {
    if out.is_empty() || s.unix_rx_len == 0 {
        return 0;
    }
    let mut count = 0usize;
    let max = out.len().min(s.unix_rx_len as usize);
    while count < max {
        out[count] = s.unix_rx_buf[s.unix_rx_tail as usize];
        s.unix_rx_tail = ((s.unix_rx_tail as usize + 1) % UNIX_STREAM_BUF_SIZE) as u16;
        s.unix_rx_len -= 1;
        count += 1;
    }
    count
}

fn unix_stream_send(id: u16, data: &[u8]) -> Result<usize, i64> {
    if data.is_empty() {
        return Ok(0);
    }

    let idx = id as usize;
    if idx >= MAX_SOCKETS {
        return Err(-ENOTSOCK);
    }

    unsafe {
        let sockets = SOCKETS.as_mut_ptr();
        let src = &mut *sockets.add(idx);
        if !src.used || src.protocol != PROTO_UNIX {
            return Err(-ENOTSOCK);
        }
        if src.state != STATE_CONNECTED {
            return Err(-ENOTCONN);
        }
        if !src.unix_peer_open {
            return Err(-EPIPE);
        }

        let peer_id = src.unix_peer;
        if peer_id == UNIX_NO_PEER {
            src.unix_peer_open = false;
            return Err(-EPIPE);
        }
        let peer_idx = peer_id as usize;
        if peer_idx >= MAX_SOCKETS || peer_idx == idx {
            src.unix_peer_open = false;
            src.unix_peer = UNIX_NO_PEER;
            return Err(-EPIPE);
        }

        let peer = &mut *sockets.add(peer_idx);
        if !peer.used || peer.protocol != PROTO_UNIX {
            src.unix_peer_open = false;
            src.unix_peer = UNIX_NO_PEER;
            return Err(-EPIPE);
        }

        let writable = data.len().min(unix_rx_space(peer));
        if writable == 0 {
            return Err(-EAGAIN);
        }
        let mut i = 0usize;
        while i < writable {
            if !unix_rx_push(peer, data[i]) {
                break;
            }
            i += 1;
        }
        Ok(i)
    }
}

fn unix_stream_recv(id: u16, out: &mut [u8]) -> Result<usize, i64> {
    let idx = id as usize;
    if idx >= MAX_SOCKETS {
        return Err(-ENOTSOCK);
    }
    unsafe {
        let s = &mut SOCKETS[idx];
        if !s.used || s.protocol != PROTO_UNIX {
            return Err(-ENOTSOCK);
        }
        if s.state != STATE_CONNECTED && s.state != STATE_SHUTDOWN {
            return Err(-ENOTCONN);
        }

        let n = unix_rx_pop(s, out);
        if n > 0 {
            return Ok(n);
        }
        if !s.unix_peer_open {
            return Ok(0);
        }
        Err(-EAGAIN)
    }
}

fn unix_notify_peer_closed(id: u16) {
    let idx = id as usize;
    if idx >= MAX_SOCKETS {
        return;
    }
    unsafe {
        let peer_id = SOCKETS[idx].unix_peer;
        if peer_id == UNIX_NO_PEER {
            return;
        }
        let peer_idx = peer_id as usize;
        if peer_idx >= MAX_SOCKETS {
            return;
        }
        let peer = &mut SOCKETS[peer_idx];
        if !peer.used || peer.protocol != PROTO_UNIX {
            return;
        }
        if peer.unix_peer == id {
            peer.unix_peer_open = false;
            peer.unix_peer = UNIX_NO_PEER;
        }
    }
}

/// Let other tasks run while a socket call waits. Returns true when the wait
/// must end because a signal arrived or the task is exiting; the caller then
/// fails with `-ERESTARTSYS`.
#[cfg(not(test))]
#[inline]
fn socket_block_yield() -> bool {
    crate::net::poll();
    crate::sched::yield_now();
    crate::signal::current_wait_interrupted()
}

#[cfg(test)]
#[inline]
fn socket_block_yield() -> bool {
    false
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new socket. `sock_type`: 1=SOCK_STREAM, 2=SOCK_DGRAM.
pub fn socket_create(domain: u32, sock_type: u32, protocol: u32) -> Result<u16, i64> {
    let proto = match domain {
        AF_INET => match sock_type {
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
        },
        AF_UNIX => match sock_type {
            1 => {
                if protocol == 0 {
                    PROTO_UNIX
                } else {
                    return Err(-EPROTONOSUPPORT);
                }
            }
            _ => return Err(-EPROTONOSUPPORT),
        },
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
        s.refs = 1;
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
            PROTO_UNIX => "UNIX",
            _ => "?",
        }
    );
    Ok(id)
}

pub fn socket_create_unix_pair() -> Result<(u16, u16), i64> {
    let a = alloc_socket().ok_or(-ENOMEM)?;
    unsafe {
        let sa = &mut SOCKETS[a as usize];
        sa.reset();
        sa.used = true;
        sa.refs = 1;
        sa.protocol = PROTO_UNIX;
        sa.state = STATE_CREATED;
    }

    let b = match alloc_socket() {
        Some(id) => id,
        None => {
            unsafe {
                SOCKETS[a as usize].reset();
            }
            return Err(-ENOMEM);
        }
    };

    unsafe {
        let sb = &mut SOCKETS[b as usize];
        sb.reset();
        sb.used = true;
        sb.refs = 1;
        sb.protocol = PROTO_UNIX;
        sb.state = STATE_CONNECTED;
        sb.unix_peer = a;
        sb.unix_peer_open = true;

        let sa = &mut SOCKETS[a as usize];
        sa.state = STATE_CONNECTED;
        sa.unix_peer = b;
        sa.unix_peer_open = true;
    }

    Ok((a, b))
}

pub fn socket_domain(id: u16) -> Result<u32, i64> {
    let s = get_socket(id)?;
    if s.protocol == PROTO_UNIX {
        Ok(AF_UNIX)
    } else {
        Ok(AF_INET)
    }
}

/// Bind a socket to a local address/port.
pub fn socket_bind(id: u16, _ip: &Ipv4Addr, port: u16) -> Result<(), i64> {
    let s = get_socket(id)?;
    if s.state != STATE_CREATED {
        return Err(-EINVAL);
    }
    if s.protocol == PROTO_UNIX {
        return Err(-EOPNOTSUPP);
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

pub fn socket_bind_unix(id: u16, path: &[u8]) -> Result<(), i64> {
    let s = get_socket(id)?;
    if s.protocol != PROTO_UNIX {
        return Err(-EOPNOTSUPP);
    }
    if s.state != STATE_CREATED {
        return Err(-EINVAL);
    }
    if path.is_empty() || path.len() > UNIX_PATH_MAX {
        return Err(-EINVAL);
    }
    if path.contains(&0) || path[0] != b'/' {
        return Err(-EINVAL);
    }
    if path[0] == 0 {
        // Abstract namespace sockets are deferred.
        return Err(-EOPNOTSUPP);
    }

    let inode = unix_binding_vfs_claim(path)?;
    if let Err(e) = unix_binding_alloc(id, path, inode) {
        unix_binding_vfs_release(path);
        return Err(e);
    }
    s.unix_bound_path_len = path.len() as u8;
    s.unix_bound_path.fill(0);
    s.unix_bound_path[..path.len()].copy_from_slice(path);
    s.state = STATE_BOUND;
    Ok(())
}

/// Start listening on a TCP socket.
pub fn socket_listen(id: u16, _backlog: u32) -> Result<(), i64> {
    let s = get_socket(id)?;
    if s.protocol == PROTO_UNIX {
        if s.state != STATE_BOUND {
            return Err(-EINVAL);
        }
        s.state = STATE_LISTENING;
        return Ok(());
    }
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
    if s.protocol == PROTO_UNIX {
        return Err(-EOPNOTSUPP);
    }

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

pub fn socket_connect_unix(id: u16, path: &[u8], nonblocking: bool) -> Result<(), i64> {
    if path.is_empty() || path.len() > UNIX_PATH_MAX {
        return Err(-EINVAL);
    }

    let idx = id as usize;
    if idx >= MAX_SOCKETS {
        return Err(-ENOTSOCK);
    }

    let listener_id = match unix_binding_lookup(path) {
        Some(binding_idx) => unsafe { UNIX_BINDINGS[binding_idx].listener },
        None => return Err(-ECONNREFUSED),
    };

    let listener_idx = listener_id as usize;
    if listener_id == UNIX_NO_PEER || listener_idx >= MAX_SOCKETS || listener_idx == idx {
        return Err(-ECONNREFUSED);
    }

    unsafe {
        let sockets = SOCKETS.as_mut_ptr();
        let client = &*sockets.add(idx);
        if !client.used || client.protocol != PROTO_UNIX {
            return Err(-ENOTSOCK);
        }
        if client.state != STATE_CREATED && client.state != STATE_BOUND {
            return Err(-EINVAL);
        }

        let listener = &*sockets.add(listener_idx);
        if !listener.used || listener.protocol != PROTO_UNIX || listener.state != STATE_LISTENING {
            return Err(-ECONNREFUSED);
        }

        if listener.unix_accept_q_len as usize >= UNIX_ACCEPT_QUEUE_SIZE {
            if nonblocking {
                return Err(-EAGAIN);
            }
            #[cfg(not(test))]
            {
                loop {
                    if listener.unix_accept_q_len as usize >= UNIX_ACCEPT_QUEUE_SIZE {
                        if socket_block_yield() {
                            return Err(-ERESTARTSYS);
                        }
                        let refresh = &mut *sockets.add(listener_idx);
                        if refresh.unix_accept_q_len as usize >= UNIX_ACCEPT_QUEUE_SIZE {
                            continue;
                        }
                    }
                    break;
                }
            }
            #[cfg(test)]
            {
                return Err(-EAGAIN);
            }
        }
    }

    let server_id = alloc_socket().ok_or(-ENOMEM)?;
    let server_idx = server_id as usize;

    unsafe {
        let sockets = SOCKETS.as_mut_ptr();
        let client = &mut *sockets.add(idx);
        let listener = &mut *sockets.add(listener_idx);
        if !client.used
            || client.protocol != PROTO_UNIX
            || (client.state != STATE_CREATED && client.state != STATE_BOUND)
        {
            return Err(-ENOTSOCK);
        }
        if !listener.used || listener.protocol != PROTO_UNIX || listener.state != STATE_LISTENING {
            return Err(-ECONNREFUSED);
        }
        if listener.unix_accept_q_len as usize >= UNIX_ACCEPT_QUEUE_SIZE {
            return Err(-EAGAIN);
        }

        let server = &mut *sockets.add(server_idx);
        server.reset();
        server.used = true;
        server.refs = 1;
        server.protocol = PROTO_UNIX;
        server.state = STATE_CONNECTED;
        server.unix_peer = id;
        server.unix_peer_open = true;

        client.state = STATE_CONNECTED;
        client.unix_peer = server_id;
        client.unix_peer_open = true;
        client.unix_peer_cred = listener.unix_listen_cred;
        client.unix_peer_cred_valid = true;

        if !unix_accept_queue_push(listener, server_id) {
            server.reset();
            client.unix_peer = UNIX_NO_PEER;
            client.unix_peer_open = false;
            client.state = if client.unix_bound_path_len > 0 {
                STATE_BOUND
            } else {
                STATE_CREATED
            };
            return Err(-EAGAIN);
        }
    }

    Ok(())
}

/// Accept a connection on a listening TCP socket.
/// Returns (new_socket_id, remote_ip, remote_port).
pub fn socket_accept(id: u16, nonblocking: bool) -> Result<(u16, Ipv4Addr, u16), i64> {
    let (listen_handle, listen_port) = {
        let s = get_socket(id)?;
        if s.protocol == PROTO_UNIX {
            return Err(-EOPNOTSUPP);
        }
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

pub fn socket_accept_unix(id: u16, nonblocking: bool) -> Result<u16, i64> {
    let idx = id as usize;
    if idx >= MAX_SOCKETS {
        return Err(-ENOTSOCK);
    }

    loop {
        unsafe {
            let listener = &mut SOCKETS[idx];
            if !listener.used || listener.protocol != PROTO_UNIX {
                return Err(-ENOTSOCK);
            }
            if listener.state != STATE_LISTENING {
                return Err(-EINVAL);
            }
            if let Some(sock_id) = unix_accept_queue_pop(listener) {
                return Ok(sock_id);
            }
        }

        if nonblocking {
            return Err(-EAGAIN);
        }
        #[cfg(test)]
        {
            return Err(-EAGAIN);
        }
        #[cfg(not(test))]
        {
            if socket_block_yield() {
                return Err(-ERESTARTSYS);
            }
        }
    }
}

/// Release transferred rights that were dropped before fd installation.
pub fn socket_release_rights(fds: &[UnixRightsFd]) {
    let mut idx = 0usize;
    while idx < fds.len() {
        unix_rights_release_one(&fds[idx]);
        idx += 1;
    }
}

fn unix_send_rights_once(id: u16, fds: &[UnixRightsFd]) -> Result<(), i64> {
    if fds.is_empty() {
        return Ok(());
    }
    if fds.len() > UNIX_RIGHTS_MAX_FDS {
        return Err(-EINVAL);
    }

    let idx = id as usize;
    if idx >= MAX_SOCKETS {
        return Err(-ENOTSOCK);
    }

    unsafe {
        let src = &mut SOCKETS[idx];
        if !src.used || src.protocol != PROTO_UNIX {
            return Err(-ENOTSOCK);
        }
        if src.state != STATE_CONNECTED && src.state != STATE_SHUTDOWN {
            return Err(-ENOTCONN);
        }
        if !src.unix_peer_open || src.unix_peer == UNIX_NO_PEER {
            src.unix_peer_open = false;
            src.unix_peer = UNIX_NO_PEER;
            return Err(-EPIPE);
        }
    }

    let mut retained = 0usize;
    while retained < fds.len() {
        if let Err(e) = unix_rights_retain_one(&fds[retained]) {
            socket_release_rights(&fds[..retained]);
            return Err(e);
        }
        retained += 1;
    }

    let enqueue_result = unsafe {
        let sockets = SOCKETS.as_mut_ptr();
        let src = &mut *sockets.add(idx);
        if !src.used || src.protocol != PROTO_UNIX {
            Err(-ENOTSOCK)
        } else {
            let peer_id = src.unix_peer;
            if !src.unix_peer_open || peer_id == UNIX_NO_PEER {
                src.unix_peer_open = false;
                src.unix_peer = UNIX_NO_PEER;
                Err(-EPIPE)
            } else if peer_id as usize >= MAX_SOCKETS || peer_id as usize == idx {
                src.unix_peer_open = false;
                src.unix_peer = UNIX_NO_PEER;
                Err(-EPIPE)
            } else {
                let peer = &mut *sockets.add(peer_id as usize);
                if !peer.used || peer.protocol != PROTO_UNIX {
                    src.unix_peer_open = false;
                    src.unix_peer = UNIX_NO_PEER;
                    Err(-EPIPE)
                } else if peer.unix_rights_q_len as usize >= UNIX_RIGHTS_QUEUE_SIZE {
                    Err(-EAGAIN)
                } else {
                    let mut msg = UnixRightsQueueEntry::empty();
                    msg.count = fds.len() as u8;
                    msg.fds[..fds.len()].copy_from_slice(fds);
                    if !unix_rights_queue_push(peer, msg) {
                        Err(-EAGAIN)
                    } else {
                        Ok(())
                    }
                }
            }
        }
    };

    match enqueue_result {
        Ok(()) => Ok(()),
        Err(e) => {
            socket_release_rights(fds);
            Err(e)
        }
    }
}

pub fn socket_send_rights(id: u16, fds: &[UnixRightsFd], nonblocking: bool) -> Result<(), i64> {
    if nonblocking {
        return unix_send_rights_once(id, fds);
    }

    loop {
        match unix_send_rights_once(id, fds) {
            Err(err) if err == -EAGAIN => {
                #[cfg(test)]
                {
                    return Err(-EAGAIN);
                }
                #[cfg(not(test))]
                {
                    if socket_block_yield() {
                        return Err(-ERESTARTSYS);
                    }
                }
            }
            other => return other,
        }
    }
}

pub fn socket_recv_rights(id: u16) -> Result<Option<UnixRightsMessage>, i64> {
    let idx = id as usize;
    if idx >= MAX_SOCKETS {
        return Err(-ENOTSOCK);
    }

    unsafe {
        let s = &mut SOCKETS[idx];
        if !s.used || s.protocol != PROTO_UNIX {
            return Err(-ENOTSOCK);
        }
        if s.state != STATE_CONNECTED && s.state != STATE_SHUTDOWN {
            return Err(-ENOTCONN);
        }
        if let Some(msg) = unix_rights_queue_pop(s) {
            return Ok(Some(UnixRightsMessage {
                count: msg.count as usize,
                fds: msg.fds,
            }));
        }
    }

    Ok(None)
}

/// Send data on a connected socket.
pub fn socket_send(id: u16, data: &[u8], nonblocking: bool) -> Result<usize, i64> {
    let proto = {
        let s = get_socket(id)?;
        s.protocol
    };

    if proto == PROTO_UNIX {
        if nonblocking {
            return unix_stream_send(id, data);
        }
        loop {
            match unix_stream_send(id, data) {
                Err(err) if err == -EAGAIN => {
                    #[cfg(test)]
                    {
                        return Err(-EAGAIN);
                    }
                    #[cfg(not(test))]
                    {
                        if socket_block_yield() {
                            return Err(-ERESTARTSYS);
                        }
                    }
                }
                other => return other,
            }
        }
    }

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
    let proto = {
        let s = get_socket(id)?;
        s.protocol
    };
    if proto == PROTO_UNIX {
        return socket_send(id, data, false);
    }
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
    let proto = {
        let s = get_socket(id)?;
        s.protocol
    };

    if proto == PROTO_UNIX {
        if nonblocking {
            return unix_stream_recv(id, buf);
        }
        loop {
            match unix_stream_recv(id, buf) {
                Err(err) if err == -EAGAIN => {
                    #[cfg(test)]
                    {
                        return Err(-EAGAIN);
                    }
                    #[cfg(not(test))]
                    {
                        if socket_block_yield() {
                            return Err(-ERESTARTSYS);
                        }
                    }
                }
                other => return other,
            }
        }
    }

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
    let proto = {
        let s = get_socket(id)?;
        s.protocol
    };

    if proto == PROTO_UNIX {
        if nonblocking {
            let n = unix_stream_recv(id, buf)?;
            return Ok((n, Ipv4Addr::ZERO, 0));
        }
        loop {
            match unix_stream_recv(id, buf) {
                Err(err) if err == -EAGAIN => {
                    #[cfg(test)]
                    {
                        return Err(-EAGAIN);
                    }
                    #[cfg(not(test))]
                    {
                        if socket_block_yield() {
                            return Err(-ERESTARTSYS);
                        }
                    }
                }
                other => return other.map(|n| (n, Ipv4Addr::ZERO, 0)),
            }
        }
    }

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
    } else if s.protocol == PROTO_UNIX {
        if s.state == STATE_LISTENING {
            // Like Linux, a listener is readable while connections are queued
            // for accept() and is never "hung up".
            if s.unix_accept_q_len > 0 {
                events |= POLLIN;
            }
            return events;
        }
        if s.unix_rx_len > 0 {
            events |= POLLIN;
        }
        if s.unix_peer_open {
            let peer_idx = s.unix_peer as usize;
            if peer_idx < MAX_SOCKETS {
                unsafe {
                    let peer = &SOCKETS[peer_idx];
                    if peer.used && peer.protocol == PROTO_UNIX && unix_rx_space(peer) > 0 {
                        events |= POLLOUT;
                    } else if !peer.used || peer.protocol != PROTO_UNIX {
                        events |= POLLHUP;
                    }
                }
            } else {
                events |= POLLHUP;
            }
        } else {
            events |= POLLHUP;
        }
    }

    events
}

/// Shutdown a socket (partial close).
pub fn socket_shutdown(id: u16, _how: i32) -> Result<(), i64> {
    let proto = {
        let s = get_socket(id)?;
        s.protocol
    };
    if proto == PROTO_UNIX {
        let close_peer = _how == 1 || _how == 2;
        if close_peer {
            unix_notify_peer_closed(id);
        }
        let s = get_socket(id)?;
        if s.state == STATE_CONNECTED {
            s.state = STATE_SHUTDOWN;
        }
        if close_peer {
            s.unix_peer_open = false;
            s.unix_peer = UNIX_NO_PEER;
        }
        return Ok(());
    }
    let s = get_socket(id)?;
    if s.protocol == PROTO_TCP && s.state == STATE_CONNECTED {
        tcp::close(s.handle as usize);
        s.state = STATE_SHUTDOWN;
    }
    Ok(())
}

/// Retain a socket reference when an FD is duplicated/forked.
/// Record the credentials of the process on the other end of `id`.
pub fn socket_set_peer_cred(id: u16, cred: UnixCred) -> Result<(), i64> {
    let s = get_socket(id)?;
    if s.protocol != PROTO_UNIX {
        return Err(-EOPNOTSUPP);
    }
    s.unix_peer_cred = cred;
    s.unix_peer_cred_valid = true;
    Ok(())
}

/// Record the credentials of the process listening on `id`.
pub fn socket_set_listen_cred(id: u16, cred: UnixCred) -> Result<(), i64> {
    let s = get_socket(id)?;
    if s.protocol != PROTO_UNIX {
        return Err(-EOPNOTSUPP);
    }
    s.unix_listen_cred = cred;
    Ok(())
}

/// `SO_PEERCRED`: credentials of the connected peer process.
pub fn socket_peer_cred(id: u16) -> Result<UnixCred, i64> {
    let s = get_socket(id)?;
    if s.protocol != PROTO_UNIX || !s.unix_peer_cred_valid {
        return Err(-ENOTCONN);
    }
    Ok(s.unix_peer_cred)
}

/// The connected peer endpoint of an `AF_UNIX` stream socket, if any.
pub fn socket_unix_peer(id: u16) -> Option<u16> {
    let s = get_socket(id).ok()?;
    if s.protocol != PROTO_UNIX || s.unix_peer == UNIX_NO_PEER {
        return None;
    }
    Some(s.unix_peer)
}

pub fn socket_retain(id: u16) -> Result<(), i64> {
    let idx = id as usize;
    if idx >= MAX_SOCKETS {
        return Err(-ENOTSOCK);
    }
    unsafe {
        if !SOCKETS[idx].used || SOCKETS[idx].refs == 0 {
            return Err(-ENOTSOCK);
        }
        SOCKETS[idx].refs = SOCKETS[idx].refs.saturating_add(1);
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
        if s.refs > 1 {
            s.refs -= 1;
            return Ok(());
        }

        if s.protocol == PROTO_UNIX {
            // Like Linux, the bound path's filesystem node outlives the
            // socket: only the binding goes away (connect then fails with
            // ECONNREFUSED), and the owner must unlink the path itself.
            unix_binding_remove_listener(id);
            unix_notify_peer_closed(id);
            unix_rights_drain_socket(s);
        } else if s.protocol == PROTO_TCP {
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
    if s.protocol == PROTO_UNIX {
        return Err(-EOPNOTSUPP);
    }
    let cfg = crate::net::get_config();
    Ok((cfg.ip, s.local_port))
}

/// Get remote address/port for a connected socket.
pub fn socket_getpeername(id: u16) -> Result<(Ipv4Addr, u16), i64> {
    let s = get_socket(id)?;
    if s.protocol == PROTO_UNIX {
        return Err(-EOPNOTSUPP);
    }
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
    match socket_create(AF_INET, 1, IPPROTO_TCP) {
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
    match socket_create(AF_INET, 2, IPPROTO_UDP) {
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
    match socket_create(AF_INET, 1, IPPROTO_TCP) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn reset_sockets() {
        unsafe {
            for s in SOCKETS.iter_mut() {
                s.reset();
            }
            for b in UNIX_BINDINGS.iter_mut() {
                *b = UnixBinding::empty();
            }
        }
    }

    #[test]
    fn unix_socketpair_send_recv_roundtrip() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        reset_sockets();

        let (a, b) = socket_create_unix_pair().expect("create pair");
        let sent = socket_send(a, b"hello", true).expect("send");
        assert_eq!(sent, 5);

        let mut out = [0u8; 8];
        let read = socket_recv(b, &mut out, true).expect("recv");
        assert_eq!(read, 5);
        assert_eq!(&out[..read], b"hello");

        let _ = socket_close(a);
        let _ = socket_close(b);
    }

    #[test]
    fn unix_socketpair_peer_close_reports_hup_and_epipe() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        reset_sockets();

        let (a, b) = socket_create_unix_pair().expect("create pair");
        let _ = socket_close(b);

        let events = socket_poll_events(a);
        assert!((events & POLLHUP) != 0);
        assert_eq!(socket_send(a, b"x", true), Err(-EPIPE));
    }

    #[test]
    fn unix_socketpair_duplicate_closes_do_not_report_hup_before_last_peer_ref() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        reset_sockets();

        let (a, b) = socket_create_unix_pair().expect("create pair");

        socket_retain(a).expect("retain a for child1");
        socket_retain(a).expect("retain a for child2");
        socket_retain(b).expect("retain b for child1");
        socket_retain(b).expect("retain b for child2");

        socket_close(a).expect("child1 closes a");
        socket_close(a).expect("child2 closes a");
        socket_close(b).expect("child2 closes b");
        socket_close(b).expect("parent closes b");

        let events = socket_poll_events(a);
        assert_eq!(events & POLLHUP, 0);
        assert_ne!(events & POLLOUT, 0);

        socket_close(a).expect("parent closes a");
        socket_close(b).expect("child1 closes b");
    }

    #[test]
    fn unix_socketpair_full_queue_yields_eagain() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        reset_sockets();

        let (a, _b) = socket_create_unix_pair().expect("create pair");
        let payload = [0x41u8; UNIX_STREAM_BUF_SIZE];
        let sent = socket_send(a, &payload, true).expect("initial send");
        assert_eq!(sent, UNIX_STREAM_BUF_SIZE);
        assert_eq!(socket_send(a, b"z", true), Err(-EAGAIN));
    }

    #[test]
    fn unix_pathname_bind_listen_connect_accept_roundtrip() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        reset_sockets();

        let listener = socket_create(AF_UNIX, 1, 0).expect("listener create");
        socket_bind_unix(listener, b"/tmp/wayland-test").expect("bind");
        socket_listen(listener, 4).expect("listen");

        let client = socket_create(AF_UNIX, 1, 0).expect("client create");
        socket_connect_unix(client, b"/tmp/wayland-test", true).expect("connect");

        let server = socket_accept_unix(listener, true).expect("accept");
        let sent = socket_send(client, b"ping", true).expect("client send");
        assert_eq!(sent, 4);

        let mut out = [0u8; 8];
        let read = socket_recv(server, &mut out, true).expect("server recv");
        assert_eq!(read, 4);
        assert_eq!(&out[..read], b"ping");

        let _ = socket_close(server);
        let _ = socket_close(client);
        let _ = socket_close(listener);
    }

    #[test]
    fn unix_bindings_hold_more_than_32_listeners_and_close_unbinds() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        reset_sockets();

        // libwayland's wl_display_add_socket_auto tries up to 33 sockets.
        let mut listeners = std::vec::Vec::new();
        for i in 0..33 {
            let path = std::format!("/tmp/wayland-{}", i);
            let l = socket_create(AF_UNIX, 1, 0).expect("listener create");
            socket_bind_unix(l, path.as_bytes()).expect("bind");
            socket_listen(l, 4).expect("listen");
            listeners.push(l);
        }

        let client = socket_create(AF_UNIX, 1, 0).expect("client create");
        socket_close(listeners[32]).expect("close last listener");
        assert_eq!(
            socket_connect_unix(client, b"/tmp/wayland-32", true),
            Err(-ECONNREFUSED)
        );
        socket_connect_unix(client, b"/tmp/wayland-0", true).expect("connect first");

        let _ = socket_close(client);
        for l in &listeners[..32] {
            let _ = socket_close(*l);
        }
    }

    #[test]
    fn unix_listener_is_readable_only_while_connections_are_pending() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        reset_sockets();

        let listener = socket_create(AF_UNIX, 1, 0).expect("listener create");
        socket_bind_unix(listener, b"/tmp/wayland-poll").expect("bind");
        socket_listen(listener, 4).expect("listen");
        assert_eq!(socket_poll_events(listener), 0, "idle listener: not readable, not HUP");

        let client = socket_create(AF_UNIX, 1, 0).expect("client create");
        socket_connect_unix(client, b"/tmp/wayland-poll", true).expect("connect");
        assert_eq!(socket_poll_events(listener), POLLIN);

        let server = socket_accept_unix(listener, true).expect("accept");
        assert_eq!(socket_poll_events(listener), 0, "queue drained after accept");
        assert_eq!(socket_accept_unix(listener, true), Err(-EAGAIN));

        for id in [server, client, listener] {
            let _ = socket_close(id);
        }
    }

    #[test]
    fn unix_peer_credentials_follow_connect_and_socketpair() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        reset_sockets();

        let server_cred = UnixCred { pid: 10, uid: 0, gid: 0 };
        let client_cred = UnixCred { pid: 20, uid: 1000, gid: 100 };

        let listener = socket_create(AF_UNIX, 1, 0).expect("listener create");
        socket_bind_unix(listener, b"/tmp/wayland-cred").expect("bind");
        socket_listen(listener, 4).expect("listen");
        socket_set_listen_cred(listener, server_cred).expect("listen cred");
        assert_eq!(socket_peer_cred(listener), Err(-ENOTCONN));

        let client = socket_create(AF_UNIX, 1, 0).expect("client create");
        socket_connect_unix(client, b"/tmp/wayland-cred", true).expect("connect");
        let server_end = socket_unix_peer(client).expect("client has a peer");
        socket_set_peer_cred(server_end, client_cred).expect("server peer cred");

        let accepted = socket_accept_unix(listener, true).expect("accept");
        assert_eq!(accepted, server_end);
        assert_eq!(socket_peer_cred(client), Ok(server_cred));
        assert_eq!(socket_peer_cred(accepted), Ok(client_cred));

        let (a, b) = socket_create_unix_pair().expect("pair");
        assert_eq!(socket_peer_cred(a), Err(-ENOTCONN));
        socket_set_peer_cred(a, client_cred).expect("a cred");
        socket_set_peer_cred(b, client_cred).expect("b cred");
        assert_eq!(socket_peer_cred(b), Ok(client_cred));

        for id in [a, b, accepted, client, listener] {
            let _ = socket_close(id);
        }
    }

    #[test]
    fn unix_pathname_rebind_after_close_is_allowed() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        reset_sockets();

        let first = socket_create(AF_UNIX, 1, 0).expect("first listener create");
        socket_bind_unix(first, b"/tmp/wayland-rebind").expect("first bind");
        let _ = socket_close(first);

        let second = socket_create(AF_UNIX, 1, 0).expect("second listener create");
        assert_eq!(socket_bind_unix(second, b"/tmp/wayland-rebind"), Ok(()));
        let _ = socket_close(second);
    }

    #[test]
    fn unix_pathname_bind_requires_absolute_non_nul_path() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        reset_sockets();

        let sock = socket_create(AF_UNIX, 1, 0).expect("socket create");
        assert_eq!(socket_bind_unix(sock, b"relative/path"), Err(-EINVAL));
        assert_eq!(socket_bind_unix(sock, b"/tmp/\0bad"), Err(-EINVAL));
        let _ = socket_close(sock);
    }

    #[test]
    fn unix_socketpair_rights_roundtrip() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        reset_sockets();

        let (a, b) = socket_create_unix_pair().expect("create pair");
        let rights = [UnixRightsFd {
            kind: task::USER_FD_KIND_VFS,
            inode: 42,
            flags: 0,
            offset: 99,
        }];

        socket_send_rights(a, &rights, true).expect("send rights");
        let msg = socket_recv_rights(b)
            .expect("recv rights")
            .expect("expected rights");
        assert_eq!(msg.count, 1);
        assert_eq!(msg.fds[0], rights[0]);

        let _ = socket_close(a);
        let _ = socket_close(b);
    }

    #[test]
    fn unix_socketpair_rights_queue_full_yields_eagain() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        reset_sockets();

        let (a, _b) = socket_create_unix_pair().expect("create pair");
        let rights = [UnixRightsFd {
            kind: task::USER_FD_KIND_VFS,
            inode: 11,
            flags: 0,
            offset: 0,
        }];

        for _ in 0..UNIX_RIGHTS_QUEUE_SIZE {
            socket_send_rights(a, &rights, true).expect("queue right");
        }
        assert_eq!(socket_send_rights(a, &rights, true), Err(-EAGAIN));
    }
}
