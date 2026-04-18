# PeetOS Kernel Status and Roadmap

This file keeps the historical `PORTING_PLAN` name, but it now tracks the
current Rust kernel implementation and forward roadmap.

## Scope

- Rust PeetOS is the primary implementation.
- `../peetos_c` is a historical comparison reference, not the active target.
- Stage 1-9 parity goals are largely complete; remaining work focuses on
  hardening, completeness, and new capability.

## Status Snapshot (Last reviewed: 2026-04-12)

### Kernel Foundation

- Bootloader handoff, higher-half entry, serial, panic, and exception reporting are in place.
- CPU setup is in place: GDT/TSS/IDT, PIC/PIT, SYSCALL MSRs, and preemption hooks.
- Memory management is in place: typed addresses, bitmap PMM, 4-level VMM, and kernel heap.

### Process and Syscall Layer

- Preemptive scheduler, kernel threads, and user-mode bootstrap are active.
- ELF loading supports `ET_EXEC` + `ET_DYN` bootstrap paths with `PT_INTERP`
  handoff and hardened segment validation.
- Table-driven syscall dispatch is active across SYSCALL/int80 paths.
- Implemented syscall families now cover core file I/O, process control,
  memory mapping, IPC/signal baseline, metadata/time/identity calls, and
  socket-based networking APIs used by current userspace.
- Shared-buffer syscall baseline now includes `memfd_create` with tmpfs-backed
  internal objects (`MFD_CLOEXEC` support; sealing deferred with `-ENOSYS`).
- Descriptor flag semantics now include `fcntl(F_SETFL)` runtime `O_NONBLOCK`
  updates for sockets/pipes/PTYs, plus stricter `accept4`/`socket`/`socketpair`
  flag validation paths for event-loop bring-up.
- Phase 2 hardening now enforces per-task caps for shared-buffer objects,
  mapped VMA pages, and epoll watch ownership, with explicit teardown cleanup
  for epoll owner watches and user address-space mappings on task exit.

### Storage and Filesystems

- VFS is active with Linux-like mount topology: block-backed ext2/ext3 at `/` plus pseudo mounts at `/boot`, `/temp`, `/dev`, and `/proc` (with runtime `/etc/fstab` declaring the root block mount as `ext3`, and transitional `/mnt/ext2` compatibility remap when root ext2 is active).
- ext2 supports mutable file/directory operations, direct+indirect block
  mapping, truncate growth/shrink behavior, and sync/flush plumbing.
- Internal ext journal handling (surfaced as ext3 mounts) supports mount-time
  replay plus runtime journal transaction emission with deferred checkpointing
  and revoke handling, with baseline runtime journal abort-on-I/O-fault state.
- Ordered-mode journaling now uses bounded credits for most mutators, plans
  variable-size paths around capped per-segment reservations, rotates those
  restartable scopes at block-walking loop boundaries and final inode closeout,
  now uses indirection-aware allocation/free step estimates instead of a flat
  loop heuristic, lets allocator/free helpers request restart headroom for
  their own metadata phases, now splits allocation helpers into bookkeeping
  versus initialization/link phases, gives truncate-side detach/free work
  explicit unlink-table versus free/revoke restart boundaries, records restart
  points as named journal phases on sealed ordered segments, now groups the
  actual metadata work for those phases through closure-scoped restart-phase
  helpers, now threads a lightweight internal journal-handle token through the
  main ordered write/truncate/alloc/free paths and the fixed-shape directory
  metadata mutators, and can split mutations into multiple sealed ordered
  transaction segments when they outgrow an initial credit reservation.
- ext2/ext3 feature acceptance is intentionally conservative: mounts currently
  allow `HAS_JOURNAL` in compat bits, `FILETYPE`/`RECOVER`/`JOURNAL_DEV` in
  incompat bits, and `SPARSE_SUPER` in ro-compat bits. Images carrying
  `EXT_ATTR`, `DIR_INDEX`, `RESIZE_INO`, `IMAGIC_INODES`, `DIR_PREALLOC`, or
  `LARGE_FILE` are rejected until the corresponding behavior exists.
- Block cache residency is bounded with CLOCK eviction and dirty-pressure flush.

### IPC and Signals

- In-kernel `SpinLock`, `Mutex`, and `Semaphore` primitives are active.
- Pipes, futex wait/wake baseline, System V shared memory baseline, and
  user-handler signal delivery (`SA_RESTORER`) are wired.

### Drivers, Display, and Input

- PCI enumeration is active.
- AHCI DMA and ATA identify paths are active with block-device inventory.
- PS/2 keyboard + canonical TTY reads are active.
- PS/2 mouse plus optional UHCI USB-tablet absolute input are active.
- Framebuffer handoff, text mirroring, cursor rendering, and basic ANSI color
  handling are active.
- Baseline Phase 3 display ABI is now present: `/dev/fb0` exists in `devfs`,
  with `ioctl(FBIOGETINFO)` metadata query support plus direct shared userspace
  `mmap` support.
- Baseline Phase 3 input ABI is now present: `/dev/input/event0` (keyboard)
  and `/dev/input/event1` (pointer) expose evdev-style timestamped records with
  nonblocking `read`, `poll`/`select`/`epoll` readiness, and `SYN_DROPPED`
  overflow signalling.
- Phase 3 display ownership is now explicitly gated: default boots remain
  console-first, while the `compositor-first` kernel feature suppresses kernel
  framebuffer console/cursor composition, keeps panic/oops fallback readable,
  and restores a visible console if init launch fails before userspace takes
  over the display.
- Phase 3 userspace bring-up now includes a compositor-first ext2 smoke profile:
  `musl-ext2-gui-smoke` launches an ext2-hosted helper that validates exported
  framebuffer/input environment variables, opens `/dev/fb0` and
  `/dev/input/event{0,1}`, maps and writes the framebuffer, and confirms
  injected keyboard/pointer events arrive through the userspace input-device
  path under `compositor-first`.
- Phase 3 compositor fallback policy now extends past init-launch failure:
  when the compositor-owned initial userspace task exits or crashes after
  successful takeover, the kernel restores framebuffer ownership to the console,
  redraws the fallback UI, and exposes that transition through a dedicated QEMU
  smoke target.
- Phase 3 console-first smoke coverage is now explicit: a dedicated shell smoke
  runner asserts the kernel keeps display ownership in console mode while the
  scripted Toybox workload completes under the default profile.
- The repository test harness is green again with current Phase 3 coverage:
  `make test` now includes the default `test-exit` QEMU smoke alongside the
  dedicated console-first/compositor-first/fallback smoke scripts and host
  unit suites without known red paths.
- Phase 4 bring-up has started with a compositor-core smoke profile:
  `musl-ext2-peet-gui-smoke` boots a compositor-first userspace helper that
  renders two overlapping surfaces to `/dev/fb0`, validates initial z-order
  plus raise-on-focus behavior, routes injected keyboard/pointer activity
  through the initial compositor event loop, validates a damage-limited
  partial redraw/commit path, and verifies that one client can detach cleanly
  without destabilizing the compositor-owned session. Real multi-process
  `peet-gui` transport is still pending.
- A separate `musl-ext2-peet-shell-demo` profile now scaffolds a more realistic
  visual shell on the same compositor-first device ABI: top bar, launcher,
  terminal/files/inspector panels, pointer cursor, focus switching, theme
  cycling, and terminal-style command feedback are rendered through an internal
  compositor path where each panel has its own surface buffer and focus raises
  the active panel in z-order. The `FILES` panel now runs as a separate forked
  userspace client over an `AF_UNIX socketpair` control channel, giving GUI
  work a more realistic visual target before full buffer handoff is finished.

### Networking

- E1000 driver, Ethernet, ARP, IPv4, ICMP, UDP, TCP, and BSD socket syscall
  layers are implemented for current userspace workloads.
- `select`/`poll`/`ppoll` plus baseline `epoll_create1`/`epoll_ctl`/`epoll_wait`
  and baseline `sendmsg`/`recvmsg` paths are implemented, including
  `SCM_RIGHTS` fd passing for Unix-domain streams.
- Local socket baseline now includes `AF_UNIX` stream `socket()`/`socketpair()`
  plus pathname `bind`/`connect`/`listen`/`accept`, with pathname conflict
  handling tied to VFS namespace entries.
- Dedicated AF_UNIX/SCM_RIGHTS QEMU smoke coverage now validates cross-process
  fd transfer plus malformed ancillary/error-path behavior.

### Userspace Runtime Bring-Up

- musl sysroot bootstrap is integrated.
- toybox staging supports interactive shell and broad applet coverage,
  including networking applets used by current smoke paths.
- QuickJS, MicroPython, and Dropbear are staged with dedicated smoke targets.

## Remaining Work

### 1. Process and Runtime Semantics

- Complete Linux-like `clone`/thread semantics (`CLONE_THREAD`, robust TLS/TID behavior, thread groups).
- Harden job-control and controlling-tty behavior beyond current baseline.
- Expand signal semantics (`sigaltstack`, richer `siginfo_t`/ucontext behavior, queued RT payloads).
- Finish dynamic-linker/runtime completeness beyond current bootstrap path
  (relocations/symbol resolution and broader compatibility).

### 2. VM and Memory Policy

- Improve reclaim policy beyond current best-effort file-backed reclaim.
- Continue page-cache unification and add broader mmap/shared-buffer stress coverage.
- Add hardening around user-pointer fault recovery and broader VM accounting
  beyond current per-task mapping caps.

### 3. Filesystem and Block Stack

- Add partition parsing (MBR/GPT) for block-device mounts.
- Continue ext2/ext3 hardening and fs/page-cache integration.
- Add additional filesystem support (for example FAT-family support) as needed.
- Improve durability guarantees around writeback scheduling and crash handling.

### 4. Driver and Platform Expansion

- Move more polling paths to interrupt-driven completion where appropriate
  (storage/network paths still contain polling-heavy flows).
- Broaden USB/input/device coverage beyond the current focused baseline.
- Continue framebuffer/terminal work toward fuller VT and pty semantics,
  including runtime compositor/console ownership negotiation, compositor-exit
  policy hardening beyond the current init-owned handoff, and richer input
  semantics.

### 5. Networking Completeness

- Fill remaining socket API gaps (fuller ancillary semantics beyond current
  `SCM_RIGHTS`, Unix abstract-namespace semantics, and advanced epoll variants).
- Continue protocol hardening (congestion behavior, out-of-order handling,
  robustness under load).

### 6. Userspace Coverage

- Expand staged userspace utility and runtime coverage.
- Replace targeted smoke checks with broader integration suites as syscall and
  runtime behavior stabilize.
- Keep the dedicated AF_UNIX/SCM_RIGHTS smoke (`run-unix-rights-smoke`) in the
  regression workflow and add dedicated epoll compositor-loop smokes for Phase 2.

## Tracking Conventions

- Keep this file focused on what is implemented and what remains.
- Keep subsystem-specific detail in dedicated docs under `docs/`.
- Keep every subsystem doc updated with a `Limitations / Simplifications` section.
