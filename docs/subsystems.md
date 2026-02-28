# Subsystem Crates

This document summarizes subsystem crates. `fs` and `drivers` contain active kernel functionality, while `net`, `syscall`, and `userabi` remain placeholders.

**Where They Live**
- `crates/drivers`
- `crates/net`
- `crates/syscall`
- `crates/userabi`
- `crates/fs`

**Current State**
- `crates/fs` now provides a no-std baseline VFS core (concrete tmpfs + virtual `devfs`/`procfs` + ext2 image import/mount/mutate support) used by kernel syscall dispatch (`open`/`openat`/`close`/`read`/`write`/`lseek`/`mkdirat`/`fstat`/`newfstatat`/`getdents64`/`unlinkat`/`renameat`/`linkat`/`symlinkat`/`readlinkat`/`utimensat`), with kernel-side glue now mounting ext2 from the block layer when available and syncing ext2 image mutations back through block I/O.
- `crates/drivers` now provides PCI enumeration plus ATA/AHCI protocol support (storage-class matching, IDE/AHCI classification, AHCI ABAR extraction, AHCI capability/port decode helpers, and AHCI command/FIS table layouts), with kernel bring-up using these surfaces for AHCI MMIO probing, AHCI DMA `IDENTIFY`, AHCI LBA48 DMA read/write commands, ATA PIO `IDENTIFY`, and block-device inventory registration.
- `crates/net`, `crates/syscall`, and `crates/userabi` remain placeholders for future refactoring of currently kernel-local code.

Limitations / Simplifications
- `crates/fs` currently provides tmpfs + minimal virtual `devfs`/`procfs` and a baseline ext2 block-backed path. The kernel block layer now has a baseline page-sized write-back cache used by ext2 dirty-run persistence, and syscall-level file reads plus file-backed shared mmap faults now use a unified kernel VM inode-page cache path. Internal ext3 journal parsing + mount-time recovery replay (`ext2_journal.rs`) are now wired, and runtime dirty-block flushes for journaled mounts now emit descriptor/data/commit records with batched tags, rotating wrap-aware journal-ring placement, deferred checkpoint writeback, runtime revoke-block emission, and fail-fast aborted-state behavior on journal I/O faults. Full fs-wide VM page-cache integration (especially shared writable mmap writeback), fuller ext3/JBD compatibility, FAT support, and additional ext2 hardening/features remain pending.
- `crates/drivers` currently implements PCI discovery and AHCI/ATA bring-up helpers including baseline DMA command issuance; broad block-layer/VFS integration remains pending. Input baseline (PS/2 keyboard + canonical TTY plus PS/2 mouse packet handling) and framebuffer bootstrap/cursor composition are currently implemented in the kernel crate rather than as reusable drivers crate surfaces.
- Public subsystem APIs are still evolving; only selected `fs`/`drivers` surfaces are wired into the kernel build today.
