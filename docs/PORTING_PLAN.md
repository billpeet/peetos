# PeetOS Kernel Status and Roadmap

This file keeps the historical `PORTING_PLAN` name, but it now tracks the
current Rust kernel implementation and forward roadmap.

## Scope

- Rust PeetOS is the primary implementation.
- `../peetos_c` is a historical comparison reference, not the active target.
- Stage 1-9 parity goals are largely complete; remaining work focuses on
  hardening, completeness, and new capability.

## Status Snapshot (Last reviewed: 2026-02-28)

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

### Storage and Filesystems

- VFS is active with Linux-like mount topology: block-backed ext2/ext3 at `/` plus pseudo mounts at `/boot`, `/temp`, `/dev`, and `/proc` (with runtime `/etc/fstab` declaring the root block mount as `ext3`, and transitional `/mnt/ext2` compatibility remap when root ext2 is active).
- ext2 supports mutable file/directory operations, direct+indirect block
  mapping, truncate growth/shrink behavior, and sync/flush plumbing.
- Internal ext journal handling (surfaced as ext3 mounts) supports mount-time
  replay plus runtime journal transaction emission with deferred checkpointing
  and revoke handling, with baseline runtime journal abort-on-I/O-fault state.
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

### Networking

- E1000 driver, Ethernet, ARP, IPv4, ICMP, UDP, TCP, and BSD socket syscall
  layers are implemented for current userspace workloads.
- `select`/`poll`/`ppoll` + `recvmsg` are implemented.

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
- Add stronger mmap/fork sharing semantics and continue page-cache unification.
- Add hardening around user-pointer fault recovery and broader VM accounting.

### 3. Filesystem and Block Stack

- Add partition parsing (MBR/GPT) for block-device mounts.
- Continue ext2/ext3 hardening and fs/page-cache integration.
- Add additional filesystem support (for example FAT-family support) as needed.
- Improve durability guarantees around writeback scheduling and crash handling.

### 4. Driver and Platform Expansion

- Move more polling paths to interrupt-driven completion where appropriate
  (storage/network paths still contain polling-heavy flows).
- Broaden USB/input/device coverage beyond the current focused baseline.
- Continue framebuffer/terminal work toward fuller VT and pty semantics.

### 5. Networking Completeness

- Fill remaining socket API gaps (`epoll`, fuller `sendmsg`, Unix-domain sockets).
- Continue protocol hardening (congestion behavior, out-of-order handling,
  robustness under load).

### 6. Userspace Coverage

- Expand staged userspace utility and runtime coverage.
- Replace targeted smoke checks with broader integration suites as syscall and
  runtime behavior stabilize.

## Tracking Conventions

- Keep this file focused on what is implemented and what remains.
- Keep subsystem-specific detail in dedicated docs under `docs/`.
- Keep every subsystem doc updated with a `Limitations / Simplifications` section.
