# PeetOS

This repository hosts the primary Rust implementation of the PeetOS kernel.

The older C tree in `../peetos_c` is still useful as a historical reference, but this project is now focused on documenting and extending the Rust kernel directly.

## Architecture Snapshot

- Target: `x86_64` bare metal, higher-half kernel.
- Boot flow: 2-stage BIOS bootloader -> 64-bit long mode -> kernel entry.
- Kernel entry: `kernel/arch/x86_64/entry.asm` jumps to Rust `kmain()`.
- Linker script: `linker/kernel.ld` (shared with the historical C project).

## Repository Layout

- `boot/` Stage1/stage2 bootloader assembly.
- `kernel/` Rust kernel binary crate.
- `crates/` Subsystem crates (`arch_x86_64`, `fs`, and `drivers` are active; `net`, `syscall`, and `userabi` are placeholders).
- `musl/` musl source/sysroot bootstrap for userspace bring-up.
- `toybox/` toybox source/bootstrap glue for staged userspace utilities.
- `quickjs/` quickjs source/bootstrap glue for JavaScript runtime smoke coverage.
- `micropython/` micropython source/bootstrap glue for Python runtime smoke coverage.
- `dropbear/` dropbear source/bootstrap glue for SSH runtime smoke coverage.
- `linker/` Linker script for the kernel.
- `docs/PORTING_PLAN.md` Current status and forward roadmap.
- `docs/` Subsystem documentation.

## Current Status (Last reviewed: 2026-02-28)

- Boots to Rust `kmain()`, initializes serial output, prints a banner, and halts in a `hlt` loop.
- Panic handler prints over serial, then halts with `cli; hlt`.
- `arch_x86_64` provides port I/O helpers and interrupt enable/disable wrappers.
- GDT/IDT setup is wired with interrupt stubs, PIC remap, PIT timer init, syscall MSR setup, and serial exception reports.
- PMM/VMM/kernel heap are active.
- Tasking/scheduler baseline is in place (kernel threads + PIT-driven preemption).
- ELF loading is hardened (header/segment validation, malformed `PT_INTERP` rejection, RWX rejection, entrypoint checks) with NXE enabled.
- User bootstrap supports a minimal `PT_INTERP` flow (`AT_ENTRY`/`AT_BASE`) and Linux-style initial stack (`argc`/`argv`/`envp`/`auxv`).
- Syscall dispatch is table-driven and shared by SYSCALL/int80 paths. Implemented syscalls include file/VFS operations, process control (`fork`/`clone`/`vfork`/`wait4`), VM mapping (`mmap`/`mprotect`/`munmap`), IPC (`pipe`/`futex`/`shm*`), signals (`rt_sigaction`/`rt_sigprocmask`/`rt_sigreturn`/`kill`), and core identity/time/process-group calls.
- File-backed `MAP_SHARED` mappings support dirty tracking and writeback via `msync`/`sync`/`fsync`; user-page allocation includes a best-effort reclaim pass for reclaim-safe file-backed pages.
- VFS now mounts block-backed ext2/ext3 at `/` (root) with Linux-like mountpoints (`/boot`, `/temp`, `/dev`, `/proc`), with `/etc/fstab` declaring the root block mount as `ext3` and preserving a `/mnt/ext2` compatibility path alias when root ext2 is active.
- ext2 includes mutable file/dir operations, direct+indirect block mapping (single/double/triple), truncate growth/shrink, lazy directory population, sparse block cache with CLOCK eviction, and block-cache sync/flush integration.
- Journaled ext mounts are surfaced as `ext3` and include internal-journal parse/replay + runtime descriptor/data/commit/revoke emission with wrap-aware sequencing, plus baseline runtime abort-on-I/O-fault handling.
- PCI enumeration is active in `crates/drivers`; storage bring-up includes AHCI DMA `IDENTIFY`, AHCI LBA48 DMA read/write, ATA PIO `IDENTIFY`, and block-device inventory.
- Keyboard/TTY input, PS/2 mouse cursor movement, optional UHCI USB tablet absolute input, and framebuffer text mirroring are wired.
- Userspace bootstrap is active via musl + toybox, with runtime smoke paths for shell, ping, QuickJS, MicroPython, and Dropbear.

## Documentation

- `docs/boot.md` Boot flow, entry sequence, and `kmain()` initialization order.
- `docs/interrupts.md` GDT/IDT setup, PIC/PIT configuration, and interrupt handling.
- `docs/memory.md` Typed addresses, PMM, VMM, and kernel heap allocator.
- `docs/elf.md` ELF loading and initial userspace image handoff.
- `docs/syscall.md` SYSCALL/MSR setup and dispatcher behavior.
- `docs/tasking.md` Task structures, user bootstrap, and scheduler behavior.
- `docs/vfs.md` VFS/tmpfs/devfs/procfs/ext2/ext3-journal path and syscall integration.
- `docs/ipc.md` Synchronization primitives and IPC baseline (`pipe`/`futex`/`shm*`/`signal`).
- `docs/drivers.md` PCI/storage/input/framebuffer driver status.
- `docs/framebuffer.md` Framebuffer handoff, mapping model, console rendering, and cursor path.
- `docs/diagnostics.md` Serial output, exception reporting, panic handling, and QEMU test exit.
- `docs/subsystems.md` Subsystem crate status and intended usage.
- `docs/musl.md` musl bootstrap workflow and current limitations.
- `docs/toybox.md` toybox bootstrap workflow and staged applet coverage.
- `docs/quickjs.md` quickjs bootstrap workflow and current limitations.
- `docs/micropython.md` micropython bootstrap workflow and current limitations.
- `docs/dropbear.md` dropbear bootstrap workflow and current limitations.
- `docs/PORTING_PLAN.md` consolidated implementation status and roadmap.

## Build Prerequisites

- `nasm`
- `ld` and `objcopy` (binutils)
- `cargo` + `rustc`
- `qemu-system-x86_64`

Optional but commonly required:

- `rust-src` component if you need to rebuild `core` for a target.
- `mke2fs` (`e2fsprogs`) to regenerate `user/sample_ext2.img`.
- `curl` or `wget` for fetching musl/toybox/quickjs/micropython/dropbear sources.

## Build and Run

Build the bootable image:

```bash
make image
```

Debug build:

```bash
make image PROFILE=debug
```

Run in QEMU with serial output:

```bash
make run
```

Notes:

- `make run` builds and boots a full runtime profile by enabling all kernel features except `test-exit`, `vga`, and `*-smoke` feature gates.
- AHCI disk0 is `build/peetos.img`; AHCI disk1 is `user/sample_ext2_musl.img`.
- Use `make run-current` to boot the current image without rebuilding.
- Use `make run-image` to force rebuild then boot.

Regenerate the sample ext2 image used by host-side fs tests:

```bash
make ext2-image
```

## Userspace Bring-Up Commands

musl:

```bash
make musl-fetch
make musl-configure
make musl
```

toybox:

```bash
make toybox-fetch
make toybox
scripts/build_toybox_sh.sh
scripts/build_toybox_core.sh
scripts/run_toybox_next_smoke.sh
scripts/run_toybox_ping_smoke.sh
```

quickjs:

```bash
make quickjs-fetch
make quickjs
scripts/run_toybox_quickjs_smoke.sh
```

micropython:

```bash
make micropython-fetch
make micropython
scripts/run_toybox_micropython_smoke.sh
```

dropbear:

```bash
make dropbear-fetch
make dropbear
scripts/run_toybox_dropbear_smoke.sh
```

Shell shortcut targets:

```bash
make run-shell
make run-shell-smoke
make run-tablet
make run-ping-smoke
make run-quickjs-smoke
make run-micropython-smoke
make run-dropbear-smoke
```

## Testing

Run the repository test harness:

```bash
make test
```

`make test` runs host unit tests (`arch_x86_64`, `drivers`, `fs`, `kernel`) and then boots QEMU with `isa-debug-exit` so the VM exits automatically after the serial smoke path.

## Historical Reference

If you need behavior comparisons with the earlier implementation, use:

```bash
../peetos_c/tests/run_tests.sh
../peetos_c/tests/run_shell_cmds.sh -clean "echo hello" "ls /bin"
```

## Notes

- The kernel is compiled with `no_std` and `panic=abort`.
- The bootloader and linker script are reused from the C codebase for stable early boot.
