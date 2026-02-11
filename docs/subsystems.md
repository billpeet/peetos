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
- `crates/drivers` now provides phase-7 PCI enumeration plus ATA/AHCI protocol support (storage-class matching, IDE/AHCI classification, AHCI ABAR extraction, AHCI capability/port decode helpers, and AHCI command/FIS table layouts), with kernel bring-up using these surfaces for AHCI MMIO probing, AHCI DMA `IDENTIFY`, AHCI LBA48 DMA read/write commands, ATA PIO `IDENTIFY`, and block-device inventory registration.
- `crates/net`, `crates/syscall`, and `crates/userabi` remain placeholders for later phases.

Limitations / Simplifications
- `crates/fs` currently provides tmpfs + minimal virtual `devfs`/`procfs` and a baseline ext2 image-backed path, but lacks a real block-device cache/flush stack, FAT support, journaling, and many ext2 edge cases (indirect blocks, full feature flags).
- `crates/drivers` currently implements PCI discovery and AHCI/ATA bring-up helpers including baseline DMA command issuance; broad block-layer/VFS integration and input/display driver stacks are still pending.
- Public subsystem APIs are still evolving; only selected `fs`/`drivers` surfaces are wired into the kernel build today.
