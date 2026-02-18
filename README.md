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
- `musl/` Stage-9 musl source/bootstrap build glue for user-space runtime bring-up.
- `toybox/` Stage-9 toybox source/bootstrap build glue for staged userspace utilities.
- `quickjs/` Stage-9 quickjs source/bootstrap build glue for staged JavaScript runtime smoke.
- `micropython/` Stage-9 micropython source/bootstrap build glue for staged Python runtime smoke.
- `linker/` Linker script for the kernel.
- `docs/PORTING_PLAN.md` High-level implementation roadmap.
- `docs/` Implementation docs grouped by subsystem, each with a limitations/simplifications section.

## Current Status (Last reviewed: 2026-02-18)

- Boots to Rust `kmain()`, initializes serial output, prints a banner, and halts in a `hlt` loop.
- Panic handler prints over serial, then halts with `cli; hlt`.
- `arch_x86_64` now provides basic port I/O helpers and interrupt enable/disable wrappers.
- GDT/IDT setup is wired in with interrupt stubs, PIC remap, PIT timer init, syscall MSR setup, a default exception report (serial-only for now), and memory management (bitmap PMM, VMM, kernel heap).
- Kernel tasking/scheduler baseline is in place (kernel threads + PIT-driven preemption) with a minimal user-mode task loaded from an embedded ELF.
- ELF loading is now hardened with stricter header/segment validation (alignment checks, malformed `PT_INTERP` rejection, RWX-segment rejection, entrypoint-in-exec checks), and NXE is enabled so non-executable mappings are effective.
- Dynamic user bootstrap now works for a minimal `PT_INTERP` flow: a PIE main image is loaded, its interpreter (`/lib/ld-peetos.so`) is mapped, and first user RIP starts in the interpreter with `AT_ENTRY`/`AT_BASE` set.
- Syscall dispatch now uses a table-driven Linux-numbered stage-4/6 baseline (`open`, `openat`, `mkdirat`, `close`, `read`, `write`, `writev`, `lseek`, `ioctl`, `stat`, `lstat`, `fstat`, `newfstatat`, `getdents64`, `unlinkat`, `renameat`, `mmap`, `mprotect`, `munmap`, `clone`, `fork`, `vfork`, `wait4`, `uname`, `rt_sigaction`, `rt_sigprocmask`, `rt_sigreturn`, `kill`, `pipe`, `pipe2`, `futex`, `shmget`, `shmat`, `shmdt`, `shmctl`, `umask`, `execve`, `exit`, `exit_group`, `yield`, `getpid`, `getppid`, `setpgid`, `setsid`, `getsid`, `arch_prctl`, `gettid`, `set_tid_address`, `brk`) with shared SYSCALL/int80 argument handling and per-task user metadata (`brk`, stack bounds, argv/envp/auxv pointers, ELF PHDR info, FS-base TLS state, FD table state, cwd inode).
- Dynamic-linker prep is now expanded: baseline VM mapping syscalls (`mmap`/`mprotect`/`munmap`) support private anonymous and private file-backed mappings from VFS fds for userspace loaders.
- Phase-6 IPC/sync baseline is now present in-kernel: `SpinLock`/`Mutex`/`Semaphore`, anonymous pipes backed by in-kernel ring buffers, baseline System V-style shared memory segments, futex wait/wake keying used by `SYS_FUTEX` and `clear_child_tid` wake-on-exit, plus user-handler signal delivery (`kill` + `rt_sigaction` + `rt_sigprocmask` + `rt_sigreturn`, `SA_RESTORER`-based handler entry/return, and `SIGPIPE` generation on broken pipes).
- Initial user bootstrap builds a conventional process stack (`argc`, `argv`, `envp`, `auxv`), now including baseline identity/random auxv entries (`AT_UID`/`AT_EUID`/`AT_GID`/`AT_EGID`/`AT_SECURE`/`AT_RANDOM`/`AT_EXECFN`); `execve` replaces the current image with caller-provided `argv`/`envp` vectors while preserving task identity and fd table state.
- A phase-5 baseline VFS is now active with concrete tmpfs behavior, virtual `devfs`/`procfs` nodes, a mount table-backed `/proc/mounts`, and an ext2 importer mounted at `/mnt/ext2` where file and directory mutations (`create`/`mkdir`/`unlink`/`rmdir`/`rename`/`write`) update ext2 data/metadata in the mounted image buffer. VFS attempts block-backed ext2 mount first and performs best-effort sync-back on mutating operations when block-backed ext2 is active.
- Phase-7 PCI enumeration is active in `crates/drivers` (config mechanism #1, bus/device/function scan, bounded boot-time serial inventory output), and ATA/AHCI bring-up now includes AHCI command-engine setup/teardown, AHCI DMA `IDENTIFY`, AHCI LBA48 DMA read/write commands behind the kernel block API, ATA PIO `IDENTIFY`, and a minimal in-kernel block-device inventory.
- Baseline keyboard/TTY input is now wired in-kernel: PS/2 IRQ1 set-1 scancode decode feeds a canonical line-buffered console input path used by user `read` on stdio.
- Baseline framebuffer setup is now wired in-kernel: VESA boot handoff parsing (`status` + `mode` blocks), higher-half framebuffer window validation, basic pixel/clear primitives, and a styled framebuffer terminal mirror with 8x16 VGA-compatible bitmap glyphs, baseline ANSI color handling, cursor blink, and preset themes.
- Stage-9 userspace bootstrap plumbing is now present: musl source/sysroot build (`make musl-fetch`, `make musl-configure`, `make musl`) and toybox staging (`make toybox-fetch`, `make toybox`) with a shell-core Toybox set (`sh`, `echo`, `cat`, `ls`, `mkdir`, `rm`, `pwd`, `cp`, `mv`, `ln`, `touch`) integrated into the ext2 `execve` smoke path.
- QuickJS userspace staging is now wired (`make quickjs-fetch`, `make quickjs`) with ext2 smoke coverage via `make run-quickjs-smoke` (`KERNEL_FEATURES=musl-ext2-quickjs-smoke`) that executes staged JS scripts with `/bin/qjs`.
- MicroPython userspace staging is now wired (`make micropython-fetch`, `make micropython`) with ext2 smoke coverage via `make run-micropython-smoke` (`KERNEL_FEATURES=musl-ext2-micropython-smoke`) that executes staged Python scripts with `/bin/micropython`.
- Toybox networking applet staging now includes `ping` in addition to `netcat`/`wget`/`httpd`/`sntp`/`microcom`/`ftpget`, backed by AF_INET datagram ICMP socket support in the kernel socket layer.
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
- `docs/musl.md` Stage-9 musl bootstrap workflow and current limitations.
- `docs/toybox.md` Stage-9 toybox bootstrap workflow and current limitations.
- `docs/quickjs.md` Stage-9 quickjs bootstrap workflow and current limitations.
- `docs/micropython.md` Stage-9 micropython bootstrap workflow and current limitations.

## Build Prerequisites

- `nasm`
- `ld` and `objcopy` (binutils)
- `cargo` + `rustc`
- `qemu-system-x86_64`

Optional but commonly required:

- `rust-src` component if you need to rebuild `core` for a target.
- `mke2fs` (`e2fsprogs`) if you want to regenerate `user/sample_ext2.img`.
- `curl` or `wget` if you want to fetch musl/toybox/quickjs/micropython source via `make musl-fetch`, `make toybox-fetch`, `make quickjs-fetch`, or `make micropython-fetch`.

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

`make run` now builds and boots the shell runtime path (`PROFILE=release KERNEL_FEATURES=musl-ext2-exec`).
Use `make run-current` to boot the current `build/peetos.img` without rebuilding.
Use `make run-image` to rebuild with current flags first.

Regenerate the sample ext2 filesystem image used by host-side fs tests:

```bash
make ext2-image
```

Bootstrap musl source/sysroot for PeetOS user-space work:

```bash
make musl-fetch
make musl-configure
make musl
```

Bootstrap toybox source and build staged toybox utilities:

```bash
make toybox-fetch
make toybox
scripts/build_toybox_sh.sh
scripts/build_toybox_core.sh
scripts/run_toybox_next_smoke.sh
scripts/run_toybox_ping_smoke.sh
```

Bootstrap quickjs source and build staged `qjs`:

```bash
make quickjs-fetch
make quickjs
scripts/run_toybox_quickjs_smoke.sh
```

Bootstrap micropython source and build staged `micropython`:

```bash
make micropython-fetch
make micropython
scripts/run_toybox_micropython_smoke.sh
```

Boot a musl-linked runtime smoke program (`echo`-style static binary) as the initial user task:

```bash
make musl
make image PROFILE=debug KERNEL_FEATURES=musl-smoke
timeout 8s qemu-system-x86_64 -drive format=raw,file=build/peetos.img -serial stdio -display none -no-reboot -no-shutdown
```

Boot non-test runtime mode into Toybox `sh` on the kernel TTY (`execve("/mnt/ext2/bin/toybox", {"sh", ...})`):

```bash
make musl
make toybox
make image PROFILE=release KERNEL_FEATURES=musl-ext2-exec
qemu-system-x86_64 \
  -drive id=disk0,file=build/peetos.img,if=none,format=raw \
  -device ich9-ahci,id=ahci \
  -device ide-hd,drive=disk0,bus=ahci.0 \
  -drive id=disk1,file=user/sample_ext2_musl.img,if=none,format=raw \
  -device ide-hd,drive=disk1,bus=ahci.1 \
  -serial stdio -no-reboot -no-shutdown
```

Shell shortcut targets:

```bash
make run-shell
make run-shell-smoke
make run-ping-smoke
make run-quickjs-smoke
make run-micropython-smoke
```

Expected serial output includes `[musl-ext2-exec] launching interactive toybox sh` when the ext2 handoff succeeds.
Keyboard input is captured from the QEMU display window and surfaced through the kernel TTY.
When framebuffer mode is active, console output is mirrored into the QEMU display via a styled 8x16 text renderer with baseline ANSI 16-color support, cursor blink, and preset themes.
Note: `PROFILE=debug` may exceed the current stage2 low-memory kernel staging window for this feature set.

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
