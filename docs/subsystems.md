# Subsystem Crate Stubs

This document summarizes subsystem crates. Most remain placeholders, while `fs` now contains a phase-5 baseline tmpfs/VFS implementation.

**Where They Live**
- `crates/drivers`
- `crates/net`
- `crates/syscall`
- `crates/userabi`
- `crates/fs`

**Current State**
- `crates/fs` now provides a no-std baseline VFS/tmpfs core used by kernel syscall dispatch (`open`/`openat`/`close`/`read`/`write`/`lseek`).
- `crates/drivers`, `crates/net`, `crates/syscall`, and `crates/userabi` are still placeholders for later phases.

Limitations / Simplifications
- `crates/fs` currently provides only a baseline in-memory tmpfs model (no mount table, no ext2/FAT/devfs/procfs yet).
- Public subsystem APIs are still evolving; only the fs surface is wired into the kernel build today.
