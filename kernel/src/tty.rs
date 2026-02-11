use core::ptr;

use crate::sync::SpinLock;
use crate::task::Task;

pub const EAGAIN: i64 = 11;
pub const ENOSPC: i64 = 28;

const TTY_READY_CAPACITY: usize = 1024;
const TTY_LINE_CAPACITY: usize = 256;
const TTY_MAX_WAITERS: usize = 64;
const TTY_WAKE_BATCH: usize = 8;

#[derive(Clone, Copy)]
struct Waiter {
    used: bool,
    task: usize,
}

impl Waiter {
    const fn empty() -> Self {
        Self {
            used: false,
            task: 0,
        }
    }
}

struct TtyState {
    ready_head: u16,
    ready_tail: u16,
    ready_len: u16,
    ready_buf: [u8; TTY_READY_CAPACITY],
    line_len: u16,
    line_buf: [u8; TTY_LINE_CAPACITY],
    waiters: [Waiter; TTY_MAX_WAITERS],
}

impl TtyState {
    const fn new() -> Self {
        Self {
            ready_head: 0,
            ready_tail: 0,
            ready_len: 0,
            ready_buf: [0; TTY_READY_CAPACITY],
            line_len: 0,
            line_buf: [0; TTY_LINE_CAPACITY],
            waiters: [Waiter::empty(); TTY_MAX_WAITERS],
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EchoAction {
    None,
    Byte(u8),
    Backspace,
    Newline,
}

static TTY_STATE: SpinLock<TtyState> = SpinLock::new(TtyState::new());

pub fn init() {
    let mut state = TTY_STATE.lock();
    *state = TtyState::new();
}

fn ready_push_byte_locked(state: &mut TtyState, byte: u8) -> bool {
    if state.ready_len as usize >= TTY_READY_CAPACITY {
        return false;
    }

    state.ready_buf[state.ready_head as usize] = byte;
    state.ready_head = ((state.ready_head as usize + 1) % TTY_READY_CAPACITY) as u16;
    state.ready_len += 1;
    true
}

fn ready_pop_locked(state: &mut TtyState, out: &mut [u8]) -> usize {
    if out.is_empty() || state.ready_len == 0 {
        return 0;
    }

    let mut count = 0usize;
    let max = core::cmp::min(out.len(), state.ready_len as usize);
    while count < max {
        out[count] = state.ready_buf[state.ready_tail as usize];
        state.ready_tail = ((state.ready_tail as usize + 1) % TTY_READY_CAPACITY) as u16;
        state.ready_len -= 1;
        count += 1;
    }
    count
}

fn commit_line_locked(state: &mut TtyState) -> usize {
    if state.line_len == 0 {
        return 0;
    }

    let mut committed = 0usize;
    let mut idx = 0usize;
    while idx < state.line_len as usize {
        if !ready_push_byte_locked(state, state.line_buf[idx]) {
            break;
        }
        committed += 1;
        idx += 1;
    }

    state.line_len = 0;
    committed
}

fn register_waiter_locked(state: &mut TtyState, task: *mut Task) -> Result<(), i64> {
    if task.is_null() {
        return Err(EAGAIN);
    }

    let mut idx = 0usize;
    while idx < TTY_MAX_WAITERS {
        let waiter = &state.waiters[idx];
        if waiter.used && waiter.task == task as usize {
            return Ok(());
        }
        idx += 1;
    }

    idx = 0;
    while idx < TTY_MAX_WAITERS {
        if !state.waiters[idx].used {
            state.waiters[idx] = Waiter {
                used: true,
                task: task as usize,
            };
            return Ok(());
        }
        idx += 1;
    }

    Err(ENOSPC)
}

fn remove_waiter_locked(state: &mut TtyState, task: *mut Task) {
    let mut idx = 0usize;
    while idx < TTY_MAX_WAITERS {
        let waiter = &mut state.waiters[idx];
        if waiter.used && waiter.task == task as usize {
            *waiter = Waiter::empty();
        }
        idx += 1;
    }
}

fn collect_waiters_locked(state: &mut TtyState, out: &mut [*mut Task; TTY_WAKE_BATCH]) -> usize {
    let mut count = 0usize;
    let mut idx = 0usize;
    while idx < TTY_MAX_WAITERS && count < out.len() {
        let waiter = &mut state.waiters[idx];
        if waiter.used && waiter.task != 0 {
            out[count] = waiter.task as *mut Task;
            *waiter = Waiter::empty();
            count += 1;
        }
        idx += 1;
    }
    count
}

fn wake_waiters() {
    loop {
        let mut tasks = [ptr::null_mut::<Task>(); TTY_WAKE_BATCH];
        let woke = {
            let mut state = TTY_STATE.lock();
            collect_waiters_locked(&mut state, &mut tasks)
        };
        if woke == 0 {
            break;
        }

        let mut idx = 0usize;
        while idx < woke {
            wake_task(tasks[idx]);
            idx += 1;
        }
    }
}

#[cfg(not(test))]
fn block_current(reason: &str) {
    crate::sched::block(reason);
}

#[cfg(test)]
fn block_current(_reason: &str) {}

#[cfg(not(test))]
fn wake_task(task: *mut Task) {
    if !task.is_null() {
        crate::sched::unblock(task);
    }
}

#[cfg(test)]
fn wake_task(_task: *mut Task) {}

#[cfg(not(test))]
#[inline]
fn can_block(task: *mut Task) -> bool {
    !task.is_null() && crate::sched::is_running()
}

#[cfg(test)]
#[inline]
fn can_block(_task: *mut Task) -> bool {
    false
}

#[cfg(not(test))]
fn emit_echo(action: EchoAction) {
    match action {
        EchoAction::None => {}
        EchoAction::Byte(byte) => crate::serial::write_bytes(&[byte]),
        EchoAction::Backspace => crate::serial::write_bytes(b"\x08 \x08"),
        EchoAction::Newline => crate::serial::write_bytes(b"\r\n"),
    }
}

#[cfg(test)]
fn emit_echo(_action: EchoAction) {}

pub fn ingest_byte(byte: u8) {
    let mut wake = false;
    let mut echo = EchoAction::None;

    {
        let mut state = TTY_STATE.lock();
        match byte {
            b'\r' | b'\n' => {
                if state.line_len as usize >= TTY_LINE_CAPACITY {
                    state.line_len = (TTY_LINE_CAPACITY - 1) as u16;
                }
                let idx = state.line_len as usize;
                state.line_buf[idx] = b'\n';
                state.line_len += 1;
                wake = commit_line_locked(&mut state) > 0;
                echo = EchoAction::Newline;
            }
            0x08 | 0x7F => {
                if state.line_len > 0 {
                    state.line_len -= 1;
                    echo = EchoAction::Backspace;
                }
            }
            b'\t' | 0x20..=0x7E => {
                if state.line_len as usize + 1 < TTY_LINE_CAPACITY {
                    let idx = state.line_len as usize;
                    state.line_buf[idx] = byte;
                    state.line_len += 1;
                    echo = EchoAction::Byte(byte);
                }
            }
            _ => {}
        }
    }

    emit_echo(echo);
    if wake {
        wake_waiters();
    }
}

pub fn read_nonblocking(out: &mut [u8]) -> usize {
    let mut state = TTY_STATE.lock();
    ready_pop_locked(&mut state, out)
}

pub fn read_console(task: *mut Task, out: &mut [u8]) -> Result<usize, i64> {
    if out.is_empty() {
        return Ok(0);
    }

    loop {
        {
            let mut state = TTY_STATE.lock();
            let read_n = ready_pop_locked(&mut state, out);
            if read_n > 0 {
                return Ok(read_n);
            }

            if !can_block(task) {
                return Ok(0);
            }

            register_waiter_locked(&mut state, task)?;
        }

        block_current("tty-read");
        let mut state = TTY_STATE.lock();
        remove_waiter_locked(&mut state, task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_is_committed_on_newline() {
        init();

        ingest_byte(b'h');
        ingest_byte(b'i');
        assert_eq!(read_nonblocking(&mut [0u8; 4]), 0);

        ingest_byte(b'\n');
        let mut out = [0u8; 8];
        let n = read_nonblocking(&mut out);
        assert_eq!(&out[..n], b"hi\n");
    }

    #[test]
    fn backspace_edits_pending_line() {
        init();

        ingest_byte(b'a');
        ingest_byte(b'b');
        ingest_byte(0x08);
        ingest_byte(b'c');
        ingest_byte(b'\n');

        let mut out = [0u8; 8];
        let n = read_nonblocking(&mut out);
        assert_eq!(&out[..n], b"ac\n");
    }

    #[test]
    fn printable_bytes_only_visible_after_newline() {
        init();

        ingest_byte(b'x');
        ingest_byte(b'y');
        let mut out = [0u8; 8];
        assert_eq!(read_nonblocking(&mut out), 0);

        ingest_byte(b'\r');
        let n = read_nonblocking(&mut out);
        assert_eq!(&out[..n], b"xy\n");
    }
}
