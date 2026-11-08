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
  - `user/sample_ext3.img` (built by `scripts/build_ext3_sample.sh`) is a stock `mke2fs -t ext3` image with an htree directory, an external xattr block, an in-inode xattr, a sparse 5 GiB file, and a fast symlink. `tests/ext3_fsck.sh` mutates it through the fs crate, syncs and checkpoints, and requires `e2fsck -fn` to report a clean filesystem.
  - `user/sample_ext2_musl.img` is built with stock `mke2fs` ext2 features (`ext_attr`, `resize_inode`, `dir_index`, `filetype`, `sparse_super`, `large_file`).
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
  - On-disk structure access goes through two seams prepared for ext4 (`docs/ext4_compat_plan.md`): group descriptors are read via `Ext2Meta::desc_size` with lo/hi accessors (`ext2_gd_location*`), and file block mapping goes through `crates/fs/src/ext2_bmap.rs` (indirect maps read/write, ext4 extent trees read-only).
  - ext4 images without `metadata_csum` (`extent 64bit flex_bg huge_file gdt_csum dir_nlink extra_isize`, for example `mke2fs -t ext4 -O ^metadata_csum,uninit_bg`) mount **read-write**: new files and directories are extent-mapped, extent trees grow and shrink with writes and truncation, lazy-init groups are initialized on first use, and crc16 group descriptor checksums are maintained.
  - ext4 images with `metadata_csum` (stock `mkfs.ext4`) also mount **read-write**. `metadata_csum` checksums are verified on read (superblock and group descriptors at mount; bitmaps, inodes, extent blocks, directory blocks when loaded) and return `EIO` on mismatch, reported through `fs::set_ext2_diag_hook`. Every metadata write refreshes its checksum (inode lo/hi, descriptor and bitmap checksums, extent tails, directory leaf tails, xattr block, superblock), and the journal is switched to JBD2 `CSUM_V3` with `64BIT` tags, as Linux uses. Journals written by Linux (`journal_64bit`, `journal_checksum_v2/v3`) are recovered at mount.
  - `fs::mount_ext2_block_backend_with_options(.., read_only = true)` mounts read-only (kernel feature `ext2-ro-root` for the root): every mutation returns `EROFS`, `/proc/mounts` shows `ro`, the journal superblock is untouched, and journal recovery still runs at mount, as on Linux.
  - Feature acceptance covers the stock `mke2fs -t ext3` feature set: compat `HAS_JOURNAL`/`EXT_ATTR`/`RESIZE_INO`/`DIR_INDEX`, incompat `FILETYPE`/`RECOVER`/`JOURNAL_DEV`, and ro-compat `SPARSE_SUPER`/`LARGE_FILE`. Everything else (for example `DIR_PREALLOC`, `IMAGIC_INODES`, and all ext4 incompat bits) is rejected with `EINVAL`.
  - `EXT_ATTR`: an inode's external xattr block (`i_file_acl`) is counted in `i_blocks` (truncate and rewrite keep it charged) and released on final unlink (refcount decremented, block freed at zero). In-inode xattrs in 256-byte inodes are preserved. Xattr contents are never read or written (no `getxattr`/`setxattr`).
  - `RESIZE_INO`: reserved GDT blocks are already marked in-use in the block bitmaps and the resize inode (7) is never touched. The kernel never resizes the filesystem.
  - `DIR_INDEX`: htree-indexed directories are read with a linear scan, which works because htree interior blocks look like empty dirents. The first entry insert into an indexed directory clears `EXT2_INDEX_FL`, so the stale index is never trusted again (`e2fsck -D` can rebuild it). Removes leave the index intact, matching Linux.
  - `LARGE_FILE`: `stat` reports the full 64-bit size of regular files that use `i_size_high`. Because the in-memory size paths are 32-bit, reading such a file returns `EOVERFLOW` and writing or truncating it returns `EFBIG`. Unlinking it frees its whole block tree.
  - Fast symlinks (target shorter than 60 bytes stored inline in `i_block`) are read inline and freed without touching the block bitmap. Short targets created by `symlinkat` are now written as fast symlinks, as Linux does and as `e2fsck` requires; longer targets use a data block.
  - For journaled mounts, dirty-block flushes now emit runtime journal descriptor/data/commit records before checkpointing home blocks.
  - Journaled mounts now track explicit journal mode state and maintain a running transaction view of dirty blocks and data blocks as groundwork for a fuller `data=ordered` implementation; current ordered-mode bring-up writes tracked file data to home blocks before the journal commit record, keeps the journal transaction focused on metadata blocks, feeds that tracking from explicit ext2 mutation scopes around top-level mutating operations, assigns transaction IDs and basic credit reservations to those scopes, uses bounded credits for fixed-shape metadata mutators and coarse estimated credits for variable-size paths such as `write`/`truncate`, now plans those variable-size scopes as capped per-segment reservations rather than one whole-operation reservation, asks the journal layer to rotate restartable segments at block-walking loop boundaries before the next data/freeing step and before final inode closeout, uses indirection-aware step estimates instead of a flat one-credit loop heuristic for allocation/free work, now lets allocator/free helpers themselves request restart headroom before dirtying bitmaps/counters/indirect tables, splits allocation helpers into internal bookkeeping versus initialization/link phases, now also gives truncate-side detach/free paths explicit unlink-table versus free/revoke restart boundaries, records those restart points as explicit named journal phases on each ordered transaction segment, now groups the real metadata work for those phases through closure-scoped restart-phase helpers instead of only tagging ambient transaction state, threads a lightweight internal journal-handle token through the main ordered write/truncate/alloc/free helpers and the fixed-shape create/unlink/rename/link/symlink directory-metadata helpers, can split restartable variable-size scopes into multiple sealed ordered transaction segments when they exceed a credit reservation, coalesces nested scopes into one top-level ordered transaction, seals per-operation metadata/data snapshots so a later cache mutation cannot silently rewrite an older pending ordered transaction, makes single-block flush, dirty-pressure writeback, and CLOCK eviction all advance sealed pending transactions in order via an explicit "flush oldest pending ordered transaction" path rather than journaling a later dirty block out of context, and now keeps open ordered mutation scopes from being background-flushed before they seal.
  - Journal replay revoke precedence now uses wrap-safe transaction ID ordering so replay remains correct across `u32` sequence rollover.
  - The journal superblock's `s_sequence` names the oldest committed-but-uncheckpointed transaction (as JBD requires), and the fs superblock carries `INCOMPAT_RECOVER` exactly while such transactions exist, so a crashed image is replayed both by this kernel and by Linux/`e2fsck`.
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
- Mutating syscall paths (`open` with `O_CREAT`/`O_TRUNC`, `write`, `mkdirat`, `unlinkat`/`rmdirat`, `renameat`, `linkat`, `symlinkat`, `utimensat`) invoke kernel-side ext2 sync when a block-backed ext2 mount is active.
- Per-task FD tables track kind/inode/offset/flags.
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
- Regular files larger than 4 GiB (non-zero `i_size_high`) are visible and unlinkable but refuse read/write/truncate. In-memory VFS node sizes (and therefore `SEEK_END`) still carry only the low 32 bits.
- htree directories lose their index on the first insert; lookups are always linear scans. Directories are also still limited to `VFS_MAX_CHILDREN` entries in the VFS layer.
- Extended attributes are preserved and released but not exposed to userspace, and POSIX ACLs stored in xattrs are not enforced.
- ext4 htree index blocks are never written: inserting into an indexed directory clears `INDEX_FL` (adding leaf checksum tails), and `dx_tail` checksums are not verified (M4 in `docs/ext4_compat_plan.md`). `stat` omits nanoseconds and `crtime`. Extent changes rewrite the inode's whole extent tree (fine for today's small files), and new inodes get zero `crtime`/nanoseconds.
- Journal bring-up is still incomplete: mount-time recovery replay is wired for internal journals, replay/revoke ordering now handles wrapped transaction IDs, runtime dirty-block flushes are journaled with multi-block transaction batching, runtime transaction placement rotates through the journal ring with wrap support, committed transactions now use deferred checkpointing with runtime revoke-block emission before home-block writeback, and runtime journal I/O faults now trigger fail-fast aborted state. Fuller JBD compatibility (for example richer transaction state/credit handling and complete ext3 mode semantics) remains pending.
- Block-backed ext2 sync uses a page-sized block write-back cache. On journaled (ext3) mounts every mutating syscall *commits* a journal transaction (ordered data, then a write barrier, then the commit record, settled by a barrier before the syscall returns), while checkpointing to home locations happens on `sync`/`fsync` or when log space runs out, as in ext3. On plain ext2 mounts durability comes only from `sync`/`fsync`, which drain the block cache and flush the drive's write cache.
- Journal commits carry JBD2 `COMPAT_CHECKSUM` CRC32s (enabled on mount when the log is empty), and journals with JBD2 incompat features other than `REVOKE` are refused.
- Dirty-pressure flush is opportunistic in filesystem paths (no dedicated background flusher thread yet).
- Ext2 still uses its own sparse block cache for on-disk block residency/dirty tracking, but syscall-level file reads and file-backed `MAP_SHARED` faults now share a kernel VM inode-page cache path. Full fs-wide VM page-cache integration (including writable shared mmap writeback and unified reclaim policy) remains pending.
- ext2 mount-from-block currently probes whole-device LBA0 ext2 images only; partition parsing (MBR/GPT) is not implemented.
- `flock` lock owners are `(pid, fd)` pairs, not open file descriptions: a `dup`ed fd or a forked child does not share its parent's lock as it would on Linux, and the lock table holds at most 32 locks.
- `fallocate` does not reserve blocks, so a later write can still hit `ENOSPC`.
- `/boot` is currently a mountpoint placeholder (`bootfs` in `/proc/mounts`); there is no dedicated boot-filesystem backend yet.
- Bare-metal kernel builds currently skip embedding the sample ext2 fallback image to keep the default kernel/image lean.
- Runtime shell/smoke flows currently require attaching `user/sample_ext2_musl.img` as a second AHCI disk in QEMU.
