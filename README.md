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

## Current Status (Last reviewed: 2026-04-12)

- Boots to Rust `kmain()`, initializes serial output, prints a banner, and halts in a `hlt` loop.
- Panic handler prints over serial, then halts with `cli; hlt`.
- `arch_x86_64` provides port I/O helpers and interrupt enable/disable wrappers.
- GDT/IDT setup is wired with interrupt stubs, PIC remap, PIT timer init, syscall MSR setup, and serial exception reports.
- PMM/VMM/kernel heap are active.
- Tasking/scheduler baseline is in place (kernel threads + PIT-driven preemption).
- ELF loading is hardened (header/segment validation, malformed `PT_INTERP` rejection, RWX rejection, entrypoint checks) with NXE enabled.
- User bootstrap supports a minimal `PT_INTERP` flow (`AT_ENTRY`/`AT_BASE`) and Linux-style initial stack (`argc`/`argv`/`envp`/`auxv`).
- Syscall dispatch is table-driven and shared by SYSCALL/int80 paths. Implemented syscalls include file/VFS operations, process control (`fork`/`clone`/`vfork`/`wait4`), VM mapping (`mmap`/`mprotect`/`munmap`), IPC (`pipe`/`futex`/`shm*`), signals (`rt_sigaction`/`rt_sigprocmask`/`rt_sigreturn`/`kill`), and core identity/time/process-group calls.
- Shared-buffer primitive baseline now includes `memfd_create` (tmpfs-backed internal namespace objects with `MFD_CLOEXEC`; sealing flags currently return `-ENOSYS`).
- Event-loop primitive baseline now includes level-triggered `epoll_create1`/`epoll_ctl`/`epoll_wait` (`EPOLL_CLOEXEC`, `ADD`/`MOD`/`DEL`, timeout wait, `-EINTR` interruption path).
- Socket coverage includes `AF_INET` networking plus baseline `AF_UNIX` local stream support (`socket`, `socketpair`, pathname `bind`/`connect`/`listen`/`accept`), with `SCM_RIGHTS` fd passing in `sendmsg`/`recvmsg`.
- AF_UNIX/SCM_RIGHTS now has a dedicated QEMU smoke target (`run-unix-rights-smoke`) covering cross-process fd transfer and key negative paths.
- File-backed `MAP_SHARED` mappings support dirty tracking and writeback via `msync`/`sync`/`fsync`; user-page allocation includes a best-effort reclaim pass for reclaim-safe file-backed pages.
- VFS now mounts block-backed ext2/ext3 at `/` (root) with Linux-like mountpoints (`/boot`, `/temp`, `/dev`, `/proc`), with `/etc/fstab` declaring the root block mount as `ext3` and preserving a `/mnt/ext2` compatibility path alias when root ext2 is active.
- ext2 includes mutable file/dir operations, direct+indirect block mapping (single/double/triple), truncate growth/shrink, lazy directory population, sparse block cache with CLOCK eviction, and block-cache sync/flush integration.
- Journaled ext mounts are surfaced as `ext3` and include internal-journal parse/replay + runtime descriptor/data/commit/revoke emission with wrap-aware sequencing, plus baseline runtime abort-on-I/O-fault handling.
- Ordered-mode journal bring-up now tracks bounded mutation credits across most ext2 mutators, plans variable-size write/truncate-style operations as capped per-segment reservations, uses indirection-aware restart estimates for allocation/free work, lets allocator/free helpers request restart headroom before their own metadata updates, splits allocation helpers into bookkeeping and initialization/link phases, gives truncate-side detach/free work explicit unlink-table versus free/revoke restart boundaries, records restart points as named journal phases on sealed ordered segments, now groups the actual metadata work for those phases through restart-phase helpers, threads a lightweight internal journal-handle token through the main ordered write/truncate/alloc/free paths and the fixed-shape create/unlink/rename/link/symlink metadata helpers, rotates restartable segments at block-walking loop boundaries and final inode closeout, and can split oversized operations into multiple sealed ordered transaction segments instead of treating them as one elastic in-memory transaction.
- ext2/ext3 feature acceptance is intentionally narrow: the mount path currently allows `HAS_JOURNAL` in compat bits, `FILETYPE`/`RECOVER`/`JOURNAL_DEV` in incompat bits, and `SPARSE_SUPER` in ro-compat bits. Images carrying `EXT_ATTR`, `DIR_INDEX`, `RESIZE_INO`, `IMAGIC_INODES`, `DIR_PREALLOC`, or `LARGE_FILE` are currently rejected until their semantics are implemented.
- PCI enumeration is active in `crates/drivers`; storage bring-up includes AHCI DMA `IDENTIFY`, AHCI LBA48 DMA read/write, ATA PIO `IDENTIFY`, and block-device inventory.
- Keyboard/TTY input, PS/2 mouse cursor movement, optional UHCI USB tablet absolute input, and framebuffer text mirroring are wired.
- Phase 3 GUI bring-up has started with a baseline display device ABI: `devfs` now exposes `/dev/fb0`, `ioctl(FBIOGETINFO)` (`0x5001`) returns framebuffer geometry/format metadata (`width`, `height`, `pitch`, `bpp`, `format`, `map_size`), and userspace can `mmap` the framebuffer with direct shared visibility for compositor-style redraw loops.
- Phase 3 input ABI is now present: `devfs` exposes `/dev/input/event0` (keyboard) and `/dev/input/event1` (pointer) with 24-byte evdev-style records, nonblocking `read`, `poll`/`select`/`epoll` readiness, and `SYN_DROPPED` overflow signalling.
- Phase 3 display ownership gating is now explicit: default boots stay console-first, while `KERNEL_FEATURES=compositor-first` suppresses kernel framebuffer console/cursor composition so a userspace compositor can own `/dev/fb0`; init-launch failure and fatal panic/exception paths reactivate a visible kernel console fallback.
- Phase 3 userspace compositor-first wiring now includes a dedicated ext2 smoke profile (`KERNEL_FEATURES="compositor-first musl-ext2-gui-smoke"`): init launches `/mnt/ext2/bin/gui_smoke.sh`, exports explicit framebuffer/input environment variables, opens `/dev/fb0` plus `/dev/input/event{0,1}`, maps and writes the framebuffer, and confirms injected keyboard/pointer events reach userspace through the event-device path.
- Phase 3 compositor fallback now covers successful takeover as well: if the compositor-owned initial userspace task exits or crashes, the kernel reactivates console ownership, redraws the fallback framebuffer UI, and exposes that path through a dedicated QEMU smoke target.
- Phase 4 compositor MVP work has started with an initial compositor-core smoke (`KERNEL_FEATURES="compositor-first musl-ext2-peet-gui-smoke"`): a userspace compositor helper renders two overlapping surfaces, validates initial z-order plus raise-on-focus behavior, validates a damage-limited partial redraw/commit, routes injected keyboard/pointer events through the compositor-first loop, and validates clean client detach handling inside the message-driven compositor path.
- Phase 4 now also has an initial visual shell scaffold (`KERNEL_FEATURES="compositor-first musl-ext2-peet-shell-demo"`): a compositor-first userspace shell demo renders desktop chrome and composes terminal/files/inspector panels as independent offscreen surfaces with focus raise, pointer cursor, theme rotation, and terminal-style command feedback directly on `/dev/fb0`. The `FILES` panel is now driven by a separate forked userspace client over a real `AF_UNIX socketpair` message path, so GUI primitives can be iterated visually before full shared-buffer transport lands.
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
- `docs/gui_wayland_plan.md` Living GUI roadmap and staged plan toward Wayland compatibility.
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
- `KERNEL_FEATURES=compositor-first` is an explicit Phase 3 display-ownership profile. It only changes kernel framebuffer ownership/handoff today, so pair it with a userspace framebuffer client or compositor-oriented init profile rather than the default shell profile.
- `make run-gui-smoke` boots the compositor-first smoke profile and validates framebuffer rendering plus injected keyboard/pointer delivery over serial logs.
- `make run-gui-fallback-smoke` boots the compositor-first smoke profile, lets the compositor-owned init complete, and asserts the kernel returns framebuffer ownership to the console afterward.
- `make run-peet-gui-smoke` boots the initial Phase 4 compositor-core smoke and validates overlapping two-surface composition, z-order plus raise-on-focus behavior, partial damage/commit redraw, focus/input routing, and client detach recovery.
- `make run-peet-shell-demo` boots the Phase 4 visual shell scaffold in compositor-first mode and leaves QEMU open for manual GUI iteration.
- `make run-peet-shell-demo-smoke` boots the visual shell scaffold, waits for a ready marker, injects a few keyboard actions, and verifies the shell logs focus/theme/command activity.
- `make run-console-first-smoke` boots the dedicated console-first smoke profile (`KERNEL_FEATURES=musl-ext2-console-first-smoke`) and asserts the kernel stays in console ownership while a minimal shell workload completes.

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
scripts/run_toybox_unix_rights_smoke.sh
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
make run-console-first-smoke
make run-gui-fallback-smoke
make run-peet-gui-smoke
make run-ping-smoke
make run-unix-rights-smoke
make run-gui-smoke
make run-quickjs-smoke
make run-micropython-smoke
make run-dropbear-smoke
```

## Testing

Run the repository test harness:

```bash
make test
```

`make test` runs the full `tests/*.sh` harness: host unit tests (`arch_x86_64`, `drivers`, `fs`, `kernel`), setup checks for staged runtime components, the default `isa-debug-exit` QEMU smoke, and the dedicated GUI/console/userspace smoke scripts currently tracked in `tests/`.

## Historical Reference

If you need behavior comparisons with the earlier implementation, use:

```bash
../peetos_c/tests/run_tests.sh
../peetos_c/tests/run_shell_cmds.sh -clean "echo hello" "ls /bin"
```

## Notes

- The kernel is compiled with `no_std` and `panic=abort`.
- The bootloader and linker script are reused from the C codebase for stable early boot.
