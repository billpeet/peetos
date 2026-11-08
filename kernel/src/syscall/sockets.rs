//! BSD socket syscalls, including SCM_RIGHTS fd passing.

use super::*;

#[inline]
pub(super) fn socket_type_base_and_open_flags(raw_type: u32) -> Result<(u32, u32), i64> {
    let allowed = SOCK_TYPE_MASK | SOCK_CLOEXEC | SOCK_NONBLOCK;
    if (raw_type & !allowed) != 0 {
        return Err(-EINVAL);
    }
    let base_type = raw_type & SOCK_TYPE_MASK;
    if base_type != SOCK_STREAM && base_type != SOCK_DGRAM {
        return Err(-EPROTONOSUPPORT);
    }
    let mut open_flags = fs::O_RDWR;
    if (raw_type & SOCK_NONBLOCK) != 0 {
        open_flags |= O_NONBLOCK;
    }
    if (raw_type & SOCK_CLOEXEC) != 0 {
        open_flags |= O_CLOEXEC;
    }
    Ok((base_type, open_flags))
}

#[inline]
pub(super) fn accept4_open_flags(flags: u32) -> Result<u32, i64> {
    if (flags & !(SOCK_CLOEXEC | SOCK_NONBLOCK)) != 0 {
        return Err(-EINVAL);
    }
    let mut open_flags = fs::O_RDWR;
    if (flags & SOCK_NONBLOCK) != 0 {
        open_flags |= O_NONBLOCK;
    }
    if (flags & SOCK_CLOEXEC) != 0 {
        open_flags |= O_CLOEXEC;
    }
    Ok(open_flags)
}

// ---------------------------------------------------------------------------
// Socket syscall helpers
// ---------------------------------------------------------------------------

/// Parse a sockaddr_in from user memory. Layout: {family: u16, port: u16 (net order), addr: [u8;4], zero: [u8;8]}
pub(super) fn parse_sockaddr_in(
    task_ptr: *mut task::Task,
    addr_ptr: u64,
    addr_len: usize,
) -> Result<(net::Ipv4Addr, u16), i64> {
    if addr_len < SOCKADDR_IN_SIZE {
        return Err(-EINVAL);
    }
    let mut buf = [0u8; SOCKADDR_IN_SIZE];
    copy_from_user_task(task_ptr, addr_ptr, &mut buf)?;
    let family = (buf[0] as u16) | ((buf[1] as u16) << 8);
    if family != AF_INET as u16 {
        return Err(-EAFNOSUPPORT);
    }
    // Port is in network byte order (big endian)
    let port = ((buf[2] as u16) << 8) | (buf[3] as u16);
    let ip = net::Ipv4Addr::new(buf[4], buf[5], buf[6], buf[7]);
    Ok((ip, port))
}

/// Parse a sockaddr_un from user memory. Layout: {family: u16, sun_path: [u8;108]}
pub(super) fn parse_sockaddr_un(
    task_ptr: *mut task::Task,
    addr_ptr: u64,
    addr_len: usize,
) -> Result<([u8; SOCKADDR_UN_PATH_MAX], usize), i64> {
    if addr_len < SOCKADDR_UN_FAMILY_SIZE {
        return Err(-EINVAL);
    }

    let copy_len = addr_len.min(SOCKADDR_UN_SIZE);
    let mut buf = [0u8; SOCKADDR_UN_SIZE];
    copy_from_user_task(task_ptr, addr_ptr, &mut buf[..copy_len])?;

    let family = (buf[0] as u16) | ((buf[1] as u16) << 8);
    if family != AF_UNIX as u16 {
        return Err(-EAFNOSUPPORT);
    }
    if copy_len <= SOCKADDR_UN_FAMILY_SIZE {
        return Err(-EINVAL);
    }

    let mut path_len = copy_len - SOCKADDR_UN_FAMILY_SIZE;
    while path_len > 0 && buf[SOCKADDR_UN_FAMILY_SIZE + path_len - 1] == 0 {
        path_len -= 1;
    }
    if path_len == 0 {
        return Err(-EINVAL);
    }
    if buf[SOCKADDR_UN_FAMILY_SIZE] == 0 {
        // Abstract namespace sockets are deferred for now.
        return Err(-EOPNOTSUPP);
    }

    let mut path = [0u8; SOCKADDR_UN_PATH_MAX];
    path[..path_len]
        .copy_from_slice(&buf[SOCKADDR_UN_FAMILY_SIZE..SOCKADDR_UN_FAMILY_SIZE + path_len]);
    Ok((path, path_len))
}

/// Write a sockaddr_in to user memory.
pub(super) fn write_sockaddr_in(
    task_ptr: *mut task::Task,
    addr_ptr: u64,
    addrlen_ptr: u64,
    ip: &net::Ipv4Addr,
    port: u16,
) -> Result<(), i64> {
    if addr_ptr == 0 {
        return Ok(());
    }
    let mut buf = [0u8; SOCKADDR_IN_SIZE];
    buf[0] = AF_INET as u8;
    buf[1] = 0; // AF_INET fits in one byte on little-endian
                // Port in network byte order (big endian)
    buf[2] = (port >> 8) as u8;
    buf[3] = (port & 0xFF) as u8;
    buf[4] = ip.0[0];
    buf[5] = ip.0[1];
    buf[6] = ip.0[2];
    buf[7] = ip.0[3];

    copy_to_user_task(task_ptr, addr_ptr, &buf)?;

    if addrlen_ptr != 0 {
        let len_val = SOCKADDR_IN_SIZE as u32;
        let len_bytes = len_val.to_le_bytes();
        copy_to_user_task(task_ptr, addrlen_ptr, &len_bytes)?;
    }
    Ok(())
}

/// Write a sockaddr_un to user memory.
pub(super) fn write_sockaddr_un(
    task_ptr: *mut task::Task,
    addr_ptr: u64,
    addrlen_ptr: u64,
    path: &[u8],
) -> Result<(), i64> {
    if addr_ptr == 0 {
        return Ok(());
    }

    let path_len = path.len().min(SOCKADDR_UN_PATH_MAX);
    let out_len = SOCKADDR_UN_FAMILY_SIZE + path_len;
    let mut buf = [0u8; SOCKADDR_UN_SIZE];
    buf[0] = AF_UNIX as u8;
    buf[1] = 0;
    if path_len > 0 {
        buf[SOCKADDR_UN_FAMILY_SIZE..SOCKADDR_UN_FAMILY_SIZE + path_len]
            .copy_from_slice(&path[..path_len]);
    }

    copy_to_user_task(task_ptr, addr_ptr, &buf[..out_len])?;

    if addrlen_ptr != 0 {
        let len_val = out_len as u32;
        let len_bytes = len_val.to_le_bytes();
        copy_to_user_task(task_ptr, addrlen_ptr, &len_bytes)?;
    }
    Ok(())
}

/// Get socket_id from an FD that is a socket. Returns socket inode (= socket table id).
pub(super) fn fd_to_socket_id(task_ptr: *mut task::Task, fd: i32) -> Result<(usize, u16), i64> {
    let idx = fd_index(fd)?;
    if !fd_slot_is_open(task_ptr, idx) {
        return Err(-EBADF);
    }
    let (kind, inode) = unsafe {
        (
            (*crate::task::leader_of(task_ptr)).user_fd_kind[idx],
            (*crate::task::leader_of(task_ptr)).user_fd_inode[idx],
        )
    };
    if kind != task::USER_FD_KIND_SOCKET {
        return Err(-ENOTSOCK);
    }
    Ok((idx, inode))
}

// ---------------------------------------------------------------------------
// Socket syscall handlers
// ---------------------------------------------------------------------------

pub(super) fn sys_socket(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let domain = call.arg(0) as u32;
    let raw_type = call.arg(1) as u32;
    let protocol = call.arg(2) as u32;

    if domain != AF_INET && domain != AF_UNIX {
        return -EAFNOSUPPORT;
    }

    let (base_type, open_flags) = match socket_type_base_and_open_flags(raw_type) {
        Ok(v) => v,
        Err(err) => return err,
    };

    let normalized_protocol = if domain == AF_INET {
        let proto = if protocol == IPPROTO_IP { 0 } else { protocol };
        if proto != 0 && proto != IPPROTO_TCP && proto != IPPROTO_UDP && proto != IPPROTO_ICMP {
            return -EPROTONOSUPPORT;
        }
        proto
    } else {
        // AF_UNIX baseline: stream sockets only, protocol must be 0.
        if base_type != SOCK_STREAM || protocol != 0 {
            return -EPROTONOSUPPORT;
        }
        0
    };

    let socket_id = match socket::socket_create(domain, base_type, normalized_protocol) {
        Ok(id) => id,
        Err(e) => return e,
    };

    let fd_flags = fd_flags_from_open_flags(open_flags);

    match fd_alloc(ctx.current, task::USER_FD_KIND_SOCKET, socket_id, fd_flags) {
        Ok(fd) => fd,
        Err(e) => {
            let _ = socket::socket_close(socket_id);
            e
        }
    }
}

pub(super) fn sys_bind(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let addr_ptr = call.arg(1);
    let addr_len = call.arg(2) as usize;

    let (_, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let domain = match socket::socket_domain(sock_id) {
        Ok(v) => v,
        Err(e) => return e,
    };

    match domain {
        AF_INET => {
            let (ip, port) = match parse_sockaddr_in(ctx.current, addr_ptr, addr_len) {
                Ok(v) => v,
                Err(e) => return e,
            };
            match socket::socket_bind(sock_id, &ip, port) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        AF_UNIX => {
            let (path_buf, path_len) = match parse_sockaddr_un(ctx.current, addr_ptr, addr_len) {
                Ok(v) => v,
                Err(e) => return e,
            };
            match socket::socket_bind_unix(sock_id, &path_buf[..path_len]) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        _ => -EAFNOSUPPORT,
    }
}

pub(super) fn sys_listen(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let backlog = call.arg(1) as u32;

    let (_, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let listen_result = socket::socket_listen(sock_id, backlog);
    if listen_result.is_ok() {
        let _ = socket::socket_set_listen_cred(sock_id, current_unix_cred(ctx.current));
    }
    match listen_result {
        Ok(()) => 0,
        Err(e) => e,
    }
}

pub(super) fn sys_connect(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let addr_ptr = call.arg(1);
    let addr_len = call.arg(2) as usize;

    let (idx, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let flags = unsafe { (*crate::task::leader_of(ctx.current)).user_fd_flags[idx] };
    let nb = (fd_open_flags(flags) & O_NONBLOCK) != 0;
    let domain = match socket::socket_domain(sock_id) {
        Ok(v) => v,
        Err(e) => return e,
    };

    match domain {
        AF_INET => {
            let (ip, port) = match parse_sockaddr_in(ctx.current, addr_ptr, addr_len) {
                Ok(v) => v,
                Err(e) => return e,
            };
            match socket::socket_connect(sock_id, &ip, port, nb) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        AF_UNIX => {
            let (path_buf, path_len) = match parse_sockaddr_un(ctx.current, addr_ptr, addr_len) {
                Ok(v) => v,
                Err(e) => return e,
            };
            let connect_result = socket::socket_connect_unix(sock_id, &path_buf[..path_len], nb);
            if connect_result.is_ok() {
                // The accepted (server-side) endpoint reports the connecting
                // process as its peer.
                if let Some(server_end) = socket::socket_unix_peer(sock_id) {
                    let _ = socket::socket_set_peer_cred(server_end, current_unix_cred(ctx.current));
                }
            }
            match connect_result {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        _ => -EAFNOSUPPORT,
    }
}

pub(super) fn sys_accept(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    // accept() is accept4() with flags=0
    let mut call4 = *call;
    // accept(fd, addr, addrlen) → accept4(fd, addr, addrlen, 0)
    call4.args[3] = 0;
    sys_accept4(ctx, &call4)
}

pub(super) fn sys_accept4(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let addr_ptr = call.arg(1);
    let addrlen_ptr = call.arg(2);
    let flags = call.arg(3) as u32;
    let open_flags = match accept4_open_flags(flags) {
        Ok(v) => v,
        Err(err) => return err,
    };

    let (idx, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let listen_flags = unsafe { (*crate::task::leader_of(ctx.current)).user_fd_flags[idx] };
    let nb = (fd_open_flags(listen_flags) & O_NONBLOCK) != 0;

    let domain = match socket::socket_domain(sock_id) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let new_sock_id = match domain {
        AF_INET => {
            let (new_sock_id, remote_ip, remote_port) = match socket::socket_accept(sock_id, nb) {
                Ok(v) => v,
                Err(e) => return e,
            };
            if addr_ptr != 0 {
                if let Err(e) =
                    write_sockaddr_in(ctx.current, addr_ptr, addrlen_ptr, &remote_ip, remote_port)
                {
                    let _ = socket::socket_close(new_sock_id);
                    return e;
                }
            }
            new_sock_id
        }
        AF_UNIX => {
            let new_sock_id = match socket::socket_accept_unix(sock_id, nb) {
                Ok(v) => v,
                Err(e) => return e,
            };
            if addr_ptr != 0 {
                if let Err(e) = write_sockaddr_un(ctx.current, addr_ptr, addrlen_ptr, &[]) {
                    let _ = socket::socket_close(new_sock_id);
                    return e;
                }
            }
            new_sock_id
        }
        _ => return -EAFNOSUPPORT,
    };

    // Allocate an FD for the new socket
    let new_fd_flags = fd_flags_from_open_flags(open_flags);

    match fd_alloc(
        ctx.current,
        task::USER_FD_KIND_SOCKET,
        new_sock_id,
        new_fd_flags,
    ) {
        Ok(fd) => fd,
        Err(e) => {
            let _ = socket::socket_close(new_sock_id);
            e
        }
    }
}

pub(super) fn sys_sendto(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let buf_ptr = call.arg(1);
    let len = call.arg(2) as usize;
    let _flags = call.arg(3) as u32;
    let addr_ptr = call.arg(4);
    let addr_len = call.arg(5) as usize;

    let (idx, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let fd_flags = unsafe { (*crate::task::leader_of(ctx.current)).user_fd_flags[idx] };
    let nb = (fd_open_flags(fd_flags) & O_NONBLOCK) != 0;

    // Copy data from user
    const SEND_MAX: usize = 4096;
    let copy_len = len.min(SEND_MAX);
    let mut kbuf = [0u8; SEND_MAX];
    if let Err(e) = copy_from_user_task(ctx.current, buf_ptr, &mut kbuf[..copy_len]) {
        return -e;
    }

    if addr_ptr != 0 && addr_len >= SOCKADDR_IN_SIZE {
        // sendto with explicit destination
        let (ip, port) = match parse_sockaddr_in(ctx.current, addr_ptr, addr_len) {
            Ok(v) => v,
            Err(e) => return e,
        };
        match socket::socket_sendto(sock_id, &kbuf[..copy_len], &ip, port) {
            Ok(n) => n as i64,
            Err(e) => e,
        }
    } else {
        // No destination — use connected address (like send())
        match socket::socket_send(sock_id, &kbuf[..copy_len], nb) {
            Ok(n) => n as i64,
            Err(e) => e,
        }
    }
}

pub(super) fn sys_recvfrom(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let buf_ptr = call.arg(1);
    let len = call.arg(2) as usize;
    let _flags = call.arg(3) as u32;
    let addr_ptr = call.arg(4);
    let addrlen_ptr = call.arg(5);

    let (idx, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let fd_flags = unsafe { (*crate::task::leader_of(ctx.current)).user_fd_flags[idx] };
    let nb = (fd_open_flags(fd_flags) & O_NONBLOCK) != 0;

    const RECV_MAX: usize = 4096;
    let recv_len = len.min(RECV_MAX);
    let mut kbuf = [0u8; RECV_MAX];

    let (n, src_ip, src_port) = match socket::socket_recvfrom(sock_id, &mut kbuf[..recv_len], nb) {
        Ok(v) => v,
        Err(e) => return e,
    };

    if n > 0 {
        if let Err(e) = copy_to_user_task(ctx.current, buf_ptr, &kbuf[..n]) {
            return -e;
        }
    }

    // Write source address to user if requested
    if addr_ptr != 0 {
        let _ = write_sockaddr_in(ctx.current, addr_ptr, addrlen_ptr, &src_ip, src_port);
    }

    n as i64
}

// ---------------------------------------------------------------------------
// sendmsg / recvmsg
// ---------------------------------------------------------------------------

#[inline]
pub(super) fn cmsg_align(len: usize) -> usize {
    let align = size_of::<usize>();
    (len + align - 1) & !(align - 1)
}

pub(super) fn unix_fd_transfer_from_task(
    task_ptr: *mut task::Task,
    fd: i32,
) -> Result<socket::UnixRightsFd, i64> {
    let idx = fd_index(fd)?;
    if !fd_slot_is_open(task_ptr, idx) {
        return Err(-EBADF);
    }
    let (kind, inode, offset, flags) = unsafe {
        (
            (*crate::task::leader_of(task_ptr)).user_fd_kind[idx],
            (*crate::task::leader_of(task_ptr)).user_fd_inode[idx],
            (*crate::task::leader_of(task_ptr)).user_fd_offset[idx],
            (*crate::task::leader_of(task_ptr)).user_fd_flags[idx],
        )
    };
    match kind {
        task::USER_FD_KIND_VFS
        | task::USER_FD_KIND_PIPE
        | task::USER_FD_KIND_SOCKET
        | task::USER_FD_KIND_PTY_MASTER
        | task::USER_FD_KIND_PTY_SLAVE => {}
        _ => return Err(-EOPNOTSUPP),
    }
    if inode == task::USER_FD_NONE {
        return Err(-EBADF);
    }
    Ok(socket::UnixRightsFd {
        kind,
        inode,
        flags,
        offset,
    })
}

pub(super) fn parse_sendmsg_rights(
    task_ptr: *mut task::Task,
    hdr: &LinuxMsgHdr,
) -> Result<([socket::UnixRightsFd; socket::UNIX_RIGHTS_MAX_FDS], usize), i64> {
    let mut rights = [socket::UnixRightsFd::EMPTY; socket::UNIX_RIGHTS_MAX_FDS];
    if hdr.msg_controllen == 0 {
        return Ok((rights, 0));
    }
    if hdr.msg_control == 0 {
        return Err(-EFAULT);
    }

    let ctrl_len = hdr.msg_controllen as usize;
    let cmsg_hdr_len = size_of::<LinuxCmsgHdr>();
    if ctrl_len < cmsg_hdr_len {
        return Err(-EINVAL);
    }

    let mut count = 0usize;
    let mut off = 0usize;
    while off + cmsg_hdr_len <= ctrl_len {
        let cmsg =
            match read_struct_from_user::<LinuxCmsgHdr>(task_ptr, hdr.msg_control + off as u64) {
                Ok(v) => v,
                Err(e) => return Err(-e),
            };
        let msg_len = cmsg.cmsg_len as usize;
        if msg_len < cmsg_hdr_len || off + msg_len > ctrl_len {
            return Err(-EINVAL);
        }
        if cmsg.cmsg_level as u32 != SOL_SOCKET || cmsg.cmsg_type as u32 != SCM_RIGHTS {
            return Err(-EOPNOTSUPP);
        }

        let payload_len = msg_len - cmsg_hdr_len;
        if (payload_len % size_of::<i32>()) != 0 {
            return Err(-EINVAL);
        }
        let fd_count = payload_len / size_of::<i32>();
        if count + fd_count > socket::UNIX_RIGHTS_MAX_FDS {
            return Err(-EINVAL);
        }

        let mut idx = 0usize;
        while idx < fd_count {
            let fd = match read_i32_from_user(
                task_ptr,
                hdr.msg_control + (off + cmsg_hdr_len + idx * size_of::<i32>()) as u64,
            ) {
                Ok(v) => v,
                Err(e) => return Err(-e),
            };
            rights[count] = unix_fd_transfer_from_task(task_ptr, fd)?;
            count += 1;
            idx += 1;
        }

        let adv = cmsg_align(msg_len);
        if adv == 0 {
            break;
        }
        off += adv;
    }

    Ok((rights, count))
}

pub(super) fn install_received_right(
    task_ptr: *mut task::Task,
    right: socket::UnixRightsFd,
    cloexec: bool,
) -> Result<i32, i64> {
    let preserved_internal = fd_internal_flags(right.flags) & !FD_INTERNAL_CLOEXEC;
    if (preserved_internal & FD_INTERNAL_MEMFD) != 0 {
        enforce_memfd_object_limit_for_inode(task_ptr, right.inode)?;
    }
    let idx = match fd_find_free_from(task_ptr, 0) {
        Some(v) => v,
        None => return Err(-EMFILE),
    };
    let mut open_flags = fd_open_flags(right.flags);
    if cloexec {
        open_flags |= O_CLOEXEC;
    }
    let fd_flags = fd_flags_from_open_flags(open_flags) | preserved_internal;
    fd_write_slot(
        task_ptr,
        idx,
        right.kind,
        right.inode,
        right.offset,
        fd_flags,
    );
    Ok(idx as i32)
}

pub(super) fn sys_sendmsg(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let msghdr_ptr = call.arg(1);
    let msg_flags_arg = call.arg(2) as u32;
    if msghdr_ptr == 0 {
        return -EFAULT;
    }

    let (idx, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let fd_flags = unsafe { (*crate::task::leader_of(ctx.current)).user_fd_flags[idx] };
    let nb = (fd_open_flags(fd_flags) & O_NONBLOCK) != 0 || (msg_flags_arg & MSG_DONTWAIT) != 0;

    let hdr = match read_struct_from_user::<LinuxMsgHdr>(ctx.current, msghdr_ptr) {
        Ok(h) => h,
        Err(e) => return -e,
    };
    let domain = match socket::socket_domain(sock_id) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let iov_count = hdr.msg_iovlen as usize;
    if iov_count > 64 {
        return -EINVAL;
    }

    let mut total_len: usize = 0;
    let mut iovecs = [LinuxIovec {
        iov_base: 0,
        iov_len: 0,
    }; 64];
    for (i, slot) in iovecs.iter_mut().enumerate().take(iov_count) {
        let entry_ptr = hdr.msg_iov + (i * size_of::<LinuxIovec>()) as u64;
        let iov = match read_struct_from_user::<LinuxIovec>(ctx.current, entry_ptr) {
            Ok(v) => v,
            Err(e) => return -e,
        };
        *slot = iov;
        total_len = total_len.saturating_add(iov.iov_len as usize);
    }

    const SENDMSG_BUF_SIZE: usize = 4096;
    let copy_len = total_len.min(SENDMSG_BUF_SIZE);
    let mut kbuf = [0u8; SENDMSG_BUF_SIZE];
    let mut remaining = copy_len;
    let mut dst_off: usize = 0;
    for iov in iovecs.iter().take(iov_count) {
        if remaining == 0 {
            break;
        }
        let chunk = remaining.min(iov.iov_len as usize);
        if chunk > 0 {
            if let Err(e) = copy_from_user_task(
                ctx.current,
                iov.iov_base,
                &mut kbuf[dst_off..dst_off + chunk],
            ) {
                return -e;
            }
            dst_off += chunk;
            remaining -= chunk;
        }
    }

    let (rights, rights_count) = match parse_sendmsg_rights(ctx.current, &hdr) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if rights_count > 0 {
        if domain != AF_UNIX || hdr.msg_name != 0 {
            return -EOPNOTSUPP;
        }
        if let Err(e) = socket::socket_send_rights(sock_id, &rights[..rights_count], nb) {
            return e;
        }
    }

    if hdr.msg_name != 0 {
        match domain {
            AF_INET => {
                let (ip, port) =
                    match parse_sockaddr_in(ctx.current, hdr.msg_name, hdr.msg_namelen as usize) {
                        Ok(v) => v,
                        Err(e) => return e,
                    };
                match socket::socket_sendto(sock_id, &kbuf[..copy_len], &ip, port) {
                    Ok(n) => n as i64,
                    Err(e) => e,
                }
            }
            AF_UNIX => -EOPNOTSUPP,
            _ => -EAFNOSUPPORT,
        }
    } else {
        match socket::socket_send(sock_id, &kbuf[..copy_len], nb) {
            Ok(n) => n as i64,
            Err(e) => e,
        }
    }
}

pub(super) fn sys_recvmsg(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let msghdr_ptr = call.arg(1);
    let msg_flags_arg = call.arg(2) as u32;

    if msghdr_ptr == 0 {
        return -EFAULT;
    }

    let (idx, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let fd_flags = unsafe { (*crate::task::leader_of(ctx.current)).user_fd_flags[idx] };
    let nb = (fd_open_flags(fd_flags) & O_NONBLOCK) != 0 || (msg_flags_arg & MSG_DONTWAIT) != 0;

    let hdr = match read_struct_from_user::<LinuxMsgHdr>(ctx.current, msghdr_ptr) {
        Ok(h) => h,
        Err(e) => return -e,
    };

    let iov_count = hdr.msg_iovlen as usize;
    if iov_count > 64 {
        return -EINVAL;
    }

    let mut total_len: usize = 0;
    let mut iovecs = [LinuxIovec {
        iov_base: 0,
        iov_len: 0,
    }; 64];
    for i in 0..iov_count {
        let entry_ptr = hdr.msg_iov + (i * size_of::<LinuxIovec>()) as u64;
        match read_struct_from_user::<LinuxIovec>(ctx.current, entry_ptr) {
            Ok(iov) => {
                iovecs[i] = iov;
                total_len = total_len.saturating_add(iov.iov_len as usize);
            }
            Err(e) => return -e,
        }
    }

    const RECVMSG_BUF_SIZE: usize = 4096;
    let recv_len = total_len.min(RECVMSG_BUF_SIZE);
    let mut kbuf = [0u8; RECVMSG_BUF_SIZE];

    let (n, src_ip, src_port) = match socket::socket_recvfrom(sock_id, &mut kbuf[..recv_len], nb) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let mut remaining = n;
    let mut src_off: usize = 0;
    for i in 0..iov_count {
        if remaining == 0 {
            break;
        }
        let chunk = remaining.min(iovecs[i].iov_len as usize);
        if chunk > 0 && iovecs[i].iov_base != 0 {
            if let Err(e) = copy_to_user_task(
                ctx.current,
                iovecs[i].iov_base,
                &kbuf[src_off..src_off + chunk],
            ) {
                return -e;
            }
        }
        src_off += chunk;
        remaining -= chunk;
    }

    let domain = match socket::socket_domain(sock_id) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let mut out_hdr = hdr;
    out_hdr.msg_namelen = 0;
    if hdr.msg_name != 0 {
        match domain {
            AF_INET => {
                out_hdr.msg_namelen = SOCKADDR_IN_SIZE as u32;
                if hdr.msg_namelen >= SOCKADDR_IN_SIZE as u32 {
                    let _ = write_sockaddr_in(ctx.current, hdr.msg_name, 0, &src_ip, src_port);
                }
            }
            AF_UNIX => {
                out_hdr.msg_namelen = SOCKADDR_UN_FAMILY_SIZE as u32;
                if hdr.msg_namelen >= SOCKADDR_UN_FAMILY_SIZE as u32 {
                    let _ = write_sockaddr_un(ctx.current, hdr.msg_name, 0, &[]);
                }
            }
            _ => return -EAFNOSUPPORT,
        }
    }

    out_hdr.msg_controllen = 0;
    out_hdr.msg_flags = 0;

    if domain == AF_UNIX {
        let rights_msg = match socket::socket_recv_rights(sock_id) {
            Ok(v) => v,
            Err(e) => return e,
        };

        if let Some(msg) = rights_msg {
            let cloexec = (msg_flags_arg & MSG_CMSG_CLOEXEC) != 0;
            let cmsg_hdr_len = size_of::<LinuxCmsgHdr>();
            let data_item_len = size_of::<i32>();

            if hdr.msg_control == 0 || hdr.msg_controllen < cmsg_hdr_len as u64 {
                out_hdr.msg_flags |= MSG_CTRUNC;
                socket::socket_release_rights(&msg.fds[..msg.count]);
            } else {
                let ctrl_cap = hdr.msg_controllen as usize;
                let max_fds_fit = (ctrl_cap - cmsg_hdr_len) / data_item_len;
                let fd_count = msg.count.min(max_fds_fit);

                if fd_count == 0 {
                    out_hdr.msg_flags |= MSG_CTRUNC;
                    socket::socket_release_rights(&msg.fds[..msg.count]);
                } else {
                    let mut installed = [0i32; socket::UNIX_RIGHTS_MAX_FDS];
                    let mut installed_count = 0usize;
                    while installed_count < fd_count {
                        match install_received_right(ctx.current, msg.fds[installed_count], cloexec)
                        {
                            Ok(new_fd) => {
                                installed[installed_count] = new_fd;
                                installed_count += 1;
                            }
                            Err(e) => {
                                let mut i = 0usize;
                                while i < installed_count {
                                    let _ = fd_close(ctx.current, installed[i]);
                                    i += 1;
                                }
                                socket::socket_release_rights(&msg.fds[installed_count..msg.count]);
                                return e;
                            }
                        }
                    }

                    let cmsg_len = cmsg_hdr_len + installed_count * data_item_len;
                    let cmsg = LinuxCmsgHdr {
                        cmsg_len: cmsg_len as u64,
                        cmsg_level: SOL_SOCKET as i32,
                        cmsg_type: SCM_RIGHTS as i32,
                    };
                    if let Err(e) = write_struct_to_user(ctx.current, hdr.msg_control, &cmsg) {
                        let mut i = 0usize;
                        while i < installed_count {
                            let _ = fd_close(ctx.current, installed[i]);
                            i += 1;
                        }
                        socket::socket_release_rights(&msg.fds[installed_count..msg.count]);
                        return -e;
                    }
                    let mut i = 0usize;
                    while i < installed_count {
                        if let Err(e) = write_i32_to_user(
                            ctx.current,
                            hdr.msg_control + (cmsg_hdr_len + i * data_item_len) as u64,
                            installed[i],
                        ) {
                            let mut j = 0usize;
                            while j < installed_count {
                                let _ = fd_close(ctx.current, installed[j]);
                                j += 1;
                            }
                            socket::socket_release_rights(&msg.fds[installed_count..msg.count]);
                            return -e;
                        }
                        i += 1;
                    }

                    out_hdr.msg_controllen = cmsg_align(cmsg_len) as u64;
                    if installed_count < msg.count {
                        out_hdr.msg_flags |= MSG_CTRUNC;
                        socket::socket_release_rights(&msg.fds[installed_count..msg.count]);
                    }
                }
            }
        }
    }

    if let Err(e) = write_struct_to_user(ctx.current, msghdr_ptr, &out_hdr) {
        return -e;
    }

    n as i64
}

pub(super) fn sys_shutdown(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let how = call.arg(1) as i32;

    if how < 0 || how > 2 {
        return -EINVAL;
    }

    let (_, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };

    match socket::socket_shutdown(sock_id, how) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

pub(super) fn sys_getsockname(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let addr_ptr = call.arg(1);
    let addrlen_ptr = call.arg(2);

    let (_, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let (ip, port) = match socket::socket_getsockname(sock_id) {
        Ok(v) => v,
        Err(e) => return e,
    };

    match write_sockaddr_in(ctx.current, addr_ptr, addrlen_ptr, &ip, port) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

pub(super) fn sys_getpeername(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let addr_ptr = call.arg(1);
    let addrlen_ptr = call.arg(2);

    let (_, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let (ip, port) = match socket::socket_getpeername(sock_id) {
        Ok(v) => v,
        Err(e) => return e,
    };

    match write_sockaddr_in(ctx.current, addr_ptr, addrlen_ptr, &ip, port) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

pub(super) fn sys_socketpair(_ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    let ctx = _ctx;
    let call = _call;

    let domain = call.arg(0) as u32;
    let raw_type = call.arg(1) as u32;
    let protocol = call.arg(2) as u32;
    let sv_ptr = call.arg(3);

    if sv_ptr == 0 {
        return -EFAULT;
    }
    if domain != AF_UNIX {
        return -EAFNOSUPPORT;
    }

    let (base_type, open_flags) = match socket_type_base_and_open_flags(raw_type) {
        Ok(v) => v,
        Err(err) => return err,
    };
    if base_type != SOCK_STREAM || protocol != 0 {
        return -EOPNOTSUPP;
    }

    let pair = socket::socket_create_unix_pair();
    if let Ok((a, b)) = pair {
        let cred = current_unix_cred(ctx.current);
        let _ = socket::socket_set_peer_cred(a, cred);
        let _ = socket::socket_set_peer_cred(b, cred);
    }
    let (sock0, sock1) = match pair {
        Ok(pair) => pair,
        Err(e) => return e,
    };

    let fd_flags = fd_flags_from_open_flags(open_flags);

    let fd0 = match fd_alloc(ctx.current, task::USER_FD_KIND_SOCKET, sock0, fd_flags) {
        Ok(fd) => fd as i32,
        Err(e) => {
            let _ = socket::socket_close(sock0);
            let _ = socket::socket_close(sock1);
            return e;
        }
    };

    let fd1 = match fd_alloc(ctx.current, task::USER_FD_KIND_SOCKET, sock1, fd_flags) {
        Ok(fd) => fd as i32,
        Err(e) => {
            let _ = fd_close(ctx.current, fd0);
            let _ = socket::socket_close(sock1);
            return e;
        }
    };

    if let Err(err) = write_i32_to_user(ctx.current, sv_ptr, fd0) {
        let _ = fd_close(ctx.current, fd0);
        let _ = fd_close(ctx.current, fd1);
        return -err;
    }
    if let Err(err) = write_i32_to_user(ctx.current, sv_ptr + size_of::<i32>() as u64, fd1) {
        let _ = fd_close(ctx.current, fd0);
        let _ = fd_close(ctx.current, fd1);
        return -err;
    }

    0
}

pub(super) fn sys_setsockopt(_ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    // Stub: silently accept common options
    0
}

pub(super) fn current_unix_cred(task_ptr: *mut task::Task) -> socket::UnixCred {
    if task_ptr.is_null() {
        return socket::UnixCred::NONE;
    }
    unsafe {
        socket::UnixCred {
            pid: (*task_ptr).pid,
            uid: (*task_ptr).user_uid,
            gid: (*task_ptr).user_gid,
        }
    }
}

/// Linux `struct ucred { pid_t pid; uid_t uid; gid_t gid; }`.
pub(super) fn ucred_bytes(cred: socket::UnixCred) -> [u8; 12] {
    let mut out = [0u8; 12];
    out[0..4].copy_from_slice(&cred.pid.to_ne_bytes());
    out[4..8].copy_from_slice(&cred.uid.to_ne_bytes());
    out[8..12].copy_from_slice(&cred.gid.to_ne_bytes());
    out
}

/// Copy an option value honouring the caller's `optlen` (truncating like
/// Linux) and write back the stored length.
pub(super) fn write_sockopt_bytes(
    task_ptr: *mut task::Task,
    optval_ptr: u64,
    optlen_ptr: u64,
    value: &[u8],
) -> i64 {
    if optlen_ptr == 0 {
        return -EFAULT;
    }
    let mut len_raw = [0u8; 4];
    if let Err(err) = copy_from_user_task(task_ptr, optlen_ptr, &mut len_raw) {
        return -err;
    }
    let requested = i32::from_ne_bytes(len_raw);
    if requested < 0 {
        return -EINVAL;
    }
    let n = core::cmp::min(requested as usize, value.len());
    if n > 0 {
        if let Err(err) = copy_to_user_task(task_ptr, optval_ptr, &value[..n]) {
            return -err;
        }
    }
    if let Err(err) = copy_to_user_task(task_ptr, optlen_ptr, &(n as u32).to_ne_bytes()) {
        return -err;
    }
    0
}

pub(super) fn sys_getsockopt(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let level = call.arg(1) as u32;
    let optname = call.arg(2) as u32;
    let optval_ptr = call.arg(3);
    let optlen_ptr = call.arg(4);

    // Validate this is a socket
    let (_, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };

    if level == SOL_SOCKET && optname == SO_PEERCRED {
        let cred = match socket::socket_peer_cred(sock_id) {
            Ok(cred) => cred,
            Err(err) => return err,
        };
        return write_sockopt_bytes(ctx.current, optval_ptr, optlen_ptr, &ucred_bytes(cred));
    }

    // Return 0 for SO_ERROR and other common queries
    if level == SOL_SOCKET && optname == SO_ERROR {
        let zero: u32 = 0;
        let bytes = zero.to_le_bytes();
        if let Err(e) = copy_to_user_task(ctx.current, optval_ptr, &bytes) {
            return -e;
        }
        if optlen_ptr != 0 {
            let len: u32 = 4;
            let len_bytes = len.to_le_bytes();
            if let Err(e) = copy_to_user_task(ctx.current, optlen_ptr, &len_bytes) {
                return -e;
            }
        }
        return 0;
    }

    // For other options, return 0 (success) with a zero value
    if optval_ptr != 0 {
        let zero: u32 = 0;
        let bytes = zero.to_le_bytes();
        let _ = copy_to_user_task(ctx.current, optval_ptr, &bytes);
    }
    if optlen_ptr != 0 {
        let len: u32 = 4;
        let len_bytes = len.to_le_bytes();
        let _ = copy_to_user_task(ctx.current, optlen_ptr, &len_bytes);
    }
    0
}
