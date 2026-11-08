//! select, poll and epoll.

use super::*;

#[inline]
pub(super) fn select_fdset_words(nfds: usize) -> usize {
    if nfds == 0 {
        0
    } else {
        (nfds + SELECT_FDSET_WORD_BITS - 1) / SELECT_FDSET_WORD_BITS
    }
}

#[inline]
pub(super) fn select_fdset_bytes(nfds: usize) -> usize {
    select_fdset_words(nfds) * size_of::<u64>()
}

#[inline]
pub(super) fn select_fdset_is_set(set: &SelectFdSet, fd: usize) -> bool {
    let word = fd / SELECT_FDSET_WORD_BITS;
    let bit = fd % SELECT_FDSET_WORD_BITS;
    (set[word] & (1u64 << bit)) != 0
}

#[inline]
pub(super) fn select_fdset_set(set: &mut SelectFdSet, fd: usize) {
    let word = fd / SELECT_FDSET_WORD_BITS;
    let bit = fd % SELECT_FDSET_WORD_BITS;
    set[word] |= 1u64 << bit;
}

pub(super) fn read_select_fdset(
    task_ptr: *mut task::Task,
    user_ptr: u64,
    nfds: usize,
) -> Result<SelectFdSet, i64> {
    let mut bits = [0u64; SELECT_FDSET_WORDS];
    if user_ptr == 0 || nfds == 0 {
        return Ok(bits);
    }

    if nfds > SELECT_MAX_FDS {
        return Err(-EINVAL);
    }

    let byte_len = select_fdset_bytes(nfds);
    let raw = unsafe { core::slice::from_raw_parts_mut(bits.as_mut_ptr() as *mut u8, byte_len) };
    copy_from_user_task(task_ptr, user_ptr, raw).map_err(|e| -e)?;
    Ok(bits)
}

pub(super) fn write_select_fdset(
    task_ptr: *mut task::Task,
    user_ptr: u64,
    nfds: usize,
    bits: &SelectFdSet,
) -> Result<(), i64> {
    if user_ptr == 0 || nfds == 0 {
        return Ok(());
    }

    if nfds > SELECT_MAX_FDS {
        return Err(-EINVAL);
    }

    let byte_len = select_fdset_bytes(nfds);
    let raw = unsafe { core::slice::from_raw_parts(bits.as_ptr() as *const u8, byte_len) };
    copy_to_user_task(task_ptr, user_ptr, raw).map_err(|e| -e)
}

pub(super) fn validate_select_fds(
    task_ptr: *mut task::Task,
    nfds: usize,
    read_bits: &SelectFdSet,
    write_bits: &SelectFdSet,
    except_bits: &SelectFdSet,
) -> i64 {
    for fd in 0..nfds {
        let requested = select_fdset_is_set(read_bits, fd)
            || select_fdset_is_set(write_bits, fd)
            || select_fdset_is_set(except_bits, fd);
        if !requested {
            continue;
        }
        if fd >= task::MAX_TASK_FDS || !fd_slot_is_open(task_ptr, fd) {
            return -EBADF;
        }
    }
    0
}

pub(super) fn do_select(
    ctx: &SyscallContext,
    nfds: usize,
    read_ptr: u64,
    write_ptr: u64,
    except_ptr: u64,
    deadline: Option<u64>,
) -> i64 {
    if nfds > SELECT_MAX_FDS {
        return -EINVAL;
    }

    let read_req = match read_select_fdset(ctx.current, read_ptr, nfds) {
        Ok(bits) => bits,
        Err(e) => return e,
    };
    let write_req = match read_select_fdset(ctx.current, write_ptr, nfds) {
        Ok(bits) => bits,
        Err(e) => return e,
    };
    let except_req = match read_select_fdset(ctx.current, except_ptr, nfds) {
        Ok(bits) => bits,
        Err(e) => return e,
    };

    let validate = validate_select_fds(ctx.current, nfds, &read_req, &write_req, &except_req);
    if validate != 0 {
        return validate;
    }

    loop {
        let mut out_read = [0u64; SELECT_FDSET_WORDS];
        let mut out_write = [0u64; SELECT_FDSET_WORDS];
        let mut out_except = [0u64; SELECT_FDSET_WORDS];
        let mut ready_count: i64 = 0;

        let eval_nfds = cmp::min(nfds, task::MAX_TASK_FDS);
        for fd in 0..eval_nfds {
            let fd_i32 = fd as i32;

            if select_fdset_is_set(&read_req, fd) {
                let revents = poll_fd_revents(ctx.current, fd_i32, POLLIN | POLLPRI);
                if (revents & (POLLIN | POLLERR | POLLHUP)) != 0 {
                    select_fdset_set(&mut out_read, fd);
                    ready_count += 1;
                }
            }

            if select_fdset_is_set(&write_req, fd) {
                let revents = poll_fd_revents(ctx.current, fd_i32, POLLOUT);
                if (revents & (POLLOUT | POLLERR)) != 0 {
                    select_fdset_set(&mut out_write, fd);
                    ready_count += 1;
                }
            }

            if select_fdset_is_set(&except_req, fd) {
                let revents = poll_fd_revents(ctx.current, fd_i32, POLLPRI);
                if (revents & (POLLPRI | POLLERR)) != 0 {
                    select_fdset_set(&mut out_except, fd);
                    ready_count += 1;
                }
            }
        }

        if ready_count > 0 || deadline == Some(0) {
            if let Err(e) = write_select_fdset(ctx.current, read_ptr, nfds, &out_read) {
                return e;
            }
            if let Err(e) = write_select_fdset(ctx.current, write_ptr, nfds, &out_write) {
                return e;
            }
            if let Err(e) = write_select_fdset(ctx.current, except_ptr, nfds, &out_except) {
                return e;
            }
            return ready_count;
        }

        if let Some(dl) = deadline {
            if pit::uptime_ms() >= dl {
                let zero = [0u64; SELECT_FDSET_WORDS];
                let _ = write_select_fdset(ctx.current, read_ptr, nfds, &zero);
                let _ = write_select_fdset(ctx.current, write_ptr, nfds, &zero);
                let _ = write_select_fdset(ctx.current, except_ptr, nfds, &zero);
                return 0;
            }
        }

        #[cfg(not(test))]
        {
            crate::net::poll();
            crate::sched::yield_now();
        }
    }
}

/// Compute revents for a single FD.
pub(super) fn poll_fd_revents(task_ptr: *mut task::Task, fd: i32, requested: i16) -> i16 {
    let idx = match fd_index(fd) {
        Ok(i) => i,
        Err(_) => return POLLNVAL,
    };
    if !fd_slot_is_open(task_ptr, idx) {
        return POLLNVAL;
    }

    let (kind, inode, flags) = unsafe {
        (
            (*task_ptr).user_fd_kind[idx],
            (*task_ptr).user_fd_inode[idx],
            (*task_ptr).user_fd_flags[idx],
        )
    };

    let mut revents: i16 = 0;

    if kind == task::USER_FD_KIND_CONSOLE {
        if (requested & POLLIN) != 0 && tty::has_ready_data() {
            revents |= POLLIN;
        }
        if (requested & POLLOUT) != 0 {
            revents |= POLLOUT; // console always writable
        }
    } else if kind == task::USER_FD_KIND_VFS {
        if let Some(device_id) = fs::inode_input_device_id(inode) {
            if (requested & POLLIN) != 0 && crate::input::device_has_ready_data(device_id) {
                revents |= POLLIN;
            }
        } else {
            // Regular files are always ready
            if (requested & POLLIN) != 0 {
                revents |= POLLIN;
            }
            if (requested & POLLOUT) != 0 {
                revents |= POLLOUT;
            }
        }
    } else if kind == task::USER_FD_KIND_PIPE {
        if let Ok((has_data, has_space, read_eof, write_broken)) = ipc::pipe_poll_status(inode) {
            let can_read = fd_can_read(flags);
            let can_write = fd_can_write(flags);
            if can_read {
                if has_data {
                    revents |= POLLIN;
                }
                if read_eof {
                    revents |= POLLHUP;
                }
            }
            if can_write {
                if has_space {
                    revents |= POLLOUT;
                }
                if write_broken {
                    revents |= POLLERR;
                }
            }
        } else {
            revents |= POLLNVAL;
        }
    } else if kind == task::USER_FD_KIND_SOCKET {
        let sock_events = socket::socket_poll_events(inode) as i16;
        // Always report POLLERR/POLLHUP/POLLNVAL regardless of requested
        revents = sock_events & (requested | POLLERR | POLLHUP | POLLNVAL);
    } else if kind == task::USER_FD_KIND_PTY_MASTER {
        let (has_data, can_write) = crate::pty::master_poll(inode);
        if (requested & POLLIN) != 0 && has_data {
            revents |= POLLIN;
        }
        if (requested & POLLOUT) != 0 && can_write {
            revents |= POLLOUT;
        }
    } else if kind == task::USER_FD_KIND_PTY_SLAVE {
        let (has_data, can_write) = crate::pty::slave_poll(inode);
        if (requested & POLLIN) != 0 && has_data {
            revents |= POLLIN;
        }
        if (requested & POLLOUT) != 0 && can_write {
            revents |= POLLOUT;
        }
    }

    revents
}

#[inline]
pub(super) fn task_has_unblocked_signal(task_ptr: *mut task::Task) -> bool {
    if task_ptr.is_null() {
        return false;
    }
    unsafe { ((*task_ptr).sig_pending & !(*task_ptr).signal_blocked) != 0 }
}

pub(super) fn epoll_instance_get_mut(id: u16) -> Result<&'static mut EpollInstance, i64> {
    let idx = id as usize;
    if idx >= EPOLL_MAX_INSTANCES {
        return Err(-EBADF);
    }
    unsafe {
        let inst = &mut EPOLL_INSTANCES[idx];
        if !inst.used {
            return Err(-EBADF);
        }
        Ok(inst)
    }
}

pub(super) fn epoll_alloc_instance() -> Result<u16, i64> {
    unsafe {
        for idx in 0..EPOLL_MAX_INSTANCES {
            if !EPOLL_INSTANCES[idx].used {
                let inst = &mut EPOLL_INSTANCES[idx];
                inst.reset();
                inst.used = true;
                inst.refs = 1;
                return Ok(idx as u16);
            }
        }
    }
    Err(-ENFILE)
}

pub(super) fn epoll_retain(id: u16) -> Result<(), i64> {
    let inst = epoll_instance_get_mut(id)?;
    if inst.refs == 0 {
        return Err(-EBADF);
    }
    inst.refs = inst.refs.saturating_add(1);
    Ok(())
}

pub(super) fn epoll_close(id: u16) -> Result<(), i64> {
    let inst = epoll_instance_get_mut(id)?;
    if inst.refs > 1 {
        inst.refs -= 1;
        return Ok(());
    }
    inst.reset();
    Ok(())
}

#[inline]
pub(super) fn epoll_watch_find(inst: &EpollInstance, owner_pid: i32, fd: i32) -> Option<usize> {
    let mut idx = 0usize;
    while idx < EPOLL_MAX_WATCHES {
        let watch = inst.watches[idx];
        if watch.used && watch.owner_pid == owner_pid && watch.fd == fd {
            return Some(idx);
        }
        idx += 1;
    }
    None
}

#[inline]
pub(super) fn epoll_watch_remove(inst: &mut EpollInstance, idx: usize) {
    if idx >= EPOLL_MAX_WATCHES {
        return;
    }
    if inst.watches[idx].used {
        inst.watches[idx] = EpollWatch::EMPTY;
        inst.watch_count = inst.watch_count.saturating_sub(1);
    }
}

pub(super) fn epoll_drop_watches_for_fd(owner_pid: i32, fd: i32, kind: u8, inode: u16) {
    unsafe {
        let mut inst_idx = 0usize;
        while inst_idx < EPOLL_MAX_INSTANCES {
            let inst = &mut EPOLL_INSTANCES[inst_idx];
            if !inst.used || inst.watch_count == 0 {
                inst_idx += 1;
                continue;
            }

            let mut watch_idx = 0usize;
            while watch_idx < EPOLL_MAX_WATCHES {
                let watch = inst.watches[watch_idx];
                if watch.used
                    && watch.owner_pid == owner_pid
                    && watch.fd == fd
                    && watch.kind == kind
                    && watch.inode == inode
                {
                    epoll_watch_remove(inst, watch_idx);
                }
                watch_idx += 1;
            }
            inst_idx += 1;
        }
    }
}

pub(super) fn epoll_watch_count_for_owner(owner_pid: i32) -> usize {
    if owner_pid <= 0 {
        return 0;
    }
    let mut count = 0usize;
    unsafe {
        let mut inst_idx = 0usize;
        while inst_idx < EPOLL_MAX_INSTANCES {
            let inst = &EPOLL_INSTANCES[inst_idx];
            if !inst.used || inst.watch_count == 0 {
                inst_idx += 1;
                continue;
            }
            let mut watch_idx = 0usize;
            while watch_idx < EPOLL_MAX_WATCHES {
                let watch = inst.watches[watch_idx];
                if watch.used && watch.owner_pid == owner_pid {
                    count = count.saturating_add(1);
                }
                watch_idx += 1;
            }
            inst_idx += 1;
        }
    }
    count
}

pub(super) fn epoll_drop_watches_for_owner(owner_pid: i32) {
    if owner_pid <= 0 {
        return;
    }
    unsafe {
        let mut inst_idx = 0usize;
        while inst_idx < EPOLL_MAX_INSTANCES {
            let inst = &mut EPOLL_INSTANCES[inst_idx];
            if !inst.used || inst.watch_count == 0 {
                inst_idx += 1;
                continue;
            }
            let mut watch_idx = 0usize;
            while watch_idx < EPOLL_MAX_WATCHES {
                let watch = inst.watches[watch_idx];
                if watch.used && watch.owner_pid == owner_pid {
                    epoll_watch_remove(inst, watch_idx);
                }
                watch_idx += 1;
            }
            inst_idx += 1;
        }
    }
}

#[inline]
pub(super) fn epoll_watch_add(inst: &mut EpollInstance, watch: EpollWatch) -> Result<(), i64> {
    let mut idx = 0usize;
    while idx < EPOLL_MAX_WATCHES {
        if !inst.watches[idx].used {
            inst.watches[idx] = watch;
            inst.watch_count = inst.watch_count.saturating_add(1);
            return Ok(());
        }
        idx += 1;
    }
    Err(-ENOSPC)
}

#[inline]
pub(super) fn epoll_events_to_poll_mask(events: u32) -> i16 {
    let mut requested: i16 = 0;
    if (events & EPOLLIN) != 0 {
        requested |= POLLIN;
    }
    if (events & EPOLLPRI) != 0 {
        requested |= POLLPRI;
    }
    if (events & EPOLLOUT) != 0 {
        requested |= POLLOUT;
    }
    requested
}

#[inline]
pub(super) fn poll_revents_to_epoll_events(requested: u32, revents: i16) -> u32 {
    let mut out = 0u32;
    if (revents & POLLIN) != 0 && (requested & EPOLLIN) != 0 {
        out |= EPOLLIN;
    }
    if (revents & POLLPRI) != 0 && (requested & EPOLLPRI) != 0 {
        out |= EPOLLPRI;
    }
    if (revents & POLLOUT) != 0 && (requested & EPOLLOUT) != 0 {
        out |= EPOLLOUT;
    }
    if (revents & POLLERR) != 0 || (revents & POLLNVAL) != 0 {
        out |= EPOLLERR;
    }
    if (revents & POLLHUP) != 0 || (revents & POLLNVAL) != 0 {
        out |= EPOLLHUP;
    }
    out
}

#[inline]
pub(super) fn epoll_target_kind_supported(kind: u8) -> bool {
    kind == task::USER_FD_KIND_CONSOLE
        || kind == task::USER_FD_KIND_VFS
        || kind == task::USER_FD_KIND_PIPE
        || kind == task::USER_FD_KIND_SOCKET
        || kind == task::USER_FD_KIND_PTY_MASTER
        || kind == task::USER_FD_KIND_PTY_SLAVE
}

pub(super) fn epoll_read_event_from_user(
    task_ptr: *mut task::Task,
    ptr: u64,
) -> Result<(u32, u64), i64> {
    let mut raw = [0u8; EPOLL_EVENT_SIZE];
    copy_from_user_task(task_ptr, ptr, &mut raw).map_err(|e| -e)?;
    let events = u32::from_ne_bytes([raw[0], raw[1], raw[2], raw[3]]);
    let data = u64::from_ne_bytes([
        raw[4], raw[5], raw[6], raw[7], raw[8], raw[9], raw[10], raw[11],
    ]);
    Ok((events, data))
}

pub(super) fn epoll_write_event_to_user(
    task_ptr: *mut task::Task,
    events_ptr: u64,
    idx: usize,
    events: u32,
    data: u64,
) -> Result<(), i64> {
    let base = events_ptr
        .checked_add((idx * EPOLL_EVENT_SIZE) as u64)
        .ok_or(-EFAULT)?;
    let mut raw = [0u8; EPOLL_EVENT_SIZE];
    raw[0..4].copy_from_slice(&events.to_ne_bytes());
    raw[4..12].copy_from_slice(&data.to_ne_bytes());
    copy_to_user_task(task_ptr, base, &raw).map_err(|e| -e)
}

pub(super) fn epoll_validate_fd_target(
    task_ptr: *mut task::Task,
    fd: i32,
) -> Result<(u8, u16), i64> {
    let idx = fd_index(fd)?;
    if !fd_slot_is_open(task_ptr, idx) {
        return Err(-EBADF);
    }
    let (kind, inode) = unsafe {
        (
            (*task_ptr).user_fd_kind[idx],
            (*task_ptr).user_fd_inode[idx],
        )
    };
    if kind == task::USER_FD_KIND_EPOLL {
        return Err(-EINVAL);
    }
    if !epoll_target_kind_supported(kind) {
        return Err(-EPERM);
    }
    if kind != task::USER_FD_KIND_CONSOLE && inode == task::USER_FD_NONE {
        return Err(-EBADF);
    }
    Ok((kind, inode))
}

pub(super) fn epoll_fd_to_id(task_ptr: *mut task::Task, epfd: i32) -> Result<u16, i64> {
    let idx = fd_index(epfd)?;
    if !fd_slot_is_open(task_ptr, idx) {
        return Err(-EBADF);
    }
    let (kind, inode) = unsafe {
        (
            (*task_ptr).user_fd_kind[idx],
            (*task_ptr).user_fd_inode[idx],
        )
    };
    if kind != task::USER_FD_KIND_EPOLL {
        return Err(-EINVAL);
    }
    if inode == task::USER_FD_NONE {
        return Err(-EBADF);
    }
    Ok(inode)
}

pub(super) fn epoll_ctl_add(
    inst: &mut EpollInstance,
    owner_pid: i32,
    owner_task: *mut task::Task,
    fd: i32,
    kind: u8,
    inode: u16,
    events: u32,
    data: u64,
) -> i64 {
    #[cfg(not(test))]
    let _ = owner_task;
    if owner_pid <= 0 {
        return -EINVAL;
    }
    if epoll_watch_find(inst, owner_pid, fd).is_some() {
        return -EEXIST;
    }
    if epoll_watch_count_for_owner(owner_pid) >= EPOLL_MAX_WATCHES_PER_TASK {
        return -ENOSPC;
    }
    if inst.watch_count as usize >= EPOLL_MAX_WATCHES {
        return -ENOSPC;
    }
    match epoll_watch_add(
        inst,
        EpollWatch {
            used: true,
            owner_pid,
            #[cfg(test)]
            owner_task,
            fd,
            kind,
            inode,
            events,
            data,
        },
    ) {
        Ok(()) => 0,
        Err(err) => err,
    }
}

pub(super) fn epoll_ctl_mod(
    inst: &mut EpollInstance,
    owner_pid: i32,
    fd: i32,
    kind: u8,
    inode: u16,
    events: u32,
    data: u64,
) -> i64 {
    let idx = match epoll_watch_find(inst, owner_pid, fd) {
        Some(v) => v,
        None => return -ENOENT,
    };
    inst.watches[idx].kind = kind;
    inst.watches[idx].inode = inode;
    inst.watches[idx].events = events;
    inst.watches[idx].data = data;
    0
}

pub(super) fn epoll_ctl_del(inst: &mut EpollInstance, owner_pid: i32, fd: i32) -> i64 {
    let idx = match epoll_watch_find(inst, owner_pid, fd) {
        Some(v) => v,
        None => return -ENOENT,
    };
    epoll_watch_remove(inst, idx);
    0
}

#[inline]
pub(super) fn epoll_watch_owner_task(watch: &EpollWatch) -> *mut task::Task {
    let owner = task::get_by_pid(watch.owner_pid);
    if !owner.is_null() {
        return owner;
    }
    #[cfg(test)]
    {
        return watch.owner_task;
    }
    #[cfg(not(test))]
    {
        ptr::null_mut()
    }
}

#[inline]
pub(super) fn epoll_watch_stale(watch: &EpollWatch) -> bool {
    let owner = epoll_watch_owner_task(watch);
    if owner.is_null() {
        return true;
    }
    let fd_idx = match fd_index(watch.fd) {
        Ok(v) => v,
        Err(_) => return true,
    };
    if !fd_slot_is_open(owner, fd_idx) {
        return true;
    }
    let (kind, inode) = unsafe {
        (
            (*owner).user_fd_kind[fd_idx],
            (*owner).user_fd_inode[fd_idx],
        )
    };
    kind != watch.kind || inode != watch.inode
}

pub(super) fn epoll_collect_ready(inst: &mut EpollInstance, out: &mut [(u32, u64)]) -> usize {
    let mut ready = 0usize;
    let maxevents = out.len();
    let mut idx = 0usize;
    while idx < EPOLL_MAX_WATCHES && ready < maxevents {
        let watch = inst.watches[idx];
        if !watch.used {
            idx += 1;
            continue;
        }

        if epoll_watch_stale(&watch) {
            epoll_watch_remove(inst, idx);
            idx += 1;
            continue;
        }

        let owner = epoll_watch_owner_task(&watch);
        if owner.is_null() {
            epoll_watch_remove(inst, idx);
            idx += 1;
            continue;
        }

        let poll_requested = epoll_events_to_poll_mask(watch.events);
        let revents = poll_fd_revents(owner, watch.fd, poll_requested);
        if (revents & POLLNVAL) != 0 {
            epoll_watch_remove(inst, idx);
            idx += 1;
            continue;
        }

        let ep_events = poll_revents_to_epoll_events(watch.events, revents);
        if ep_events != 0 {
            out[ready] = (ep_events, watch.data);
            ready += 1;
        }
        idx += 1;
    }
    ready
}

pub(super) fn do_poll(
    ctx: &SyscallContext,
    fds_ptr: u64,
    nfds: usize,
    deadline: Option<u64>,
) -> i64 {
    if nfds > POLL_MAX_FDS {
        return -EINVAL;
    }
    if fds_ptr == 0 && nfds > 0 {
        return -EFAULT;
    }

    // Read PollFd array from user
    let mut fds = [PollFd {
        fd: -1,
        events: 0,
        revents: 0,
    }; POLL_MAX_FDS];
    if nfds > 0 {
        let byte_len = nfds * size_of::<PollFd>();
        let bytes =
            unsafe { core::slice::from_raw_parts_mut(fds.as_mut_ptr() as *mut u8, byte_len) };
        if let Err(e) = copy_from_user_task(ctx.current, fds_ptr, bytes) {
            return -(e);
        }
    }

    loop {
        let mut ready_count: i64 = 0;
        for i in 0..nfds {
            fds[i].revents = 0;
            if fds[i].fd < 0 {
                continue;
            }
            fds[i].revents = poll_fd_revents(ctx.current, fds[i].fd, fds[i].events);
            if fds[i].revents != 0 {
                ready_count += 1;
            }
        }

        if ready_count > 0 || deadline == Some(0) {
            // Write back
            if nfds > 0 {
                let byte_len = nfds * size_of::<PollFd>();
                let bytes =
                    unsafe { core::slice::from_raw_parts(fds.as_ptr() as *const u8, byte_len) };
                if let Err(e) = copy_to_user_task(ctx.current, fds_ptr, bytes) {
                    return -(e);
                }
            }
            return ready_count;
        }

        // Check deadline
        if let Some(dl) = deadline {
            if pit::uptime_ms() >= dl {
                // Timeout — write back zero revents
                if nfds > 0 {
                    let byte_len = nfds * size_of::<PollFd>();
                    let bytes =
                        unsafe { core::slice::from_raw_parts(fds.as_ptr() as *const u8, byte_len) };
                    let _ = copy_to_user_task(ctx.current, fds_ptr, bytes);
                }
                return 0;
            }
        }

        // Drive NIC + TCP tick, then yield
        #[cfg(not(test))]
        {
            crate::net::poll();
            crate::sched::yield_now();
        }
    }
}

pub(super) fn sys_ppoll(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fds_ptr = call.arg(0);
    let nfds = call.arg(1) as usize;
    let tspec_ptr = call.arg(2);
    // arg3 = sigmask (ignored)

    let deadline = if tspec_ptr == 0 {
        None // block indefinitely
    } else {
        match read_struct_from_user::<LinuxTimeSpec>(ctx.current, tspec_ptr) {
            Ok(ts) => {
                if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
                    return -EINVAL;
                }
                let timeout_ms = (ts.tv_sec as u64)
                    .saturating_mul(1000)
                    .saturating_add((ts.tv_nsec as u64) / 1_000_000);
                Some(pit::uptime_ms().saturating_add(timeout_ms))
            }
            Err(e) => return -(e),
        }
    };

    do_poll(ctx, fds_ptr, nfds, deadline)
}

pub(super) fn sys_select(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let nfds_raw = call.arg(0) as i64;
    if nfds_raw < 0 {
        return -EINVAL;
    }
    let nfds = nfds_raw as usize;
    let read_ptr = call.arg(1);
    let write_ptr = call.arg(2);
    let except_ptr = call.arg(3);
    let timeout_ptr = call.arg(4);

    let deadline = if timeout_ptr == 0 {
        None
    } else {
        match read_struct_from_user::<LinuxTimeVal>(ctx.current, timeout_ptr) {
            Ok(tv) => {
                if tv.tv_sec < 0 || tv.tv_usec < 0 || tv.tv_usec >= 1_000_000 {
                    return -EINVAL;
                }
                let sec_ms = (tv.tv_sec as u64).saturating_mul(1000);
                let usec_ms = (tv.tv_usec as u64).saturating_add(999) / 1000;
                Some(pit::uptime_ms().saturating_add(sec_ms.saturating_add(usec_ms)))
            }
            Err(e) => return -e,
        }
    };

    do_select(ctx, nfds, read_ptr, write_ptr, except_ptr, deadline)
}

pub(super) fn sys_poll(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fds_ptr = call.arg(0);
    let nfds = call.arg(1) as usize;
    let timeout_ms = call.arg(2) as i32;

    let deadline = if timeout_ms < 0 {
        None // block indefinitely
    } else if timeout_ms == 0 {
        Some(0u64) // non-blocking
    } else {
        Some(pit::uptime_ms().saturating_add(timeout_ms as u64))
    };

    do_poll(ctx, fds_ptr, nfds, deadline)
}

pub(super) fn sys_epoll_create1(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let flags = call.arg(0) as u32;
    if (flags & !EPOLL_CLOEXEC) != 0 {
        return -EINVAL;
    }

    let epoll_id = match epoll_alloc_instance() {
        Ok(v) => v,
        Err(err) => return err,
    };

    let mut open_flags = fs::O_RDWR;
    if (flags & EPOLL_CLOEXEC) != 0 {
        open_flags |= O_CLOEXEC;
    }
    let fd_flags = fd_flags_from_open_flags(open_flags);

    match fd_alloc(ctx.current, task::USER_FD_KIND_EPOLL, epoll_id, fd_flags) {
        Ok(fd) => fd,
        Err(err) => {
            let _ = epoll_close(epoll_id);
            err
        }
    }
}

pub(super) fn sys_epoll_ctl(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let epfd = call.arg(0) as i32;
    let op = call.arg(1) as i32;
    let fd = call.arg(2) as i32;
    let event_ptr = call.arg(3);

    if fd == epfd {
        return -EINVAL;
    }

    let epoll_id = match epoll_fd_to_id(ctx.current, epfd) {
        Ok(v) => v,
        Err(err) => return err,
    };

    let owner_pid = unsafe { (*ctx.current).pid };

    match op {
        EPOLL_CTL_ADD | EPOLL_CTL_MOD => {
            if event_ptr == 0 {
                return -EFAULT;
            }
            let (events, data) = match epoll_read_event_from_user(ctx.current, event_ptr) {
                Ok(v) => v,
                Err(err) => return err,
            };
            if (events & !EPOLL_SUPPORTED_EVENTS_MASK) != 0 {
                return -EINVAL;
            }
            let (kind, inode) = match epoll_validate_fd_target(ctx.current, fd) {
                Ok(v) => v,
                Err(err) => return err,
            };
            let inst = match epoll_instance_get_mut(epoll_id) {
                Ok(v) => v,
                Err(err) => return err,
            };
            if op == EPOLL_CTL_ADD {
                epoll_ctl_add(inst, owner_pid, ctx.current, fd, kind, inode, events, data)
            } else {
                epoll_ctl_mod(inst, owner_pid, fd, kind, inode, events, data)
            }
        }
        EPOLL_CTL_DEL => {
            let inst = match epoll_instance_get_mut(epoll_id) {
                Ok(v) => v,
                Err(err) => return err,
            };
            epoll_ctl_del(inst, owner_pid, fd)
        }
        _ => -EINVAL,
    }
}

pub(super) fn sys_epoll_wait(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let epfd = call.arg(0) as i32;
    let events_ptr = call.arg(1);
    let maxevents = call.arg(2) as i32;
    let timeout = call.arg(3) as i32;

    if events_ptr == 0 {
        return -EFAULT;
    }
    if maxevents <= 0 {
        return -EINVAL;
    }
    if timeout < -1 {
        return -EINVAL;
    }

    let epoll_id = match epoll_fd_to_id(ctx.current, epfd) {
        Ok(v) => v,
        Err(err) => return err,
    };

    let maxevents = maxevents as usize;
    let deadline = if timeout < 0 {
        None
    } else if timeout == 0 {
        Some(0u64)
    } else {
        Some(pit::uptime_ms().saturating_add(timeout as u64))
    };
    let mut ready_events = [(0u32, 0u64); EPOLL_MAX_WATCHES];
    let event_cap = cmp::min(maxevents, ready_events.len());

    loop {
        let ready = {
            let inst = match epoll_instance_get_mut(epoll_id) {
                Ok(v) => v,
                Err(err) => return err,
            };
            epoll_collect_ready(inst, &mut ready_events[..event_cap])
        };

        if ready > 0 {
            let mut idx = 0usize;
            while idx < ready {
                let (events, data) = ready_events[idx];
                if let Err(err) =
                    epoll_write_event_to_user(ctx.current, events_ptr, idx, events, data)
                {
                    return err;
                }
                idx += 1;
            }
            return ready as i64;
        }

        if deadline == Some(0) {
            return 0;
        }
        if task_has_unblocked_signal(ctx.current) {
            return -EINTR;
        }
        if let Some(dl) = deadline {
            if pit::uptime_ms() >= dl {
                return 0;
            }
        }

        #[cfg(not(test))]
        {
            crate::net::poll();
            crate::sched::yield_now();
        }
    }
}

pub(super) fn sys_epoll_pwait(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let sigmask = call.arg(4);
    if sigmask != 0 {
        return -ENOSYS;
    }
    sys_epoll_wait(ctx, call)
}
