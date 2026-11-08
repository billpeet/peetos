//! File extensions used by event-loop/IPC stacks such as libwayland:
//! `flock` advisory locks, `fallocate` preallocation, and memfd sealing
//! (`MFD_ALLOW_SEALING`, `F_ADD_SEALS`, `F_GET_SEALS`).

use super::*;
use crate::sync::SpinLock;

// ---------------------------------------------------------------------------
// flock (73)
// ---------------------------------------------------------------------------

pub(super) const LOCK_SH: u32 = 1;
pub(super) const LOCK_EX: u32 = 2;
pub(super) const LOCK_NB: u32 = 4;
pub(super) const LOCK_UN: u32 = 8;
/// System-wide held `flock` locks. A libwayland compositor holds one lock
/// per display socket (`wl_display_add_socket_auto` can hold 33).
pub(super) const FLOCK_MAX_ENTRIES: usize = 128;

/// One held lock. The lock owner is the `(tgid, fd)` pair that acquired it
/// (Linux ties flock owners to the open file description instead; see the
/// limitations in `docs/vfs.md`).
#[derive(Clone, Copy)]
struct FlockEntry {
    used: bool,
    inode: u16,
    pid: i32,
    fd: i32,
    exclusive: bool,
}

impl FlockEntry {
    const EMPTY: Self = Self {
        used: false,
        inode: 0,
        pid: 0,
        fd: 0,
        exclusive: false,
    };
}

static FLOCKS: SpinLock<[FlockEntry; FLOCK_MAX_ENTRIES]> =
    SpinLock::new([FlockEntry::EMPTY; FLOCK_MAX_ENTRIES]);

/// Try to take (or convert) a lock without blocking. `-EAGAIN` on conflict.
pub(super) fn flock_try_acquire(inode: u16, pid: i32, fd: i32, exclusive: bool) -> Result<(), i64> {
    let mut table = FLOCKS.lock();
    let mut mine = None;
    for (idx, entry) in table.iter().enumerate() {
        if !entry.used || entry.inode != inode {
            continue;
        }
        if entry.pid == pid && entry.fd == fd {
            mine = Some(idx);
        } else if exclusive || entry.exclusive {
            return Err(-EAGAIN);
        }
    }
    let idx = match mine {
        Some(idx) => idx,
        None => table.iter().position(|e| !e.used).ok_or(-ENOLCK)?,
    };
    table[idx] = FlockEntry {
        used: true,
        inode,
        pid,
        fd,
        exclusive,
    };
    Ok(())
}

/// Drop the lock held through `(pid, fd)`, if any. Called on `LOCK_UN` and
/// whenever the fd is closed (including exec close-on-exec and task exit).
pub(super) fn flock_release_fd(pid: i32, fd: i32) {
    let mut table = FLOCKS.lock();
    for entry in table.iter_mut() {
        if entry.used && entry.pid == pid && entry.fd == fd {
            *entry = FlockEntry::EMPTY;
        }
    }
}

#[cfg(test)]
pub(super) fn flock_reset_for_test() {
    *FLOCKS.lock() = [FlockEntry::EMPTY; FLOCK_MAX_ENTRIES];
}

pub(super) fn sys_flock(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let operation = call.arg(1) as u32;
    let idx = match fd_index(fd) {
        Ok(idx) => idx,
        Err(err) => return err,
    };
    if !fd_slot_is_open(ctx.current, idx) {
        return -EBADF;
    }
    let (kind, inode, pid) = unsafe {
        (
            (*crate::task::leader_of(ctx.current)).user_fd_kind[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_inode[idx],
            (*ctx.current).tgid,
        )
    };
    if kind != task::USER_FD_KIND_VFS {
        return -EINVAL;
    }

    let nonblocking = (operation & LOCK_NB) != 0;
    let exclusive = match operation & !LOCK_NB {
        LOCK_SH => false,
        LOCK_EX => true,
        LOCK_UN => {
            flock_release_fd(pid, fd);
            return 0;
        }
        _ => return -EINVAL,
    };

    loop {
        match flock_try_acquire(inode, pid, fd, exclusive) {
            Err(err) if err == -EAGAIN && !nonblocking => {
                if signal::wait_interrupted(ctx.current) {
                    return -ERESTARTSYS;
                }
                #[cfg(not(test))]
                crate::sched::yield_now();
                #[cfg(test)]
                return -EAGAIN;
            }
            Err(err) => return err,
            Ok(()) => return 0,
        }
    }
}

// ---------------------------------------------------------------------------
// memfd seals
// ---------------------------------------------------------------------------

pub(super) const F_ADD_SEALS: u32 = 1033;
pub(super) const F_GET_SEALS: u32 = 1034;
pub(super) const F_SEAL_SEAL: u32 = 0x0001;
pub(super) const F_SEAL_SHRINK: u32 = 0x0002;
pub(super) const F_SEAL_GROW: u32 = 0x0004;
pub(super) const F_SEAL_WRITE: u32 = 0x0008;
pub(super) const F_SEAL_FUTURE_WRITE: u32 = 0x0010;
pub(super) const F_SEAL_ALL: u32 =
    F_SEAL_SEAL | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_WRITE | F_SEAL_FUTURE_WRITE;
pub(super) const MEMFD_SEAL_MAX_ENTRIES: usize = 64;

#[derive(Clone, Copy)]
struct MemfdSeal {
    used: bool,
    inode: u16,
    seals: u32,
}

impl MemfdSeal {
    const EMPTY: Self = Self {
        used: false,
        inode: 0,
        seals: 0,
    };
}

static MEMFD_SEALS: SpinLock<[MemfdSeal; MEMFD_SEAL_MAX_ENTRIES]> =
    SpinLock::new([MemfdSeal::EMPTY; MEMFD_SEAL_MAX_ENTRIES]);

/// Record the initial seal state of a new memfd. Without `MFD_ALLOW_SEALING`
/// the object starts with `F_SEAL_SEAL`, so no seals can ever be added.
pub(super) fn memfd_seals_register(inode: u16, allow_sealing: bool) {
    let initial = if allow_sealing { 0 } else { F_SEAL_SEAL };
    let mut table = MEMFD_SEALS.lock();
    let slot = table
        .iter()
        .position(|e| e.used && e.inode == inode)
        .or_else(|| table.iter().position(|e| !e.used));
    match slot {
        Some(idx) => {
            table[idx] = MemfdSeal {
                used: true,
                inode,
                seals: initial,
            }
        }
        // Table full: fall back to "sealing not allowed" semantics, which is
        // what an unregistered inode reports anyway.
        None => {}
    }
}

/// Current seals of a memfd inode (`F_SEAL_SEAL` if it was never registered).
pub(super) fn memfd_seals_get(inode: u16) -> u32 {
    let table = MEMFD_SEALS.lock();
    table
        .iter()
        .find(|e| e.used && e.inode == inode)
        .map(|e| e.seals)
        .unwrap_or(F_SEAL_SEAL)
}

pub(super) fn memfd_seals_add(inode: u16, seals: u32) -> Result<(), i64> {
    if (seals & !F_SEAL_ALL) != 0 {
        return Err(-EINVAL);
    }
    let mut table = MEMFD_SEALS.lock();
    let entry = match table.iter_mut().find(|e| e.used && e.inode == inode) {
        Some(entry) => entry,
        None => return Err(-EPERM),
    };
    if (entry.seals & F_SEAL_SEAL) != 0 {
        return Err(-EPERM);
    }
    entry.seals |= seals;
    Ok(())
}

#[cfg(test)]
pub(super) fn memfd_seals_reset_for_test() {
    *MEMFD_SEALS.lock() = [MemfdSeal::EMPTY; MEMFD_SEAL_MAX_ENTRIES];
}

/// Seals that apply to an fd: memfd fds report their inode's seals, other
/// files are never sealed.
pub(super) fn fd_seals(fd_flags: u32, inode: u16) -> u32 {
    if fd_is_memfd(fd_flags) {
        memfd_seals_get(inode)
    } else {
        0
    }
}

/// Check a size change against `F_SEAL_SHRINK` / `F_SEAL_GROW`.
pub(super) fn seals_allow_resize(seals: u32, current_size: u64, new_size: u64) -> Result<(), i64> {
    if new_size < current_size && (seals & F_SEAL_SHRINK) != 0 {
        return Err(-EPERM);
    }
    if new_size > current_size && (seals & F_SEAL_GROW) != 0 {
        return Err(-EPERM);
    }
    Ok(())
}

/// Check a data write against `F_SEAL_WRITE` / `F_SEAL_FUTURE_WRITE`.
pub(super) fn seals_allow_write(seals: u32) -> Result<(), i64> {
    if (seals & (F_SEAL_WRITE | F_SEAL_FUTURE_WRITE)) != 0 {
        return Err(-EPERM);
    }
    Ok(())
}

/// `fcntl(F_ADD_SEALS / F_GET_SEALS)` on an open fd.
pub(super) fn fcntl_seals(task_ptr: *mut task::Task, idx: usize, cmd: u32, arg: u64) -> i64 {
    let (kind, inode, flags) = unsafe {
        (
            (*crate::task::leader_of(task_ptr)).user_fd_kind[idx],
            (*crate::task::leader_of(task_ptr)).user_fd_inode[idx],
            (*crate::task::leader_of(task_ptr)).user_fd_flags[idx],
        )
    };
    if kind != task::USER_FD_KIND_VFS || !fd_is_memfd(flags) {
        return -EINVAL;
    }
    match cmd {
        F_GET_SEALS => memfd_seals_get(inode) as i64,
        F_ADD_SEALS => {
            if !fd_can_write(flags) {
                return -EPERM;
            }
            match memfd_seals_add(inode, arg as u32) {
                Ok(()) => 0,
                Err(err) => err,
            }
        }
        _ => -EINVAL,
    }
}

// ---------------------------------------------------------------------------
// fallocate (285)
// ---------------------------------------------------------------------------

pub(super) fn sys_fallocate(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let mode = call.arg(1) as u32;
    let offset = call.arg(2) as i64;
    let len = call.arg(3) as i64;

    let idx = match fd_index(fd) {
        Ok(idx) => idx,
        Err(err) => return err,
    };
    if !fd_slot_is_open(ctx.current, idx) {
        return -EBADF;
    }
    if offset < 0 || len <= 0 {
        return -EINVAL;
    }
    let end = match (offset as u64).checked_add(len as u64) {
        Some(end) => end,
        None => return -EFBIG,
    };
    let (kind, inode, flags) = unsafe {
        (
            (*crate::task::leader_of(ctx.current)).user_fd_kind[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_inode[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_flags[idx],
        )
    };
    match kind {
        task::USER_FD_KIND_VFS => {}
        task::USER_FD_KIND_PIPE => return -ESPIPE,
        _ => return -ENODEV,
    }
    if !fd_can_write(flags) {
        return -EBADF;
    }
    // Only plain allocation is supported. Callers such as libwayland's
    // os_resize_anonymous_file() fall back to ftruncate() on EOPNOTSUPP.
    if mode != 0 {
        return -EOPNOTSUPP;
    }
    if fs::inode_is_framebuffer(inode) || fs::inode_input_device_id(inode).is_some() {
        return -ENODEV;
    }

    let size = match vfs_file_size(inode) {
        Ok(size) => size,
        Err(err) => return -fs_to_errno(err),
    };
    if end <= size {
        // Blocks are allocated lazily on write; nothing to reserve.
        return 0;
    }
    if let Err(err) = seals_allow_resize(fd_seals(flags, inode), size, end) {
        return err;
    }
    match vfs_truncate_to(inode, end) {
        Ok(()) => {
            vm_page_cache_invalidate_inode(inode);
            sync_ext2_backing();
            0
        }
        Err(err) => -fs_to_errno(err),
    }
}
