//! `eventfd`, `timerfd`, and `signalfd` syscalls plus their read/write/poll
//! hooks. Object state lives in `crate::evfd`; this file handles user-memory
//! copies, fd-table plumbing, `O_NONBLOCK`, and blocking.

use super::*;
use crate::evfd;

pub(super) const EFD_SEMAPHORE: u32 = 1;
pub(super) const EVFD_CREATE_FLAGS: u32 = O_CLOEXEC | O_NONBLOCK;
pub(super) const TFD_TIMER_ABSTIME: u32 = 1;
pub(super) const TFD_TIMER_CANCEL_ON_SET: u32 = 2;
pub(super) const CLOCK_BOOTTIME: i32 = 7;
pub(super) const SIGNALFD_SIGINFO_SIZE: usize = 128;
const NSEC_PER_SEC: u64 = 1_000_000_000;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct LinuxItimerSpec {
    pub(super) it_interval_sec: i64,
    pub(super) it_interval_nsec: i64,
    pub(super) it_value_sec: i64,
    pub(super) it_value_nsec: i64,
}

/// Nanoseconds on the boot-relative clock behind `CLOCK_MONOTONIC` and
/// `CLOCK_REALTIME` (see `sys_clock_gettime`).
#[cfg(not(test))]
pub(super) fn evfd_now_ns() -> u64 {
    let (sec, nsec) = ticks_to_seconds_nanos(pit::ticks());
    (sec as u64).saturating_mul(NSEC_PER_SEC).saturating_add(nsec as u64)
}

/// Host tests have no running PIT, so they drive this clock explicitly.
#[cfg(test)]
pub(super) static EVFD_TEST_NOW_NS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
pub(super) fn evfd_now_ns() -> u64 {
    EVFD_TEST_NOW_NS.load(core::sync::atomic::Ordering::Relaxed)
}

pub(super) fn timespec_parts_to_ns(sec: i64, nsec: i64) -> Result<u64, i64> {
    if sec < 0 || !(0..NSEC_PER_SEC as i64).contains(&nsec) {
        return Err(-EINVAL);
    }
    Ok((sec as u64).saturating_mul(NSEC_PER_SEC).saturating_add(nsec as u64))
}

pub(super) fn ns_to_timespec_parts(ns: u64) -> (i64, i64) {
    ((ns / NSEC_PER_SEC) as i64, (ns % NSEC_PER_SEC) as i64)
}

fn itimerspec_from_setting(setting: evfd::TimerSetting) -> LinuxItimerSpec {
    let (it_interval_sec, it_interval_nsec) = ns_to_timespec_parts(setting.interval_ns);
    let (it_value_sec, it_value_nsec) = ns_to_timespec_parts(setting.value_ns);
    LinuxItimerSpec {
        it_interval_sec,
        it_interval_nsec,
        it_value_sec,
        it_value_nsec,
    }
}

fn evfd_fd_flags(flags: u32) -> u32 {
    fd_flags_from_open_flags(fs::O_RDWR | (flags & EVFD_CREATE_FLAGS))
}

fn alloc_evfd_fd(ctx: &SyscallContext, kind: u8, id: u16, flags: u32) -> i64 {
    match fd_alloc(ctx.current, kind, id, evfd_fd_flags(flags)) {
        Ok(fd) => fd,
        Err(err) => {
            evfd::release(evfd_kind(kind), id);
            err
        }
    }
}

/// Look up an open fd of the given kind and return `(object id, fd flags)`.
fn evfd_lookup(task_ptr: *mut task::Task, fd: i32, want_kind: u8) -> Result<(u16, u32), i64> {
    let idx = fd_index(fd)?;
    if !fd_slot_is_open(task_ptr, idx) {
        return Err(-EBADF);
    }
    let (kind, inode, flags) = unsafe {
        (
            (*crate::task::leader_of(task_ptr)).user_fd_kind[idx],
            (*crate::task::leader_of(task_ptr)).user_fd_inode[idx],
            (*crate::task::leader_of(task_ptr)).user_fd_flags[idx],
        )
    };
    if kind != want_kind {
        return Err(-EINVAL);
    }
    Ok((inode, flags))
}

#[inline]
pub(super) fn is_evfd_kind(kind: u8) -> bool {
    kind == task::USER_FD_KIND_EVENTFD
        || kind == task::USER_FD_KIND_TIMERFD
        || kind == task::USER_FD_KIND_SIGNALFD
}

/// Shared blocking policy: retry `attempt` until it stops returning
/// `-EAGAIN`, unless the fd is nonblocking or the wait is interrupted. An
/// interrupted wait returns `-ERESTARTSYS`, which the syscall exit path turns
/// into a restart or `EINTR` according to `SA_RESTART`.
fn evfd_block_on<T>(
    task_ptr: *mut task::Task,
    nonblocking: bool,
    mut attempt: impl FnMut() -> Result<T, i64>,
) -> Result<T, i64> {
    loop {
        match attempt() {
            Err(err) if err == -EAGAIN && !nonblocking => {
                if signal::wait_interrupted(task_ptr) {
                    return Err(-ERESTARTSYS);
                }
                #[cfg(not(test))]
                crate::sched::yield_now();
                #[cfg(test)]
                return Err(-EAGAIN);
            }
            other => return other,
        }
    }
}

// ---------------------------------------------------------------------------
// eventfd (284) / eventfd2 (290)
// ---------------------------------------------------------------------------

pub(super) fn sys_eventfd2(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let initval = call.arg(0) as u32;
    let flags = call.arg(1) as u32;
    if (flags & !(EVFD_CREATE_FLAGS | EFD_SEMAPHORE)) != 0 {
        return -EINVAL;
    }
    let id = match evfd::eventfd_create(initval, (flags & EFD_SEMAPHORE) != 0) {
        Ok(id) => id,
        Err(err) => return err,
    };
    alloc_evfd_fd(ctx, task::USER_FD_KIND_EVENTFD, id, flags)
}

pub(super) fn sys_eventfd(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let id = match evfd::eventfd_create(call.arg(0) as u32, false) {
        Ok(id) => id,
        Err(err) => return err,
    };
    alloc_evfd_fd(ctx, task::USER_FD_KIND_EVENTFD, id, 0)
}

// ---------------------------------------------------------------------------
// timerfd_create (283) / timerfd_settime (286) / timerfd_gettime (287)
// ---------------------------------------------------------------------------

pub(super) fn sys_timerfd_create(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let clock_id = call.arg(0) as i32;
    let flags = call.arg(1) as u32;
    if clock_id != CLOCK_REALTIME && clock_id != CLOCK_MONOTONIC && clock_id != CLOCK_BOOTTIME {
        return -EINVAL;
    }
    if (flags & !EVFD_CREATE_FLAGS) != 0 {
        return -EINVAL;
    }
    let id = match evfd::timerfd_create(clock_id) {
        Ok(id) => id,
        Err(err) => return err,
    };
    alloc_evfd_fd(ctx, task::USER_FD_KIND_TIMERFD, id, flags)
}

pub(super) fn sys_timerfd_settime(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let flags = call.arg(1) as u32;
    let new_ptr = call.arg(2);
    let old_ptr = call.arg(3);
    if (flags & !(TFD_TIMER_ABSTIME | TFD_TIMER_CANCEL_ON_SET)) != 0 {
        return -EINVAL;
    }
    let (id, _) = match evfd_lookup(ctx.current, fd, task::USER_FD_KIND_TIMERFD) {
        Ok(v) => v,
        Err(err) => return err,
    };
    let spec = match read_struct_from_user::<LinuxItimerSpec>(ctx.current, new_ptr) {
        Ok(spec) => spec,
        Err(err) => return -err,
    };
    let value_ns = match timespec_parts_to_ns(spec.it_value_sec, spec.it_value_nsec) {
        Ok(v) => v,
        Err(err) => return err,
    };
    let interval_ns = match timespec_parts_to_ns(spec.it_interval_sec, spec.it_interval_nsec) {
        Ok(v) => v,
        Err(err) => return err,
    };
    let old = match evfd::timerfd_settime(
        id,
        evfd_now_ns(),
        (flags & TFD_TIMER_ABSTIME) != 0,
        value_ns,
        interval_ns,
    ) {
        Ok(old) => old,
        Err(err) => return err,
    };
    if old_ptr != 0 {
        if let Err(err) = write_struct_to_user(ctx.current, old_ptr, &itimerspec_from_setting(old)) {
            return -err;
        }
    }
    0
}

pub(super) fn sys_timerfd_gettime(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let (id, _) = match evfd_lookup(ctx.current, call.arg(0) as i32, task::USER_FD_KIND_TIMERFD) {
        Ok(v) => v,
        Err(err) => return err,
    };
    let setting = match evfd::timerfd_gettime(id, evfd_now_ns()) {
        Ok(s) => s,
        Err(err) => return err,
    };
    match write_struct_to_user(ctx.current, call.arg(1), &itimerspec_from_setting(setting)) {
        Ok(()) => 0,
        Err(err) => -err,
    }
}

// ---------------------------------------------------------------------------
// signalfd (282) / signalfd4 (289)
// ---------------------------------------------------------------------------

/// Signal masks use the Linux `sigset_t` layout throughout the kernel, so a
/// user mask is used as-is except that SIGKILL and SIGSTOP can never be read
/// through a signalfd.
pub(super) fn signalfd_mask_from_user(user_mask: u64) -> u64 {
    user_mask & !signal::signal_bit(signal::SIGKILL) & !signal::signal_bit(signal::SIGSTOP)
}

fn signalfd_common(ctx: &SyscallContext, fd: i32, mask_ptr: u64, sizemask: u64, flags: u32) -> i64 {
    if sizemask != 8 {
        return -EINVAL;
    }
    if (flags & !EVFD_CREATE_FLAGS) != 0 {
        return -EINVAL;
    }
    let user_mask = match read_u64_from_user(ctx.current, mask_ptr) {
        Ok(v) => v,
        Err(err) => return -err,
    };
    let mask = signalfd_mask_from_user(user_mask);
    if fd == -1 {
        let id = match evfd::signalfd_create(mask) {
            Ok(id) => id,
            Err(err) => return err,
        };
        return alloc_evfd_fd(ctx, task::USER_FD_KIND_SIGNALFD, id, flags);
    }
    let (id, _) = match evfd_lookup(ctx.current, fd, task::USER_FD_KIND_SIGNALFD) {
        Ok(v) => v,
        Err(err) => return err,
    };
    match evfd::signalfd_set_mask(id, mask) {
        Ok(()) => fd as i64,
        Err(err) => err,
    }
}

pub(super) fn sys_signalfd4(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    signalfd_common(
        ctx,
        call.arg(0) as i32,
        call.arg(1),
        call.arg(2),
        call.arg(3) as u32,
    )
}

pub(super) fn sys_signalfd(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    signalfd_common(ctx, call.arg(0) as i32, call.arg(1), call.arg(2), 0)
}

/// Build the 128-byte `struct signalfd_siginfo` for one dequeued signal.
pub(super) fn signalfd_siginfo_bytes(signum: u32) -> [u8; SIGNALFD_SIGINFO_SIZE] {
    let mut info = [0u8; SIGNALFD_SIGINFO_SIZE];
    info[0..4].copy_from_slice(&signum.to_ne_bytes()); // ssi_signo
    // ssi_errno, ssi_code (SI_USER = 0), ssi_pid, ssi_uid, ... stay zero:
    // the kernel does not track sender identity for pending signals.
    info
}

/// Visit `task_ptr` first, then every other live thread in its thread group.
/// A signalfd reports both signals sent to the reading thread and
/// process-directed signals, which `signal::send_to_process` may have queued
/// on any thread of the group.
fn for_each_group_thread(task_ptr: *mut task::Task, mut visit: impl FnMut(*mut task::Task) -> bool) {
    if task_ptr.is_null() {
        return;
    }
    if !visit(task_ptr) {
        return;
    }
    let tgid = unsafe { (*task_ptr).tgid };
    for idx in 0..task::MAX_TASKS {
        let t = task::get_by_index(idx);
        if t.is_null() || t == task_ptr {
            continue;
        }
        let same_group = unsafe { (*t).tgid == tgid } && task::is_live(t);
        if same_group && !visit(t) {
            return;
        }
    }
}

/// Pending signals visible to a signalfd read by `task_ptr`.
pub(super) fn signalfd_pending(task_ptr: *mut task::Task) -> u64 {
    let mut pending = 0u64;
    for_each_group_thread(task_ptr, |t| {
        pending |= unsafe { (*t).sig_pending };
        true
    });
    pending
}

/// Dequeue one pending signal matching `mask`, preferring signals queued on
/// the reading thread itself.
pub(super) fn signalfd_dequeue(task_ptr: *mut task::Task, mask: u64) -> Option<u32> {
    let mut found = None;
    for_each_group_thread(task_ptr, |t| unsafe {
        match evfd::signalfd_pick((*t).sig_pending, mask) {
            Some(signum) => {
                (*t).sig_pending &= !signal::signal_bit(signum);
                found = Some(signum);
                false
            }
            None => true,
        }
    });
    found
}

// ---------------------------------------------------------------------------
// read / write / poll hooks
// ---------------------------------------------------------------------------

/// `read(2)` on an eventfd/timerfd/signalfd slot.
pub(super) fn evfd_read(
    ctx: &SyscallContext,
    kind: u8,
    id: u16,
    flags: u32,
    buf: u64,
    len: usize,
) -> i64 {
    let nonblocking = (fd_open_flags(flags) & O_NONBLOCK) != 0;
    match kind {
        task::USER_FD_KIND_EVENTFD | task::USER_FD_KIND_TIMERFD => {
            if len < 8 {
                return -EINVAL;
            }
            let result = evfd_block_on(ctx.current, nonblocking, || {
                if kind == task::USER_FD_KIND_EVENTFD {
                    evfd::eventfd_read(id)
                } else {
                    evfd::timerfd_read(id, evfd_now_ns())
                }
            });
            match result {
                Ok(value) => match copy_to_user_task(ctx.current, buf, &value.to_ne_bytes()) {
                    Ok(()) => 8,
                    Err(err) => -err,
                },
                Err(err) => err,
            }
        }
        task::USER_FD_KIND_SIGNALFD => {
            let max = len / SIGNALFD_SIGINFO_SIZE;
            if max == 0 {
                return -EINVAL;
            }
            let mask = match evfd::signalfd_mask(id) {
                Ok(m) => m,
                Err(err) => return err,
            };
            let first = evfd_block_on(ctx.current, nonblocking, || {
                signalfd_dequeue(ctx.current, mask).ok_or(-EAGAIN)
            });
            let mut signum = match first {
                Ok(s) => s,
                Err(err) => return err,
            };
            let mut copied = 0usize;
            loop {
                let info = signalfd_siginfo_bytes(signum);
                let dst = buf + (copied * SIGNALFD_SIGINFO_SIZE) as u64;
                if let Err(err) = copy_to_user_task(ctx.current, dst, &info) {
                    return if copied > 0 {
                        (copied * SIGNALFD_SIGINFO_SIZE) as i64
                    } else {
                        -err
                    };
                }
                copied += 1;
                if copied >= max {
                    break;
                }
                match signalfd_dequeue(ctx.current, mask) {
                    Some(next) => signum = next,
                    None => break,
                }
            }
            (copied * SIGNALFD_SIGINFO_SIZE) as i64
        }
        _ => -EBADF,
    }
}

/// `write(2)` on an eventfd/timerfd/signalfd slot.
pub(super) fn evfd_write(
    ctx: &SyscallContext,
    kind: u8,
    id: u16,
    flags: u32,
    buf: u64,
    len: usize,
) -> i64 {
    if kind != task::USER_FD_KIND_EVENTFD {
        return -EINVAL;
    }
    if len < 8 {
        return -EINVAL;
    }
    let value = match read_u64_from_user(ctx.current, buf) {
        Ok(v) => v,
        Err(err) => return -err,
    };
    let nonblocking = (fd_open_flags(flags) & O_NONBLOCK) != 0;
    match evfd_block_on(ctx.current, nonblocking, || evfd::eventfd_write(id, value)) {
        Ok(()) => 8,
        Err(err) => err,
    }
}

/// poll/select/epoll readiness for an eventfd/timerfd/signalfd slot.
pub(super) fn evfd_poll_revents(
    task_ptr: *mut task::Task,
    kind: u8,
    id: u16,
    requested: i16,
) -> i16 {
    let (readable, writable) = match kind {
        task::USER_FD_KIND_EVENTFD => match evfd::eventfd_poll(id) {
            Ok(state) => state,
            Err(_) => return POLLNVAL,
        },
        task::USER_FD_KIND_TIMERFD => match evfd::timerfd_poll(id, evfd_now_ns()) {
            Ok(ready) => (ready, false),
            Err(_) => return POLLNVAL,
        },
        task::USER_FD_KIND_SIGNALFD => match evfd::signalfd_mask(id) {
            Ok(mask) => (evfd::signalfd_pick(signalfd_pending(task_ptr), mask).is_some(), false),
            Err(_) => return POLLNVAL,
        },
        _ => return POLLNVAL,
    };
    let mut revents = 0i16;
    if readable && (requested & POLLIN) != 0 {
        revents |= POLLIN;
    }
    if writable && (requested & POLLOUT) != 0 {
        revents |= POLLOUT;
    }
    revents
}
