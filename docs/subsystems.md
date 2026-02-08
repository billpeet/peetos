# Subsystem Crate Stubs

This document summarizes subsystem crates. Most remain placeholders, while `fs` now contains the active phase-5 VFS/filesystem implementation.

**Where They Live**
- `crates/drivers`
- `crates/net`
- `crates/syscall`
- `crates/userabi`
- `crates/fs`

**Current State**
- `crates/fs` now provides a no-std baseline VFS core (concrete tmpfs + virtual `devfs`/`procfs` + ext2 image import/mount/mutate support) used by kernel syscall dispatch (`open`/`openat`/`close`/`read`/`write`/`lseek`/`mkdirat`/`fstat`/`newfstatat`/`getdents64`/`unlinkat`/`renameat`).
- `crates/drivers`, `crates/net`, `crates/syscall`, and `crates/userabi` are still placeholders for later phases.

Limitations / Simplifications
- `crates/fs` currently provides tmpfs + minimal virtual `devfs`/`procfs` and a baseline ext2 image-backed path, but lacks a real block-device cache/flush stack, FAT support, journaling, and many ext2 edge cases (indirect blocks, full feature flags).
- Public subsystem APIs are still evolving; only the fs surface is wired into the kernel build today.
