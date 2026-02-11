# Subsystem Crate Stubs

This document summarizes subsystem crates. `fs` and `drivers` now contain active phase work, while `net`, `syscall`, and `userabi` remain placeholders.

**Where They Live**
- `crates/drivers`
- `crates/net`
- `crates/syscall`
- `crates/userabi`
- `crates/fs`

**Current State**
- `crates/fs` now provides a no-std baseline VFS core (concrete tmpfs + virtual `devfs`/`procfs` + ext2 image import/mount/mutate support) used by kernel syscall dispatch (`open`/`openat`/`close`/`read`/`write`/`lseek`/`mkdirat`/`fstat`/`newfstatat`/`getdents64`/`unlinkat`/`renameat`).
- `crates/drivers` now provides a phase-7 baseline PCI enumerator plus ATA/AHCI discovery scaffolding (storage-class matching, IDE/AHCI classification, AHCI ABAR extraction, and AHCI capability/port decode helpers), with kernel bring-up using this data for AHCI MMIO probing and ATA IDENTIFY diagnostics.
- `crates/net`, `crates/syscall`, and `crates/userabi` remain placeholders for later phases.

Limitations / Simplifications
- `crates/fs` currently provides tmpfs + minimal virtual `devfs`/`procfs` and a baseline ext2 image-backed path, but lacks a real block-device cache/flush stack, FAT support, journaling, and many ext2 edge cases (indirect blocks, full feature flags).
- `crates/drivers` currently implements PCI discovery and storage-controller discovery metadata; full ATA/AHCI data paths and input/display driver stacks are still pending.
- Public subsystem APIs are still evolving; only selected `fs`/`drivers` surfaces are wired into the kernel build today.
