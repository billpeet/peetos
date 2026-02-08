# VFS and tmpfs Baseline

This document describes the phase-5 baseline VFS/filesystem implementation now wired into the Rust kernel.

**Where It Lives**
- `crates/fs/src/lib.rs`
- `kernel/src/vfs.rs`
- `kernel/src/syscall.rs`
- `kernel/src/task.rs`
- `kernel/src/user.rs`

**Scope Implemented**
- In-kernel VFS state with inode-like records and directory children.
- tmpfs-backed root filesystem with static capacity limits.
- Path resolution for absolute paths with `.` and `..` handling.
- Basic directory creation (`mkdir`) in the VFS layer.
- File operations:
  - `open`/`openat` with `O_CREAT`, `O_EXCL`, `O_TRUNC`, `O_APPEND`, `O_DIRECTORY`
  - `close`
  - `read`
  - `write`
  - `lseek`

**Default Filesystem Layout**
On initialization, the kernel creates a tmpfs root and baseline directories/files:
- `/tmp`
- `/dev`
- `/etc`
- `/dev/console`

**Syscall Integration**
- The syscall dispatcher now routes Linux-numbered file syscalls to the VFS baseline:
  - `open` (`2`)
  - `close` (`3`)
  - `lseek` (`8`)
  - `openat` (`257`)
  - `read` (`0`)
  - `write` (`1`)
- Per-task FD tables are tracked in `Task` metadata (`kind`, `inode`, `offset`, `flags`).
- User tasks initialize stdio (`fd 0/1/2`) as console descriptors at spawn.
- Console writes still go to serial so existing user bring-up remains visible in QEMU logs.

Limitations / Simplifications
- tmpfs is fixed-size/static-structure (no dynamic allocation growth).
- Only absolute path handling is supported for `openat` (`AT_FDCWD` only).
- No mount table, superblock abstraction, or multi-filesystem dispatch yet.
- No permissions/UID/GID enforcement, timestamps, links, or unlink/rmdir.
- Directory enumeration (`readdir`) is not exported through syscalls yet.
- Console I/O is simplified; reads from console currently return EOF.
- No locking/concurrency protections for VFS state yet (single-core assumption).
