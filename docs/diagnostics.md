# Diagnostics and Debug Output

This document covers serial output, exception reporting, and test-only QEMU exit handling.

**Where It Lives**
- `kernel/src/serial.rs`
- `kernel/src/exception.rs`
- `kernel/src/main.rs` (panic handler)
- `kernel/src/qemu.rs`
- `crates/arch_x86_64/src/port.rs`

**Serial Console**
The serial console is configured for COM1 (`0x3F8`) at 38400 baud with 8N1 framing. Output is synchronous and translates `\n` to CRLF. Macros `serial_print!` and `serial_println!` wrap the formatter.
When framebuffer mode is active, serial bytes are also mirrored into a simple on-screen text grid (`kernel/src/fb_console.rs`) so output is visible in the QEMU display window.

Limitations / Simplifications
- Busy-wait transmit loop; no interrupts or buffering.
- No serial RX input path, flow control, or log levels.
- No locking for concurrent writers.

Keyboard-backed console input is now provided separately via the phase-7 PS/2 + TTY path (`kernel/src/keyboard.rs` + `kernel/src/tty.rs`), with typed bytes echoed through the same serial path (and therefore mirrored to framebuffer when available).

**Exception Reporting**
`kernel/src/exception.rs` prints a register dump and decodes page fault error codes. For page faults, it reads CR2 and prints the access type, privilege level, and present/reserved bits. Optional VGA printing exists behind the `vga` feature.

Limitations / Simplifications
- Reporting is serial-only by default and provides no stack traces or symbolization.
- Exceptions are treated as fatal and halt the CPU.
- Assumes the `InterruptFrame` layout used by the IDT stubs.

**Panic Handler**
The panic handler in `kernel/src/main.rs` initializes the serial port (if needed), prints the panic message, then halts with interrupts disabled.

Limitations / Simplifications
- No unwind or recovery path; the kernel halts on panic.
- No crash dump persistence or structured panic metadata yet.

**QEMU Test Exit**
`kernel/src/qemu.rs` writes a magic value to port `0xF4` (isa-debug-exit) to signal success during smoke tests. This is gated behind the `test-exit` feature and only used in test builds.

Limitations / Simplifications
- Only works under QEMU with `isa-debug-exit` configured.
- Exit codes are fixed to the success path only.
