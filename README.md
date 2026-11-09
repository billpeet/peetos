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

## Current Status (Last reviewed: 2026-03-14)

- Boots to Rust `kmain()`, initializes serial output, prints a banner, and halts in a `hlt` loop.
- Panic handler prints over serial, then halts with `cli; hlt`.
- `arch_x86_64` provides port I/O helpers and interrupt enable/disable wrappers.
- GDT/IDT setup is wired with interrupt stubs, PIC remap, PIT timer init, syscall MSR setup, and serial exception reports.
- PMM/VMM/kernel heap are active.
- SMP:
  - When an ACPI MADT is present, the kernel switches IRQ delivery from the 8259 PIC to the LAPIC/IOAPIC.
  - It wakes every application processor with INIT/SIPI and checks each one with an IPI self-test.
  - Every CPU runs tasks from a shared run queue. Each CPU has its own GDT/TSS/GS area and LAPIC timer tick, and reschedule and TLB-shootdown IPIs keep them coordinated.
  - Most kernel code is still serialised by a big kernel lock. The scheduler, PMM and heap have their own locks, so timer ticks and task switches keep running on every CPU even while one CPU holds the big lock in a long syscall.
  - See `docs/smp.md`.
- Tasking/scheduler baseline is in place (kernel threads + PIT-driven preemption).
- libwayland 1.23.1 (with libffi 3.4.6) is ported as static musl libraries (`make -C wayland`). A dedicated QEMU smoke (`run-wayland-smoke`) runs a libwayland compositor and a separate libwayland client process that connect over `/tmp/wayland-0`, share a sealed memfd `wl_shm` pool, present two frames (growing the pool in between), and exercise `eventfd`, `timerfd`, `signalfd`, `flock`, `fallocate`, `mremap`, and `SO_PEERCRED`. A second profile (`run-wayland-tests`, part of `make test`) runs 21 of upstream libwayland's own test binaries (131 cases) inside PeetOS. See `docs/wayland.md`.
- `peetwl`, the Wayland compositor, runs xdg-shell clients on the framebuffer with seat input (real XKB keymaps via libxkbcommon; pick a layout with `--xkb-layout`), server-side decorations, click-to-focus, title-bar move and close: `make run-wayland-demo` for an interactive session, `make run-wlcomp-smoke` for the automated screendump-based test.
- Dynamically linked programs run through musl's loader (`/lib/ld-musl-x86_64.so.1`), with shared libraries, `dlopen` and TLS in shared objects: `make -C musl shared` builds the shared libc and `make run-dynlink-smoke` tests it (see `docs/elf.md`). libwayland, libffi and libxkbcommon are built as shared libraries too, and `peetwl`/`wl_demo` link against them. The plan toward a full desktop is in `docs/desktop_roadmap.md`.
- `/dev/dri/card0` is a minimal DRM/KMS device over the boot framebuffer: legacy mode setting, dumb buffers, page flips with events, PRIME. libdrm is ported, and `peetwl` presents through it. Test it with `make run-drm-smoke`; see `docs/drm.md`.
- A software rendering stack (pixman, freetype, fontconfig with DejaVu fonts) is ported as shared libraries. `make run-text-smoke` renders anti-aliased text on screen through DRM; see `docs/rendering.md`.
- C++ programs run on PeetOS: `make -C toolchain` builds a GCC 15.2 cross toolchain for musl, and libstdc++, libgcc_s and libatomic ship in the image. `make run-cxx-smoke` checks exceptions across shared objects, RTTI, iostreams, `std::thread`, `thread_local` and `dlopen()`ed C++ plugins; see `docs/cxx.md`.
- harfbuzz (built with that compiler) and utf8proc are part of the rendering stack, so text is shaped: kerning, combining marks and Arabic joining in `make run-text-smoke`, and fcft/foot use them for shaping and grapheme clustering.
- cairo, pango and GLib are ported (with zlib, libpng, pcre2 and fribidi): `make run-pango-smoke` draws a cairo + pango frame with markup and bidirectional text, and checks GLib, GIO and cairo's PNG/PDF/SVG output (see `docs/rendering.md`).
- GTK 3.24 runs under `peetwl` (Wayland backend, with libepoxy, ATK and gdk-pixbuf): `make run-gtk-smoke` types into a `GtkEntry` and clicks a `GtkButton` with real input; see `docs/gtk.md`.
- The foot terminal runs under `peetwl`, with fcft/freetype text and a shell inside it: `make run-foot-demo` opens two foot windows in a QEMU window, with copy and paste between them (Ctrl+Shift+C/V), and `make run-foot-smoke` is the automated test (see `docs/rendering.md`).
- The kernel manages up to 4 GiB of RAM through a direct map of physical memory; give QEMU more with `-m` (the dynamic-linking smoke uses 512 MiB). Per process there are 256 VM region slots and a 256 MiB mapping budget. System-wide, there are 256 tasks and 256 sockets, and memfd objects can be up to 256 MiB.
- Per-task x87/SSE register state is saved and restored with `FXSAVE`/`FXRSTOR` on every context switch, copied on `fork`, reset on `execve`, and preserved across signal handlers (saved in the signal frame and restored by `rt_sigreturn`). A dedicated QEMU smoke target (`run-fpu-smoke`) checks all three paths.
- `fork` is copy-on-write, and `clone(CLONE_THREAD)` thread groups support musl pthreads, including `pthread_cancel`, timed waits and `execve` from any thread. Signals use the Linux `rt_sigframe` ABI (signals 1..=64, `siginfo_t`/`ucontext_t`) and interrupt blocking syscalls with `SA_RESTART`-aware restart (`make run-proc-smoke`; see `docs/process_model.md`).
- ext4: stock `mkfs.ext4` images (`metadata_csum`, `64bit`, `flex_bg`, extents) mount read-write with extent trees, lazy-init groups, crc32c metadata checksums on every write, and a JBD2 csum-v3 journal (`tests/musl_ext4_csum_journal_smoke.sh` runs the QEMU crash/recovery smoke on such a root). Images without `metadata_csum` (`mke2fs -t ext4 -O ^metadata_csum,uninit_bg`, crc16 descriptor checksums) are writable too (`tests/musl_ext4_journal_smoke.sh`). Building the kernel with `KERNEL_FEATURES=ext2-ro-root` mounts the root read-only (`make run-ext4-ro-smoke`). Hashed (htree) directory indexes are used and maintained on ext3/ext4, so directories with thousands of entries work. Files may exceed 4 GiB (sparse growth), timestamps survive 2038, an unlinked file stays usable while open (orphan inodes are finished after a crash), and images with `orphan_file` (newer `mkfs.ext4` default) mount. See `docs/ext4_compat_plan.md`.
- ext3 (journaled) roots are crash-safe: `make run-journal-smoke` boots an ext3 copy of the runtime image, kills QEMU mid-workload, then checks that both `e2fsck` and the kernel recover it (needs `e2fsprogs`; see `docs/ext3_parity_audit.md`).
- ELF loading is hardened (header/segment validation, malformed `PT_INTERP` rejection, RWX rejection, entrypoint checks) with NXE enabled.
- User bootstrap supports a minimal `PT_INTERP` flow (`AT_ENTRY`/`AT_BASE`) and Linux-style initial stack (`argc`/`argv`/`envp`/`auxv`).
- Syscall dispatch is table-driven and shared by SYSCALL/int80 paths. Implemented syscalls include file/VFS operations, process control (`fork`/`clone`/`vfork`/`wait4`, threads via `clone(CLONE_THREAD)`, `tkill`/`tgkill`), VM mapping (`mmap`/`mprotect`/`munmap`), IPC (`pipe`/`futex`/`shm*`), signals (`rt_sigaction`/`rt_sigprocmask`/`rt_sigreturn`/`kill`/`tkill`/`tgkill`), and core identity/time/process-group calls.
- Shared-buffer primitive baseline now includes `memfd_create` (tmpfs-backed internal namespace objects with `MFD_CLOEXEC`; sealing flags currently return `-ENOSYS`).
- Event-loop primitive baseline now includes level-triggered `epoll_create1`/`epoll_ctl`/`epoll_wait` (`EPOLL_CLOEXEC`, `ADD`/`MOD`/`DEL`, timeout wait, `-EINTR` interruption path).
- Socket coverage includes `AF_INET` networking plus baseline `AF_UNIX` local stream support (`socket`, `socketpair`, pathname `bind`/`connect`/`listen`/`accept`), with `SCM_RIGHTS` fd passing in `sendmsg`/`recvmsg`.
- AF_UNIX/SCM_RIGHTS now has a dedicated QEMU smoke target (`run-unix-rights-smoke`) covering cross-process fd transfer and key negative paths.
- File-backed `MAP_SHARED` mappings support dirty tracking and writeback via `msync`/`sync`/`fsync`; user-page allocation includes a best-effort reclaim pass for reclaim-safe file-backed pages.
- VFS now mounts block-backed ext2/ext3 at `/` (root) with Linux-like mountpoints (`/boot`, `/temp`, `/dev`, `/proc`), with `/etc/fstab` declaring the root block mount as `ext3` and preserving a `/mnt/ext2` compatibility path alias when root ext2 is active.
- ext2 includes mutable file/dir operations, direct+indirect block mapping (single/double/triple), truncate growth/shrink, lazy directory population, sparse block cache with CLOCK eviction, and block-cache sync/flush integration.
- Journaled ext mounts are surfaced as `ext3` and include internal-journal parse/replay + runtime descriptor/data/commit/revoke emission with wrap-aware sequencing, plus baseline runtime abort-on-I/O-fault handling.
- PCI enumeration is active in `crates/drivers`; storage bring-up includes AHCI DMA `IDENTIFY`, AHCI LBA48 DMA read/write, ATA PIO `IDENTIFY`, and block-device inventory.
- Keyboard/TTY input, PS/2 mouse cursor movement, optional UHCI USB tablet absolute input, and framebuffer text mirroring are wired.
- Phase 3 display/input device bring-up is now in place: `devfs` exposes `/dev/fb0`, `ioctl(FBIOGETINFO)` (`0x5001`) returns framebuffer geometry/format metadata (`width`, `height`, `pitch`, `bpp`, `format`, `map_size`), and userspace can `mmap` the framebuffer with direct shared visibility for compositor-style redraw loops.
- Phase 3 input ABI is now present: `devfs` exposes `/dev/input/event0` (keyboard) and `/dev/input/event1` (pointer) with 24-byte evdev-style records, nonblocking `read`, `poll`/`select`/`epoll` readiness, and `SYN_DROPPED` overflow signalling.
- Phase 3 display ownership gating is now explicit: default boots stay console-first, while `KERNEL_FEATURES=compositor-first` suppresses kernel framebuffer console/cursor composition so a userspace compositor can own `/dev/fb0`; init-launch failure, successful compositor exit, fatal user faults, and fatal panic/exception paths all reactivate a visible kernel console fallback.
- The GUI regression path now covers the completed Phase 4 userspace-compositor MVP: the compositor-first smoke profile launches a dedicated userspace compositor process plus multiple smoke clients over inherited `AF_UNIX` streams, clients run an explicit `create -> attach -> commit -> damage -> destroy` lifecycle with shared-memory buffers via `memfd_create` + `SCM_RIGHTS`, the compositor coalesces clipped dirty-region redraws, uses hover pointer focus plus click-to-focus keyboard routing for an input-aware client, rejects malformed clients plus invalid attach fds and stale post-destroy traffic, survives transient client disconnect, and the harness still verifies console fallback after compositor exit.
- A separate `make run-gui-demo` profile now boots the same compositor stack in a less test-oriented layout so you can watch multiple surfaces, transient surface teardown, compositor-owned status bars, and routed pointer/keyboard feedback in the QEMU window.
- Userspace bootstrap is active via musl + toybox, with runtime smoke paths for shell, ping, QuickJS, MicroPython, and Dropbear.

## Documentation

- `docs/boot.md` Boot flow, entry sequence, and `kmain()` initialization order.
- `docs/interrupts.md` GDT/IDT setup, PIC/PIT configuration, and interrupt handling.
- `docs/memory.md` Typed addresses, PMM, VMM, and kernel heap allocator.
- `docs/wayland.md` libwayland port: build, kernel interfaces it uses, and the end-to-end smoke test.
- `docs/elf.md` ELF loading and initial userspace image handoff.
- `docs/syscall.md` SYSCALL/MSR setup and dispatcher behavior.
- `docs/tasking.md` Task structures, user bootstrap, and scheduler behavior.
- `docs/process_model.md` Copy-on-write fork, thread groups, signals and syscall restart, futexes, and process/thread exit.
- `docs/vfs.md` VFS/tmpfs/devfs/procfs/ext2/ext3-journal path and syscall integration.
- `docs/ipc.md` Synchronization primitives and IPC baseline (`pipe`/`futex`/`shm*`/`signal`).
- `docs/drivers.md` PCI/storage/input/framebuffer driver status.
- `docs/framebuffer.md` Framebuffer handoff, mapping model, console rendering, and cursor path.
- `docs/compositor.md` Userspace compositor MVP architecture, smoke protocol, and current limitations.
- `docs/gui_wayland_plan.md` Living GUI roadmap and staged plan toward Wayland compatibility.
- `docs/diagnostics.md` Serial output, exception reporting, panic handling, and QEMU test exit.
- `docs/subsystems.md` Subsystem crate status and intended usage.
- `docs/musl.md` musl bootstrap workflow and current limitations.
- `docs/gtk.md` GTK 3 port: build, Wayland backend, the GTK smoke test and limitations.
- `docs/cxx.md` C++ cross toolchain (GCC + libstdc++ for musl), runtime staging and the C++ smoke test.
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
- `mke2fs` (`e2fsprogs`) to regenerate `user/sample_ext2.img`, `user/sample_ext3.img`, and the ext4 fixtures `user/sample_ext4*.img` (`scripts/build_ext3_sample.sh` and `scripts/build_ext4_sample.sh` also use `debugfs`).
- `e2fsck` (`e2fsprogs`) for `tests/ext3_fsck.sh`, which checks images written by the kernel's ext2/ext3 code.
- `curl` or `wget` for fetching musl/toybox/quickjs/micropython/dropbear sources.
- For the C++ toolchain (`make -C toolchain`): a host C/C++ compiler (`g++`) and `m4`. The graphics userland needs it too (harfbuzz is C++).
- For the graphics userland (`make -C wayland`): host `cc`, `python3`, `bison`, expat headers (`libexpat1-dev`), `meson`, `ninja`, `gperf`, and GLib's `glib-compile-resources`/`glib-compile-schemas` (`libglib2.0-dev-bin`, for GTK). Meson, ninja and gperf can live in a private virtualenv at `~/.peetos-venv`; see `docs/rendering.md`.
- `rg` (ripgrep), `file` and `readelf`: the musl build and image scripts use them to check their outputs, and the userspace smokes in `make test` fail without them.
- On Ubuntu (including WSL2) this covers everything: `apt install build-essential nasm qemu-system-x86 qemu-system-gui e2fsprogs meson ninja-build gperf bison flex m4 pkg-config libexpat1-dev libglib2.0-dev-bin ripgrep curl`, plus `rustup target add x86_64-unknown-none`. Under WSLg, `make run*` targets open their QEMU window on the Windows desktop.
- `make toybox` and `make -C wayland` are not safe with `-j` on a fresh checkout. Run `make toybox` serially, and run `make -C wayland fetch` once before `make -C wayland -j$(nproc)`.

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

- `make run` now boots the compositor-first GUI demo profile so the current userspace compositor path is the default manual bring-up experience.
- AHCI disk0 is `build/peetos.img`; AHCI disk1 is `user/sample_ext2_musl.img`.
- Add `QEMU_SMP=<n>` to any `make run*` target to boot with several virtual CPUs (for example `make run QEMU_SMP=4`). The serial log reports `[SMP] <n> of <n> CPU(s) online`.
- Use `make run-current` to boot the current image without rebuilding.
- Use `make run-image` to force rebuild then boot.
- Use `make run-shell` if you want the older console-first shell profile instead of the GUI demo.
- `KERNEL_FEATURES=compositor-first` is an explicit Phase 3 display-ownership profile. It only changes kernel framebuffer ownership/handoff today, so pair it with a userspace framebuffer client or compositor-oriented init profile rather than the default shell profile.
- `make run-shell-smoke` keeps the default console-first profile and now asserts that shell bring-up stays on the kernel-owned display path.
- `make run-gui-smoke` builds the compositor-first smoke profile (`KERNEL_FEATURES="musl-ext2-gui-smoke compositor-first"`) and boots a staged userspace compositor smoke session with hover pointer focus plus click-to-focus keyboard routing, explicit surface create/attach/commit/damage/destroy flow, malformed-client rejection, disconnect-survival checks, framebuffer/input ownership, and console fallback checks.
- `make run-gui-demo` builds the compositor-first demo profile (`KERNEL_FEATURES="musl-ext2-gui-demo compositor-first"`) and opens a visual compositor demo with an input-aware surface, multiple independent client surfaces, and transient surface teardown.

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
scripts/run_toybox_proc_smoke.sh
scripts/run_toybox_journal_smoke.sh
scripts/run_toybox_ext4_ro_smoke.sh
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
make run-unix-rights-smoke
make run-proc-smoke
make run-journal-smoke
make run-ext4-ro-smoke
make run-quickjs-smoke
make run-micropython-smoke
make run-dropbear-smoke
make run-fpu-smoke
make run-wayland-smoke
make run-wayland-tests
make run-wlcomp-smoke
make run-wayland-demo
make run-dynlink-smoke
make run-drm-smoke
make run-text-smoke
make run-cxx-smoke
make run-pango-smoke
make run-gtk-smoke
make run-foot-smoke
make run-foot-demo
make run-gui-smoke
make run-gui-demo
```

## Testing

Run the repository test harness:

```bash
make test
```

`make test` runs host unit tests plus the shell/setup/QEMU regression scripts under [`tests/`](/home/bill/source/peetos/tests/), including the console-first shell smoke, the compositor-first GUI smoke (`tests/musl_ext2_gui_smoke.sh`), and setup coverage for the separate GUI demo entrypoint.

The host unit tests (`tests/unit.sh`) build the embedded default-build user binaries first (`make user-elf`), so `nasm` and `ld` are needed even for unit tests. Generated user ELFs under `user/` are build outputs and are not tracked in git.

The harness expects to run under Linux or WSL bash. `.gitattributes` forces LF line endings in the working tree, so a Windows checkout's scripts also run under WSL.

## Historical Reference

If you need behavior comparisons with the earlier implementation, use:

```bash
../peetos_c/tests/run_tests.sh
../peetos_c/tests/run_shell_cmds.sh -clean "echo hello" "ls /bin"
```

## Notes

- The kernel is compiled with `no_std` and `panic=abort`.
- The bootloader and linker script are reused from the C codebase for stable early boot.
