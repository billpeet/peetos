#![allow(dead_code)]

use core::fmt;
use core::fmt::Write;

use arch_x86_64::port::{inb, outb};

const COM1_BASE: u16 = 0x3F8;

fn write_str_with_crlf<F: FnMut(u8)>(s: &str, mut write: F) {
    for byte in s.bytes() {
        if byte == b'\n' {
            write(b'\r');
        }
        write(byte);
    }
}

pub struct SerialPort {
    base: u16,
    initialized: bool,
}

impl SerialPort {
    pub const fn new(base: u16) -> Self {
        Self {
            base,
            initialized: false,
        }
    }

    pub fn init(&mut self) {
        unsafe {
            // Disable interrupts
            outb(self.base + 1, 0x00);
            // Enable DLAB
            outb(self.base + 3, 0x80);
            // Set divisor to 3 (38400 baud)
            outb(self.base + 0, 0x03);
            outb(self.base + 1, 0x00);
            // 8 bits, no parity, one stop bit
            outb(self.base + 3, 0x03);
            // Enable FIFO, clear them, 14-byte threshold
            outb(self.base + 2, 0xC7);
            // IRQs enabled, RTS/DSR set
            outb(self.base + 4, 0x0B);
        }

        self.initialized = true;
    }

    fn ensure_init(&mut self) {
        if !self.initialized {
            self.init();
        }
    }

    fn is_transmit_ready(&self) -> bool {
        unsafe { (inb(self.base + 5) & 0x20) != 0 }
    }

    fn write_byte(&mut self, byte: u8) {
        self.ensure_init();
        while !self.is_transmit_ready() {}
        unsafe { outb(self.base, byte) };
    }
}

impl Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.ensure_init();
        write_str_with_crlf(s, |byte| self.write_byte(byte));
        Ok(())
    }
}

static mut SERIAL1: SerialPort = SerialPort::new(COM1_BASE);

pub fn init() {
    unsafe { SERIAL1.init() };
}

pub fn _print(args: fmt::Arguments) {
    unsafe {
        let _ = SERIAL1.write_fmt(args);
    }
}

/// ANSI escape stripping state for serial output.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum AnsiFilterState {
    Normal = 0,
    Esc = 1,
    Csi = 2,
}

/// Returns true if the byte should be suppressed (part of an ANSI escape).
fn ansi_filter(state: &mut AnsiFilterState, byte: u8) -> bool {
    match *state {
        AnsiFilterState::Normal => {
            if byte == 0x1B {
                *state = AnsiFilterState::Esc;
                return true;
            }
            false
        }
        AnsiFilterState::Esc => {
            if byte == b'[' {
                *state = AnsiFilterState::Csi;
                return true;
            }
            // Not a CSI, emit the byte normally
            *state = AnsiFilterState::Normal;
            false
        }
        AnsiFilterState::Csi => {
            // CSI: drop all bytes until a final letter (0x40-0x7E)
            if byte >= 0x40 && byte <= 0x7E {
                *state = AnsiFilterState::Normal;
            }
            true
        }
    }
}

pub fn write_bytes(bytes: &[u8]) {
    // Send raw bytes to framebuffer console (ANSI-aware rendering)
    crate::fb_console::write_bytes(bytes);

    // Send filtered bytes to serial (ANSI escapes stripped)
    let mut filter_state = AnsiFilterState::Normal;
    unsafe {
        for &byte in bytes {
            if ansi_filter(&mut filter_state, byte) {
                continue;
            }
            if byte == b'\n' {
                SERIAL1.write_byte(b'\r');
            }
            SERIAL1.write_byte(byte);
        }
    }
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::serial::_print(core::format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($fmt:expr) => ($crate::serial_print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => ($crate::serial_print!(
        concat!($fmt, "\n"), $($arg)*
    ));
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::{write_str_with_crlf, ansi_filter, AnsiFilterState};

    #[test]
    fn crlf_translation() {
        let mut out = std::vec::Vec::new();
        write_str_with_crlf("hi\nthere", |byte| out.push(byte));
        assert_eq!(out, b"hi\r\nthere");
    }

    #[test]
    fn empty_string_is_noop() {
        let mut out = std::vec::Vec::new();
        write_str_with_crlf("", |byte| out.push(byte));
        assert!(out.is_empty());
    }

    #[test]
    fn passthrough_without_newlines() {
        let mut out = std::vec::Vec::new();
        write_str_with_crlf("serial", |byte| out.push(byte));
        assert_eq!(out, b"serial");
    }

    #[test]
    fn ansi_filter_strips_csi_sequences() {
        let input = b"\x1b[2Jhello\x1b[1;32mworld";
        let mut state = AnsiFilterState::Normal;
        let mut out = std::vec::Vec::new();
        for &b in input {
            if !ansi_filter(&mut state, b) {
                out.push(b);
            }
        }
        assert_eq!(out, b"helloworld");
    }

    #[test]
    fn ansi_filter_passes_non_csi_after_esc() {
        let input = b"\x1bXhello";
        let mut state = AnsiFilterState::Normal;
        let mut out = std::vec::Vec::new();
        for &b in input {
            if !ansi_filter(&mut state, b) {
                out.push(b);
            }
        }
        // ESC is suppressed, X falls through, hello passes
        assert_eq!(out, b"Xhello");
    }
}
