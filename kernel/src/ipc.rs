use core::ptr;

use crate::sync::SpinLock;
use crate::task::Task;

pub const EINVAL: i64 = 22;
pub const ENOSPC: i64 = 28;
pub const EPIPE: i64 = 32;
pub const EAGAIN: i64 = 11;
pub const ENOENT: i64 = 2;

pub const FUTEX_WAIT: u32 = 0;
pub const FUTEX_WAKE: u32 = 1;

const PIPE_CAPACITY: usize = 1024;
const IPC_MAX_PIPES: usize = 64;
const IPC_MAX_WAITERS: usize = 128;
const WAKE_BATCH: usize = 16;

const WAIT_KIND_PIPE_READ: u8 = 1;
const WAIT_KIND_PIPE_WRITE: u8 = 2;
const WAIT_KIND_FUTEX: u8 = 3;

#[derive(Clone, Copy)]
struct Pipe {
    used: bool,
    read_ends: u16,
    write_ends: u16,
    head: u16,
    tail: u16,
    len: u16,
    buf: [u8; PIPE_CAPACITY],
}

impl Pipe {
    const fn empty() -> Self {
        Self {
            used: false,
            read_ends: 0,
            write_ends: 0,
            head: 0,
            tail: 0,
            len: 0,
            buf: [0; PIPE_CAPACITY],
        }
    }
}

#[derive(Clone, Copy)]
struct Waiter {
    used: bool,
    key: u64,
    task: usize,
}

impl Waiter {
    const fn empty() -> Self {
        Self {
            used: false,
            key: 0,
            task: 0,
        }
    }
}

struct IpcState {
    pipes: [Pipe; IPC_MAX_PIPES],
    waiters: [Waiter; IPC_MAX_WAITERS],
}

impl IpcState {
    const fn new() -> Self {
        Self {
            pipes: [Pipe::empty(); IPC_MAX_PIPES],
            waiters: [Waiter::empty(); IPC_MAX_WAITERS],
        }
    }
}

static IPC_STATE: SpinLock<IpcState> = SpinLock::new(IpcState::new());

#[inline]
fn pipe_read_key(pipe_id: u16) -> u64 {
    ((WAIT_KIND_PIPE_READ as u64) << 56) | pipe_id as u64
}

#[inline]
fn pipe_write_key(pipe_id: u16) -> u64 {
    ((WAIT_KIND_PIPE_WRITE as u64) << 56) | pipe_id as u64
}

#[inline]
fn futex_key(addr: u64) -> u64 {
    ((WAIT_KIND_FUTEX as u64) << 56) | (addr & 0x00FF_FFFF_FFFF_FFFF)
}

fn register_waiter_locked(state: &mut IpcState, key: u64, task: *mut Task) -> Result<(), i64> {
    if task.is_null() {
        return Err(EAGAIN);
    }

    let mut idx = 0usize;
    while idx < IPC_MAX_WAITERS {
        let waiter = &state.waiters[idx];
        if waiter.used && waiter.task == task as usize && waiter.key == key {
            return Ok(());
        }
        idx += 1;
    }

    idx = 0;
    while idx < IPC_MAX_WAITERS {
        if !state.waiters[idx].used {
            state.waiters[idx] = Waiter {
                used: true,
                key,
                task: task as usize,
            };
            return Ok(());
        }
        idx += 1;
    }

    Err(ENOSPC)
}

fn remove_waiter_locked(state: &mut IpcState, key: u64, task: *mut Task) {
    let mut idx = 0usize;
    while idx < IPC_MAX_WAITERS {
        let waiter = &mut state.waiters[idx];
        if waiter.used && waiter.key == key && waiter.task == task as usize {
            *waiter = Waiter::empty();
        }
        idx += 1;
    }
}

fn collect_waiters_locked(
    state: &mut IpcState,
    key: u64,
    max_count: usize,
    out: &mut [*mut Task; WAKE_BATCH],
) -> usize {
    let mut count = 0usize;
    let mut idx = 0usize;
    while idx < IPC_MAX_WAITERS && count < max_count && count < out.len() {
        let waiter = &mut state.waiters[idx];
        if waiter.used && waiter.key == key && waiter.task != 0 {
            out[count] = waiter.task as *mut Task;
            *waiter = Waiter::empty();
            count += 1;
        }
        idx += 1;
    }
    count
}

fn wake_key(key: u64, max_count: usize) -> usize {
    let mut total = 0usize;
    let mut remaining = max_count;

    while remaining > 0 {
        let mut tasks = [ptr::null_mut::<Task>(); WAKE_BATCH];
        let woke = {
            let mut state = IPC_STATE.lock();
            collect_waiters_locked(&mut state, key, remaining, &mut tasks)
        };

        if woke == 0 {
            break;
        }

        let mut idx = 0usize;
        while idx < woke {
            wake_task(tasks[idx]);
            idx += 1;
        }
        total += woke;
        remaining -= woke;
    }

    total
}

fn alloc_pipe_locked(state: &mut IpcState) -> Result<u16, i64> {
    let mut idx = 0usize;
    while idx < IPC_MAX_PIPES {
        if !state.pipes[idx].used {
            state.pipes[idx] = Pipe::empty();
            state.pipes[idx].used = true;
            return Ok(idx as u16);
        }
        idx += 1;
    }
    Err(ENOSPC)
}

fn block_current(reason: &str) {
    #[cfg(not(test))]
    {
        crate::sched::block(reason);
    }
    #[cfg(test)]
    {
        let _ = reason;
    }
}

fn wake_task(task: *mut Task) {
    #[cfg(not(test))]
    {
        if !task.is_null() {
            crate::sched::unblock(task);
        }
    }
    #[cfg(test)]
    {
        let _ = task;
    }
}

pub fn init() {
    let mut state = IPC_STATE.lock();
    let mut idx = 0usize;
    while idx < IPC_MAX_PIPES {
        state.pipes[idx] = Pipe::empty();
        idx += 1;
    }
    idx = 0;
    while idx < IPC_MAX_WAITERS {
        state.waiters[idx] = Waiter::empty();
        idx += 1;
    }
}

pub fn pipe_open() -> Result<u16, i64> {
    let mut state = IPC_STATE.lock();
    let pipe_id = alloc_pipe_locked(&mut state)?;
    state.pipes[pipe_id as usize].read_ends = 1;
    state.pipes[pipe_id as usize].write_ends = 1;
    if cfg!(feature = "musl-ext2-cmd-smoke") {
        crate::serial_println!("[IPC] pipe {} open r=1 w=1", pipe_id);
    }
    Ok(pipe_id)
}

pub fn pipe_retain(pipe_id: u16, retain_read: bool, retain_write: bool) -> Result<(), i64> {
    let mut state = IPC_STATE.lock();
    let idx = pipe_id as usize;
    if idx >= IPC_MAX_PIPES || !state.pipes[idx].used {
        return Err(ENOENT);
    }

    let pipe = &mut state.pipes[idx];
    if retain_read {
        pipe.read_ends = pipe.read_ends.saturating_add(1);
    }
    if retain_write {
        pipe.write_ends = pipe.write_ends.saturating_add(1);
    }
    if cfg!(feature = "musl-ext2-cmd-smoke") {
        crate::serial_println!(
            "[IPC] pipe {} retain read={} write={} -> r={} w={}",
            pipe_id,
            retain_read,
            retain_write,
            pipe.read_ends,
            pipe.write_ends
        );
    }
    Ok(())
}

pub fn pipe_close(pipe_id: u16, close_read: bool, close_write: bool) -> Result<(), i64> {
    let mut state = IPC_STATE.lock();
    let idx = pipe_id as usize;
    if idx >= IPC_MAX_PIPES || !state.pipes[idx].used {
        return Err(ENOENT);
    }

    let pipe = &mut state.pipes[idx];
    if close_read && pipe.read_ends > 0 {
        pipe.read_ends -= 1;
    }
    if close_write && pipe.write_ends > 0 {
        pipe.write_ends -= 1;
    }
    if cfg!(feature = "musl-ext2-cmd-smoke") {
        crate::serial_println!(
            "[IPC] pipe {} close read={} write={} -> r={} w={}",
            pipe_id,
            close_read,
            close_write,
            pipe.read_ends,
            pipe.write_ends
        );
    }

    let free_pipe = pipe.read_ends == 0 && pipe.write_ends == 0;
    drop(state);

    let _ = wake_key(pipe_read_key(pipe_id), usize::MAX);
    let _ = wake_key(pipe_write_key(pipe_id), usize::MAX);

    if free_pipe {
        let mut state = IPC_STATE.lock();
        if (pipe_id as usize) < IPC_MAX_PIPES {
            state.pipes[pipe_id as usize] = Pipe::empty();
        }
    }
    Ok(())
}

pub fn pipe_read(
    task: *mut Task,
    pipe_id: u16,
    out: &mut [u8],
    nonblocking: bool,
) -> Result<usize, i64> {
    if out.is_empty() {
        return Ok(0);
    }
    let read_key = pipe_read_key(pipe_id);
    let write_key = pipe_write_key(pipe_id);

    loop {
        let mut read_n = 0usize;
        let mut eof = false;
        let mut should_block = false;
        {
            let mut state = IPC_STATE.lock();
            let idx = pipe_id as usize;
            if idx >= IPC_MAX_PIPES || !state.pipes[idx].used {
                return Err(ENOENT);
            }

            let pipe = &mut state.pipes[idx];
            if pipe.len > 0 {
                let mut count = 0usize;
                let max = core::cmp::min(pipe.len as usize, out.len());
                while count < max {
                    out[count] = pipe.buf[pipe.tail as usize];
                    pipe.tail = ((pipe.tail as usize + 1) % PIPE_CAPACITY) as u16;
                    pipe.len -= 1;
                    count += 1;
                }
                read_n = count;
            } else if pipe.write_ends == 0 {
                eof = true;
            } else if nonblocking {
                return Err(EAGAIN);
            } else {
                register_waiter_locked(&mut state, read_key, task)?;
                should_block = true;
            }
        }

        if read_n > 0 {
            let _ = wake_key(write_key, usize::MAX);
            return Ok(read_n);
        }
        if eof {
            return Ok(0);
        }
        if should_block {
            block_current("pipe-read");
            let mut state = IPC_STATE.lock();
            remove_waiter_locked(&mut state, read_key, task);
        }
    }
}

pub fn pipe_write(
    task: *mut Task,
    pipe_id: u16,
    src: &[u8],
    nonblocking: bool,
) -> Result<usize, i64> {
    if src.is_empty() {
        return Ok(0);
    }
    let read_key = pipe_read_key(pipe_id);
    let write_key = pipe_write_key(pipe_id);
    let mut written = 0usize;

    while written < src.len() {
        let mut wrote_now = 0usize;
        let mut should_block = false;
        {
            let mut state = IPC_STATE.lock();
            let idx = pipe_id as usize;
            if idx >= IPC_MAX_PIPES || !state.pipes[idx].used {
                return Err(ENOENT);
            }
            let pipe = &mut state.pipes[idx];
            if pipe.read_ends == 0 {
                return Err(EPIPE);
            }

            if (pipe.len as usize) < PIPE_CAPACITY {
                while written + wrote_now < src.len() && (pipe.len as usize) < PIPE_CAPACITY {
                    pipe.buf[pipe.head as usize] = src[written + wrote_now];
                    pipe.head = ((pipe.head as usize + 1) % PIPE_CAPACITY) as u16;
                    pipe.len += 1;
                    wrote_now += 1;
                }
            } else if nonblocking {
                if written > 0 {
                    return Ok(written);
                }
                return Err(EAGAIN);
            } else {
                register_waiter_locked(&mut state, write_key, task)?;
                should_block = true;
            }
        }

        if wrote_now > 0 {
            written += wrote_now;
            let _ = wake_key(read_key, usize::MAX);
            continue;
        }

        if should_block {
            block_current("pipe-write");
            let mut state = IPC_STATE.lock();
            remove_waiter_locked(&mut state, write_key, task);
        }
    }

    Ok(written)
}

/// Non-destructive poll of pipe state. Returns
/// `(has_data, has_space, read_eof, write_broken)`.
pub fn pipe_poll_status(pipe_id: u16) -> Result<(bool, bool, bool, bool), i64> {
    let state = IPC_STATE.lock();
    let idx = pipe_id as usize;
    if idx >= IPC_MAX_PIPES || !state.pipes[idx].used {
        return Err(ENOENT);
    }
    let pipe = &state.pipes[idx];
    let has_data = pipe.len > 0;
    let has_space = (pipe.len as usize) < PIPE_CAPACITY;
    let read_eof = pipe.write_ends == 0;
    let write_broken = pipe.read_ends == 0;
    Ok((has_data, has_space, read_eof, write_broken))
}

pub fn futex_wait(task: *mut Task, addr: u64) -> Result<(), i64> {
    let key = futex_key(addr);
    {
        let mut state = IPC_STATE.lock();
        register_waiter_locked(&mut state, key, task)?;
    }
    block_current("futex-wait");
    {
        let mut state = IPC_STATE.lock();
        remove_waiter_locked(&mut state, key, task);
    }
    Ok(())
}

pub fn futex_wake(addr: u64, count: u32) -> usize {
    if count == 0 {
        return 0;
    }
    wake_key(futex_key(addr), count as usize)
}

pub fn remove_task_waiters(task: *mut Task) {
    if task.is_null() {
        return;
    }
    let mut state = IPC_STATE.lock();
    let mut idx = 0usize;
    while idx < IPC_MAX_WAITERS {
        let waiter = &mut state.waiters[idx];
        if waiter.used && waiter.task == task as usize {
            *waiter = Waiter::empty();
        }
        idx += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn pipe_roundtrip_and_eof() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();
        let pipe = pipe_open().expect("pipe alloc");

        let wrote = pipe_write(ptr::null_mut(), pipe, b"abc", false).expect("pipe write");
        assert_eq!(wrote, 3);

        let mut buf = [0u8; 8];
        let read_n = pipe_read(ptr::null_mut(), pipe, &mut buf, false).expect("pipe read");
        assert_eq!(&buf[..read_n], b"abc");

        pipe_close(pipe, false, true).expect("close writer");
        let eof_n = pipe_read(ptr::null_mut(), pipe, &mut buf, false).expect("eof read");
        assert_eq!(eof_n, 0);
    }

    #[test]
    fn pipe_write_fails_without_readers() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();
        let pipe = pipe_open().expect("pipe alloc");
        pipe_close(pipe, true, false).expect("close readers");
        let err = pipe_write(ptr::null_mut(), pipe, b"x", false).unwrap_err();
        assert_eq!(err, EPIPE);
    }

    #[test]
    fn pipe_retain_balances_additional_descriptors() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();
        let pipe = pipe_open().expect("pipe alloc");

        pipe_retain(pipe, true, false).expect("retain read end");
        pipe_close(pipe, true, false).expect("close one read end");
        pipe_close(pipe, true, false).expect("close second read end");

        let err = pipe_write(ptr::null_mut(), pipe, b"x", false).unwrap_err();
        assert_eq!(err, EPIPE);
    }

    #[test]
    fn pipe_nonblocking_empty_read_returns_eagain() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();
        let pipe = pipe_open().expect("pipe alloc");
        let mut buf = [0u8; 8];
        let err = pipe_read(ptr::null_mut(), pipe, &mut buf, true).unwrap_err();
        assert_eq!(err, EAGAIN);
    }

    #[test]
    fn pipe_nonblocking_full_write_returns_partial_then_eagain() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();
        let pipe = pipe_open().expect("pipe alloc");

        let fill = [0xABu8; PIPE_CAPACITY];
        let wrote = pipe_write(ptr::null_mut(), pipe, &fill, true).expect("fill");
        assert_eq!(wrote, PIPE_CAPACITY);

        let err = pipe_write(ptr::null_mut(), pipe, b"x", true).unwrap_err();
        assert_eq!(err, EAGAIN);
    }

    #[test]
    fn futex_wake_clears_waiters() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();
        let addr = 0x1000u64;
        let t1 = 0x1111usize as *mut Task;
        let t2 = 0x2222usize as *mut Task;

        {
            let mut state = IPC_STATE.lock();
            let key = futex_key(addr);
            register_waiter_locked(&mut state, key, t1).expect("waiter 1");
            register_waiter_locked(&mut state, key, t2).expect("waiter 2");
        }

        let woke = futex_wake(addr, 1);
        assert_eq!(woke, 1);
        let woke2 = futex_wake(addr, 8);
        assert_eq!(woke2, 1);
        let woke3 = futex_wake(addr, 1);
        assert_eq!(woke3, 0);
    }
}
