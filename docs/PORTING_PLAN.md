# PeetOS Rust Porting Plan

This document captures a high-level roadmap to reimplement the existing PeetOS C kernel in Rust from scratch, while preserving feature parity with the current C implementation.

## Parity Target (Source of Truth)

The current feature set is summarized in `../peetos_c/docs/ROADMAP.md` and the kernel sources in `../peetos_c/kernel/src`. The Rust port should aim for parity with Stages 1–9 before attempting Stage 10 optimizations.

## Current Status (Last reviewed: 2026-02-11)

- Phase 0: Complete. Workspace, subsystem crates, bootloader/linker reuse, Makefile, `.cargo/config.toml`, and kernel entry assembly are present.
- Phase 1: Complete. The kernel boots to Rust `kmain()`, initializes serial output, prints a banner, and halts in a loop. The panic handler reports over serial before halting.
- Phase 2: Complete (baseline). Added `arch_x86_64` port I/O helpers, interrupt enable/disable wrappers (plus RAII guard), GDT/IDT setup with interrupt stubs and handler registration, PIC remap, PIT timer init, syscall entry/MSR setup, and a serial exception report.
- Phase 3: Complete (baseline). Added typed address helpers, bitmap-based PMM, VMM mapping/address-space support, and a kernel heap allocator.
- Phase 4: In progress. Baseline tasking + scheduler (kernel threads, run queue, PIT-driven preemption, yield/block/sleep paths) are in place. A minimal user-mode task now boots via `int 0x80` with a Linux-numbered syscall subset (`open`/`openat`, `mkdirat`, `close`, `read`, `write`, `lseek`, `fstat`, `newfstatat`, `getdents64`, `unlinkat`, `renameat`, `rt_sigaction`, `rt_sigprocmask`, `rt_sigreturn`, `kill`, `shmget`, `shmat`, `shmdt`, `shmctl`, `exit`/`exit_group`, `yield`, `getpid`, `getppid`, `gettid`, `set_tid_address`, `brk`) backed by a table-driven shared SYSCALL/int80 dispatcher and per-task user metadata (`brk`, stack bounds, initial stack pointers, ELF PHDR/base/dynamic flags, per-task FD table, cwd inode). Initial user bootstrap now builds `argc`/`argv`/`envp`/`auxv` for ring-3 entry, and a minimal dynamic-linking bootstrap is wired (`PT_INTERP` mapping + interpreter-first entry with `AT_ENTRY`/`AT_BASE`). The ELF loader includes stricter header/segment validation (alignment checks, malformed `PT_INTERP` rejection, RWX-segment rejection, entrypoint-in-exec-range checks), plus `ET_EXEC` loading and `ET_DYN` metadata support (`PT_INTERP`, `PT_DYNAMIC`, `load_at`). NXE is enabled in `IA32_EFER` so non-executable mappings are enforced. Full musl/toybox syscall coverage and full reloc/symbol dynamic linker semantics remain outstanding.
- Phase 5: In progress (expanded baseline). Added a minimal VFS with concrete tmpfs regular-file behavior, virtual fs backends for `devfs` (`/dev/null`, `/dev/zero`, `/dev/console`) and `procfs` (`/proc/uptime`, `/proc/meminfo`, `/proc/mounts`, `/proc/version`), a mount table abstraction, and a baseline ext2 importer (`mount_ext2_image`) used to mount an embedded sample image at `/mnt/ext2` with direct-block ext2 read/write/create/mkdir/truncate/unlink/rmdir/rename support that updates inode/block bitmaps and ext2 metadata in the mounted image. Syscall wiring for baseline file-descriptor and metadata operations is in place over these backends.
- Phase 6: In progress (baseline+). Added in-kernel synchronization primitives (`SpinLock`, `Mutex`, `Semaphore`) plus IPC plumbing for anonymous pipes, System V-style shared memory, futex wait/wake, and signal delivery (`kill`, `rt_sigaction`, `rt_sigprocmask`, `rt_sigreturn`, default/ignore actions, user-handler delivery via `SA_RESTORER`, syscall-boundary pending delivery, `SIGPIPE` generation on broken pipes). Syscall integration includes `pipe`/`pipe2`, `shmget`/`shmat`/`shmdt`/`shmctl(IPC_RMID)`, `futex`, and signal syscalls, with fd-close teardown for pipe endpoints, shared-memory detach on task exit, and best-effort `clear_child_tid` wake on task exit.
- Phase 7: In progress (baseline+). Added a PCI config-space enumerator (mechanism #1 via `0xCF8`/`0xCFC`) in the `drivers` crate, wired during kernel bring-up with bounded serial reporting of discovered device functions. Added ATA/AHCI discovery over PCI class codes (IDE mode classification + AHCI controller detection/ABAR capture), AHCI command-register setup (`MEM_SPACE` + `BUS_MASTER`), AHCI HBA MMIO capability/port signature probing, AHCI command-engine `IDENTIFY` DMA on active SATA ports, ATA PIO `IDENTIFY` probing on IDE channels, and a minimal in-kernel block-device inventory.
- Phases 8–10: Not started.

## Phase 0: Project Scaffolding

- Establish a Rust workspace with `kernel` plus subsystem crates (`arch_x86_64`, `drivers`, `fs`, `net`, `syscall`, `userabi`).
- Reuse bootloader assembly and linker scripts from the C codebase.
- Add build tooling (`Makefile`, `.cargo/config.toml`) and a minimal kernel entry.

## Phase 1: Kernel Bring-Up

- Boot to a Rust `kmain()` using the existing stage1/stage2 bootloader.
- Implement serial output and a minimal panic handler.
- Verify a clean boot to `hlt` loop in QEMU.

## Phase 2: CPU + Interrupts

- Port I/O helpers and safe interrupt enable/disable wrappers.
- RAII-style critical section guard for interrupt masking.
- GDT/IDT setup with interrupt stubs and handler registration.
- PIC remap and PIT timer init (IRQ0).
- Syscall entry stub and MSR setup.
- Exception handling details (serial report; VGA optional).
- Syscall entry stub and interrupt context framing.
- Establish safe Rust wrappers for interrupt enable/disable and critical sections (basic `enable`/`disable` + `without_interrupts` done; scoped guards pending).

## Phase 3: Memory Management

- Physical frame allocator (bitmap allocator) - done.
- Virtual memory manager (4-level paging, higher-half kernel) - done.
- Kernel heap allocator with safe wrappers - done.
- Introduce typed addresses (`PhysAddr`, `VirtAddr`) and RAII mappings - done.

## Phase 4: Tasking + Syscalls

- Task structs, context switching, scheduler.
- User mode, ELF loader, per-process address spaces.
- Implement syscall dispatch surface required by musl/toybox.

## Phase 5: VFS + Filesystems

- VFS core (inode/dentry model).
- tmpfs, devfs, procfs.
- ext2 baseline support (image import + mount + direct-block read/write/create/mkdir/truncate with inode/block bitmap updates) done; block-device-backed persistence, indirect blocks, and journal semantics pending.
- FAT32 support pending.
- Block device layer and partitioning.

## Phase 6: IPC + Sync

- Spinlocks, mutexes, semaphores.
- Pipes, shared memory, futexes, and signal delivery baseline (including `rt_sigreturn` + `SA_RESTORER` handler return) done.
- Advanced signal semantics (alternate stacks, queued RT signals, full `siginfo_t`/ucontext compatibility) pending.

## Phase 7: Device Drivers

- PCI enumeration baseline done (full bus/device/function scan over PCI config mechanism #1 with serial inventory output during boot).
- ATA/AHCI bring-up in progress: PCI-based controller discovery + AHCI BAR capture/command-register enablement + AHCI MMIO capability/port probing + AHCI command-engine `IDENTIFY` DMA + ATA `IDENTIFY` probing + minimal block-device inventory are done; general AHCI DMA read/write paths and VFS-facing block integration remain pending.
- TTY, keyboard, framebuffer.

## Phase 8: Networking

- E1000 NIC driver.
- Ethernet, ARP, IPv4, ICMP, UDP, TCP.
- BSD sockets interface.

## Phase 9: User Space Integration

- Dynamic linker support.
- musl integration and toybox utilities.
- Validate full shell usage and test scripts.

## Phase 10: Post-Parity Improvements (Optional)

- SMP, ACPI, demand paging, COW fork.
- Page cache, async I/O, security hardening.

## Parity Verification

- Re-run the shell test scripts from the C project (`../peetos_c/tests/*.sh`).
- Validate the “What Works” list in `../peetos_c/docs/ROADMAP.md`.
 - Use `make test` for Rust-port checks (host unit tests plus QEMU serial banner check; QEMU exits via `isa-debug-exit`).
