//! Time, uname, getrandom, sysinfo and SysV shm syscalls.

use super::*;

pub(super) fn write_uts_field(dst: &mut [u8; UTS_FIELD_LEN], value: &[u8]) {
    let len = cmp::min(value.len(), UTS_FIELD_LEN - 1);
    dst[..len].copy_from_slice(&value[..len]);
    dst[len] = 0;
}

pub(super) fn timespec_to_unix_seconds(
    spec: LinuxTimeSpec,
    now: u64,
    current: u64,
) -> Result<u64, i64> {
    match spec.tv_nsec {
        UTIME_NOW => Ok(now),
        UTIME_OMIT => Ok(current),
        nsec => {
            if spec.tv_sec < 0 || nsec < 0 || nsec >= 1_000_000_000 {
                return Err(-EINVAL);
            }
            Ok(spec.tv_sec as u64)
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(super) struct LinuxITimerVal {
    pub(super) it_interval: LinuxTimeVal,
    pub(super) it_value: LinuxTimeVal,
}

pub(super) const ITIMER_REAL: i32 = 0;

/// Interval-timer `timeval` to timer ticks, rounding up so a timer never
/// fires early (and a non-zero value never becomes "disarmed").
pub(super) fn itimer_timeval_to_ticks(tv: LinuxTimeVal) -> Result<u64, i64> {
    if tv.tv_sec < 0 || tv.tv_usec < 0 || tv.tv_usec >= 1_000_000 {
        return Err(-EINVAL);
    }
    let hz = pit::TIMER_FREQUENCY as u128;
    let total_us = (tv.tv_sec as u128) * 1_000_000 + tv.tv_usec as u128;
    let ticks = (total_us * hz + 999_999) / 1_000_000;
    Ok(cmp::min(ticks, u64::MAX as u128 / 2) as u64)
}

pub(super) fn itimer_ticks_to_timeval(ticks: u64) -> LinuxTimeVal {
    let (sec, nsec) = ticks_to_seconds_nanos(ticks);
    LinuxTimeVal {
        tv_sec: sec,
        tv_usec: nsec / 1000,
    }
}

/// Current `ITIMER_REAL` setting of a process leader.
pub(super) fn itimer_real_read(deadline: u64, interval: u64, now: u64) -> LinuxITimerVal {
    let remaining = if deadline == 0 {
        0
    } else {
        // Armed but not yet delivered by the tick handler: report one tick.
        cmp::max(deadline.saturating_sub(now), 1)
    };
    LinuxITimerVal {
        it_interval: itimer_ticks_to_timeval(interval),
        it_value: itimer_ticks_to_timeval(remaining),
    }
}

/// Replace the calling process's `ITIMER_REAL`; returns the old setting.
fn itimer_real_swap(ctx: &SyscallContext, new: LinuxITimerVal) -> Result<LinuxITimerVal, i64> {
    let value = itimer_timeval_to_ticks(new.it_value)?;
    let interval = itimer_timeval_to_ticks(new.it_interval)?;
    let leader = crate::task::leader_of(ctx.current);
    Ok(arch_x86_64::interrupts::without_interrupts(|| unsafe {
        let now = pit::ticks();
        let old = itimer_real_read((*leader).itimer_real_deadline, (*leader).itimer_real_interval, now);
        if value == 0 {
            (*leader).itimer_real_deadline = 0;
            (*leader).itimer_real_interval = 0;
        } else {
            (*leader).itimer_real_deadline = now.saturating_add(value);
            (*leader).itimer_real_interval = interval;
        }
        old
    }))
}

/// `getitimer(which, curr)`. Only `ITIMER_REAL` is implemented.
pub(super) fn sys_getitimer(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    if call.arg(0) as i32 != ITIMER_REAL {
        return -EINVAL;
    }
    let leader = crate::task::leader_of(ctx.current);
    let cur = arch_x86_64::interrupts::without_interrupts(|| unsafe {
        itimer_real_read((*leader).itimer_real_deadline, (*leader).itimer_real_interval, pit::ticks())
    });
    match write_struct_to_user(ctx.current, call.arg(1), &cur) {
        Ok(()) => 0,
        Err(e) => -e,
    }
}

/// `setitimer(which, new, old)`: `ITIMER_REAL` delivers `SIGALRM` to the
/// process from the timer tick (`signal::itimer_real_take_expiry`, then `itimer_real_fire` once `SCHED_LOCK` is released).
pub(super) fn sys_setitimer(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    if call.arg(0) as i32 != ITIMER_REAL {
        return -EINVAL;
    }
    let new = if call.arg(1) == 0 {
        LinuxITimerVal::default()
    } else {
        match read_struct_from_user::<LinuxITimerVal>(ctx.current, call.arg(1)) {
            Ok(v) => v,
            Err(e) => return -e,
        }
    };
    let old = match itimer_real_swap(ctx, new) {
        Ok(old) => old,
        Err(e) => return e,
    };
    if call.arg(2) != 0 {
        if let Err(e) = write_struct_to_user(ctx.current, call.arg(2), &old) {
            return -e;
        }
    }
    0
}

/// Seconds `alarm()` reports for a remaining timer: rounded to nearest,
/// but never 0 while the timer is still armed (Linux semantics).
pub(super) fn alarm_seconds_left(old: LinuxTimeVal) -> u64 {
    if old.tv_sec == 0 && old.tv_usec == 0 {
        return 0;
    }
    let mut secs = old.tv_sec as u64;
    if old.tv_usec >= 500_000 || secs == 0 {
        secs += 1;
    }
    secs
}

/// `alarm(seconds)`: one-shot `ITIMER_REAL`.
pub(super) fn sys_alarm(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let new = LinuxITimerVal {
        it_interval: LinuxTimeVal::default(),
        it_value: LinuxTimeVal {
            tv_sec: call.arg(0) as u32 as i64,
            tv_usec: 0,
        },
    };
    match itimer_real_swap(ctx, new) {
        Ok(old) => alarm_seconds_left(old.it_value) as i64,
        Err(e) => e,
    }
}

#[inline]
pub(super) fn ticks_to_seconds_nanos(ticks: u64) -> (i64, i64) {
    let hz = pit::TIMER_FREQUENCY as u64;
    let sec = ticks / hz;
    let rem_ticks = ticks % hz;
    let nsec = (rem_ticks as u128 * 1_000_000_000u128 / hz as u128) as u64;
    (sec as i64, nsec as i64)
}

pub(super) fn timespec_to_sleep_ticks(spec: &LinuxTimeSpec) -> Result<u64, i64> {
    if spec.tv_sec < 0 || spec.tv_nsec < 0 || spec.tv_nsec >= 1_000_000_000 {
        return Err(-EINVAL);
    }

    let secs = spec.tv_sec as u128;
    let nanos = spec.tv_nsec as u128;
    let total_nanos = secs
        .checked_mul(1_000_000_000u128)
        .and_then(|v| v.checked_add(nanos))
        .ok_or(-EINVAL)?;
    if total_nanos == 0 {
        return Ok(0);
    }

    let hz = pit::TIMER_FREQUENCY as u128;
    let ticks = total_nanos
        .checked_mul(hz)
        .and_then(|v| v.checked_add(1_000_000_000u128 - 1))
        .map(|v| v / 1_000_000_000u128)
        .ok_or(-EINVAL)?;
    if ticks > u64::MAX as u128 {
        return Err(-EINVAL);
    }
    Ok(ticks as u64)
}

pub(super) fn sys_gettimeofday(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let tv_ptr = call.arg(0);
    let _tz_ptr = call.arg(1);

    if tv_ptr != 0 {
        let ticks = pit::ticks();
        let (sec, nsec) = ticks_to_seconds_nanos(ticks);
        let tv = LinuxTimeVal {
            tv_sec: sec,
            tv_usec: nsec / 1_000,
        };
        if let Err(err) = write_struct_to_user(ctx.current, tv_ptr, &tv) {
            return -err;
        }
    }
    0
}

pub(super) fn sys_clock_gettime(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let clock_id = call.arg(0) as i32;
    if clock_id != CLOCK_REALTIME && clock_id != CLOCK_MONOTONIC {
        return -EINVAL;
    }

    let ticks = pit::ticks();
    let (sec, nsec) = ticks_to_seconds_nanos(ticks);
    let ts = LinuxTimeSpec {
        tv_sec: sec,
        tv_nsec: nsec,
    };
    match write_struct_to_user(ctx.current, call.arg(1), &ts) {
        Ok(()) => 0,
        Err(err) => -err,
    }
}

pub(super) fn sys_nanosleep(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let req = match read_struct_from_user::<LinuxTimeSpec>(ctx.current, call.arg(0)) {
        Ok(req) => req,
        Err(err) => return -err,
    };
    let ticks = match timespec_to_sleep_ticks(&req) {
        Ok(ticks) => ticks,
        Err(err) => return err,
    };
    let deadline = pit::ticks().saturating_add(ticks);
    if ticks > 0 {
        crate::sched::sleep(ticks);
    }

    // Woken early by a signal: report the time left, like Linux.
    let now = pit::ticks();
    let interrupted = now < deadline && signal::wait_interrupted(ctx.current);
    if call.arg(1) != 0 {
        let left = if interrupted { deadline - now } else { 0 };
        let (sec, nsec) = ticks_to_seconds_nanos(left);
        let rem = LinuxTimeSpec {
            tv_sec: sec,
            tv_nsec: nsec,
        };
        if let Err(err) = write_struct_to_user(ctx.current, call.arg(1), &rem) {
            return -err;
        }
    }
    if interrupted {
        return -EINTR;
    }
    0
}

pub(super) fn sys_shmget(_ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let key = call.arg(0) as i32;
    let size = call.arg(1);
    let flags = call.arg(2) as i32;
    match shm::shm_get(key, size, flags) {
        Ok(shmid) => shmid as i64,
        Err(err) => -shm_to_errno(err),
    }
}

pub(super) fn sys_shmat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let shmid = call.arg(0) as i32;
    let addr = call.arg(1);
    let flags = call.arg(2) as i32;
    match shm::shm_attach(ctx.current, shmid, addr, flags) {
        Ok(vaddr) => vaddr as i64,
        Err(err) => -shm_to_errno(err),
    }
}

pub(super) fn sys_shmdt(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let addr = call.arg(0);
    match shm::shm_detach(ctx.current, addr) {
        Ok(()) => 0,
        Err(err) => -shm_to_errno(err),
    }
}

pub(super) fn sys_shmctl(_ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let shmid = call.arg(0) as i32;
    let cmd = call.arg(1) as i32;
    match shm::shm_ctl(shmid, cmd) {
        Ok(()) => 0,
        Err(err) => -shm_to_errno(err),
    }
}

pub(super) fn sys_uname(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let mut uts = LinuxUtsName {
        sysname: [0; UTS_FIELD_LEN],
        nodename: [0; UTS_FIELD_LEN],
        release: [0; UTS_FIELD_LEN],
        version: [0; UTS_FIELD_LEN],
        machine: [0; UTS_FIELD_LEN],
        domainname: [0; UTS_FIELD_LEN],
    };
    write_uts_field(&mut uts.sysname, b"PeetOS");
    write_uts_field(&mut uts.nodename, b"peetos");
    write_uts_field(&mut uts.release, b"0.1.0");
    write_uts_field(&mut uts.version, b"#1");
    write_uts_field(&mut uts.machine, b"x86_64");
    write_uts_field(&mut uts.domainname, b"localdomain");

    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&uts as *const LinuxUtsName) as *const u8,
            core::mem::size_of::<LinuxUtsName>(),
        )
    };
    match copy_to_user_task(ctx.current, call.arg(0), bytes) {
        Ok(()) => 0,
        Err(err) => -err,
    }
}

// ---------------------------------------------------------------------------
// getrandom (318)
// ---------------------------------------------------------------------------
pub(super) fn sys_getrandom(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let buf_ptr = call.arg(0);
    let buflen = call.arg(1) as usize;
    let _flags = call.arg(2);

    if buf_ptr == 0 {
        return -EFAULT;
    }

    let cap = cmp::min(buflen, 256);
    let mut tmp = [0u8; 256];

    let mut state = 0x9E37_79B9_7F4A_7C15u64 ^ pit::ticks().wrapping_mul(0xA076_1D64_78BD_642F);
    let pid = if ctx.has_task() {
        unsafe { (*ctx.current).pid as u64 }
    } else {
        0
    };
    state ^= pid.wrapping_mul(0x517C_C1B7_2722_0A95);
    if state == 0 {
        state = 1;
    }

    for chunk in tmp[..cap].chunks_mut(8) {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let sample = state.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes();
        chunk.copy_from_slice(&sample[..chunk.len()]);
    }

    match copy_to_user_task(ctx.current, buf_ptr, &tmp[..cap]) {
        Ok(()) => cap as i64,
        Err(err) => -err,
    }
}

// ---------------------------------------------------------------------------
// sysinfo (99)
// ---------------------------------------------------------------------------
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct LinuxSysinfo {
    pub(super) uptime: i64,
    pub(super) loads: [u64; 3],
    pub(super) totalram: u64,
    pub(super) freeram: u64,
    pub(super) sharedram: u64,
    pub(super) bufferram: u64,
    pub(super) totalswap: u64,
    pub(super) freeswap: u64,
    pub(super) procs: u16,
    pub(super) pad: u16,
    pub(super) pad2: u32,
    pub(super) totalhigh: u64,
    pub(super) freehigh: u64,
    pub(super) mem_unit: u32,
    pub(super) _padding: [u8; 4],
}

pub(super) fn sys_sysinfo(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let out_ptr = call.arg(0);
    if out_ptr == 0 {
        return -EFAULT;
    }

    let pmm = pmm::get_stats();

    let info = LinuxSysinfo {
        uptime: pit::uptime_seconds() as i64,
        loads: [0; 3],
        totalram: pmm.total_bytes,
        freeram: pmm.free_bytes,
        sharedram: 0,
        bufferram: 0,
        totalswap: 0,
        freeswap: 0,
        procs: 1,
        pad: 0,
        pad2: 0,
        totalhigh: 0,
        freehigh: 0,
        mem_unit: 1,
        _padding: [0; 4],
    };

    match write_struct_to_user(ctx.current, out_ptr, &info) {
        Ok(()) => 0,
        Err(err) => -err,
    }
}

pub(super) const PR_SET_PDEATHSIG: u64 = 1;
pub(super) const PR_GET_PDEATHSIG: u64 = 2;
pub(super) const PR_GET_DUMPABLE: u64 = 3;
pub(super) const PR_SET_DUMPABLE: u64 = 4;
pub(super) const PR_SET_NAME: u64 = 15;
pub(super) const PR_GET_NAME: u64 = 16;
pub(super) const PR_SET_CHILD_SUBREAPER: u64 = 36;
pub(super) const PR_SET_NO_NEW_PRIVS: u64 = 38;

/// `prctl(2)`: thread names, plus flags that are accepted without effect
/// (death signal, dumpable, subreaper, no_new_privs); anything else is
/// `EINVAL`, as for an unknown option on Linux.
pub(super) fn sys_prctl(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let option = call.arg(0);
    let arg2 = call.arg(1);
    match option {
        PR_SET_NAME => {
            let mut name = [0u8; 16];
            if let Err(e) = copy_from_user_task(ctx.current, arg2, &mut name[..15]) {
                return -e;
            }
            let len = name.iter().position(|&b| b == 0).unwrap_or(15);
            unsafe {
                let task_name = &mut (*ctx.current).name;
                task_name.fill(0);
                task_name[..len].copy_from_slice(&name[..len]);
            }
            0
        }
        PR_GET_NAME => {
            let mut name = [0u8; 16];
            unsafe {
                let task_name = &(*ctx.current).name;
                let len = task_name.iter().position(|&b| b == 0).unwrap_or(task_name.len()).min(15);
                name[..len].copy_from_slice(&task_name[..len]);
            }
            match copy_to_user_task(ctx.current, arg2, &name) {
                Ok(()) => 0,
                Err(e) => -e,
            }
        }
        _ => prctl_flag_result(option, arg2),
    }
}

/// Result for the prctl options that only take or report a flag.
pub(super) fn prctl_flag_result(option: u64, arg2: u64) -> i64 {
    match option {
        PR_SET_PDEATHSIG => {
            if arg2 > 64 {
                -EINVAL
            } else {
                0
            }
        }
        PR_GET_DUMPABLE => 1,
        PR_SET_DUMPABLE => {
            if arg2 > 1 {
                -EINVAL
            } else {
                0
            }
        }
        PR_SET_CHILD_SUBREAPER => 0,
        PR_SET_NO_NEW_PRIVS => {
            if arg2 != 1 {
                -EINVAL
            } else {
                0
            }
        }
        PR_GET_PDEATHSIG => -EINVAL, // needs a user pointer; handled as unsupported
        _ => -EINVAL,
    }
}

/// `membarrier(2)`: only `MEMBARRIER_CMD_QUERY`, reporting no supported
/// commands, so musl uses its signal-based fallback.
pub(super) fn sys_membarrier(_ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    membarrier_result(call.arg(0))
}

/// `inotify_init`/`inotify_init1`/`inotify_add_watch`/`inotify_rm_watch`:
/// not implemented. Known (rather than unknown) syscalls so they return
/// `-ENOSYS` quietly; GLib then reports directory monitoring as unsupported
/// and file managers simply do not auto-refresh on outside changes.
pub(super) fn sys_inotify_unsupported(_ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    -ENOSYS
}

pub(super) fn membarrier_result(cmd: u64) -> i64 {
    if cmd == 0 {
        0
    } else {
        -EINVAL
    }
}
