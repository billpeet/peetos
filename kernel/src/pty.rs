use core::ptr;

use crate::sync::SpinLock;
use crate::task::Task;
use crate::tty::{self, LinuxTermios, Tty};

pub const PTY_MAX: usize = 4;
const PTY_BUF_SIZE: usize = 1024;
const PTY_MAX_WAITERS: usize = 8;
const PTY_WAKE_BATCH: usize = 8;

const EAGAIN: i64 = 11;
const ENOSPC: i64 = 28;
const EIO: i64 = 5;

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

struct PtyEntry {
    in_use: bool,
    locked: bool,
    master_refs: u16,
    slave_refs: u16,
    tty: Tty,
    master_buf: [u8; PTY_BUF_SIZE],
    master_head: u16,
    master_tail: u16,
    master_len: u16,
    master_waiters: [Waiter; PTY_MAX_WAITERS],
    ws_row: u16,
    ws_col: u16,
}

impl PtyEntry {
    const fn empty() -> Self {
        Self {
            in_use: false,
            locked: true,
            master_refs: 0,
            slave_refs: 0,
            tty: Tty::new(),
            master_buf: [0; PTY_BUF_SIZE],
            master_head: 0,
            master_tail: 0,
            master_len: 0,
            master_waiters: [Waiter::empty(); PTY_MAX_WAITERS],
            ws_row: 25,
            ws_col: 80,
        }
    }

    fn master_buf_push_byte(&mut self, byte: u8) -> bool {
        if self.master_len as usize >= PTY_BUF_SIZE {
            return false;
        }
        self.master_buf[self.master_head as usize] = byte;
        self.master_head = ((self.master_head as usize + 1) % PTY_BUF_SIZE) as u16;
        self.master_len += 1;
        true
    }

    fn master_buf_pop(&mut self, out: &mut [u8]) -> usize {
        if out.is_empty() || self.master_len == 0 {
            return 0;
        }
        let mut count = 0usize;
        let max = core::cmp::min(out.len(), self.master_len as usize);
        while count < max {
            out[count] = self.master_buf[self.master_tail as usize];
            self.master_tail = ((self.master_tail as usize + 1) % PTY_BUF_SIZE) as u16;
            self.master_len -= 1;
            count += 1;
        }
        count
    }

    fn register_master_waiter(&mut self, task: *mut Task) -> Result<(), i64> {
        if task.is_null() {
            return Err(EAGAIN);
        }
        let mut idx = 0usize;
        while idx < PTY_MAX_WAITERS {
            if self.master_waiters[idx].used && self.master_waiters[idx].task == task as usize {
                return Ok(());
            }
            idx += 1;
        }
        idx = 0;
        while idx < PTY_MAX_WAITERS {
            if !self.master_waiters[idx].used {
                self.master_waiters[idx] = Waiter {
                    used: true,
                    task: task as usize,
                };
                return Ok(());
            }
            idx += 1;
        }
        Err(ENOSPC)
    }

    fn remove_master_waiter(&mut self, task: *mut Task) {
        let mut idx = 0usize;
        while idx < PTY_MAX_WAITERS {
            if self.master_waiters[idx].used && self.master_waiters[idx].task == task as usize {
                self.master_waiters[idx] = Waiter::empty();
            }
            idx += 1;
        }
    }

    fn collect_master_waiters(&mut self, out: &mut [*mut Task; PTY_WAKE_BATCH]) -> usize {
        let mut count = 0usize;
        let mut idx = 0usize;
        while idx < PTY_MAX_WAITERS && count < out.len() {
            if self.master_waiters[idx].used && self.master_waiters[idx].task != 0 {
                out[count] = self.master_waiters[idx].task as *mut Task;
                self.master_waiters[idx] = Waiter::empty();
                count += 1;
            }
            idx += 1;
        }
        count
    }
}

struct PtyState {
    entries: [PtyEntry; PTY_MAX],
}

impl PtyState {
    const fn new() -> Self {
        Self {
            entries: [const { PtyEntry::empty() }; PTY_MAX],
        }
    }
}

static PTY_STATE: SpinLock<PtyState> = SpinLock::new(PtyState::new());

pub fn init() {
    let mut state = PTY_STATE.lock();
    let mut i = 0;
    while i < PTY_MAX {
        state.entries[i] = PtyEntry::empty();
        i += 1;
    }
}

pub fn pty_alloc() -> Result<u16, i64> {
    let mut state = PTY_STATE.lock();
    let mut i = 0usize;
    while i < PTY_MAX {
        if !state.entries[i].in_use {
            state.entries[i] = PtyEntry::empty();
            state.entries[i].in_use = true;
            state.entries[i].locked = true;
            state.entries[i].master_refs = 1;
            state.entries[i].slave_refs = 0;
            state.entries[i].tty = Tty::new_for_pty(i as u16);
            return Ok(i as u16);
        }
        i += 1;
    }
    Err(ENOSPC)
}

fn valid_pty(state: &PtyState, id: u16) -> bool {
    (id as usize) < PTY_MAX && state.entries[id as usize].in_use
}

pub fn master_read(
    task: *mut Task,
    pty_id: u16,
    out: &mut [u8],
    nonblocking: bool,
) -> Result<usize, i64> {
    if out.is_empty() {
        return Ok(0);
    }

    loop {
        {
            let mut state = PTY_STATE.lock();
            if !valid_pty(&state, pty_id) {
                return Err(EIO);
            }
            let entry = &mut state.entries[pty_id as usize];

            let n = entry.master_buf_pop(out);
            if n > 0 {
                return Ok(n);
            }

            // EOF: no slave refs
            if entry.slave_refs == 0 {
                return Ok(0);
            }

            if nonblocking {
                return Err(EAGAIN);
            }

            if !can_block(task) {
                return Ok(0);
            }

            entry.register_master_waiter(task)?;
        }

        block_current("pty-mread");
        let mut state = PTY_STATE.lock();
        if valid_pty(&state, pty_id) {
            state.entries[pty_id as usize].remove_master_waiter(task);
        }
    }
}

pub fn master_write(
    _task: *mut Task,
    pty_id: u16,
    data: &[u8],
    _nonblocking: bool,
) -> Result<usize, i64> {
    if data.is_empty() {
        return Ok(0);
    }

    let mut echo_bufs: [[u8; tty::ECHO_BUF_MAX]; 64] = [[0; tty::ECHO_BUF_MAX]; 64];
    let mut echo_lens: [u8; 64] = [0; 64];
    let mut echo_count = 0usize;
    let mut signal = 0u32;
    let mut wake_slave = false;

    {
        let mut state = PTY_STATE.lock();
        if !valid_pty(&state, pty_id) {
            return Err(EIO);
        }
        let entry = &mut state.entries[pty_id as usize];

        if entry.slave_refs == 0 {
            return Err(EIO);
        }

        for &byte in data {
            let result = entry.tty.ingest_byte_locked(byte);

            if result.echo_len > 0 && echo_count < 64 {
                echo_bufs[echo_count] = result.echo_buf;
                echo_lens[echo_count] = result.echo_len;
                echo_count += 1;
            }

            if result.signal != 0 {
                signal = result.signal;
            }
            if result.wake {
                wake_slave = true;
            }
        }

        // Push echo output to master_buf so master can read it
        for i in 0..echo_count {
            let len = echo_lens[i] as usize;
            for j in 0..len {
                entry.master_buf_push_byte(echo_bufs[i][j]);
            }
        }
    }

    // Wake slave readers if data was committed to slave input
    if wake_slave {
        wake_slave_waiters(pty_id);
    }

    // Wake master readers if echo was produced
    if echo_count > 0 {
        wake_master_waiters(pty_id);
    }

    if signal != 0 {
        emit_signal_for_pty(pty_id, signal);
    }

    Ok(data.len())
}

pub fn master_close(pty_id: u16) {
    let mut wake_tasks = [ptr::null_mut::<Task>(); PTY_WAKE_BATCH];
    let mut slave_wake_count = 0usize;

    {
        let mut state = PTY_STATE.lock();
        if !valid_pty(&state, pty_id) {
            return;
        }
        let entry = &mut state.entries[pty_id as usize];
        if entry.master_refs > 0 {
            entry.master_refs -= 1;
        }

        if entry.master_refs == 0 {
            // Wake slave readers so they get EOF
            slave_wake_count = entry.tty.collect_waiters(&mut wake_tasks);

            if entry.slave_refs == 0 {
                *entry = PtyEntry::empty();
            }
        }
    }

    for i in 0..slave_wake_count {
        wake_task(wake_tasks[i]);
    }
}

pub fn slave_open(pty_id: u16) -> Result<(), i64> {
    let mut state = PTY_STATE.lock();
    if !valid_pty(&state, pty_id) {
        return Err(EIO);
    }
    let entry = &mut state.entries[pty_id as usize];
    if entry.locked {
        return Err(EIO);
    }
    entry.slave_refs += 1;
    Ok(())
}

pub fn slave_read(
    task: *mut Task,
    pty_id: u16,
    out: &mut [u8],
    nonblocking: bool,
) -> Result<usize, i64> {
    if out.is_empty() {
        return Ok(0);
    }

    loop {
        {
            let mut state = PTY_STATE.lock();
            if !valid_pty(&state, pty_id) {
                return Err(EIO);
            }
            let entry = &mut state.entries[pty_id as usize];

            // Check EOF
            if entry.tty.eof_pending {
                entry.tty.eof_pending = false;
                return Ok(0);
            }

            let n = entry.tty.input_pop(out);
            if n > 0 {
                return Ok(n);
            }

            // No master → EOF
            if entry.master_refs == 0 {
                return Ok(0);
            }

            if nonblocking {
                return Err(EAGAIN);
            }

            if !can_block(task) {
                return Ok(0);
            }

            entry.tty.register_waiter(task)?;
        }

        block_current("pty-sread");
        let mut state = PTY_STATE.lock();
        if valid_pty(&state, pty_id) {
            state.entries[pty_id as usize].tty.remove_waiter(task);
        }
    }
}

pub fn slave_write(
    task: *mut Task,
    pty_id: u16,
    data: &[u8],
    nonblocking: bool,
) -> Result<usize, i64> {
    let _ = task;
    if data.is_empty() {
        return Ok(0);
    }

    let mut written = 0usize;
    let mut wake_master = false;

    {
        let mut state = PTY_STATE.lock();
        if !valid_pty(&state, pty_id) {
            return Err(EIO);
        }
        let entry = &mut state.entries[pty_id as usize];

        if entry.master_refs == 0 {
            return Err(EIO);
        }

        for &byte in data {
            if !entry.master_buf_push_byte(byte) {
                if nonblocking {
                    if written == 0 {
                        return Err(EAGAIN);
                    }
                    break;
                }
                break;
            }
            written += 1;
            wake_master = true;
        }
    }

    if wake_master {
        wake_master_waiters(pty_id);
    }

    Ok(written)
}

pub fn slave_close(pty_id: u16) {
    let mut wake_tasks = [ptr::null_mut::<Task>(); PTY_WAKE_BATCH];
    let mut master_wake_count = 0usize;

    {
        let mut state = PTY_STATE.lock();
        if !valid_pty(&state, pty_id) {
            return;
        }
        let entry = &mut state.entries[pty_id as usize];
        if entry.slave_refs > 0 {
            entry.slave_refs -= 1;
        }

        if entry.slave_refs == 0 {
            // Wake master readers so they get EOF
            master_wake_count = entry.collect_master_waiters(&mut wake_tasks);

            if entry.master_refs == 0 {
                *entry = PtyEntry::empty();
            }
        }
    }

    for i in 0..master_wake_count {
        wake_task(wake_tasks[i]);
    }
}

/// Push data to master_buf from TTY echo output callback.
pub fn master_buf_push(pty_id: u16, data: &[u8]) {
    let mut state = PTY_STATE.lock();
    if !valid_pty(&state, pty_id) {
        return;
    }
    let entry = &mut state.entries[pty_id as usize];
    for &byte in data {
        entry.master_buf_push_byte(byte);
    }
}

pub fn master_retain(pty_id: u16) {
    let mut state = PTY_STATE.lock();
    if valid_pty(&state, pty_id) {
        state.entries[pty_id as usize].master_refs += 1;
    }
}

pub fn slave_retain(pty_id: u16) {
    let mut state = PTY_STATE.lock();
    if valid_pty(&state, pty_id) {
        state.entries[pty_id as usize].slave_refs += 1;
    }
}

pub fn master_poll(pty_id: u16) -> (bool, bool) {
    let state = PTY_STATE.lock();
    if !valid_pty(&state, pty_id) {
        return (false, false);
    }
    let entry = &state.entries[pty_id as usize];
    let has_data = entry.master_len > 0;
    let can_write = entry.slave_refs > 0;
    (has_data, can_write)
}

pub fn slave_poll(pty_id: u16) -> (bool, bool) {
    let state = PTY_STATE.lock();
    if !valid_pty(&state, pty_id) {
        return (false, false);
    }
    let entry = &state.entries[pty_id as usize];
    let has_data = entry.tty.has_ready_data();
    let can_write = entry.master_refs > 0 && (entry.master_len as usize) < PTY_BUF_SIZE;
    (has_data, can_write)
}

pub fn get_winsize(pty_id: u16) -> (u16, u16) {
    let state = PTY_STATE.lock();
    if !valid_pty(&state, pty_id) {
        return (25, 80);
    }
    let entry = &state.entries[pty_id as usize];
    (entry.ws_row, entry.ws_col)
}

pub fn set_winsize(pty_id: u16, row: u16, col: u16) {
    let mut state = PTY_STATE.lock();
    if valid_pty(&state, pty_id) {
        state.entries[pty_id as usize].ws_row = row;
        state.entries[pty_id as usize].ws_col = col;
    }
}

pub fn get_termios(pty_id: u16) -> LinuxTermios {
    let state = PTY_STATE.lock();
    if !valid_pty(&state, pty_id) {
        return tty::default_termios();
    }
    state.entries[pty_id as usize].tty.termios
}

pub fn set_termios(pty_id: u16, tio: &LinuxTermios) {
    let mut state = PTY_STATE.lock();
    if valid_pty(&state, pty_id) {
        state.entries[pty_id as usize].tty.apply_termios(tio);
    }
}

pub fn unlock(pty_id: u16) {
    let mut state = PTY_STATE.lock();
    if valid_pty(&state, pty_id) {
        state.entries[pty_id as usize].locked = false;
    }
}

pub fn is_in_use(pty_id: u16) -> bool {
    let state = PTY_STATE.lock();
    valid_pty(&state, pty_id)
}

pub fn is_locked(pty_id: u16) -> bool {
    let state = PTY_STATE.lock();
    if !valid_pty(&state, pty_id) {
        return true;
    }
    state.entries[pty_id as usize].locked
}

fn wake_slave_waiters(pty_id: u16) {
    loop {
        let mut tasks = [ptr::null_mut::<Task>(); PTY_WAKE_BATCH];
        let woke = {
            let mut state = PTY_STATE.lock();
            if !valid_pty(&state, pty_id) {
                return;
            }
            state.entries[pty_id as usize]
                .tty
                .collect_waiters(&mut tasks)
        };
        if woke == 0 {
            break;
        }
        for i in 0..woke {
            wake_task(tasks[i]);
        }
    }
}

fn wake_master_waiters(pty_id: u16) {
    loop {
        let mut tasks = [ptr::null_mut::<Task>(); PTY_WAKE_BATCH];
        let woke = {
            let mut state = PTY_STATE.lock();
            if !valid_pty(&state, pty_id) {
                return;
            }
            state.entries[pty_id as usize].collect_master_waiters(&mut tasks)
        };
        if woke == 0 {
            break;
        }
        for i in 0..woke {
            wake_task(tasks[i]);
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
fn emit_signal_for_pty(pty_id: u16, signum: u32) {
    // Send signal to processes with this PTY's foreground group
    // For now, send to all user tasks (same as console Ctrl-C)
    let _ = pty_id;
    crate::signal::send_to_foreground_group(0, signum);
}

#[cfg(test)]
fn emit_signal_for_pty(_pty_id: u16, _signum: u32) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn alloc_and_free_cycle() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();

        let id = pty_alloc().expect("alloc");
        assert!(is_in_use(id));
        assert!(is_locked(id));

        // Unlock so slave can open
        unlock(id);
        assert!(!is_locked(id));

        // Close master (refs go to 0, entry freed since no slave)
        master_close(id);
        assert!(!is_in_use(id));
    }

    #[test]
    fn master_write_to_slave_read() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();

        let id = pty_alloc().expect("alloc");
        unlock(id);
        slave_open(id).expect("slave open");

        // Master writes "hello\n" → line discipline processes → slave reads
        master_write(ptr::null_mut(), id, b"hello\n", false).expect("master write");

        let mut out = [0u8; 32];
        let n = slave_read(ptr::null_mut(), id, &mut out, false).expect("slave read");
        assert_eq!(&out[..n], b"hello\n");

        slave_close(id);
        master_close(id);
    }

    #[test]
    fn slave_write_to_master_read() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();

        let id = pty_alloc().expect("alloc");
        unlock(id);
        slave_open(id).expect("slave open");

        // Slave writes directly to master_buf
        slave_write(ptr::null_mut(), id, b"output data", false).expect("slave write");

        let mut out = [0u8; 32];
        let n = master_read(ptr::null_mut(), id, &mut out, false).expect("master read");
        assert_eq!(&out[..n], b"output data");

        slave_close(id);
        master_close(id);
    }

    #[test]
    fn echo_goes_to_master_buf() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();

        let id = pty_alloc().expect("alloc");
        unlock(id);
        slave_open(id).expect("slave open");

        // Master writes "a" → echo 'a' goes to master_buf
        master_write(ptr::null_mut(), id, b"a", false).expect("master write");

        let mut out = [0u8; 32];
        let n = master_read(ptr::null_mut(), id, &mut out, false).expect("master read");
        assert_eq!(&out[..n], b"a"); // echo of 'a'

        slave_close(id);
        master_close(id);
    }

    #[test]
    fn ctrl_c_through_pty() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();

        let id = pty_alloc().expect("alloc");
        unlock(id);
        slave_open(id).expect("slave open");

        // Type some chars then Ctrl-C
        master_write(ptr::null_mut(), id, b"abc", false).expect("write");
        master_write(ptr::null_mut(), id, &[0x03], false).expect("ctrl-c");

        // Slave should have no data (line was discarded)
        let mut out = [0u8; 32];
        let n = slave_read(ptr::null_mut(), id, &mut out, false).expect("read");
        assert_eq!(n, 0);

        // Master should see echo: "abc^C\r\n"
        let n = master_read(ptr::null_mut(), id, &mut out, false).expect("master read");
        assert_eq!(&out[..n], b"abc^C\r\n");

        slave_close(id);
        master_close(id);
    }

    #[test]
    fn eof_on_master_close() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();

        let id = pty_alloc().expect("alloc");
        unlock(id);
        slave_open(id).expect("slave open");

        master_close(id);

        // Slave read should return 0 (EOF)
        let mut out = [0u8; 32];
        let n = slave_read(ptr::null_mut(), id, &mut out, false).expect("read");
        assert_eq!(n, 0);

        slave_close(id);
    }

    #[test]
    fn ref_counting() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();

        let id = pty_alloc().expect("alloc");
        unlock(id);
        slave_open(id).expect("slave open");
        slave_retain(id);
        master_retain(id);

        // master_refs=2, slave_refs=2
        master_close(id); // master_refs=1
        assert!(is_in_use(id));
        slave_close(id); // slave_refs=1
        assert!(is_in_use(id));
        master_close(id); // master_refs=0
        assert!(is_in_use(id)); // slave still holds
        slave_close(id); // slave_refs=0, both zero → freed
        assert!(!is_in_use(id));
    }

    #[test]
    fn nonblocking_master_read_returns_eagain_when_empty() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();

        let id = pty_alloc().expect("alloc");
        unlock(id);
        slave_open(id).expect("slave open");

        let mut out = [0u8; 8];
        let err = master_read(ptr::null_mut(), id, &mut out, true).unwrap_err();
        assert_eq!(err, EAGAIN);

        slave_close(id);
        master_close(id);
    }

    #[test]
    fn nonblocking_slave_read_returns_eagain_when_empty() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();

        let id = pty_alloc().expect("alloc");
        unlock(id);
        slave_open(id).expect("slave open");

        let mut out = [0u8; 8];
        let err = slave_read(ptr::null_mut(), id, &mut out, true).unwrap_err();
        assert_eq!(err, EAGAIN);

        slave_close(id);
        master_close(id);
    }

    #[test]
    fn nonblocking_slave_write_returns_eagain_when_master_buf_full() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();

        let id = pty_alloc().expect("alloc");
        unlock(id);
        slave_open(id).expect("slave open");

        let fill = [0xCDu8; PTY_BUF_SIZE];
        let n = slave_write(ptr::null_mut(), id, &fill, false).expect("fill");
        assert_eq!(n, PTY_BUF_SIZE);

        let err = slave_write(ptr::null_mut(), id, b"x", true).unwrap_err();
        assert_eq!(err, EAGAIN);

        slave_close(id);
        master_close(id);
    }
}
