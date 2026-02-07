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

    use super::write_str_with_crlf;

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
}
