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
- `dropbear/` Stage-9 dropbear source/bootstrap build glue for staged SSH runtime smoke.
- `linker/` Linker script for the kernel.
- `docs/PORTING_PLAN.md` High-level implementation roadmap.
- `docs/` Implementation docs grouped by subsystem, each with a limitations/simplifications section.

## Current Status (Last reviewed: 2026-02-19)

- Boots to Rust `kmain()`, initializes serial output, prints a banner, and halts in a `hlt` loop.
- Panic handler prints over serial, then halts with `cli; hlt`.
- `arch_x86_64` now provides basic port I/O helpers and interrupt enable/disable wrappers.
- GDT/IDT setup is wired in with interrupt stubs, PIC remap, PIT timer init, syscall MSR setup, a default exception report (serial-only for now), and memory management (bitmap PMM, VMM, kernel heap).
- Kernel tasking/scheduler baseline is in place (kernel threads + PIT-driven preemption) with a minimal user-mode task loaded from an embedded ELF.
- ELF loading is now hardened with stricter header/segment validation (alignment checks, malformed `PT_INTERP` rejection, RWX-segment rejection, entrypoint-in-exec checks), and NXE is enabled so non-executable mappings are effective.
- Dynamic user bootstrap now works for a minimal `PT_INTERP` flow: a PIE main image is loaded, its interpreter (`/lib/ld-peetos.so`) is mapped, and first user RIP starts in the interpreter with `AT_ENTRY`/`AT_BASE` set.
- Syscall dispatch now uses a table-driven Linux-numbered stage-4/6 baseline (`open`, `openat`, `mkdirat`, `close`, `read`, `write`, `writev`, `lseek`, `ioctl`, `stat`, `lstat`, `fstat`, `newfstatat`, `getdents64`, `unlinkat`, `renameat`, `mmap`, `mprotect`, `munmap`, `clone`, `fork`, `vfork`, `wait4`, `uname`, `rt_sigaction`, `rt_sigprocmask`, `rt_sigreturn`, `kill`, `pipe`, `pipe2`, `futex`, `shmget`, `shmat`, `shmdt`, `shmctl`, `umask`, `execve`, `exit`, `exit_group`, `yield`, `getpid`, `getppid`, `setpgid`, `setsid`, `getsid`, `arch_prctl`, `gettid`, `set_tid_address`, `brk`) with shared SYSCALL/int80 argument handling and per-task user metadata (`brk`, stack bounds, argv/envp/auxv pointers, ELF PHDR info, FS-base TLS state, FD table state, cwd inode).
- Dynamic-linker prep is now expanded: baseline VM mapping syscalls (`mmap`/`mprotect`/`munmap`) support private anonymous mappings plus private/shared file-backed mappings from VFS fds, with per-task VMA tracking, lazy page-fault population, and shared file-page faults now backed directly by the kernel VM page cache. File-backed writable `MAP_SHARED` mappings now use baseline dirty tracking and writeback via `msync`/`sync`/`fsync`. Under user-page allocation pressure, the kernel now performs best-effort reclaim by unmapping reclaim-safe file-backed pages and evicting unpinned VM page-cache pages before retrying allocation.
- Phase-6 IPC/sync baseline is now present in-kernel: `SpinLock`/`Mutex`/`Semaphore`, anonymous pipes backed by in-kernel ring buffers, baseline System V-style shared memory segments, futex wait/wake keying used by `SYS_FUTEX` and `clear_child_tid` wake-on-exit, plus user-handler signal delivery (`kill` + `rt_sigaction` + `rt_sigprocmask` + `rt_sigreturn`, `SA_RESTORER`-based handler entry/return, and `SIGPIPE` generation on broken pipes).
- Initial user bootstrap builds a conventional process stack (`argc`, `argv`, `envp`, `auxv`), now including baseline identity/random auxv entries (`AT_UID`/`AT_EUID`/`AT_GID`/`AT_EGID`/`AT_SECURE`/`AT_RANDOM`/`AT_EXECFN`); `execve` replaces the current image with caller-provided `argv`/`envp` vectors while preserving task identity and fd table state.
- A phase-5 baseline VFS is now active with concrete tmpfs behavior, virtual `devfs`/`procfs` nodes, a mount table-backed `/proc/mounts`, and block-backed ext2 mounted at `/mnt/ext2` where file and directory mutations (`create`/`mkdir`/`unlink`/`rmdir`/`rename`/`write`) update on-disk ext2 data/metadata. Ext2 regular-file writes now allocate through direct and indirect block mappings (single/double/triple pointer paths), `truncate`/`ftruncate` length changes now resize ext2 regular files (including zero-fill growth), and truncate paths reclaim indirect allocations; ext2 directory entry growth can also allocate through indirect mappings. VFS attempts block-backed ext2 mount first; mount now supports internal ext3 journal metadata parsing plus mount-time replay of committed journal transactions when recovery is required, clears recover markers before exposing the filesystem, and journaled dirty-block flushes now emit runtime descriptor+data+commit records with rotating wrap-aware placement, deferred checkpoint writeback, runtime revoke-block emission, and wrap-safe transaction-ID ordering for replay/revoke decisions. Block-backed ext2 uses a sparse lazy block cache (first-access reads with no full-image pre-read/preallocation), cache residency is now bounded with CLOCK eviction (dirty victims are written back before eviction), ext2 directory VFS nodes are populated lazily on first lookup/readdir, ext2 image export reconstructs full snapshots by reading uncached blocks from the backend, mutating operations enqueue explicitly tracked dirty ext2 block runs through a page-sized block write-back cache, and `sync`/`fsync` flush that cache.
- Phase-7 PCI enumeration is active in `crates/drivers` (config mechanism #1, bus/device/function scan, bounded boot-time serial inventory output), and ATA/AHCI bring-up now includes AHCI command-engine setup/teardown, AHCI DMA `IDENTIFY`, AHCI LBA48 DMA read/write commands behind the kernel block API, ATA PIO `IDENTIFY`, and a minimal in-kernel block-device inventory.
- Baseline keyboard/TTY input is now wired in-kernel: PS/2 IRQ1 set-1 scancode decode feeds a canonical line-buffered console input path used by user `read` on stdio.
- Baseline pointer input is now wired in-kernel: PS/2 IRQ12 packet decode drives relative cursor movement by default, and an optional `usb-tablet` kernel feature enables UHCI USB HID-tablet polling for absolute framebuffer cursor positioning.
- Baseline framebuffer setup is now wired in-kernel: VESA boot handoff parsing (`status` + `mode` blocks), higher-half framebuffer window validation, basic pixel/clear primitives, and a styled framebuffer terminal mirror with 8x16 VGA-compatible bitmap glyphs, baseline ANSI color handling, cursor blink, preset themes, a top `PeetOS` desktop header, a `Terminal` title bar, and a movable mouse cursor overlay.
- Stage-9 userspace bootstrap plumbing is now present: musl source/sysroot build (`make musl-fetch`, `make musl-configure`, `make musl`) and toybox staging (`make toybox-fetch`, `make toybox`) with a shell-core Toybox set (`sh`, `echo`, `cat`, `ls`, `mkdir`, `rm`, `pwd`, `cp`, `mv`, `ln`, `touch`) integrated into the ext2 `execve` smoke path.
- In `musl-ext2-exec` interactive shell mode, launcher `PS1` now shows cwd directly in the prompt line and renders that prompt in green.
- QuickJS userspace staging is now wired (`make quickjs-fetch`, `make quickjs`) with ext2 smoke coverage via `make run-quickjs-smoke` (`KERNEL_FEATURES=musl-ext2-quickjs-smoke`) that executes staged JS scripts with `/bin/qjs`.
- MicroPython userspace staging is now wired (`make micropython-fetch`, `make micropython`) with ext2 smoke coverage via `make run-micropython-smoke` (`KERNEL_FEATURES=musl-ext2-micropython-smoke`) that executes staged Python scripts with `/bin/micropython`.
- Dropbear userspace staging is now wired (`make dropbear-fetch`, `make dropbear`) with ext2 smoke coverage via `make run-dropbear-smoke` (`KERNEL_FEATURES=musl-ext2-dropbear-smoke`) that validates staged `/bin/dropbear` + `/bin/dropbearkey` bring-up, staged host-key usage, and daemonization startup marker.
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
- `docs/dropbear.md` Stage-9 dropbear bootstrap workflow and current limitations.

## Build Prerequisites

- `nasm`
- `ld` and `objcopy` (binutils)
- `cargo` + `rustc`
- `qemu-system-x86_64`

Optional but commonly required:

- `rust-src` component if you need to rebuild `core` for a target.
- `mke2fs` (`e2fsprogs`) if you want to regenerate `user/sample_ext2.img`.
- `curl` or `wget` if you want to fetch musl/toybox/quickjs/micropython/dropbear source via `make musl-fetch`, `make toybox-fetch`, `make quickjs-fetch`, `make micropython-fetch`, or `make dropbear-fetch`.

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

`make run` now builds and boots a full runtime profile by enabling every kernel feature defined in `kernel/Cargo.toml` except `test-exit` (test-only auto-exit), `vga` (legacy text-mode path), and all `*-smoke` features, with AHCI disk0 (`build/peetos.img`) and AHCI disk1 (`user/sample_ext2_musl.img`) attached.
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

Bootstrap dropbear source and build staged `dropbear`/`dropbearkey`:

```bash
make dropbear-fetch
make dropbear
scripts/run_toybox_dropbear_smoke.sh
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
make run-tablet
make run-ping-smoke
make run-quickjs-smoke
make run-micropython-smoke
make run-dropbear-smoke
```

Expected shell startup now opens directly at the prompt without a launcher banner line.
Interactive shell prompt is configured as a green cwd prompt (for example, `/mnt/ext2 $ `).
Keyboard input is captured from the QEMU display window and surfaced through the kernel TTY.
Mouse input is captured from the QEMU display window and drives the on-screen software cursor when framebuffer mode is active.
Use `make run-tablet` to launch QEMU with `-usb -device usb-tablet` for absolute pointer input tests.
When framebuffer mode is active, console output is mirrored into the QEMU display via a styled 8x16 text renderer with baseline ANSI 16-color support, cursor blink, preset themes, a top `PeetOS` header, a `Terminal` title bar, and the software mouse cursor overlay.
Note: kernel size is no longer bounded by the stage2 low-memory chunk buffer; image builds are now constrained by disk image capacity and the current 16-bit sector-count metadata field.

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
