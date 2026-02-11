# PeetOS (Rust Port)

This repository is the Rust reimplementation of the PeetOS C kernel located in `../peetos_c`. The goal is to achieve feature parity with the existing C system, then iterate on modernization and safety.

## Architecture Snapshot

- Target: `x86_64` bare metal, higher-half kernel.
- Boot flow: 2-stage BIOS bootloader -> 64-bit long mode -> kernel entry.
- Kernel entry: `kernel/arch/x86_64/entry.asm` jumps to Rust `kmain()`.
- Linker script: `linker/kernel.ld` (reused from the C kernel).

Key references in the C project:

- Feature scope: `../peetos_c/docs/ROADMAP.md`
- Kernel subsystems: `../peetos_c/kernel/src`
- Bootloader: `../peetos_c/boot`

## Repository Layout

- `boot/` Reused stage1 and stage2 bootloader assembly.
- `kernel/` Rust kernel binary crate.
- `crates/` Subsystem crates (`arch_x86_64`, `fs`, and now `drivers` for baseline PCI enumeration are active; `net`, `syscall`, and `userabi` remain placeholders).
- `linker/` Linker script for the kernel.
- `docs/PORTING_PLAN.md` High-level implementation roadmap.
- `docs/` Implementation docs grouped by subsystem, each with a limitations/simplifications section.

## Current Status (Last reviewed: 2026-02-11)

- Boots to Rust `kmain()`, initializes serial output, prints a banner, and halts in a `hlt` loop.
- Panic handler prints over serial, then halts with `cli; hlt`.
- `arch_x86_64` now provides basic port I/O helpers and interrupt enable/disable wrappers.
- GDT/IDT setup is wired in with interrupt stubs, PIC remap, PIT timer init, syscall MSR setup, a default exception report (serial-only for now), and memory management (bitmap PMM, VMM, kernel heap).
- Kernel tasking/scheduler baseline is in place (kernel threads + PIT-driven preemption) with a minimal user-mode task loaded from an embedded ELF.
- ELF loading is now hardened with stricter header/segment validation (alignment checks, malformed `PT_INTERP` rejection, RWX-segment rejection, entrypoint-in-exec checks), and NXE is enabled so non-executable mappings are effective.
- Dynamic user bootstrap now works for a minimal `PT_INTERP` flow: a PIE main image is loaded, its interpreter (`/lib/ld-peetos.so`) is mapped, and first user RIP starts in the interpreter with `AT_ENTRY`/`AT_BASE` set.
- Syscall dispatch now uses a table-driven Linux-numbered stage-4/6 baseline (`open`, `openat`, `mkdirat`, `close`, `read`, `write`, `lseek`, `fstat`, `newfstatat`, `getdents64`, `unlinkat`, `renameat`, `rt_sigaction`, `rt_sigprocmask`, `rt_sigreturn`, `kill`, `pipe`, `pipe2`, `futex`, `shmget`, `shmat`, `shmdt`, `shmctl`, `exit`, `exit_group`, `yield`, `getpid`, `getppid`, `gettid`, `set_tid_address`, `brk`) with shared SYSCALL/int80 argument handling and per-task user metadata (`brk`, stack bounds, argv/envp/auxv pointers, ELF PHDR info, FD table state, cwd inode).
- Phase-6 IPC/sync baseline is now present in-kernel: `SpinLock`/`Mutex`/`Semaphore`, anonymous pipes backed by in-kernel ring buffers, baseline System V-style shared memory segments, futex wait/wake keying used by `SYS_FUTEX` and `clear_child_tid` wake-on-exit, plus user-handler signal delivery (`kill` + `rt_sigaction` + `rt_sigprocmask` + `rt_sigreturn`, `SA_RESTORER`-based handler entry/return, and `SIGPIPE` generation on broken pipes).
- Initial user bootstrap now builds a conventional process stack (`argc`, `argv`, `envp`, `auxv`) so libc-facing usermode setup has a stable base for future `execve` work.
- A phase-5 baseline VFS is now active with concrete tmpfs behavior, virtual `devfs`/`procfs` nodes, a mount table-backed `/proc/mounts`, and an ext2 importer mounted at `/mnt/ext2` where file and directory mutations (`create`/`mkdir`/`unlink`/`rmdir`/`rename`/`write`) update ext2 data/metadata in the mounted image buffer. VFS now attempts block-backed ext2 mount first and performs best-effort sync-back on mutating operations.
- Phase-7 PCI enumeration is active in `crates/drivers` (config mechanism #1, bus/device/function scan, bounded boot-time serial inventory output), and ATA/AHCI bring-up now includes AHCI command-engine setup/teardown, AHCI DMA `IDENTIFY`, AHCI LBA48 DMA read/write commands behind the kernel block API, ATA PIO `IDENTIFY`, and a minimal in-kernel block-device inventory.
- Baseline keyboard/TTY input is now wired in-kernel: PS/2 IRQ1 set-1 scancode decode feeds a canonical line-buffered console input path used by user `read` on stdio.
- Baseline framebuffer setup is now wired in-kernel: VESA boot handoff parsing (`status` + `mode` blocks), higher-half framebuffer window validation, and basic pixel/clear primitives.
- Remaining placeholder crates are `net`, `syscall`, and `userabi`; advanced device drivers and full user space are still pending.

## Documentation

- `docs/boot.md` Boot flow, entry sequence, and `kmain()` initialization order.
- `docs/interrupts.md` GDT/IDT setup, PIC/PIT configuration, and interrupt handling.
- `docs/memory.md` Typed addresses, PMM, VMM, and kernel heap allocator.
- `docs/elf.md` Minimal ELF loader and embedded user image bring-up.
- `docs/syscall.md` SYSCALL/MSR setup and the current syscall stub.
- `docs/tasking.md` Task structures, kernel threads, and scheduler behavior.
- `docs/vfs.md` Baseline VFS/tmpfs/devfs/procfs/ext2 design and syscall integration.
- `docs/ipc.md` Phase-6 synchronization primitives and IPC baseline (`pipe`/`futex`/`shm*`/`signal`).
- `docs/drivers.md` Phase-7 baseline PCI enumeration design and integration.
- `docs/framebuffer.md` Framebuffer boot handoff, mapping assumptions, and drawing primitives.
- `docs/diagnostics.md` Serial output, exception reporting, panic handling, and QEMU test exit.
- `docs/subsystems.md` Subsystem crate status and intended usage.

## Build Prerequisites

- `nasm`
- `ld` and `objcopy` (binutils)
- `cargo` + `rustc`
- `qemu-system-x86_64`

Optional but commonly required:

- `rust-src` component if you need to rebuild `core` for a target.
- `mke2fs` (`e2fsprogs`) if you want to regenerate `user/sample_ext2.img`.

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

Regenerate the sample ext2 filesystem image used by the kernel:

```bash
make ext2-image
```

## Testing

Initial smoke test is a QEMU boot to the kernel `kmain()` loop. Use the repo test runner:

```bash
make test
```

`make test` runs host unit tests (`arch_x86_64`, `drivers`, `fs`, `kernel`) and then boots QEMU with the `isa-debug-exit` device so the VM exits automatically after the kernel prints the serial banner.

As features land, re-run the C project shell tests for parity:

```bash
../peetos_c/tests/run_tests.sh
../peetos_c/tests/run_shell_cmds.sh -clean "echo hello" "ls /bin"
```

## Notes

- The kernel is compiled with `no_std` and `panic=abort`.
- The bootloader and linker script are reused from the C codebase to keep early bring-up stable.
- The Rust kernel currently contains a minimal `kmain()` and panic handler; subsystems will be layered in phase-by-phase per `docs/PORTING_PLAN.md`.
