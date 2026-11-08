//! Per-task file descriptor table, dup/fcntl, pipes and memfd.

use super::*;

#[inline]
pub(super) fn fd_index(fd: i32) -> Result<usize, i64> {
    if fd < 0 || fd as usize >= task::MAX_TASK_FDS {
        return Err(-EBADF);
    }
    Ok(fd as usize)
}

pub(super) fn fd_slot_is_open(task_ptr: *mut task::Task, idx: usize) -> bool {
    unsafe { (*task_ptr).user_fd_kind[idx] != task::USER_FD_KIND_EMPTY }
}

#[inline]
pub(super) fn fd_flags_from_open_flags(flags: u32) -> u32 {
    let mut fd_flags = flags & !O_CLOEXEC;
    if (flags & O_CLOEXEC) != 0 {
        fd_flags |= FD_INTERNAL_CLOEXEC;
    }
    fd_flags
}

#[inline]
pub(super) fn fd_open_flags(flags: u32) -> u32 {
    flags & !FD_INTERNAL_MASK
}

#[inline]
pub(super) fn fd_internal_flags(flags: u32) -> u32 {
    flags & FD_INTERNAL_MASK
}

#[inline]
pub(super) fn fd_is_memfd(flags: u32) -> bool {
    (flags & FD_INTERNAL_MEMFD) != 0
}

#[inline]
pub(super) fn fd_has_cloexec(flags: u32) -> bool {
    (flags & FD_INTERNAL_CLOEXEC) != 0
}

#[inline]
pub(super) fn fd_set_cloexec(flags: u32, enabled: bool) -> u32 {
    if enabled {
        flags | FD_INTERNAL_CLOEXEC
    } else {
        flags & !FD_INTERNAL_CLOEXEC
    }
}

pub(super) fn fd_find_free_from(task_ptr: *mut task::Task, start: usize) -> Option<usize> {
    if task_ptr.is_null() || start >= task::MAX_TASK_FDS {
        return None;
    }
    for idx in start..task::MAX_TASK_FDS {
        if unsafe { (*task_ptr).user_fd_kind[idx] == task::USER_FD_KIND_EMPTY } {
            return Some(idx);
        }
    }
    None
}

pub(super) fn task_memfd_contains_inode(task_ptr: *mut task::Task, inode: u16) -> bool {
    if task_ptr.is_null() || inode == task::USER_FD_NONE {
        return false;
    }

    let mut idx = 0usize;
    while idx < task::MAX_TASK_FDS {
        let is_match = unsafe {
            fd_slot_is_open(task_ptr, idx)
                && fd_is_memfd((*task_ptr).user_fd_flags[idx])
                && (*task_ptr).user_fd_inode[idx] == inode
        };
        if is_match {
            return true;
        }
        idx += 1;
    }
    false
}

pub(super) fn task_memfd_object_count(task_ptr: *mut task::Task) -> usize {
    if task_ptr.is_null() {
        return 0;
    }

    let mut count = 0usize;
    let mut seen = [task::USER_FD_NONE; task::MAX_TASK_FDS];
    let mut idx = 0usize;
    while idx < task::MAX_TASK_FDS {
        let (open, flags, inode) = unsafe {
            (
                fd_slot_is_open(task_ptr, idx),
                (*task_ptr).user_fd_flags[idx],
                (*task_ptr).user_fd_inode[idx],
            )
        };
        if open && fd_is_memfd(flags) && inode != task::USER_FD_NONE {
            let mut known = false;
            let mut seen_idx = 0usize;
            while seen_idx < count {
                if seen[seen_idx] == inode {
                    known = true;
                    break;
                }
                seen_idx += 1;
            }
            if !known && count < seen.len() {
                seen[count] = inode;
                count += 1;
            }
        }
        idx += 1;
    }
    count
}

pub(super) fn enforce_memfd_object_limit_for_inode(
    task_ptr: *mut task::Task,
    inode: u16,
) -> Result<(), i64> {
    if task_ptr.is_null() {
        return Err(-ESRCH);
    }
    if inode == task::USER_FD_NONE {
        return Err(-EINVAL);
    }
    if task_memfd_contains_inode(task_ptr, inode) {
        return Ok(());
    }
    if task_memfd_object_count(task_ptr) >= MEMFD_MAX_BUFFERS_PER_TASK {
        return Err(-EMFILE);
    }
    Ok(())
}

pub(super) fn enforce_memfd_create_limit(task_ptr: *mut task::Task) -> Result<(), i64> {
    if task_ptr.is_null() {
        return Err(-ESRCH);
    }
    if task_memfd_object_count(task_ptr) >= MEMFD_MAX_BUFFERS_PER_TASK {
        return Err(-EMFILE);
    }
    Ok(())
}

pub(super) fn fd_write_slot(
    task_ptr: *mut task::Task,
    idx: usize,
    kind: u8,
    inode: u16,
    offset: u64,
    flags: u32,
) {
    unsafe {
        (*task_ptr).user_fd_kind[idx] = kind;
        (*task_ptr).user_fd_inode[idx] = if kind == task::USER_FD_KIND_VFS
            || kind == task::USER_FD_KIND_PIPE
            || kind == task::USER_FD_KIND_SOCKET
            || kind == task::USER_FD_KIND_PTY_MASTER
            || kind == task::USER_FD_KIND_PTY_SLAVE
            || kind == task::USER_FD_KIND_EPOLL
        {
            inode
        } else {
            task::USER_FD_NONE
        };
        (*task_ptr).user_fd_offset[idx] = offset;
        (*task_ptr).user_fd_flags[idx] = flags;
    }
}

pub(super) fn fd_alloc(
    task_ptr: *mut task::Task,
    kind: u8,
    inode: u16,
    flags: u32,
) -> Result<i64, i64> {
    if task_ptr.is_null() {
        return Err(-ESRCH);
    }

    let idx = match fd_find_free_from(task_ptr, 3) {
        Some(idx) => idx,
        None => return Err(-EMFILE),
    };
    fd_write_slot(task_ptr, idx, kind, inode, 0, flags);
    Ok(idx as i64)
}

pub(super) fn fd_dup_from(
    task_ptr: *mut task::Task,
    old_fd: i32,
    min_fd: usize,
    cloexec: bool,
) -> Result<i64, i64> {
    if task_ptr.is_null() {
        return Err(-ESRCH);
    }
    let src_idx = fd_index(old_fd)?;
    if !fd_slot_is_open(task_ptr, src_idx) {
        return Err(-EBADF);
    }

    let dst_idx = match fd_find_free_from(task_ptr, min_fd) {
        Some(idx) => idx,
        None => return Err(-EMFILE),
    };

    let (kind, inode, offset, src_flags) = unsafe {
        (
            (*task_ptr).user_fd_kind[src_idx],
            (*task_ptr).user_fd_inode[src_idx],
            (*task_ptr).user_fd_offset[src_idx],
            (*task_ptr).user_fd_flags[src_idx],
        )
    };
    if fd_is_memfd(src_flags) {
        enforce_memfd_object_limit_for_inode(task_ptr, inode)?;
    }
    let new_flags = fd_set_cloexec(src_flags, cloexec);
    fd_write_slot(task_ptr, dst_idx, kind, inode, offset, new_flags);
    if kind == task::USER_FD_KIND_PTY_MASTER && inode != task::USER_FD_NONE {
        crate::pty::master_retain(inode);
    }
    if kind == task::USER_FD_KIND_PTY_SLAVE && inode != task::USER_FD_NONE {
        crate::pty::slave_retain(inode);
    }
    if kind == task::USER_FD_KIND_SOCKET && inode != task::USER_FD_NONE {
        socket::socket_retain(inode)?;
    }
    if kind == task::USER_FD_KIND_EPOLL && inode != task::USER_FD_NONE {
        epoll_retain(inode)?;
    }
    Ok(dst_idx as i64)
}

pub(super) fn fd_dup_to(
    task_ptr: *mut task::Task,
    old_fd: i32,
    new_fd: i32,
    cloexec: bool,
    reject_same: bool,
) -> Result<i64, i64> {
    if task_ptr.is_null() {
        return Err(-ESRCH);
    }
    let src_idx = fd_index(old_fd)?;
    let dst_idx = fd_index(new_fd)?;
    if !fd_slot_is_open(task_ptr, src_idx) {
        return Err(-EBADF);
    }

    if src_idx == dst_idx {
        if reject_same {
            return Err(-EINVAL);
        }
        return Ok(new_fd as i64);
    }

    if fd_slot_is_open(task_ptr, dst_idx) {
        fd_close(task_ptr, new_fd)?;
    }

    let (kind, inode, offset, src_flags) = unsafe {
        (
            (*task_ptr).user_fd_kind[src_idx],
            (*task_ptr).user_fd_inode[src_idx],
            (*task_ptr).user_fd_offset[src_idx],
            (*task_ptr).user_fd_flags[src_idx],
        )
    };
    if fd_is_memfd(src_flags) {
        enforce_memfd_object_limit_for_inode(task_ptr, inode)?;
    }
    let new_flags = fd_set_cloexec(src_flags, cloexec);
    fd_write_slot(task_ptr, dst_idx, kind, inode, offset, new_flags);
    if kind == task::USER_FD_KIND_PTY_MASTER && inode != task::USER_FD_NONE {
        crate::pty::master_retain(inode);
    }
    if kind == task::USER_FD_KIND_PTY_SLAVE && inode != task::USER_FD_NONE {
        crate::pty::slave_retain(inode);
    }
    if kind == task::USER_FD_KIND_SOCKET && inode != task::USER_FD_NONE {
        socket::socket_retain(inode)?;
    }
    if kind == task::USER_FD_KIND_EPOLL && inode != task::USER_FD_NONE {
        epoll_retain(inode)?;
    }
    Ok(new_fd as i64)
}

pub(super) fn fd_close(task_ptr: *mut task::Task, fd: i32) -> Result<(), i64> {
    let idx = fd_index(fd)?;
    if task_ptr.is_null() {
        return Err(-ESRCH);
    }
    if !fd_slot_is_open(task_ptr, idx) {
        return Err(-EBADF);
    }

    let (kind, inode, flags) = unsafe {
        (
            (*task_ptr).user_fd_kind[idx],
            (*task_ptr).user_fd_inode[idx],
            (*task_ptr).user_fd_flags[idx],
        )
    };
    let owner_pid = unsafe { (*task_ptr).pid };

    epoll_drop_watches_for_fd(owner_pid, fd, kind, inode);

    if kind == task::USER_FD_KIND_PIPE {
        let close_read = fd_can_read(flags);
        let close_write = fd_can_write(flags);
        if inode != task::USER_FD_NONE {
            let _ = ipc::pipe_close(inode, close_read, close_write);
        }
    } else if kind == task::USER_FD_KIND_SOCKET {
        if inode != task::USER_FD_NONE {
            let _ = socket::socket_close(inode);
        }
    } else if kind == task::USER_FD_KIND_PTY_MASTER {
        if inode != task::USER_FD_NONE {
            crate::pty::master_close(inode);
        }
    } else if kind == task::USER_FD_KIND_PTY_SLAVE {
        if inode != task::USER_FD_NONE {
            crate::pty::slave_close(inode);
        }
    } else if kind == task::USER_FD_KIND_EPOLL {
        if inode != task::USER_FD_NONE {
            let _ = epoll_close(inode);
        }
    }

    unsafe {
        (*task_ptr).user_fd_kind[idx] = task::USER_FD_KIND_EMPTY;
        (*task_ptr).user_fd_inode[idx] = task::USER_FD_NONE;
        (*task_ptr).user_fd_offset[idx] = 0;
        (*task_ptr).user_fd_flags[idx] = 0;
    }
    Ok(())
}

pub(super) fn close_all_fds(task_ptr: *mut task::Task) {
    if task_ptr.is_null() {
        return;
    }

    let mut fd = 0usize;
    while fd < task::MAX_TASK_FDS {
        let _ = fd_close(task_ptr, fd as i32);
        fd += 1;
    }
}

pub(super) fn close_cloexec_fds(task_ptr: *mut task::Task) {
    if task_ptr.is_null() {
        return;
    }

    let mut fd = 0usize;
    while fd < task::MAX_TASK_FDS {
        let should_close = unsafe {
            fd_slot_is_open(task_ptr, fd) && fd_has_cloexec((*task_ptr).user_fd_flags[fd])
        };
        if should_close {
            let _ = fd_close(task_ptr, fd as i32);
        }
        fd += 1;
    }
}

pub(super) fn fd_access_mode(flags: u32) -> u32 {
    fd_open_flags(flags) & fs::O_ACCMODE
}

pub(super) fn fd_can_read(flags: u32) -> bool {
    let mode = fd_access_mode(flags);
    mode == fs::O_RDONLY || mode == fs::O_RDWR
}

pub(super) fn fd_can_write(flags: u32) -> bool {
    let mode = fd_access_mode(flags);
    mode == fs::O_WRONLY || mode == fs::O_RDWR
}

#[inline]
pub(super) fn fcntl_setfl_mutable_mask(kind: u8) -> u32 {
    match kind {
        task::USER_FD_KIND_VFS => fs::O_APPEND | O_NONBLOCK,
        task::USER_FD_KIND_PIPE
        | task::USER_FD_KIND_SOCKET
        | task::USER_FD_KIND_PTY_MASTER
        | task::USER_FD_KIND_PTY_SLAVE => O_NONBLOCK,
        _ => 0,
    }
}

pub(super) fn fd_apply_fcntl_setfl(current_flags: u32, kind: u8, requested_flags: u32) -> u32 {
    let current_open = fd_open_flags(current_flags);
    let preserved_access = current_open & fs::O_ACCMODE;
    let mutable = fcntl_setfl_mutable_mask(kind);
    let mut next_open = current_open & !mutable;
    next_open |= requested_flags & mutable;
    next_open = (next_open & !fs::O_ACCMODE) | preserved_access;
    fd_internal_flags(current_flags) | next_open
}

pub(super) fn memfd_validate_flags(flags: u32) -> Result<bool, i64> {
    let supported = MFD_CLOEXEC | MFD_ALLOW_SEALING;
    if (flags & !supported) != 0 {
        return Err(-EINVAL);
    }
    if (flags & MFD_ALLOW_SEALING) != 0 {
        return Err(-ENOSYS);
    }
    Ok((flags & MFD_CLOEXEC) != 0)
}

pub(super) fn memfd_storage_dir_ready() -> Result<(), i64> {
    match fs::lookup(MEMFD_STORAGE_DIR) {
        Ok(inode) => match fs::inode_is_dir(inode) {
            Ok(true) => Ok(()),
            Ok(false) => Err(-ENOTDIR),
            Err(err) => Err(-fs_to_errno(err)),
        },
        Err(fs::ENOENT) => match fs::mkdir(MEMFD_STORAGE_DIR) {
            Ok(()) => Ok(()),
            Err(fs::EEXIST) => Ok(()),
            Err(err) => Err(-fs_to_errno(err)),
        },
        Err(err) => Err(-fs_to_errno(err)),
    }
}

pub(super) fn memfd_name_sanitize(raw: &[u8], out: &mut [u8; MEMFD_NAME_MAX]) -> usize {
    let mut len = 0usize;
    for &byte in raw {
        if len >= out.len() {
            break;
        }
        out[len] = match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => byte,
            _ => b'_',
        };
        len += 1;
    }
    if len == 0 {
        out[0] = b'a';
        out[1] = b'n';
        out[2] = b'o';
        out[3] = b'n';
        4
    } else {
        len
    }
}

pub(super) fn memfd_push_bytes(dst: &mut [u8], len: &mut usize, src: &[u8]) -> Result<(), i64> {
    let end = match len.checked_add(src.len()) {
        Some(v) => v,
        None => return Err(-EINVAL),
    };
    if end > dst.len() {
        return Err(-EINVAL);
    }
    dst[*len..end].copy_from_slice(src);
    *len = end;
    Ok(())
}

pub(super) fn memfd_push_u32_decimal(
    dst: &mut [u8],
    len: &mut usize,
    mut value: u32,
) -> Result<(), i64> {
    let mut tmp = [0u8; 10];
    let mut digits = 0usize;
    loop {
        tmp[digits] = b'0' + (value % 10) as u8;
        digits += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    let mut idx = digits;
    while idx > 0 {
        idx -= 1;
        memfd_push_bytes(dst, len, &tmp[idx..idx + 1])?;
    }
    Ok(())
}

pub(super) fn memfd_build_path<'a>(
    pid: i32,
    sequence: u32,
    raw_name: &[u8],
    out: &'a mut [u8; fs::VFS_MAX_PATH],
) -> Result<&'a str, i64> {
    let mut len = 0usize;
    memfd_push_bytes(out, &mut len, MEMFD_PATH_PREFIX)?;
    let pid_u32 = if pid > 0 { pid as u32 } else { 0 };
    memfd_push_u32_decimal(out, &mut len, pid_u32)?;
    memfd_push_bytes(out, &mut len, b"-")?;
    memfd_push_u32_decimal(out, &mut len, sequence)?;
    memfd_push_bytes(out, &mut len, b"-")?;

    let mut sanitized = [0u8; MEMFD_NAME_MAX];
    let sanitized_len = memfd_name_sanitize(raw_name, &mut sanitized);
    memfd_push_bytes(out, &mut len, &sanitized[..sanitized_len])?;

    core::str::from_utf8(&out[..len]).map_err(|_| -EINVAL)
}

pub(super) fn sys_memfd_create(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let cloexec = match memfd_validate_flags(call.arg(1) as u32) {
        Ok(value) => value,
        Err(err) => return err,
    };

    let mut name_buf = [0u8; MEMFD_NAME_MAX + 1];
    let name_len = match copy_cstr_from_user_current(call.arg(0), &mut name_buf) {
        Ok(len) => len,
        Err(err) => return -err,
    };
    if let Err(err) = enforce_memfd_create_limit(ctx.current) {
        return err;
    }

    if let Err(err) = memfd_storage_dir_ready() {
        return err;
    }

    let pid = unsafe { (*ctx.current).pid };
    let mut path_buf = [0u8; fs::VFS_MAX_PATH];

    for _ in 0..MEMFD_CREATE_MAX_RETRIES {
        let sequence = MEMFD_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = match memfd_build_path(pid, sequence, &name_buf[..name_len], &mut path_buf) {
            Ok(path) => path,
            Err(err) => return err,
        };

        let opened = match fs::open(path, fs::O_CREAT | fs::O_EXCL | fs::O_RDWR, 0o600) {
            Ok(opened) => opened,
            Err(fs::EEXIST) => continue,
            Err(fs::ENOSPC) => return -ENFILE,
            Err(err) => return -fs_to_errno(err),
        };

        let mut open_flags = fs::O_RDWR;
        if cloexec {
            open_flags |= O_CLOEXEC;
        }
        let fd_flags = fd_flags_from_open_flags(open_flags) | FD_INTERNAL_MEMFD;
        return match fd_alloc(ctx.current, task::USER_FD_KIND_VFS, opened.inode, fd_flags) {
            Ok(fd) => fd,
            Err(err) => {
                let _ = fs::unlink(path);
                err
            }
        };
    }

    -EAGAIN
}

pub(super) fn write_pipe_fds(
    task_ptr: *mut task::Task,
    user_dst: u64,
    read_fd: i32,
    write_fd: i32,
) -> Result<(), i64> {
    let mut raw = [0u8; 8];
    raw[..4].copy_from_slice(&read_fd.to_ne_bytes());
    raw[4..8].copy_from_slice(&write_fd.to_ne_bytes());
    copy_to_user_task(task_ptr, user_dst, &raw)
}

pub(super) fn sys_pipe2(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let user_pipefd = call.arg(0);
    let flags = call.arg(1) as u32;
    if (flags & !(O_CLOEXEC | O_NONBLOCK)) != 0 {
        return -EINVAL;
    }
    let mut read_open_flags = fs::O_RDONLY;
    let mut write_open_flags = fs::O_WRONLY;
    if (flags & O_NONBLOCK) != 0 {
        read_open_flags |= O_NONBLOCK;
        write_open_flags |= O_NONBLOCK;
    }
    if (flags & O_CLOEXEC) != 0 {
        read_open_flags |= O_CLOEXEC;
        write_open_flags |= O_CLOEXEC;
    }
    let read_flags = fd_flags_from_open_flags(read_open_flags);
    let write_flags = fd_flags_from_open_flags(write_open_flags);

    let pipe_id = match ipc::pipe_open() {
        Ok(id) => id,
        Err(err) => return -ipc_to_errno(err),
    };

    let read_fd = match fd_alloc(ctx.current, task::USER_FD_KIND_PIPE, pipe_id, read_flags) {
        Ok(fd) => fd as i32,
        Err(err) => {
            let _ = ipc::pipe_close(pipe_id, true, true);
            return err;
        }
    };
    let write_fd = match fd_alloc(ctx.current, task::USER_FD_KIND_PIPE, pipe_id, write_flags) {
        Ok(fd) => fd as i32,
        Err(err) => {
            let _ = fd_close(ctx.current, read_fd);
            return err;
        }
    };

    if let Err(err) = write_pipe_fds(ctx.current, user_pipefd, read_fd, write_fd) {
        let _ = fd_close(ctx.current, read_fd);
        let _ = fd_close(ctx.current, write_fd);
        return -err;
    }

    0
}

pub(super) fn sys_pipe(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let wrapped = SyscallArgs {
        num: SYS_PIPE2,
        args: [call.arg(0), 0, 0, 0, 0, 0],
    };
    sys_pipe2(ctx, &wrapped)
}

pub(super) fn sys_close(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    match fd_close(ctx.current, call.arg(0) as i32) {
        Ok(()) => 0,
        Err(err) => err,
    }
}

pub(super) fn parse_dup_min_fd(arg: u64) -> Result<usize, i64> {
    let min = i64::try_from(arg).map_err(|_| -EINVAL)?;
    if min < 0 {
        return Err(-EINVAL);
    }
    let min = min as usize;
    if min >= task::MAX_TASK_FDS {
        return Err(-EINVAL);
    }
    Ok(min)
}

pub(super) fn sys_dup(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    match fd_dup_from(ctx.current, call.arg(0) as i32, 0, false) {
        Ok(fd) => fd,
        Err(err) => err,
    }
}

pub(super) fn sys_dup2(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    match fd_dup_to(
        ctx.current,
        call.arg(0) as i32,
        call.arg(1) as i32,
        false,
        false,
    ) {
        Ok(fd) => fd,
        Err(err) => err,
    }
}

pub(super) fn sys_dup3(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let flags = call.arg(2) as u32;
    if (flags & !O_CLOEXEC) != 0 {
        return -EINVAL;
    }
    let cloexec = (flags & O_CLOEXEC) != 0;
    match fd_dup_to(
        ctx.current,
        call.arg(0) as i32,
        call.arg(1) as i32,
        cloexec,
        true,
    ) {
        Ok(fd) => fd,
        Err(err) => err,
    }
}

pub(super) fn sys_fcntl(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let cmd = call.arg(1) as u32;
    let arg = call.arg(2);
    let idx = match fd_index(fd) {
        Ok(idx) => idx,
        Err(err) => return err,
    };
    if !fd_slot_is_open(ctx.current, idx) {
        return -EBADF;
    }

    match cmd {
        F_GETFD => {
            let flags = unsafe { (*ctx.current).user_fd_flags[idx] };
            if fd_has_cloexec(flags) {
                FD_CLOEXEC as i64
            } else {
                0
            }
        }
        F_SETFD => {
            let cloexec = (arg as u32 & FD_CLOEXEC) != 0;
            unsafe {
                let flags = (*ctx.current).user_fd_flags[idx];
                (*ctx.current).user_fd_flags[idx] = fd_set_cloexec(flags, cloexec);
            }
            0
        }
        F_GETFL => {
            let flags = unsafe { (*ctx.current).user_fd_flags[idx] };
            fd_open_flags(flags) as i64
        }
        F_SETFL => {
            unsafe {
                let current_flags = (*ctx.current).user_fd_flags[idx];
                let kind = (*ctx.current).user_fd_kind[idx];
                let next_flags = fd_apply_fcntl_setfl(current_flags, kind, arg as u32);
                (*ctx.current).user_fd_flags[idx] = next_flags;
            }
            0
        }
        F_DUPFD => {
            let min_fd = match parse_dup_min_fd(arg) {
                Ok(v) => v,
                Err(err) => return err,
            };
            match fd_dup_from(ctx.current, fd, min_fd, false) {
                Ok(new_fd) => new_fd,
                Err(err) => err,
            }
        }
        F_DUPFD_CLOEXEC => {
            let min_fd = match parse_dup_min_fd(arg) {
                Ok(v) => v,
                Err(err) => return err,
            };
            match fd_dup_from(ctx.current, fd, min_fd, true) {
                Ok(new_fd) => new_fd,
                Err(err) => err,
            }
        }
        _ => -EINVAL,
    }
}
