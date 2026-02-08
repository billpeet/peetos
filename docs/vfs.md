# VFS and Filesystems

This document describes the phase-5 VFS/filesystem implementation currently wired into the Rust kernel.

**Where It Lives**
- `crates/fs/src/lib.rs`
- `kernel/src/vfs.rs`
- `kernel/src/syscall.rs`
- `kernel/src/task.rs`
- `kernel/src/user.rs`

**Scope Implemented**
- In-kernel VFS state with inode-like records and path traversal.
- Concrete tmpfs-backed regular files and directories.
- Virtual filesystems backed by dedicated node behavior:
  - `devfs`: `/dev/null`, `/dev/zero`, `/dev/console`
  - `procfs`: `/proc/uptime`, `/proc/meminfo`, `/proc/mounts`, `/proc/version`
- File operations:
  - `open`/`openat` with `O_CREAT`, `O_EXCL`, `O_TRUNC`, `O_APPEND`, `O_DIRECTORY`
  - `close`
  - `read`
  - `write`
  - `lseek`

**Default Filesystem Layout**
On initialization, the kernel creates:
- tmpfs: `/`, `/tmp`, `/etc`
- devfs nodes under `/dev`
- procfs nodes under `/proc`

**Behavior Highlights**
- tmpfs files are mutable and support offset-based reads/writes with sparse-zero fill on gaps.
- `devfs` behavior:
  - `/dev/null`: reads EOF, writes are discarded
  - `/dev/zero`: reads return zero bytes, writes are discarded
  - `/dev/console`: opened as a console descriptor; kernel routes writes to serial
- `procfs` behavior:
  - `/proc/uptime` and `/proc/meminfo` are generated from runtime kernel stats
  - proc files are read-only (`-EROFS` on write)

**Syscall Integration**
- Linux-numbered file syscalls are dispatched through the VFS layer:
  - `open` (`2`)
  - `close` (`3`)
  - `lseek` (`8`)
  - `openat` (`257`)
  - `read` (`0`)
  - `write` (`1`)
- Per-task FD tables track kind/inode/offset/flags.
- User stdio (`fd 0/1/2`) is pre-seeded as console descriptors.

Limitations / Simplifications
- VFS storage model remains static-capacity and single-address-space; no dynamic mount table abstraction yet.
- No permissions/UID/GID enforcement, timestamps, links, unlink/rmdir, or rename yet.
- `openat` currently supports only `AT_FDCWD` with absolute paths.
- `readdir`, `stat`, and broader metadata syscalls are not implemented yet.
- Console input is not implemented yet; console reads currently return EOF.
- No explicit synchronization/locking in fs state (single-core assumption).
