use core::ptr;

use crate::sync::SpinLock;
use crate::task::Task;

pub const EAGAIN: i64 = 11;
pub const ENOSPC: i64 = 28;

pub const TTY_FLAG_ECHO: u32 = 0x01;
pub const TTY_FLAG_CANONICAL: u32 = 0x02;
pub const TTY_FLAG_ISIG: u32 = 0x04;
pub const TTY_FLAG_DEFAULT: u32 = TTY_FLAG_ECHO | TTY_FLAG_CANONICAL | TTY_FLAG_ISIG;

const TTY_INPUT_CAPACITY: usize = 2048;
const TTY_LINE_CAPACITY: usize = 256;
const TTY_MAX_WAITERS: usize = 8;
const TTY_WAKE_BATCH: usize = 8;

pub const ECHO_BUF_MAX: usize = 8;

// c_lflag bits (Linux standard values)
pub const LFLAG_ISIG: u32 = 0o0000001;
pub const LFLAG_ICANON: u32 = 0o0000002;
pub const LFLAG_ECHO: u32 = 0o0000010;
pub const LFLAG_ECHOE: u32 = 0o0000020;
pub const LFLAG_ECHOK: u32 = 0o0000040;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct LinuxTermios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_line: u8,
    pub c_cc: [u8; 19],
}

pub fn default_termios() -> LinuxTermios {
    let mut tio = LinuxTermios {
        c_iflag: 0,
        c_oflag: 0,
        c_cflag: 0,
        c_lflag: 0,
        c_line: 0,
        c_cc: [0; 19],
    };

    tio.c_iflag = 0o0000400 | 0o0002000; // ICRNL | IXON
    tio.c_oflag = 0o0000001 | 0o0000004; // OPOST | ONLCR
    tio.c_cflag = 0o0000060 | 0o0000400 | 0o0000600 | 0o0040000; // CS8 | CREAD | B38400 | CLOCAL
    tio.c_lflag = LFLAG_ISIG | LFLAG_ICANON | LFLAG_ECHO | LFLAG_ECHOE | LFLAG_ECHOK;
    tio.c_cc[0] = 3; // VINTR
    tio.c_cc[1] = 28; // VQUIT
    tio.c_cc[2] = 127; // VERASE
    tio.c_cc[3] = 21; // VKILL
    tio.c_cc[4] = 4; // VEOF
    tio.c_cc[5] = 0; // VTIME
    tio.c_cc[6] = 1; // VMIN
    tio.c_cc[8] = 17; // VSTART
    tio.c_cc[9] = 19; // VSTOP
    tio.c_cc[10] = 26; // VSUSP
    tio
}

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TtyOutputKind {
    Console = 0,
    Pty = 1,
}

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

pub struct IngestResult {
    pub echo_buf: [u8; ECHO_BUF_MAX],
    pub echo_len: u8,
    pub wake: bool,
    pub signal: u32,
}

pub struct Tty {
    line_len: u16,
    line_buf: [u8; TTY_LINE_CAPACITY],
    input_head: u16,
    input_tail: u16,
    input_len: u16,
    input_buf: [u8; TTY_INPUT_CAPACITY],
    pub(crate) flags: u32,
    pub(crate) eof_pending: bool,
    pub(crate) termios: LinuxTermios,
    pub(crate) output_kind: TtyOutputKind,
    pub(crate) output_backend_id: u16,
    waiters: [Waiter; TTY_MAX_WAITERS],
}

impl Tty {
    pub const fn new() -> Self {
        Self {
            line_len: 0,
            line_buf: [0; TTY_LINE_CAPACITY],
            input_head: 0,
            input_tail: 0,
            input_len: 0,
            input_buf: [0; TTY_INPUT_CAPACITY],
            flags: TTY_FLAG_DEFAULT,
            eof_pending: false,
            termios: LinuxTermios {
                c_iflag: 0,
                c_oflag: 0,
                c_cflag: 0,
                c_lflag: 0,
                c_line: 0,
                c_cc: [0; 19],
            },
            output_kind: TtyOutputKind::Console,
            output_backend_id: 0,
            waiters: [Waiter::empty(); TTY_MAX_WAITERS],
        }
    }

    pub fn init_defaults(&mut self) {
        self.flags = TTY_FLAG_DEFAULT;
        self.termios = default_termios();
    }

    pub fn new_for_pty(pty_id: u16) -> Self {
        let mut tty = Self::new();
        tty.output_kind = TtyOutputKind::Pty;
        tty.output_backend_id = pty_id;
        tty.flags = TTY_FLAG_DEFAULT;
        tty.termios = default_termios();
        tty
    }

    fn input_push_byte(&mut self, byte: u8) -> bool {
        if self.input_len as usize >= TTY_INPUT_CAPACITY {
            return false;
        }
        self.input_buf[self.input_head as usize] = byte;
        self.input_head = ((self.input_head as usize + 1) % TTY_INPUT_CAPACITY) as u16;
        self.input_len += 1;
        true
    }

    pub fn input_pop(&mut self, out: &mut [u8]) -> usize {
        if out.is_empty() || self.input_len == 0 {
            return 0;
        }
        let mut count = 0usize;
        let max = core::cmp::min(out.len(), self.input_len as usize);
        while count < max {
            out[count] = self.input_buf[self.input_tail as usize];
            self.input_tail = ((self.input_tail as usize + 1) % TTY_INPUT_CAPACITY) as u16;
            self.input_len -= 1;
            count += 1;
        }
        count
    }

    fn commit_line(&mut self) -> usize {
        if self.line_len == 0 {
            return 0;
        }
        let mut committed = 0usize;
        let mut idx = 0usize;
        while idx < self.line_len as usize {
            if !self.input_push_byte(self.line_buf[idx]) {
                break;
            }
            committed += 1;
            idx += 1;
        }
        self.line_len = 0;
        committed
    }

    pub fn has_ready_data(&self) -> bool {
        self.input_len > 0 || self.eof_pending
    }

    pub fn ingest_byte_locked(&mut self, byte: u8) -> IngestResult {
        let mut result = IngestResult {
            echo_buf: [0; ECHO_BUF_MAX],
            echo_len: 0,
            wake: false,
            signal: 0,
        };

        let echo_on = (self.flags & TTY_FLAG_ECHO) != 0;
        let canonical = (self.flags & TTY_FLAG_CANONICAL) != 0;
        let isig = (self.flags & TTY_FLAG_ISIG) != 0;

        match byte {
            0x03 if isig => {
                if echo_on {
                    result.echo_buf[0] = b'^';
                    result.echo_buf[1] = b'C';
                    result.echo_buf[2] = b'\r';
                    result.echo_buf[3] = b'\n';
                    result.echo_len = 4;
                }
                self.line_len = 0;
                result.signal = 2; // SIGINT
                result.wake = true;
            }
            0x04 if canonical => {
                if self.line_len == 0 {
                    self.eof_pending = true;
                    result.wake = true;
                } else {
                    result.wake = self.commit_line() > 0;
                }
            }
            b'\r' | b'\n' => {
                if canonical {
                    if self.line_len as usize >= TTY_LINE_CAPACITY {
                        self.line_len = (TTY_LINE_CAPACITY - 1) as u16;
                    }
                    let idx = self.line_len as usize;
                    self.line_buf[idx] = b'\n';
                    self.line_len += 1;
                    result.wake = self.commit_line() > 0;
                } else {
                    self.input_push_byte(byte);
                    result.wake = true;
                }
                if echo_on {
                    result.echo_buf[0] = b'\r';
                    result.echo_buf[1] = b'\n';
                    result.echo_len = 2;
                }
            }
            0x08 | 0x7F => {
                if canonical {
                    if self.line_len > 0 {
                        self.line_len -= 1;
                        if echo_on {
                            result.echo_buf[0] = 0x08;
                            result.echo_buf[1] = b' ';
                            result.echo_buf[2] = 0x08;
                            result.echo_len = 3;
                        }
                    }
                } else {
                    self.input_push_byte(byte);
                    result.wake = true;
                    if echo_on {
                        result.echo_buf[0] = byte;
                        result.echo_len = 1;
                    }
                }
            }
            b'\t' | 0x20..=0x7E => {
                if canonical {
                    if self.line_len as usize + 1 < TTY_LINE_CAPACITY {
                        let idx = self.line_len as usize;
                        self.line_buf[idx] = byte;
                        self.line_len += 1;
                        if echo_on {
                            result.echo_buf[0] = byte;
                            result.echo_len = 1;
                        }
                    }
                } else {
                    self.input_push_byte(byte);
                    result.wake = true;
                    if echo_on {
                        result.echo_buf[0] = byte;
                        result.echo_len = 1;
                    }
                }
            }
            _ => {
                if !canonical {
                    self.input_push_byte(byte);
                    result.wake = true;
                }
            }
        }

        result
    }

    pub fn register_waiter(&mut self, task: *mut Task) -> Result<(), i64> {
        if task.is_null() {
            return Err(EAGAIN);
        }
        let mut idx = 0usize;
        while idx < TTY_MAX_WAITERS {
            let waiter = &self.waiters[idx];
            if waiter.used && waiter.task == task as usize {
                return Ok(());
            }
            idx += 1;
        }
        idx = 0;
        while idx < TTY_MAX_WAITERS {
            if !self.waiters[idx].used {
                self.waiters[idx] = Waiter {
                    used: true,
                    task: task as usize,
                };
                return Ok(());
            }
            idx += 1;
        }
        Err(ENOSPC)
    }

    pub fn remove_waiter(&mut self, task: *mut Task) {
        let mut idx = 0usize;
        while idx < TTY_MAX_WAITERS {
            let waiter = &mut self.waiters[idx];
            if waiter.used && waiter.task == task as usize {
                *waiter = Waiter::empty();
            }
            idx += 1;
        }
    }

    pub fn collect_waiters(&mut self, out: &mut [*mut Task; TTY_WAKE_BATCH]) -> usize {
        let mut count = 0usize;
        let mut idx = 0usize;
        while idx < TTY_MAX_WAITERS && count < out.len() {
            let waiter = &mut self.waiters[idx];
            if waiter.used && waiter.task != 0 {
                out[count] = waiter.task as *mut Task;
                *waiter = Waiter::empty();
                count += 1;
            }
            idx += 1;
        }
        count
    }

    pub fn apply_termios(&mut self, tio: &LinuxTermios) {
        self.termios = *tio;
        self.flags = 0;
        if (tio.c_lflag & LFLAG_ECHO) != 0 {
            self.flags |= TTY_FLAG_ECHO;
        }
        if (tio.c_lflag & LFLAG_ICANON) != 0 {
            self.flags |= TTY_FLAG_CANONICAL;
        }
        if (tio.c_lflag & LFLAG_ISIG) != 0 {
            self.flags |= TTY_FLAG_ISIG;
        }
    }
}

static CONSOLE_TTY: SpinLock<Tty> = SpinLock::new(Tty::new());

pub fn init() {
    let mut tty = CONSOLE_TTY.lock();
    *tty = Tty::new();
    tty.init_defaults();
}

pub fn ingest_byte(byte: u8) {
    let result = {
        let mut tty = CONSOLE_TTY.lock();
        tty.ingest_byte_locked(byte)
    };

    if result.echo_len > 0 {
        emit_echo_bytes(&result.echo_buf[..result.echo_len as usize]);
    }
    if result.signal != 0 {
        emit_signal(result.signal);
    }
    if result.wake {
        wake_waiters();
    }
}

pub fn has_ready_data() -> bool {
    let tty = CONSOLE_TTY.lock();
    tty.input_len > 0
}

pub fn read_nonblocking(out: &mut [u8]) -> usize {
    let mut tty = CONSOLE_TTY.lock();
    tty.input_pop(out)
}

pub fn read_console(task: *mut Task, out: &mut [u8]) -> Result<usize, i64> {
    if out.is_empty() {
        return Ok(0);
    }

    loop {
        {
            let mut tty = CONSOLE_TTY.lock();

            if tty.eof_pending {
                tty.eof_pending = false;
                return Ok(0);
            }

            let read_n = tty.input_pop(out);
            if read_n > 0 {
                return Ok(read_n);
            }

            if !can_block(task) {
                return Ok(0);
            }

            tty.register_waiter(task)?;
        }

        block_current("tty-read");
        {
            let mut tty = CONSOLE_TTY.lock();
            tty.remove_waiter(task);
        }
        if crate::signal::must_abort_wait(task) {
            return Err(EAGAIN);
        }
    }
}

pub fn console_termios() -> LinuxTermios {
    let tty = CONSOLE_TTY.lock();
    tty.termios
}

pub fn set_console_termios(tio: &LinuxTermios) {
    let mut tty = CONSOLE_TTY.lock();
    tty.apply_termios(tio);
}

fn wake_waiters() {
    loop {
        let mut tasks = [ptr::null_mut::<Task>(); TTY_WAKE_BATCH];
        let woke = {
            let mut tty = CONSOLE_TTY.lock();
            tty.collect_waiters(&mut tasks)
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
fn emit_echo_bytes(bytes: &[u8]) {
    crate::serial::write_bytes(bytes);
}

#[cfg(test)]
fn emit_echo_bytes(_bytes: &[u8]) {}

#[cfg(not(test))]
fn emit_signal(signum: u32) {
    crate::signal::send_to_foreground_group(0, signum);
}

#[cfg(test)]
fn emit_signal(_signum: u32) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn line_is_committed_on_newline() {
        let _guard = TEST_LOCK.lock().expect("test lock");
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
        let _guard = TEST_LOCK.lock().expect("test lock");
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
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();

        ingest_byte(b'x');
        ingest_byte(b'y');
        let mut out = [0u8; 8];
        assert_eq!(read_nonblocking(&mut out), 0);

        ingest_byte(b'\r');
        let n = read_nonblocking(&mut out);
        assert_eq!(&out[..n], b"xy\n");
    }

    #[test]
    fn ctrl_c_clears_line_and_no_ready_data() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();

        ingest_byte(b'a');
        ingest_byte(b'b');
        ingest_byte(0x03);
        assert!(!has_ready_data());
    }

    #[test]
    fn ctrl_d_on_empty_line_sets_eof() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();

        ingest_byte(0x04);
        {
            let tty = CONSOLE_TTY.lock();
            assert!(tty.eof_pending);
        }
    }

    #[test]
    fn ctrl_d_on_non_empty_line_flushes() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();

        ingest_byte(b'h');
        ingest_byte(b'i');
        ingest_byte(0x04);

        let mut out = [0u8; 8];
        let n = read_nonblocking(&mut out);
        assert_eq!(&out[..n], b"hi");
    }

    #[test]
    fn echo_suppressed_when_flag_cleared() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        init();
        {
            let mut tty = CONSOLE_TTY.lock();
            tty.flags &= !TTY_FLAG_ECHO;
        }
        ingest_byte(b'a');
        ingest_byte(b'\n');
        let mut out = [0u8; 8];
        let n = read_nonblocking(&mut out);
        assert_eq!(&out[..n], b"a\n");
    }
}
