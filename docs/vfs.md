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
- ext2 baseline importer:
  - `mount_ext2_image(path, image)` parses a raw ext2 image and imports files/dirs as read-only VFS nodes.
  - Kernel init mounts `user/sample_ext2.img` at `/mnt/ext2`.
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
- ext2 sample mount under `/mnt/ext2` (if sample image parses successfully)

**Behavior Highlights**
- tmpfs files are mutable and support offset-based reads/writes with sparse-zero fill on gaps.
- `devfs` behavior:
  - `/dev/null`: reads EOF, writes are discarded
  - `/dev/zero`: reads return zero bytes, writes are discarded
  - `/dev/console`: opened as a console descriptor; kernel routes writes to serial
- `procfs` behavior:
  - `/proc/uptime` and `/proc/meminfo` are generated from runtime kernel stats
  - `/proc/mounts` reflects ext2 only when the ext2 sample is actually mounted
  - proc files are read-only (`-EROFS` on write)
- ext2 behavior:
  - Imported files/directories are exposed as read-only nodes (`-EROFS` on write/truncate/create under mount).
  - Mount import is rollback-safe: on parse/import failure, partially imported nodes are removed.

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
- ext2 importer is intentionally minimal: read-only, regular files/directories only, direct data blocks only (no indirect blocks), no journal/recovery, no ownership/permission metadata mapping, and imported file size still capped by `TMPFS_MAX_FILE_SIZE`.
