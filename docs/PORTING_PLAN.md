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
- SMP phase 1 is in place (`docs/smp.md`):
  - ACPI MADT discovery.
  - LAPIC + IOAPIC interrupt delivery, replacing the 8259 PIC when a MADT exists.
  - INIT/SIPI bring-up of every AP through a real-mode trampoline.
  - An IPI self-test.
  - `tests/smoke.sh` boots with `-smp 1` and `-smp 4`, and checks that timer ticks arrive through the IOAPIC.
  - APs park after check-in; the scheduler still runs only on the BSP.
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

### 1a. SMP Phase 2 (Scheduling on All Cores)

- Add per-CPU state via GS base: CPU id, `current` task, idle task, and a per-CPU GDT/TSS with RSP0 (and later IST stacks).
- Make the scheduler multi-core: either a big kernel lock as a first step, or per-CPU run queues with a global lock and IPI-driven reschedule.
- Add TLB shootdown IPIs for `munmap`/`mprotect`/COW and address-space teardown.
- Replace "interrupts off" critical sections in the PMM, heap, VMM, VFS, net, and syscall globals with real locks. That needs an IRQ-safe `SpinLock` that also masks interrupts.
- Calibrate the LAPIC timer and use it as the per-CPU preemption tick.

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
