# PeetOS Kernel Status and Roadmap

This file keeps the historical `PORTING_PLAN` name, but it now tracks the
current Rust kernel implementation and forward roadmap.

## Scope

- Rust PeetOS is the primary implementation.
- `../peetos_c` is a historical comparison reference, not the active target.
- Stage 1-9 parity goals are largely complete; remaining work focuses on
  hardening, completeness, and new capability.

## Status Snapshot (Last reviewed: 2026-11-08)

### Kernel Foundation

- Bootloader handoff, higher-half entry, serial, panic, and exception reporting are in place.
- CPU setup is in place: GDT/TSS/IDT, PIC/PIT, SYSCALL MSRs, and preemption hooks.
- SMP phases 1 and 2 are in place (`docs/smp.md`):
  - ACPI MADT discovery.
  - LAPIC + IOAPIC interrupt delivery, replacing the 8259 PIC when a MADT exists.
  - INIT/SIPI bring-up of every AP through a real-mode trampoline.
  - An IPI self-test.
  - `tests/smoke.sh` boots with `-smp 1` and `-smp 4`, and checks that timer ticks arrive through the IOAPIC.
  - Every CPU runs tasks. This uses:
    - per-CPU GS/GDT/TSS,
    - a shared run queue with per-CPU current/idle,
    - a LAPIC timer tick on the APs,
    - reschedule and TLB-shootdown IPIs.
  - Kernel code is serialised by a big kernel lock (a FIFO ticket lock whose ownership follows tasks across switches).
  - The scheduler, PMM and heap have their own interrupt-safe locks.
  - Timer ticks, reschedule IPIs and context switches never take the big lock. A smoke test proves that tasks keep switching on other CPUs while one CPU holds it.
  - `tests/smoke.sh` checks that kernel threads run on more than one CPU, and the userspace suites pass at `-smp 4`.
- Memory management is in place: typed addresses, bitmap PMM, 4-level VMM, and kernel heap.

### Process and Syscall Layer

- Preemptive scheduler, kernel threads, and user-mode bootstrap are active.
- Per-task x87/SSE state is switched eagerly with `FXSAVE`/`FXRSTOR`, copied on fork, reset on exec, and saved/restored around signal handlers. The `musl-ext2-fpu-smoke` QEMU profile (`make run-fpu-smoke`) covers all three; before this, user XMM/MXCSR state leaked between preempted processes.
- `fork` is copy-on-write (PMM share counts + `PTE_COW` write faults), and
  `clone(CLONE_THREAD)` thread groups run musl pthreads (mutexes, condvars,
  TLS, join, `tgkill`, process-wide exit). Covered by `make run-proc-smoke`;
  see `docs/process_model.md`.
- Signals follow the Linux ABI: `rt_sigframe` with `siginfo_t`/`ucontext_t`,
  signals 1..=64, `SA_RESTART`-aware syscall restart for interrupted waits,
  futex timeouts and requeue. musl `pthread_cancel`,
  `pthread_cond_timedwait` and `execve` from any thread work.
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
- Images formatted with stock `mke2fs -t ext3` defaults now mount read-write:
  `ext_attr`, `resize_inode`, `dir_index`, and `large_file` are accepted, each
  with explicit handling (xattr block accounting and release, maintained htree
  indexes, and 64-bit file sizes). Fast
  symlinks are read and created inline. `tests/ext3_fsck.sh` requires
  `e2fsck -fn` to stay clean after kernel-side mutations of a stock ext3 image.
- Ordered-journal fixes: freeing a block dirtied in the same transaction now
  "forgets" it (revoke only, as in JBD), reuse cancels a pending revoke, and
  sealed transactions left with only revokes (or nothing at all) no longer wedge
  the pending queue, which previously made batched flushes fail with `EIO`.
- Crash consistency: the runtime used to write the next transaction ID into the
  journal superblock while `s_start` pointed at an older live transaction, so
  recovery replayed nothing after a crash. `s_sequence` now follows JBD, and
  `INCOMPAT_RECOVER` is set while the log holds committed transactions.
  `tests/ext3_fsck.sh` makes `e2fsck` replay a kernel-written crash image.
- Transaction atomicity: each sealed ordered transaction now commits as one
  journal transaction (dirty-pressure writeback used to tear it into one-block
  commits), newer transactions keep their own snapshots of shared metadata
  blocks, and committed revokes remove freed blocks from older checkpoint
  lists so checkpointing cannot overwrite reused file data.
- The runtime musl image (`scripts/build_ext2_musl_execve.sh`) is now built
  with stock `mke2fs` features (no `-O ^dir_index,...` overrides).
- Journal write barriers: the ext2 backend has a flush hook, implemented in the
  kernel by `block::flush_device` (block-cache drain + AHCI `FLUSH CACHE EXT`).
  The journal issues barriers around each commit record and before retiring
  checkpointed log space, and mount-time recovery retires the log only after
  replayed blocks are durable. `sync`/`fsync`/`msync` now also flush the drive
  cache. Transactions may span several descriptor blocks, so sealed
  transactions commit atomically up to the size of the journal. A
  reordering-write-cache crash test checks ~230 crash points with `e2fsck`.
- ext3 runtime semantics: mutating syscalls commit (durable) and `sync`/`fsync`
  checkpoint. Commits carry JBD2 CRC32 checksums, journals with unknown JBD2
  incompat features are refused, and each commit costs one barrier. A new
  QEMU smoke (`make run-journal-smoke`, `tests/musl_ext3_journal_smoke.sh`)
  SIGKILLs QEMU mid-workload on an ext3 root and checks recovery by both
  `e2fsck` and the kernel; it found two stale-cache bugs (partial checkpoint
  rolling back cached bitmaps, eviction of uncheckpointed blocks) that caused
  double allocation, both fixed.
- Block cache residency is bounded with CLOCK eviction and dirty-pressure flush.

### IPC and Signals

- `eventfd`, `timerfd`, and `signalfd` are implemented as pollable fd kinds (`kernel/src/evfd.rs`), alongside `flock`, `fallocate`, memfd sealing, `mremap`, and `SO_PEERCRED`, which together cover everything libwayland 1.23 needs from the kernel.
- Every fd duplication path (dup, fork, `SCM_RIGHTS`) now retains the underlying object through one helper. Before, `dup`/`dup2` did not retain pipes, so `dup2(pipe, 1); close(pipe)` dropped the pipe's writer or reader count early.

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
  and restores a visible console if init launch fails or the tracked userspace
  display owner later exits after taking over the framebuffer.
- A dedicated Phase 3 compositor-first smoke profile is now wired through the
  userspace init path (`musl-ext2-gui-smoke`): a userspace compositor process
  now maps `/dev/fb0`, polls `/dev/input/event0` and `/dev/input/event1`,
  composites multiple smoke-client surfaces received over inherited `AF_UNIX`
  stream channels, and is covered by a headless QEMU regression target that
  also verifies console fallback after compositor exit.
- A separate compositor-first demo profile (`musl-ext2-gui-demo`) now reuses
  the same userspace compositor stack in a manual QEMU bring-up path with a
  larger multi-surface layout and routed client input feedback.
- The current landed Phase 4 compositor-MVP baseline is now complete and includes dedicated
  compositor/client process topology, compositor-owned framebuffer/input
  handling, and shared-buffer attach/commit over inherited `AF_UNIX` streams
  using `memfd_create` + `SCM_RIGHTS`, explicit create/attach/commit/damage/
  destroy surface lifecycle with pending vs committed state, conservative
  dirty-region redraw/coalescing with clipped framebuffer presentation,
  bounded client/buffer limits, host unit coverage for protocol parsing/object
  validation/scene transitions/dirty accumulation, smoke coverage for
  malformed-client rejection, invalid-fd and stale post-destroy message
  rejection, transient client disconnect survival, baseline hover pointer
  focus plus click-to-focus keyboard routing to an input-aware client, and a
  green default regression run through `make test`.
- The console-first shell smoke now explicitly asserts that the kernel keeps
  framebuffer ownership in console mode, but the broader toybox shell workflow
  still needs runtime stabilization before Phase 3 can be considered fully
  closed.

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

- libwayland 1.23.1 and libffi 3.4.6 build as static musl libraries (`make -C wayland`), and the `musl-ext2-wayland-smoke` profile runs a libwayland compositor plus a separate libwayland client end to end in QEMU (`docs/wayland.md`).
- 21 of upstream libwayland's own test binaries (131 cases, all but the `dlsym`-based `os-wrappers-test` and the host-only scanner/C++ checks) pass in QEMU in the `musl-ext2-wayland-tests` profile, part of `make test`. Getting there fixed several kernel gaps: `mkdir`/`rmdir`, `/dev/fd`, unlinked-but-open (and mapped) files, `fstat` on non-file fds, `setitimer`/`alarm`, `WIFSIGNALED` wait status, `MAP_SHARED | MAP_ANONYMOUS`, AF_UNIX path lifetime, and the fd/socket/flock/directory capacity limits. See `docs/wayland.md`.
- The `peetwl` Wayland compositor (libwayland-server + xdg-shell from wayland-protocols 1.38) owns `/dev/fb0` and `/dev/input` in compositor-first mode, with server-side decorations, click-to-focus, title-bar move and close. `make run-wlcomp-smoke` verifies it with injected PS/2 input and screendumps; `make run-wayland-demo` is the interactive session.
- memfd contents are now page-backed (up to 64 MiB) instead of being limited to tmpfs's 32 KiB inline files, and the kernel raises `SIGCHLD` when a child process exits.
- Dynamic linking works: musl's shared `libc.so` runs as `/lib/ld-musl-x86_64.so.1` and loads `NEEDED` libraries, `dlopen` plugins and shared-object TLS (`make run-dynlink-smoke`, part of `make test`; see "Dynamic Linking" in `docs/elf.md`). `mprotect` now covers the execve-loaded program/loader range for RELRO, and `kfree` of `kmalloc_aligned` buffers no longer misreads stale bytes as a block header.
- libwayland, libffi and libxkbcommon also build as shared libraries (staged in `/usr/lib`), and `peetwl`/`wl_demo` now link dynamically against them; `make run-wlcomp-smoke` covers it.
- The VM region table grew from 32 to 256 slots per task. The table is no longer copied onto the kernel stack: transactional updates use a heap scratch copy, and `mprotect` re-merges the regions it splits. `make run-dynlink-smoke` checks 120 separate mappings alongside the loaded shared libraries.
- Platform limits raised for desktop workloads:
  - The kernel now has a direct map of all RAM, so the PMM manages up to 4 GiB (it was capped at the bootloader's 128 MiB window).
  - Per-process mapping budget: 256 MiB.
  - memfd objects: up to 256 MiB.
  - Tasks: 256.
  - Sockets: 256.
  - User-range checks now stop at the end of the lower canonical half, so user mappings can't reach the shared kernel half.
- DRM/KMS: `/dev/dri/card0` is a simpledrm-style device over the boot framebuffer (`docs/drm.md`). It supports legacy mode setting, dumb buffers, page flips with completion events, DIRTYFB and PRIME. libdrm 2.4.123 is ported (`make -C wayland`). `peetwl` now presents through DRM page flips and paces frame callbacks on flip completion. `make run-drm-smoke` covers it, and `make run-wlcomp-smoke` covers it through the compositor.
- The path from here to a full desktop is tracked in `docs/desktop_roadmap.md`.
- libxkbcommon 1.7.0 is ported (`make -C wayland`) with a build-time-verified xkeyboard-config subset staged at `/usr/share/X11/xkb`; `peetwl` serves real XKB keymaps. Adjacent compatible VM regions now merge, which musl's allocator needs for workloads like keymap compilation within the region slots per task (32 then, 256 now).

- musl sysroot bootstrap is integrated.
- toybox staging supports interactive shell and broad applet coverage,
  including networking applets used by current smoke paths.
- QuickJS, MicroPython, and Dropbear are staged with dedicated smoke targets.

## Remaining Work

### 1. Process and Runtime Semantics

- Remaining thread/signal gaps: robust futex lists and `FUTEX_WAKE_OP`, a shared process-wide pending-signal set, queued real-time payloads, `sigaltstack`, and fault delivery as `SIGSEGV`/`SIGBUS` to handlers.
- Per-process `/proc` entries (`/proc/<pid>/{stat,status,cmdline}`, `/proc/self`, `task/` for threads) so `ps`-style tools work.
- Harden job-control and controlling-tty behavior beyond current baseline.
- Add `XSAVE`/AVX state support if userspace workloads need AVX (today only the legacy `FXSAVE` image is managed and `CR4.OSXSAVE` stays off).
- Finish dynamic-linker/runtime completeness beyond current bootstrap path
  (relocations/symbol resolution and broader compatibility).

### 1a. SMP Phase 3 (Breaking Up the Big Kernel Lock)

- Done:
  - the IRQ-safe `IrqSpinLock`,
  - self-locked PMM, heap, VMM and scheduler (run queue, task table, task-owned BKL hand-off),
  - lock-free `sched_yield`/`getpid`/`gettid`/`getppid`/`clock_gettime`/`gettimeofday`/`nanosleep`,
  - user copies made atomic per page under `VMM_LOCK` (stress-tested by the proc smoke's `munmap`-vs-`clock_gettime` race at `-smp 2`),
  - zombies in kernel mode are never preempted.
- Next:
  - give the VFS, net stack, IPC and syscall globals their own locks,
  - replace the global `VMM_LOCK` with per-address-space locks,
  - shrink the BKL until it can be removed.
- Move to per-CPU run queues with load balancing (and later affinity).
- Batch TLB shootdowns per operation (`munmap` ranges, fork COW marking) and consider PCID.
- Deliver signals to tasks running in user mode on another CPU promptly (kick them with a reschedule IPI).
- Route device IRQs to more than the BSP, and add IST stacks for #DF/NMI.

### 2. VM and Memory Policy

- Improve reclaim policy beyond current best-effort file-backed reclaim.
- Continue page-cache unification and add broader mmap/shared-buffer stress coverage.
- Add hardening around user-pointer fault recovery and broader VM accounting
  beyond current per-task mapping caps.

### 3. Filesystem and Block Stack

- Add partition parsing (MBR/GPT) for block-device mounts.
- Continue ext2/ext3 hardening and fs/page-cache integration.
- ext4 compatibility, staged as M0-M5 in `docs/ext4_compat_plan.md` (read-only
  mount first, then extent writes, then `metadata_csum` writes). M0 is done:
  ext4 fixtures, 64-byte group descriptor support, crc32c/crc16 checksum
  verifiers validated against `mkfs.ext4` output, and the block-map seam.
  M1 is done: stock ext4 images mount read-only with extent reads,
  `metadata_csum` verification, and recovery of Linux `64bit`/`csum_v3`
  journals; a QEMU smoke boots from an ext4 root (`make run-ext4-ro-smoke`).
  M2 is done: ext4 without `metadata_csum` mounts read-write (extent tree
  writes, lazy group init, crc16 descriptors, ext4 inode layout), checked by
  `e2fsck`, the crash-point suite, and a QEMU crash/recovery smoke on an
  ext4 root.
  M3 is done: stock `mkfs.ext4` images (`metadata_csum`) mount read-write
  with crc32c checksums maintained on every metadata write and a JBD2
  csum-v3/64bit journal that `e2fsck` replays; read-only is now a mount
  option.
  M4 is done: htree indexes are used for lookups and maintained on insert
  (all Linux hashes, leaf/node splits, two levels, `dx_tail` checksums), and
  the VFS node/child tables grow on demand with lazy single-name ext2
  imports, so directories with thousands of entries work.
  M5 is done: 64-bit file sizes (no 32 KiB or 4 GiB limits, sparse growth),
  post-2038 timestamps, reference-counted VFS files with unlink-while-open
  semantics backed by the ext4 orphan list, orphan processing at mount, and
  `orphan_file` support. Remaining gaps: no wall clock, xattrs not exposed.
- Add additional filesystem support (for example FAT-family support) as needed.
- Improve durability guarantees around writeback scheduling and crash handling.

### 4. Driver and Platform Expansion

- Move more polling paths to interrupt-driven completion where appropriate
  (storage/network paths still contain polling-heavy flows).
- Broaden USB/input/device coverage beyond the current focused baseline.
- Continue framebuffer/terminal work toward fuller VT and pty semantics,
  including runtime compositor/console ownership negotiation and richer input
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
  regression workflow, keep the dedicated compositor-first GUI smoke
  (`run-gui-smoke`) in the regression workflow, and evolve the current
  two-client shared-buffer compositor-session smoke into broader focus-routing,
  disconnect-survival, and scene-management compositor integration as runtime
  semantics harden further.

## Tracking Conventions

- Keep this file focused on what is implemented and what remains.
- Keep subsystem-specific detail in dedicated docs under `docs/`.
- Keep every subsystem doc updated with a `Limitations / Simplifications` section.
