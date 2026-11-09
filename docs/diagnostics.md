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
When framebuffer mode is active, serial bytes are also mirrored into an on-screen styled text grid (`kernel/src/fb_console.rs`) using 8x16 bitmap glyphs, baseline ANSI 16-color handling, and a PIT-driven blinking cursor so output is visible in the QEMU display window.

Limitations / Simplifications
- Busy-wait transmit loop; no interrupts or buffering.
- No serial RX input path, flow control, or log levels.
- No locking for concurrent writers.

Keyboard-backed console input is now provided separately via the PS/2 + TTY path (`kernel/src/keyboard.rs` + `kernel/src/tty.rs`), with typed bytes echoed through the same serial path (and therefore mirrored to framebuffer when available).

**Exception Reporting**
`kernel/src/exception.rs` prints a register dump and decodes page fault error codes. For page faults, it reads CR2 and prints the access type, privilege level, and present/reserved bits. Optional VGA printing exists behind the `vga` feature.

A fatal exception (one `exception::handle` doesn't resolve) is reported in this order:
1. **Serial first.** The task (pid, name, last syscall number and arguments) and the register dump go out before anything touches the framebuffer console. Redrawing the console is the riskiest step, and the original fault must not be lost if it fails.
2. **Kernel-mode faults** also report whether any process maps the frame holding the faulting RSP, the 128 bytes at RSP, and every task's kernel-stack high-water mark (see `docs/tasking.md`).
3. **Console.** The console is then activated with `fb_console::activate_for_fatal_report`, which only uses the console lock if it is free. The faulting code may hold it, and waiting with interrupts off would deadlock. That used to turn crashes into silent hangs. If the lock is busy, the report names its holder (function, caller `file:line`, pid) and later console writes skip the console.
4. **Nested faults** during reporting print a note and skip the console entirely.

Limitations / Simplifications
- Reporting is serial-only by default and provides no stack traces or symbolization (resolve addresses with `addr2line -e build/kernel.elf`).
- Exceptions are treated as fatal and halt the CPU.
- Assumes the `InterruptFrame` layout used by the IDT stubs.

**Panic Handler**
The panic handler in `kernel/src/main.rs` initializes the serial port (if needed), activates the console without waiting for its lock (as above), prints the panic message, then halts with interrupts disabled.

Limitations / Simplifications
- No unwind or recovery path; the kernel halts on panic.
- No crash dump persistence or structured panic metadata yet.

**Scheduler Block Tracing**
Building with the `syscall-trace` feature logs every table-dispatched syscall as `[STRACE] <pid> name(args)` and its result as `[STRACE] <pid> name -> ret`, a minimal strace for working out where a program stops (`DBUS_SMOKE_EXTRA_FEATURES=syscall-trace make run-dbus-smoke`). It is very noisy and off by default. The lock-free fast-path syscalls (`clock_gettime`, `getpid`, ...) are not traced.

Building with the `sched-trace` feature makes `sched::block` print `[SCHED] Blocking task '<name>' (<reason>)` every time a task blocks (for example `pty-sread`, `futex-wait`, `pipe-read`). It is useful for working out where a task is stuck, but it fires on every keystroke, futex wait and frame in an interactive session, so it is off by default and left out of `make run`'s feature set. Enable it explicitly, for example `make run-current` after `make image KERNEL_FEATURES="... sched-trace"`. `make run-foot-smoke` fails if the line appears in a build without the feature.

Limitations / Simplifications
- All-or-nothing: there is no per-task or per-reason filter and no rate limit.
- Output goes through the synchronous serial path, so tracing slows the system and changes its timing.

**QEMU Test Exit**
`kernel/src/qemu.rs` writes a magic value to port `0xF4` (isa-debug-exit) to signal success during smoke tests. This is gated behind the `test-exit` feature and only used in test builds.

Limitations / Simplifications
- Only works under QEMU with `isa-debug-exit` configured.
- Exit codes are fixed to the success path only.
