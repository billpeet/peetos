# PeetOS Rust Porting Plan

This document captures a high-level roadmap to reimplement the existing PeetOS C kernel in Rust from scratch, while preserving feature parity with the current C implementation.

## Parity Target (Source of Truth)

The current feature set is summarized in `../peetos/docs/ROADMAP.md` and the kernel sources in `../peetos/kernel/src`. The Rust port should aim for parity with Stages 1–9 before attempting Stage 10 optimizations.

## Current Status (Last reviewed: 2026-02-07)

- Phase 0: Complete. Workspace, subsystem crates, bootloader/linker reuse, Makefile, `.cargo/config.toml`, and kernel entry assembly are present.
- Phase 1: Complete. The kernel boots to Rust `kmain()`, initializes serial output, prints a banner, and halts in a loop. The panic handler reports over serial before halting.
- Phase 2: Complete (baseline). Added `arch_x86_64` port I/O helpers, interrupt enable/disable wrappers (plus RAII guard), GDT/IDT setup with interrupt stubs and handler registration, PIC remap, PIT timer init, syscall entry/MSR setup, and a serial exception report.
- Phases 3–10: Not started.

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

- Physical frame allocator (bitmap allocator).
- Virtual memory manager (4-level paging, higher-half kernel).
- Kernel heap allocator with safe wrappers.
- Introduce typed addresses (`PhysAddr`, `VirtAddr`) and RAII mappings.

## Phase 4: Tasking + Syscalls

- Task structs, context switching, scheduler.
- User mode, ELF loader, per-process address spaces.
- Implement syscall dispatch surface required by musl/toybox.

## Phase 5: VFS + Filesystems

- VFS core (inode/dentry model).
- tmpfs, devfs, procfs.
- ext2 and FAT32 read/write support.
- Block device layer and partitioning.

## Phase 6: IPC + Sync

- Spinlocks, mutexes, semaphores.
- Signals, pipes, shared memory, futexes.

## Phase 7: Device Drivers

- PCI enumeration.
- ATA/AHCI storage drivers.
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

- Re-run the shell test scripts from the C project (`../peetos/tests/*.sh`).
- Validate the “What Works” list in `../peetos/docs/ROADMAP.md`.
 - Use `make test` for Rust-port checks (host unit tests plus QEMU serial banner check; QEMU exits via `isa-debug-exit`).
