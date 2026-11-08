# VFS and Filesystems

This document describes the current VFS/filesystem implementation wired into the Rust kernel.

**Where It Lives**
- `crates/fs/src/lib.rs`
- `crates/fs/src/ext2_api.rs`
- `crates/fs/src/ext2_alloc.rs`
- `crates/fs/src/ext2_backend.rs`
- `crates/fs/src/ext2_dir.rs`
- `crates/fs/src/ext2_journal.rs`
- `crates/fs/src/ext2_sync.rs`
- `kernel/src/vfs.rs`
- `kernel/src/syscall/` (`file.rs`, `io.rs`, `fd.rs`)
- `kernel/src/task.rs`
- `kernel/src/user.rs`

**Scope Implemented**
- In-kernel VFS state with inode-like records, path traversal, and a coarse global fs lock for synchronization.
- Concrete tmpfs-backed regular files and directories.
- Virtual filesystems backed by dedicated node behavior:
  - `devfs`: `/dev/null`, `/dev/zero`, `/dev/console`
  - `procfs`: `/proc/uptime`, `/proc/meminfo`, `/proc/mounts`, `/proc/version`
- Mount table abstraction used by `/proc/mounts` (tmpfs/devfs/procfs baseline entries plus dynamic ext2/ext3 entry when mounted).
- ext2 baseline importer:
  - `mount_ext2_block_backend(path, image_len, head)` mounts from a block backend without pre-reading or preallocating the full device image.
  - Block-backed ext2 uses a sparse per-block cache: blocks are loaded on first access and tracked as dirty by block index.
  - Cache residency is bounded with CLOCK eviction (referenced-bit second chance, dirty victim write-back before eviction).
  - `export_ext2_image(out)` snapshots the current ext2 image by combining cached blocks with backend reads for uncached blocks.
  - `ext2_backend_kind()`, `set_ext2_block_backend(...)`, `clear_ext2_block_backend()`, and `set_ext2_cache_policy(max_blocks, dirty_high_watermark_percent)` are available as block-backed ext2 staging hooks.
  - Kernel init mounts ext2 only from discovered block devices (`kernel/src/block.rs`).
  - Host-side fs tests use `user/sample_ext2.img` through in-memory block-backend callbacks.
  - Runtime shell/smoke flows (`musl-ext2-exec`, `musl-ext2-cmd-smoke`, `musl-ext2-ping-smoke`, `musl-ext2-quickjs-smoke`, `musl-ext2-micropython-smoke`, `musl-ext2-dropbear-smoke`) use `user/sample_ext2_musl.img` as a second block-backed ext2 payload so Toybox `sh`/staged userspace tools can be launched via `/bin/...` on root ext2 (with transitional `/mnt/ext2/...` compatibility remap when root ext2 is active).
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
- block-backed ext2/ext3 root mount at `/` when a compatible block-backed image is detected
- runtime `/etc/fstab` with root declared as `block:auto / ext3 rw 0 1`
- tmpfs mountpoints: `/temp` (runtime scratch) and `/boot` (boot-drive placeholder mountpoint)
- tmpfs bootstrap paths: `/tmp`, `/etc` (transitional bootstrap before/without root ext2)
- devfs nodes under `/dev`
- procfs nodes under `/proc`
- legacy compatibility path remap from `/mnt/ext2/...` to `/...` only when ext2 is mounted at `/`

**Behavior Highlights**
- tmpfs files are mutable and support offset-based reads/writes with sparse-zero fill on gaps.
- `devfs` behavior:
  - `/dev/null`: reads EOF, writes are discarded
  - `/dev/zero`: reads return zero bytes, writes are discarded
  - `/dev/console`: opened as a console descriptor; kernel routes writes to serial (mirrored to framebuffer text when available) and reads through canonical line-buffered TTY input
  - `/dev/fd`: a directory whose `getdents64` listing is synthesized per process (`dev_fd_dir_entry` in `kernel/src/syscall/file.rs`): one `DT_LNK` entry per open descriptor of the caller, named by number, read live (so it includes the descriptor used to read it, as Linux's `/proc/self/fd` does). Upstream libwayland's fd-leak checks count these entries.
- Unlinked files stay usable while open, as POSIX requires:
  - every open file description holds an fs reference (`fs::inode_open_retain`/`inode_open_release`), taken when `openat`/`memfd_create` create a VFS fd and kept balanced by the kernel's `fd_object_retain`/`fd_object_release` across `dup*`, `fork`, `SCM_RIGHTS` and close;
  - every file some task maps holds one more reference (`vm_sync_inode_pins` in `kernel/src/syscall/vm.rs`, recomputed after `mmap`, `munmap` and address-space teardown);
  - `unlink`/`rmdir` of a referenced node detaches it from its directory (it reports `st_nlink == 0`) and frees it on the last release, including the ext2 inode and blocks; cached pages of any freed inode are dropped so a reused inode slot never sees stale data.
- `procfs` behavior:
  - `/proc/uptime` and `/proc/meminfo` are generated from runtime kernel stats
  - `/proc/mounts` is generated from the mount table and reflects mounted ext2 entries (journaled mounts are surfaced as ext3), plus pseudo mounts such as `/boot`, `/temp`, `/dev`, and `/proc`
  - proc files are read-only (`-EROFS` on write)
- ext2 behavior:
  - Imported files/directories support baseline mutable regular-file and directory operations (`read`, `write`, `truncate`, `create`, `mkdir`, `unlink`, `rmdir`, `rename`) through the existing VFS file APIs.
  - Ext2 reads/writes operate on ext2 inode/data structures in the mounted image (not copied tmpfs file blobs), and size/offset tracking follows ext2 inode state.
  - Logical-block reads/writes resolve direct and single/double/triple-indirect ext2 inode block pointers, and write growth allocates indirect tables/data blocks as needed.
  - `truncate`/`O_TRUNC` and length-based `truncate_to` on ext2-backed regular files reclaim/resize direct and indirect data allocations (zero-fill on growth).
  - ext2 directory entry growth now allocates additional directory data blocks through direct/indirect mappings.
  - `create`/`mkdir` allocate ext2 inodes/blocks, update inode/block bitmaps and free counters, and insert directory entries in parent directories.
  - inode metadata (`mode`, `uid`, `gid`, link count, basic timestamps) is tracked and surfaced via `stat` paths.
  - `/proc/mounts` reports non-journaled root mounts as `ext2 / ext2 rw` and journaled root mounts as `ext3 / ext3 rw`.
  - Mount import is rollback-safe: on parse/import failure, partially imported nodes are removed.
  - Ext2 mount now includes ext3 internal journal parsing and mount-time replay of committed transactions when recovery is required, then clears recover markers before exposing the mount.
  - Feature acceptance is intentionally conservative: mounts currently allow `HAS_JOURNAL` in compat bits, `FILETYPE`/`RECOVER`/`JOURNAL_DEV` in incompat bits, and `SPARSE_SUPER` in ro-compat bits; other ext2/ext3-era feature bits are rejected until implemented.
  - For journaled mounts, dirty-block flushes now emit runtime journal descriptor/data/commit records before checkpointing home blocks.
  - Journaled mounts now track explicit journal mode state and maintain a running transaction view of dirty blocks and data blocks as groundwork for a fuller `data=ordered` implementation; current ordered-mode bring-up writes tracked file data to home blocks before the journal commit record, keeps the journal transaction focused on metadata blocks, feeds that tracking from explicit ext2 mutation scopes around top-level mutating operations, assigns transaction IDs and basic credit reservations to those scopes, uses bounded credits for fixed-shape metadata mutators and coarse estimated credits for variable-size paths such as `write`/`truncate`, now plans those variable-size scopes as capped per-segment reservations rather than one whole-operation reservation, asks the journal layer to rotate restartable segments at block-walking loop boundaries before the next data/freeing step and before final inode closeout, uses indirection-aware step estimates instead of a flat one-credit loop heuristic for allocation/free work, now lets allocator/free helpers themselves request restart headroom before dirtying bitmaps/counters/indirect tables, splits allocation helpers into internal bookkeeping versus initialization/link phases, now also gives truncate-side detach/free paths explicit unlink-table versus free/revoke restart boundaries, records those restart points as explicit named journal phases on each ordered transaction segment, now groups the real metadata work for those phases through closure-scoped restart-phase helpers instead of only tagging ambient transaction state, threads a lightweight internal journal-handle token through the main ordered write/truncate/alloc/free helpers and the fixed-shape create/unlink/rename/link/symlink directory-metadata helpers, can split restartable variable-size scopes into multiple sealed ordered transaction segments when they exceed a credit reservation, coalesces nested scopes into one top-level ordered transaction, seals per-operation metadata/data snapshots so a later cache mutation cannot silently rewrite an older pending ordered transaction, makes single-block flush, dirty-pressure writeback, and CLOCK eviction all advance sealed pending transactions in order via an explicit "flush oldest pending ordered transaction" path rather than journaling a later dirty block out of context, and now keeps open ordered mutation scopes from being background-flushed before they seal.
  - Journal replay revoke precedence now uses wrap-safe transaction ID ordering so replay remains correct across `u32` sequence rollover.
  - Runtime journal paths now enter an aborted state on journal I/O faults and fail fast on later flush/checkpoint attempts.
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
  - `mkdir` (`83`) and `rmdir` (`84`): legacy wrappers over `mkdirat`/`unlinkat(AT_REMOVEDIR)`; musl's `mkdir()`/`rmdir()` use them on x86_64
- `fstat` (and `fstatat(fd, "", AT_EMPTY_PATH)`) also answer for non-file descriptors with a synthetic `stat` (`synthetic_fd_stat_identity`): `S_IFSOCK` for sockets, `S_IFIFO` for pipes, `S_IFCHR` for console/PTYs, and an anon-inode style mode for epoll/eventfd/timerfd/signalfd, each kind on its own pseudo `st_dev`.
- Mutating syscall paths (`open` with `O_CREAT`/`O_TRUNC`, `write`, `mkdirat`, `unlinkat`/`rmdirat`, `renameat`, `linkat`, `symlinkat`, `utimensat`) invoke kernel-side ext2 sync when a block-backed ext2 mount is active.
- Per-process FD tables track kind/inode/offset/flags for up to 128 descriptors (`task::MAX_TASK_FDS`).
- `flock(fd, op)` (`73`) provides advisory `LOCK_SH`/`LOCK_EX`/`LOCK_UN` locks with `LOCK_NB`; blocking requests yield until the lock is free and return `-EINTR` on a pending signal. Locks are dropped on `LOCK_UN` and whenever the locking fd closes (including close-on-exec and task exit).
- `fallocate(fd, 0, offset, len)` (`285`) extends a regular file to `offset + len` if it is shorter (blocks are allocated lazily on write) and respects memfd `F_SEAL_GROW`. Other modes return `-EOPNOTSUPP`, which libc callers such as libwayland treat as "fall back to `ftruncate`".
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
- Journal bring-up is still incomplete: mount-time recovery replay is wired for internal journals, replay/revoke ordering now handles wrapped transaction IDs, runtime dirty-block flushes are journaled with multi-block transaction batching, runtime transaction placement rotates through the journal ring with wrap support, committed transactions now use deferred checkpointing with runtime revoke-block emission before home-block writeback, and runtime journal I/O faults now trigger fail-fast aborted state. Fuller JBD compatibility (for example richer transaction state/credit handling and complete ext3 mode semantics) remains pending.
- Block-backed ext2 sync is currently best-effort and uses a page-sized block write-back cache (dirty block runs are enqueued on mutating syscalls, then flushed on `sync`/`fsync`), without full end-to-end durability guarantees until block-cache flush completion.
- Dirty-pressure flush is opportunistic in filesystem paths (no dedicated background flusher thread yet).
- Ext2 still uses its own sparse block cache for on-disk block residency/dirty tracking, but syscall-level file reads and file-backed `MAP_SHARED` faults now share a kernel VM inode-page cache path. Full fs-wide VM page-cache integration (including writable shared mmap writeback and unified reclaim policy) remains pending.
- ext2 mount-from-block currently probes whole-device LBA0 ext2 images only; partition parsing (MBR/GPT) is not implemented.
- `flock` lock owners are `(pid, fd)` pairs, not open file descriptions: a `dup`ed fd or a forked child does not share its parent's lock as it would on Linux, and the system-wide lock table holds at most 128 locks.
- Capacities are static: 256 VFS nodes in total, 128 entries per directory node (`VFS_MAX_CHILDREN`), and ext2 directories import at most 64 entries when first loaded (the import buffers names on the kernel stack).
- `/dev/fd/N` cannot be opened (there is no reopen-by-descriptor), and `/proc/self/fd` does not exist.
- Orphaned (unlinked but open) ext2 files are freed only at last close; if the system stops first, the on-disk inode stays allocated with zero links, since there is no ext2 orphan list or fsck-style recovery.
- File offsets live in the per-process fd slot, so `dup`ed descriptors do not share an offset as Linux open file descriptions do.
- `fallocate` does not reserve blocks, so a later write can still hit `ENOSPC`.
- `/boot` is currently a mountpoint placeholder (`bootfs` in `/proc/mounts`); there is no dedicated boot-filesystem backend yet.
- Bare-metal kernel builds currently skip embedding the sample ext2 fallback image to keep the default kernel/image lean.
- Runtime shell/smoke flows currently require attaching `user/sample_ext2_musl.img` as a second AHCI disk in QEMU.
