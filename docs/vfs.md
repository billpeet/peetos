# VFS and Filesystems

This document describes the phase-5 VFS/filesystem implementation currently wired into the Rust kernel.

**Where It Lives**
- `crates/fs/src/lib.rs`
- `crates/fs/src/ext2_api.rs`
- `crates/fs/src/ext2_alloc.rs`
- `crates/fs/src/ext2_backend.rs`
- `crates/fs/src/ext2_dir.rs`
- `crates/fs/src/ext2_journal.rs`
- `crates/fs/src/ext2_sync.rs`
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
  - `mount_ext2_block_backend(path, image_len, head)` mounts from a block backend without pre-reading or preallocating the full device image.
  - Block-backed ext2 uses a sparse per-block cache: blocks are loaded on first access and tracked as dirty by block index.
  - Cache residency is bounded with CLOCK eviction (referenced-bit second chance, dirty victim write-back before eviction).
  - `export_ext2_image(out)` snapshots the current ext2 image by combining cached blocks with backend reads for uncached blocks.
  - `ext2_backend_kind()`, `set_ext2_block_backend(...)`, `clear_ext2_block_backend()`, and `set_ext2_cache_policy(max_blocks, dirty_high_watermark_percent)` are available as block-backed ext2 staging hooks.
  - Kernel init mounts ext2 only from discovered block devices (`kernel/src/block.rs`).
  - Host-side fs tests use `user/sample_ext2.img` through in-memory block-backend callbacks.
  - Runtime shell/smoke flows (`musl-ext2-exec`, `musl-ext2-cmd-smoke`, `musl-ext2-ping-smoke`, `musl-ext2-quickjs-smoke`, `musl-ext2-micropython-smoke`, `musl-ext2-dropbear-smoke`) use `user/sample_ext2_musl.img` as a second block-backed ext2 payload so Toybox `sh`/staged userspace tools can be launched via `/mnt/ext2/bin/...`.
- File operations:
  - `open`/`openat` with `O_CREAT`, `O_EXCL`, `O_TRUNC`, `O_APPEND`, `O_DIRECTORY`
  - `mkdir`/`mkdirat`
  - `close`
  - `read`
  - `write`
  - `lseek`
  - `readdir` (`getdents64`)
  - `stat`/`fstat`/`newfstatat`
  - `unlink`/`unlinkat`, `rmdir`/`unlinkat(AT_REMOVEDIR)`, `rename`/`renameat`, `linkat`, `symlinkat`
  - `readlinkat` (baseline `/proc/self/exe` path), `utimensat` (baseline atime/mtime)

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
  - `/dev/console`: opened as a console descriptor; kernel routes writes to serial (mirrored to framebuffer text when available) and reads through canonical line-buffered TTY input
- `procfs` behavior:
  - `/proc/uptime` and `/proc/meminfo` are generated from runtime kernel stats
  - `/proc/mounts` is generated from the mount table and reflects mounted ext2 entries
  - proc files are read-only (`-EROFS` on write)
- ext2 behavior:
  - Imported files/directories support baseline mutable regular-file and directory operations (`read`, `write`, `truncate`, `create`, `mkdir`, `unlink`, `rmdir`, `rename`) through the existing VFS file APIs.
  - Ext2 reads/writes operate on ext2 inode/data structures in the mounted image (not copied tmpfs file blobs), and size/offset tracking follows ext2 inode state.
  - Logical-block reads/writes resolve direct and single/double/triple-indirect ext2 inode block pointers, and write growth allocates indirect tables/data blocks as needed.
  - `truncate`/`O_TRUNC` and length-based `truncate_to` on ext2-backed regular files reclaim/resize direct and indirect data allocations (zero-fill on growth).
  - ext2 directory entry growth now allocates additional directory data blocks through direct/indirect mappings.
  - `create`/`mkdir` allocate ext2 inodes/blocks, update inode/block bitmaps and free counters, and insert directory entries in parent directories.
  - inode metadata (`mode`, `uid`, `gid`, link count, basic timestamps) is tracked and surfaced via `stat` paths.
  - `/proc/mounts` reports ext2 as `rw` when mounted.
  - Mount import is rollback-safe: on parse/import failure, partially imported nodes are removed.
  - Ext2 mount now includes ext3 internal journal parsing and mount-time replay of committed transactions when recovery is required, then clears recover markers before exposing the mount.
  - For journaled mounts, dirty-block flushes now emit runtime journal descriptor/data/commit records before checkpointing home blocks.
  - Journal replay revoke precedence now uses wrap-safe transaction ID ordering so replay remains correct across `u32` sequence rollover.
  - Block-backed ext2 now uses sparse per-block residency and dirty tracking: blocks are loaded into cache on first access, and uncached blocks are never allocated in RAM up front.
  - Block cache residency is bounded by a configurable block cap, with CLOCK eviction and dirty-block write-back on dirty victim eviction.
  - Dirty-pressure flushing runs opportunistically when dirty/cached ratio crosses the configured threshold, reducing write burst size at sync points.
  - Ext2 directory children are now populated lazily per directory (first path lookup/readdir) instead of recursively importing the full tree at mount.
  - When ext2 is mounted from a block device, mutating VFS syscalls enqueue best-effort dirty-run updates through the kernel block write-back cache (`block::write_cached`) using contiguous changed ext2 blocks from explicit per-block dirty tracking.
  - `sync`/`fsync` request a flush of the ext2 backing device's cached dirty block entries.

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
  - `linkat` (`265`)
  - `symlinkat` (`266`)
  - `readlinkat` (`267`)
  - `utimensat` (`280`)
  - `read` (`0`)
  - `write` (`1`)
- Mutating syscall paths (`open` with `O_CREAT`/`O_TRUNC`, `write`, `mkdirat`, `unlinkat`/`rmdirat`, `renameat`, `linkat`, `symlinkat`, `utimensat`) invoke kernel-side ext2 sync when a block-backed ext2 mount is active.
- Per-task FD tables track kind/inode/offset/flags.
- User stdio (`fd 0/1/2`) is pre-seeded as console descriptors, and each user task tracks a cwd inode (currently rooted at `/`).

Limitations / Simplifications
- VFS storage model remains static-capacity and single-address-space.
- Synchronization is a single global fs lock; no finer-grained inode/dentry locking yet.
- Metadata plumbing exists (`mode`/`uid`/`gid`/timestamps), but full POSIX permission policy and ownership-changing syscalls (`chmod`/`chown`) are not implemented yet.
- `rename` currently behaves as no-replace (`EEXIST` if destination exists) rather than full Linux overwrite semantics.
- Hard links are baseline-implemented (`linkat`), with ext2-backed inode links and a pragmatic tmpfs fallback.
- Symbolic links are baseline-implemented (`symlinkat` + `readlinkat` target reads), but general symlink-follow path resolution is not implemented yet.
- Console input path is baseline-only: canonical line mode with basic backspace editing and newline commit; no `termios`, raw mode, job control, or pty support.
- ext2 importer is intentionally minimal: regular files/directories only, and imported file size is still capped by `TMPFS_MAX_FILE_SIZE`.
- Journal bring-up is still incomplete: mount-time recovery replay is wired for internal journals, replay/revoke ordering now handles wrapped transaction IDs, runtime dirty-block flushes are journaled with multi-block transaction batching, runtime transaction placement rotates through the journal ring with wrap support, and committed transactions now use deferred checkpointing with runtime revoke-block emission before home-block writeback. Full JBD parity (for example richer transaction state/credit handling and complete ext3 mode semantics) remains pending.
- Block-backed ext2 sync is currently best-effort and uses a page-sized block write-back cache (dirty block runs are enqueued on mutating syscalls, then flushed on `sync`/`fsync`), without full end-to-end durability guarantees until block-cache flush completion.
- Dirty-pressure flush is opportunistic in filesystem paths (no dedicated background flusher thread yet).
- Ext2 still uses its own sparse block cache for on-disk block residency/dirty tracking, but syscall-level file reads and file-backed `MAP_SHARED` faults now share a kernel VM inode-page cache path. Full fs-wide VM page-cache integration (including writable shared mmap writeback and unified reclaim policy) remains pending.
- ext2 mount-from-block currently probes whole-device LBA0 ext2 images only; partition parsing (MBR/GPT) is not implemented.
- Bare-metal kernel builds currently skip embedding the sample ext2 fallback image to keep the default kernel/image lean.
- Runtime shell/smoke flows currently require attaching `user/sample_ext2_musl.img` as a second AHCI disk in QEMU.
