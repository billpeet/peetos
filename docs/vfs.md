# VFS and Filesystems

This document describes the phase-5 VFS/filesystem implementation currently wired into the Rust kernel.

**Where It Lives**
- `crates/fs/src/lib.rs`
- `kernel/src/vfs.rs`
- `kernel/src/syscall.rs`
- `kernel/src/task.rs`
- `kernel/src/user.rs`

**Scope Implemented**
- In-kernel VFS state with inode-like records, path traversal, and a coarse global fs lock for synchronization.
- Concrete tmpfs-backed regular files and directories.
- Virtual filesystems backed by dedicated node behavior:
  - `devfs`: `/dev/null`, `/dev/zero`, `/dev/console`
  - `procfs`: `/proc/uptime`, `/proc/meminfo`, `/proc/mounts`, `/proc/version`
- Mount table abstraction used by `/proc/mounts` (tmpfs/devfs/procfs baseline entries plus dynamic ext2 entry when mounted).
- ext2 baseline importer:
  - `mount_ext2_image(path, image)` parses a raw ext2 image and imports files/dirs as ext2-backed VFS nodes.
  - The mounted ext2 image is kept as mutable fs state, and `export_ext2_image(out)` can snapshot the current mutated image bytes.
  - Kernel init first attempts to load ext2 from discovered block devices (`kernel/src/block.rs`).
  - Host-side fs tests use `user/sample_ext2.img`; current bare-metal kernel builds do not embed that sample fallback image by default.
  - A feature-gated runtime smoke path (`KERNEL_FEATURES=musl-ext2-exec`) embeds `user/sample_ext2_musl.img` so `/mnt/ext2/bin/echo` can be used for end-to-end `execve` validation.
- File operations:
  - `open`/`openat` with `O_CREAT`, `O_EXCL`, `O_TRUNC`, `O_APPEND`, `O_DIRECTORY`
  - `mkdir`/`mkdirat`
  - `close`
  - `read`
  - `write`
  - `lseek`
  - `readdir` (`getdents64`)
  - `stat`/`fstat`/`newfstatat`
  - `unlink`/`unlinkat`, `rmdir`/`unlinkat(AT_REMOVEDIR)`, `rename`/`renameat`

**Default Filesystem Layout**
On initialization, the kernel creates:
- tmpfs: `/`, `/tmp`, `/etc`
- devfs nodes under `/dev`
- procfs nodes under `/proc`
- ext2 mount under `/mnt/ext2` when a block-backed image is detected

**Behavior Highlights**
- tmpfs files are mutable and support offset-based reads/writes with sparse-zero fill on gaps.
- `devfs` behavior:
  - `/dev/null`: reads EOF, writes are discarded
  - `/dev/zero`: reads return zero bytes, writes are discarded
  - `/dev/console`: opened as a console descriptor; kernel routes writes to serial and reads through canonical line-buffered TTY input
- `procfs` behavior:
  - `/proc/uptime` and `/proc/meminfo` are generated from runtime kernel stats
  - `/proc/mounts` is generated from the mount table and reflects mounted ext2 entries
  - proc files are read-only (`-EROFS` on write)
- ext2 behavior:
  - Imported files/directories support baseline mutable regular-file and directory operations (`read`, `write`, `truncate`, `create`, `mkdir`, `unlink`, `rmdir`, `rename`) through the existing VFS file APIs.
  - Ext2 reads/writes operate on ext2 inode/data structures in the mounted image (not copied tmpfs file blobs), and size/offset tracking follows ext2 inode state.
  - Logical-block reads now resolve direct and single/double/triple-indirect ext2 inode block pointers.
  - `create`/`mkdir` allocate ext2 inodes/blocks, update inode/block bitmaps and free counters, and insert directory entries in parent directories.
  - inode metadata (`mode`, `uid`, `gid`, link count, basic timestamps) is tracked and surfaced via `stat` paths.
  - `/proc/mounts` reports ext2 as `rw` when mounted.
  - Mount import is rollback-safe: on parse/import failure, partially imported nodes are removed.
  - When ext2 is mounted from a block device, mutating VFS syscalls trigger best-effort image export + write-back through `block::write`.

**Syscall Integration**
- Linux-numbered file syscalls are dispatched through the VFS layer:
  - `open` (`2`)
  - `close` (`3`)
  - `fstat` (`5`)
  - `lseek` (`8`)
  - `getdents64` (`217`)
  - `openat` (`257`)
  - `mkdirat` (`258`)
  - `newfstatat` (`262`)
  - `unlinkat` (`263`)
  - `renameat` (`264`)
  - `read` (`0`)
  - `write` (`1`)
- Mutating syscall paths (`open` with `O_CREAT`/`O_TRUNC`, `write`, `mkdirat`, `unlinkat`/`rmdirat`, `renameat`) invoke kernel-side ext2 sync when a block-backed ext2 mount is active.
- Per-task FD tables track kind/inode/offset/flags.
- User stdio (`fd 0/1/2`) is pre-seeded as console descriptors, and each user task tracks a cwd inode (currently rooted at `/`).

Limitations / Simplifications
- VFS storage model remains static-capacity and single-address-space.
- Synchronization is a single global fs lock; no finer-grained inode/dentry locking yet.
- Metadata plumbing exists (`mode`/`uid`/`gid`/timestamps), but full POSIX permission policy and ownership-changing syscalls (`chmod`/`chown`) are not implemented yet.
- `rename` currently behaves as no-replace (`EEXIST` if destination exists) rather than full Linux overwrite semantics.
- Hard links and symbolic links are not implemented yet.
- Console input path is baseline-only: canonical line mode with basic backspace editing and newline commit; no `termios`, raw mode, job control, or pty support.
- ext2 importer is intentionally minimal: regular files/directories only, no journal/recovery, and imported file size still capped by `TMPFS_MAX_FILE_SIZE`. Indirect-block pointer traversal is implemented for reads; indirect-block allocation/writes are not yet implemented.
- Block-backed ext2 sync is currently best-effort and coarse-grained (export whole mounted image and write it back), with no journal, crash-consistency protocol, or batched/page-cache flush policy.
- ext2 mount-from-block currently probes whole-device LBA0 ext2 images only; partition parsing (MBR/GPT) is not implemented.
- Bare-metal kernel builds currently skip embedding the sample ext2 fallback image to stay within stage2 low-memory kernel staging limits.
- Feature-gated ext2 embedding for runtime smoke (`musl-ext2-exec`) is intended for debugging and may need image-size tuning if kernel staging-window limits are hit.
