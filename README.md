# PeetOS (Rust Port)

This repository is the Rust reimplementation of the PeetOS C kernel located in `../peetos`. The goal is to achieve feature parity with the existing C system, then iterate on modernization and safety.

## Architecture Snapshot

- Target: `x86_64` bare metal, higher-half kernel.
- Boot flow: 2-stage BIOS bootloader -> 64-bit long mode -> kernel entry.
- Kernel entry: `kernel/arch/x86_64/entry.asm` jumps to Rust `kmain()`.
- Linker script: `linker/kernel.ld` (reused from the C kernel).

Key references in the C project:

- Feature scope: `../peetos/docs/ROADMAP.md`
- Kernel subsystems: `../peetos/kernel/src`
- Bootloader: `../peetos/boot`

## Repository Layout

- `boot/` Reused stage1 and stage2 bootloader assembly.
- `kernel/` Rust kernel binary crate.
- `crates/` Subsystem crates (currently `arch_x86_64` provides port I/O and interrupt wrappers; others are placeholders).
- `linker/` Linker script for the kernel.
- `docs/PORTING_PLAN.md` High-level implementation roadmap.
- `docs/` Implementation docs grouped by subsystem, each with a limitations/simplifications section.

## Current Status (Last reviewed: 2026-02-08)

- Boots to Rust `kmain()`, initializes serial output, prints a banner, and halts in a `hlt` loop.
- Panic handler prints over serial, then halts with `cli; hlt`.
- `arch_x86_64` now provides basic port I/O helpers and interrupt enable/disable wrappers.
- GDT/IDT setup is wired in with interrupt stubs, PIC remap, PIT timer init, syscall MSR setup, a default exception report (serial-only for now), and memory management (bitmap PMM, VMM, kernel heap).
- Kernel tasking/scheduler baseline is in place (kernel threads + PIT-driven preemption) with a minimal user-mode task loaded from an embedded ELF.
- ELF loading is now hardened with stricter header/segment validation (alignment checks, malformed `PT_INTERP` rejection, RWX-segment rejection, entrypoint-in-exec checks), and NXE is enabled so non-executable mappings are effective.
- Dynamic user bootstrap now works for a minimal `PT_INTERP` flow: a PIE main image is loaded, its interpreter (`/lib/ld-peetos.so`) is mapped, and first user RIP starts in the interpreter with `AT_ENTRY`/`AT_BASE` set.
- Syscall dispatch now uses a table-driven Linux-numbered stage-4 baseline (`write`, `exit`, `exit_group`, `yield`, `getpid`, `getppid`, `gettid`, `set_tid_address`, `brk`) with shared SYSCALL/int80 argument handling and per-task user metadata (`brk`, stack bounds, argv/envp/auxv pointers, ELF PHDR info).
- Initial user bootstrap now builds a conventional process stack (`argc`, `argv`, `envp`, `auxv`) so libc-facing usermode setup has a stable base for future `execve` work.
- Subsystem crates (`drivers`, `fs`, `net`, `syscall`, `userabi`) are placeholders; device drivers and user space are not yet implemented.

## Documentation

- `docs/boot.md` Boot flow, entry sequence, and `kmain()` initialization order.
- `docs/interrupts.md` GDT/IDT setup, PIC/PIT configuration, and interrupt handling.
- `docs/memory.md` Typed addresses, PMM, VMM, and kernel heap allocator.
- `docs/elf.md` Minimal ELF loader and embedded user image bring-up.
- `docs/syscall.md` SYSCALL/MSR setup and the current syscall stub.
- `docs/tasking.md` Task structures, kernel threads, and scheduler behavior.
- `docs/diagnostics.md` Serial output, exception reporting, panic handling, and QEMU test exit.
- `docs/subsystems.md` Placeholder subsystem crates and intended usage.

## Build Prerequisites

- `nasm`
- `ld` and `objcopy` (binutils)
- `cargo` + `rustc`
- `qemu-system-x86_64`

Optional but commonly required:

- `rust-src` component if you need to rebuild `core` for a target.

## Build

The build is driven by the Makefile, which assembles the bootloader, compiles the kernel, and creates a bootable disk image.

```bash
make image
```

Build a debug kernel:

```bash
make image PROFILE=debug
```

Run in QEMU with serial output:

```bash
make run
```

## Testing

Initial smoke test is a QEMU boot to the kernel `kmain()` loop. Use the repo test runner:

```bash
make test
```

`make test` runs host unit tests (currently serial formatting) and then boots QEMU with the `isa-debug-exit` device so the VM exits automatically after the kernel prints the serial banner.

As features land, re-run the C project shell tests for parity:

```bash
../peetos/tests/run_tests.sh
../peetos/tests/run_shell_cmds.sh -clean "echo hello" "ls /bin"
```

## Notes

- The kernel is compiled with `no_std` and `panic=abort`.
- The bootloader and linker script are reused from the C codebase to keep early bring-up stable.
- The Rust kernel currently contains a minimal `kmain()` and panic handler; subsystems will be layered in phase-by-phase per `docs/PORTING_PLAN.md`.
