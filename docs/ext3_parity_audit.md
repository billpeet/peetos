# Ext3 Journal Status Audit (2026-04-12, updated 2026-11-08)

## Scope

This audit covers ext3-style journaling behavior in the Rust kernel (`/home/bill/source/peetos`) and compares it against:

- The current Rust implementation goals and behavior.
- Historical implementation context in `/home/bill/source/peetos_c` (which is ext2-focused and does not implement ext3/JBD).

## Current Status

Rust currently implements a partial ext3-compatible path on top of the ext2 mount flow:

1. Journal feature detection and mount-time setup.
2. Internal journal replay during mount when recovery is needed.
3. Runtime journal transaction emission for dirty block flushes.
4. Deferred checkpointing of committed transactions to home blocks.
5. Revoke parsing/emission, including wrapped transaction sequence ordering.
6. Mount-table/runtime labeling of journaled mounts as `ext3`.
7. Runtime journal abort state on I/O fault with fail-fast behavior.
8. Feature-bit acceptance matching the stock `mke2fs -t ext3` set, with explicit
   rejection of everything else.
9. An ordered-mode transaction scaffold with explicit mutation scopes, sealed
   pending transactions, bounded credit reservations, segmented restart for
   variable-size mutators, and deterministic ordered writeback.
10. Allocator/free helper participation in restart planning, including
    phase-split allocation helper behavior (bookkeeping vs initialization/link).
11. Explicit named restart phases recorded on ordered transaction segments,
    moving the restart path closer to a sub-handle model.
12. Closure-scoped restart-phase helpers that group real metadata work into
    explicit sub-handle-like units instead of only tagging ambient transaction
    state.
13. A lightweight internal journal-handle token threaded through the main
    ordered write/truncate/alloc/free helpers, so those paths no longer rely
    solely on implicit ambient running-transaction lookup.
14. The same internal handle token now reaches the main fixed-shape ext2
    metadata mutators and directory-entry helpers, including create/mkdir,
    unlink/rmdir, rename, link, and symlink paths.
15. Stock ext3 images mount read-write: `ext_attr`, `resize_inode`,
    `dir_index`, and `large_file` are implemented (see section 6), fast
    symlinks are read and created inline, and an `e2fsck -fn` cross-check
    (`tests/ext3_fsck.sh`) guards the on-disk results.
16. JBD-style forget/cancel-revoke semantics in the running ordered
    transaction, plus a fix for revoke-only or empty sealed transactions that
    used to wedge the pending queue (section 7).
17. Crash-consistent journal superblock: `s_sequence` now names the oldest live
    transaction and `INCOMPAT_RECOVER` tracks whether the log holds committed
    transactions (section 8). A simulated crash image is replayed both by the
    kernel and by `e2fsck` (Linux jbd2 recovery code).
18. Transaction atomicity and checkpoint safety (section 9): a sealed
    transaction commits as one journal transaction, newer transactions keep
    their own snapshots of shared blocks, and committed revokes remove freed
    blocks from older checkpoint lists.
19. Write barriers and multi-descriptor commits (section 10): journal I/O is
    ordered through a backend flush hook (block-cache drain + ATA FLUSH CACHE
    EXT), mount-time recovery retires the log only after replayed blocks are
    durable, and a transaction may span several descriptor blocks.
20. Commit checksums, journal feature validation, ext3 commit semantics, and
    a QEMU crash/recovery smoke on real AHCI I/O (section 11), which found and
    fixed two cache-staleness bugs.

Key code paths:

- `crates/fs/src/ext2_journal.rs`
- `crates/fs/src/ext2_api.rs`
- `crates/fs/src/ext2_sync.rs`
- `crates/fs/src/ext2_alloc.rs`
- `crates/fs/src/lib.rs`
- `kernel/src/vfs.rs`

Representative tests:

- `ext2_mount_replays_internal_journal_and_clears_recover_state`
- `ext2_mount_replay_revoke_handles_wrapped_transaction_sequences`
- `ext2_journaled_flush_emits_runtime_transaction`
- `ext2_journaled_flush_batches_multiple_blocks_in_one_transaction`
- `ext2_journaled_flush_rotates_runtime_head_between_transactions`
- `ext2_journaled_flush_wraps_runtime_transaction_layout`
- `ext2_journaled_flush_emits_revoke_block_from_pending_revoke_set`
- `ext2_journal_runtime_io_error_aborts_and_blocks_future_flushes`
- `ext2_journal_checkpoint_io_error_keeps_committed_tx_and_aborts`
- `ext2_mount_rejects_journal_feature_bits`
- `ext2_mount_rejects_unknown_incompat_features`
- `ext2_ordered_mode_restartable_scope_seals_multiple_segments`
- `ext2_ordered_mode_prepare_running_tx_credits_rotates_before_next_step`
- `ext2_ordered_mode_step_credit_estimators_account_for_indirection`
- `ext2_ordered_mode_alloc_block_helper_rotates_before_bitmap_update`
- `ext2_ordered_mode_alloc_inode_helper_rotates_before_inode_bitmap_update`
- `ext2_ordered_mode_indirect_step_estimate_rotates_before_allocation`
- `ext2_ordered_mode_large_write_ends_with_metadata_closeout_segment`
- `ext2_ordered_mode_restartable_phase_helper_groups_work_into_named_subhandles`
- `ext2_ordered_mode_handle_phase_rejects_stale_handle`
- `ext2_ordered_mode_public_metadata_mutator_reserves_bounded_credits`
- `ext3_stock_image_mounts_with_default_feature_set`
- `ext3_htree_directory_reads_linearly`
- `ext3_htree_insert_clears_index_flag_and_persists`
- `ext3_xattr_block_is_counted_and_released_on_unlink`
- `ext3_shared_xattr_block_only_drops_refcount`
- `ext3_fast_symlink_reads_inline_target_and_unlinks_without_freeing_blocks`
- `ext3_large_file_reports_full_size_and_refuses_32bit_io`
- `ext3_freed_dirty_blocks_do_not_wedge_pending_transactions`
- `ext3_stock_image_mutations_sync_and_remount` (also feeds `tests/ext3_fsck.sh`)
- `ext3_recover_flag_tracks_live_journal_transactions`
- `ext3_crash_image_needs_recovery_and_replays` (also feeds `tests/ext3_fsck.sh`)
- `ext3_newer_transaction_keeps_its_snapshot_of_shared_blocks`
- `ext3_checkpoint_does_not_overwrite_reused_freed_block`
- `ext3_large_transaction_spans_descriptor_blocks_and_replays` (also feeds `tests/ext3_fsck.sh`)
- `ext3_crash_at_every_write_with_reordering_cache_stays_consistent` (also feeds `tests/ext3_fsck.sh`)
- kernel: `flush_device_writes_back_cache_before_device_flush`, `flush_device_without_flush_op_only_drains_cache`
- `ext3_crc32_be_matches_jbd2_definition`
- `ext3_mount_enables_journal_checksums_and_commits_carry_crc32`
- `ext3_corrupted_journal_block_stops_replay_at_that_transaction` (also feeds `tests/ext3_fsck.sh`)
- `ext3_mount_rejects_unknown_journal_incompat_features`
- `ext3_flush_issues_one_barrier_per_commit_plus_one`
- `ext3_partial_checkpoint_does_not_roll_back_cached_bitmaps`
- `ext3_evicting_uncheckpointed_block_does_not_resurrect_stale_bitmap`
- QEMU: `tests/musl_ext3_journal_smoke.sh` (`make run-journal-smoke`)

All of the above are in `crates/fs/src/lib.rs` test module.

## Implemented

### 1. Mount-time journal discovery and replay

- Parses ext journal superblock (JBD v1/v2 header format).
- Supports replay of committed descriptor/data/(optional revoke)/commit sequences.
- Applies replayed updates to home blocks and clears recover state before exposing the mount.
- Handles wrap-safe transaction ordering for revoke precedence.

### 2. Runtime journal write path

- Dirty ext2 blocks can flush through journal transactions (descriptor + data + optional revoke + commit).
- Supports multi-block batching in a single transaction.
- Supports wrap-aware ring placement and advancing runtime head.
- Handles escaped data blocks whose first 4 bytes match JBD magic.
- Journaled mounts now carry explicit journal-mode state, currently defaulting
  to an ordered-mode scaffold with running transaction tracking for dirty and
  data blocks.
- Ordered-mode bring-up now separates tracked file data from metadata during
  flush: file data is written to home blocks before the journal commit record,
  while the journal transaction itself carries metadata blocks only.
- Ordered-mode metadata selection now prefers the running transaction's tracked
  metadata set instead of constructing commit payloads from unrelated global
  dirty blocks when an active transaction is present.
- Metadata/data tracking now enters through explicit ext2 mutation scopes around
  top-level filesystem mutators instead of every later dirty-block mark
  automatically joining the pending transaction.
- When a top-level ordered-mode mutation scope closes, the pending transaction
  now snapshots the exact metadata and file-data block images that scope
  produced. Flush and checkpoint paths commit from those sealed snapshots
  instead of rereading whatever cache contents happen to exist later.
- Ordered-mode revoke ownership now follows those sealed transactions as well:
  block-free revoke records from scoped mutations are queued with the matching
  pending transaction instead of only through the shared global revoke set.
- If a later scope dirties the same block before an older ordered transaction
  checkpoints, the older transaction no longer clears that newer dirty cache
  state when it commits or checkpoints. This keeps pending ordered
  transactions deterministic even when metadata blocks are shared across
  consecutive top-level operations.
- Single-block and dirty-pressure flushes now respect sealed ordered
  transaction order as well: if asked to flush a block owned by a later pending
  transaction, the runtime writeback path advances older pending transactions
  first instead of emitting an out-of-order one-block journal commit.
- Dirty-pressure writeback now has an explicit "flush oldest sealed ordered
  transaction" path instead of choosing an arbitrary dirty block and relying on
  that block's ownership to discover transaction order indirectly.
- CLOCK eviction now uses that same oldest-pending-transaction path for dirty
  ordered-mode cache victims, so eviction no longer bypasses transaction order
  by flushing the victim block directly.
- While an ordered mutation scope is still open, dirty-pressure writeback no
  longer flushes those in-flight blocks early, and eviction will now refuse to
  evict a cache working set made entirely of dirty running-transaction blocks
  instead of committing them before the scope seals.
- Ordered running and sealed transactions now carry explicit transaction IDs,
  coalesce nested scopes into one top-level transaction identity, and maintain
  basic reserved-vs-used credit accounting for unique tracked metadata/data/
  revoke blocks.
- Fixed-shape public metadata mutators (`create`, `mkdir`, `unlink`/`rmdir`,
  `rename`, `link`, `symlink`, `chmod`, `chown`, `utimens`) now reserve
  bounded credits at scope entry.
- Variable-size paths (`write`, `truncate`, and `O_TRUNC`) now reserve coarse
  estimated credit budgets based on file/block geometry and feed those through
  an explicit per-segment planner instead of opening fully unbounded scopes.
- In practice this means the journal layer now enforces bounded intent across
  most public ext2 mutators, but the estimates are intentionally conservative
  and may still over-reserve substantially compared with real JBD handle
  credit planning.
- Those mutators now start with a capped segment-sized reservation instead of
  reserving the whole coarse estimate up front, so large writes/truncates enter
  the restart path intentionally rather than only after a huge one-shot
  reservation overflows later.
- The ext2 block-walking mutation loops now call into the journal layer before
  their next data/freeing step, so restartable ordered segments rotate at
  loop boundaries instead of waiting for a later per-block dirty-note to force
  the split.
- That loop-level restart planning is now indirection-aware rather than a flat
  one-credit heuristic: existing data writes, direct allocation, indirect
  table growth, direct free, and detach/free paths reserve different restart
  budgets based on the current block-mapping shape.
- The lower-level allocator/free helpers now participate directly as well:
  block allocation/free, inode allocation/free, and indirect-table entry
  creation all ask the journal layer for restart headroom before dirtying their
  own bitmaps/counters/table blocks, so restart boundaries can happen at real
  helper phases instead of only at outer mutator loops.
- Allocation helpers are now split into explicit metadata phases internally:
  they scan for a target first, restart before bitmap/counter bookkeeping,
  then can restart again before block/inode initialization or indirect-table
  linking. That is closer to a sub-handle model than the earlier single
  helper-entry preflight.
- The shrink side now follows the same pattern more closely: indirect detach
  paths can restart before unlinking table entries, and free-block helpers can
  restart again before revoke emission. That makes truncate-side segmentation
  less opaque and brings detach/free work closer to explicit sub-handle phases.
- Those restart points are now recorded through explicit named journal phases
  (`DataWrite`, `DataZero`, `InodeCloseout`, allocation/free bookkeeping,
  revoke, indirect link/unlink, and related helper phases) rather than being
  just scattered credit-preflight calls. Sealed ordered transaction segments
  now retain the last phase kind plus a phase-entry count, giving the journal
  layer a more explicit sub-handle-like picture of how a mutation was split.
- The main ext2 mutation paths now use a closure-scoped restart-phase helper
  to group real metadata work inside those phases. Allocation/free helpers,
  indirect link/unlink updates, data-write/zero steps, and inode-closeout
  metadata are now expressed as explicit grouped phase bodies instead of
  "phase label now, work later" sequencing. This is still not a true JBD
  handle API, but it narrows the gap between the current ordered-mode scaffold
  and explicit sub-handle boundaries.
- Those same write/truncate/alloc/free paths now also carry a lightweight
  internal journal-handle token keyed to the logical ordered operation. The
  grouped phase helper can validate that token against the current running
  ordered transaction before performing phase work, and stale handles are now
  rejected in tests. This is still much smaller than a real JBD handle object,
  but it is a concrete step away from purely ambient mutation context.
- That handle token is no longer limited to variable-size and allocator-heavy
  paths. The fixed-shape ext2 metadata mutators now thread it through
  create-child allocation, directory-entry insertion/removal, rename relink,
  dotdot patching for moved directories, unlink cleanup, and symlink
  construction/rollback paths. The ordered-mode scaffold therefore now has
  explicit handle ownership across most important ext2 mutators, even though
  the handle object is still much lighter than a true JBD handle.
- Variable-size mutators also rotate before their final inode metadata write,
  so a large write/truncate can end in a metadata-closeout tail segment instead
  of always mixing the inode update into the last data/freeing segment.
- Variable-size restartable scopes no longer just grow one in-memory budget:
  when they exhaust a credit reservation they now seal the current ordered
  transaction segment into the pending queue, allocate a fresh transaction ID,
  and continue the same logical operation in a later segment.
- This is still weaker than real JBD restart semantics because the restart is
  driven by local dirty-block tracking rather than explicit handle stop/start
  against the committing journal, but it now matches the high-level "split the
  oversized mutation into multiple ordered commits" shape instead of behaving
  like one elastic transaction.

### 3. Deferred checkpointing

- Committed transactions are tracked and checkpointed later to home blocks.
- Journal `start` is updated based on oldest uncheckpointed transaction.
- `sync`/`fsync` plumbing in the kernel triggers journal flush + checkpoint path.

### 4. Runtime revoke support

- Freed blocks can be added to pending revoke set.
- Revoke blocks can be emitted in runtime transactions.
- Replay honors revoke precedence with wrapped sequence comparisons.

### 5. Runtime abort semantics (baseline)

- Runtime journal write/checkpoint paths now track an aborted state after journal
  I/O failure (`EIO`).
- Once aborted, subsequent runtime journal flush/checkpoint/revoke-note paths
  fail fast with the recorded abort errno.
- Checkpoint failure no longer drops queued committed transactions from
  in-memory state before a successful checkpoint pass.

### 6. Feature acceptance (stock ext3 set)

- The allowlist matches what `mke2fs -t ext3` enables by default:
  - compat: `HAS_JOURNAL`, `EXT_ATTR`, `RESIZE_INO`, `DIR_INDEX`
  - incompat: `FILETYPE`, `RECOVER`, `JOURNAL_DEV`
  - ro-compat: `SPARSE_SUPER`, `LARGE_FILE`
- Each accepted bit has explicit handling:
  - `EXT_ATTR`: `i_file_acl` is parsed, counted in `i_blocks` recalculation
    (so truncate/rewrite keep the xattr block charged), and released on final
    unlink. The block's `h_refcount` is decremented, and the block is freed
    (with a revoke) when it reaches zero. Blocks without the xattr magic are
    left allocated. In-inode xattrs survive because inode writes only touch
    the fields the kernel owns.
  - `RESIZE_INO`: reserved GDT blocks are pre-marked in the bitmaps and the
    resize inode is never modified; the kernel does not resize.
  - `DIR_INDEX`: htree directories are scanned linearly. The first insert
    clears `EXT2_INDEX_FL` via a dedicated flag-clear helper, so later inode
    writes from stale in-memory copies cannot restore it. Linear insertion may
    overwrite dx interior data, which is safe once the flag is gone. Removes
    keep the index, as Linux does.
  - `LARGE_FILE`: `i_size_high` is parsed; `stat` reports 64-bit sizes for
    regular files; reads return `EOVERFLOW` and writes/truncates `EFBIG` for
    files that do not fit in 32 bits. Unlink still frees the full block tree.
- Fast symlinks (`i_blocks` minus the xattr block equal to zero, size < 60) are
  read from `i_block` and freed without treating the target text as block
  pointers. Before this, unlinking a fast symlink from an `mke2fs -d` image
  would have freed arbitrary blocks. `symlinkat` writes short targets as fast
  symlinks, which modern `e2fsck` requires.
- Journaled mounts require an internal journal inode and reject `journal_dev`
  / external-journal topology.

### 7. Ordered-journal revoke and queue fixes

- Freeing a block that the running ordered transaction already tracks (for
  example an indirect table emptied during unlink) now removes it from the
  transaction's metadata/data sets and clears its dirty state ("forget"), so
  only the revoke is committed. Previously the stale contents were journaled
  and the revoke was dropped as superseded.
- Tracking a block that has a pending revoke in the running transaction or the
  global revoke set cancels that revoke ("cancel revoke"), so reuse in a later
  commit is never suppressed by an older free.
- When a commit journals a block that also has a revoke in the same pending
  transaction, the revoke is now retired along with it instead of lingering.
- Sealed transactions can lose all their blocks when an older transaction
  commits a shared metadata block (`drop_pending_tx_blocks` strips it from every
  pending transaction). A transaction left with only revokes, or with nothing,
  used to make `flush_oldest_ordered_pending_tx` return 0 forever, so
  `flush_dirty_block` failed with `EIO` and dirty-pressure writeback stalled.
  Such transactions now fold their revokes into the next pending transaction
  (or the global revoke set) and are retired.

### 8. Crash consistency of the journal superblock

- Runtime commits and checkpoints used to write the *next* transaction ID
  into the journal superblock's `s_sequence` while `s_start` still pointed at
  the oldest uncheckpointed transaction. Recovery (ours and Linux's) requires
  the transaction at `s_start` to carry `s_sequence`, so it hit a mismatch at
  the first descriptor and replayed nothing: a crash between commit and
  checkpoint lost every committed transaction. `s_sequence` is now the
  oldest live transaction's ID, or the next ID when the log is empty.
- `INCOMPAT_RECOVER` is written to the on-disk superblock before the commit
  record that makes an empty log non-empty, and cleared after a checkpoint
  writes `s_start = 0`. Only that bit is changed on disk (the cached superblock
  may hold uncommitted counters), and every later home write of the
  superblock block is patched to the current marker so a stale cached or
  journaled copy cannot flip it. Linux keeps the flag set for the whole
  read-write mount; tracking actual log contents instead means a synced
  image is immediately clean for `e2fsck`.
- `tests/ext3_fsck.sh` exports a crash image (committed, not checkpointed),
  requires `needs_recovery`, lets `e2fsck -fy` replay it, then requires a
  clean `e2fsck -fn` and the committed file contents.

### 9. Transaction atomicity and checkpoint safety

- **Whole-transaction commits.** Dirty-pressure writeback passed its soft
  budget (`cached_blocks / 8`, often 1) as the commit size, so one sealed
  ordered transaction (a single create, say) was committed as several
  one-block journal transactions; a crash between them replayed half an
  operation. `flush_oldest_ordered_pending_tx` now commits the oldest sealed
  transaction's metadata as one journal transaction, limited only by the
  journal's per-transaction capacity.
- **Newer transactions keep shared blocks.** Committing (or writing ordered data
  home for) the oldest pending transaction used to remove those blocks from
  *every* pending transaction and from the running one. Shared metadata such as
  bitmaps, group descriptors, the superblock, and parent directory blocks
  vanished from newer transactions, which then committed without their own
  changes to them; a crash right after replayed, for example, a file whose
  inode was not marked in the inode bitmap. Each release now names its source
  (`TxSource::PendingHead` or `TxSource::Running`) and only that transaction
  gives the blocks up; the cache stays dirty while a newer owner exists.
- **Checkpoint after reuse.** Once a commit carrying revoke records is on disk,
  those blocks are removed from older committed transactions' checkpoint
  lists, as JBD does. Previously a block journaled by T0, freed in T1, and
  reused as ordered file data by T2 (written straight home) was overwritten
  with T0's stale copy when T0 was checkpointed. The drop happens only after
  the revoke is durable, so a crash can never leave checkpointed metadata
  pointing at a block that missed its journaled contents.

### 10. Write barriers and multi-descriptor transactions

- `Ext2BlockBackend` has an optional `flush` hook: after it returns, every
  earlier `write_block` is on stable storage. The kernel implements it with
  `block::flush_device` (write back the device's dirty block-cache lines, then
  ATA `FLUSH CACHE EXT` on AHCI). Before this, journal blocks went into the
  kernel's LRU write-back cache and reached the disk in eviction order, so a
  commit record could become durable before the blocks it sealed.
- Barrier points, matching JBD:
  1. Before the commit record: descriptors, journaled copies, revoke blocks,
     ordered file data, and the `INCOMPAT_RECOVER` marker are durable.
  2. After the commit record: the commit is durable before any checkpoint
     write of that transaction's blocks.
  3. In checkpoint: home blocks are durable before the journal tail moves,
     and the empty tail is durable before `INCOMPAT_RECOVER` is cleared.
- Mount-time recovery used to zero the journal tail *before* flushing the
  replayed home blocks (replay only dirtied them in the cache). A crash in
  between lost the replayed transactions and emptied the log. It now keeps
  `INCOMPAT_RECOVER` set, flushes replayed blocks, barriers, retires the log,
  barriers, and only then clears the marker.
- The pre-commit journal superblock write also used the wrong `s_sequence`
  when older transactions were still live; it now uses the same
  oldest-live-transaction rule as the post-commit and checkpoint writes.
- A transaction now uses as many descriptor blocks as it needs
  (`[desc, data..., desc, data..., revoke..., commit]`), so its capacity is the
  journal window rather than one descriptor's tag count (126 for 1 KiB
  blocks). Larger sealed transactions are no longer split into independent
  commits; replay in both this kernel and e2fsck already handled multiple
  descriptors per transaction.
- `ext3_crash_at_every_write_with_reordering_cache_stays_consistent` models a
  volatile write cache that may persist only the newest undrained writes, and
  crashes after every backend write of a create/write/unlink/mkdir/sync
  sequence. Every crash image must mount and show each operation all-or-
  nothing, and `tests/ext3_fsck.sh` runs `e2fsck -fn` on all ~230 recovered
  images. With the commit barriers removed, the test finds a torn file
  (correct size, zeroed contents).

### 11. Checksums, journal features, commit semantics, QEMU crash testing

- **Commit checksums.** The kernel enables JBD2 `COMPAT_CHECKSUM` on mount
  (v2 journal superblocks, only once the log is empty) and writes a CRC32
  (`crc32_be`, seed `~0`) over each transaction's descriptor and journaled
  blocks into the commit block (`h_chksum_type = 1`, `h_chksum_size = 4`).
  Replay verifies it with jbd2's rules: a mismatch ends the log at that
  transaction; an all-zero checksum is accepted only before the first
  checksummed commit. `e2fsck` checks the same CRCs, so the fsck script proves
  both that our CRCs are right (it replays them) and that a damaged block is
  rejected (it refuses that transaction).
- **Journal feature validation.** Journal superblock features used to be
  ignored entirely. JBD2 incompat bits other than `REVOKE` (64-bit tags,
  `ASYNC_COMMIT`, `CSUM_V2/V3`, ...) and any ro-compat bit now make the mount
  fail with `EINVAL` instead of misparsing the log.
- **One barrier per commit.** The post-commit barrier is deferred
  (`commit_barrier_pending`) and settled before checkpoint home writes and at
  the end of an explicit flush/sync; the next commit's pre-commit barrier also
  settles it. N commits in one flush cost N + 1 device flushes instead of 2N.
- **ext3 commit semantics.** Mutating syscalls used to flush *and checkpoint*,
  emptying the log after every call. They now only commit (durable before the
  syscall returns); `sync`/`fsync` checkpoint, and the journal checkpoints by
  itself when log space runs out.
- **QEMU crash/recovery smoke** (`scripts/run_toybox_journal_smoke.sh`, profile
  `musl-ext3-journal-smoke`, helper `user/journal_smoke_helper.c`): boot an
  ext3 copy of the runtime image, sync a baseline, write 48 files, SIGKILL
  QEMU (losing the PeetOS block cache), require `needs_recovery` and
  `journal_checksum`, let `e2fsck` recover a copy cleanly, then boot the
  crashed image again so the kernel recovers it, verify every surviving file
  byte for byte, and require a clean `e2fsck -fn`.
- **Bugs this found** (both latent while every syscall checkpointed, and both
  invisible to the host tests until checkpoints lagged commits):
  1. `write_checkpoint_home_block` copied a checkpointed (older) snapshot into
     a clean cache entry. A partial checkpoint after a newer commit rolled
     cached bitmaps back, so the allocator reused blocks and inodes that a
     later transaction owned (e2fsck: multiply-claimed blocks). Checkpoint
     writes no longer touch cached contents.
  2. CLOCK eviction dropped clean blocks whose newest version was only in the
     journal, so re-reads came from a stale home copy. Eviction now
     checkpoints through the last transaction that journaled the block.

## Gaps

### 1. Not full ext3/JBD compatibility

- No full JBD transaction-handle model:
  - no journal handles
  - no callsite-exact handle credit model
  - no richer transaction lifecycle or per-handle attachment of metadata
    updates beyond the current named-phase ordered-mode scaffold
- No ext3 mount option semantics:
  - no `data=journal`
  - no complete `data=ordered` implementation yet (mode/state, running
    transaction tracking, and metadata-vs-data flush separation exist, but the
    filesystem still lacks a full handle-based ext3 transaction model)
  - no `data=writeback`
  - no commit interval / barrier / error-policy surface
- No separate ext3 driver/API surface; behavior is still ext2 plus optional
  internal journal handling, with journaled mounts surfaced as `ext3` in mount
  metadata.

### 2. Journal topology limits

- External journal device is explicitly rejected.
- Internal journal only (`journal_inode` path).

### 3. Feature coverage limits

- Stock ext3 images mount, but support for some features is partial:
  - xattrs are preserved/released, not exposed (`getxattr`/`setxattr` absent),
    and POSIX ACLs stored in them are not enforced.
  - htree indexes are dropped on the first insert rather than maintained, so
    large directories degrade to linear lookups until `e2fsck -D`.
  - files over 4 GiB are read-only-by-refusal (no 64-bit I/O path).
- `IMAGIC_INODES`, `DIR_PREALLOC`, and all ext4-era bits (extents, flex_bg,
  64bit, metadata_csum, huge_file, ...) are still rejected.
- Sharing of xattr blocks is only honored on release; the kernel never creates
  or deduplicates xattr blocks.

### 4. Broader filesystem integration still incomplete

- Current docs already note this is not complete ext3 mode behavior.
- Filesystem writeback still flows through ext2 dirty-block tracking and a
  journal-on-flush path rather than a complete ext3-style ordered writeback
  model.
- Ordered-mode commit contents are now seal-time snapshots, but the filesystem
  still lacks a true JBD handle/credit model and still permits background
  writeback to reuse parts of the older per-block journal machinery rather than
  a fully unified JBD-style commit engine.
- Credit accounting is only a basic scaffold today: existing mutators still use
  coarse estimates in several places, and there is no full ext3/JBD-style
  commit-and-restart semantics, per-buffer attachment lifecycle, or
  callsite-exact credit planning. Current "restart" behavior can split one
  oversized logical mutation into multiple sealed ordered transaction segments,
  but it is still not a true JBD handle restart against a separately
  committing/newly started transaction pair.
- Full fs-wide VM page-cache integration and broader hardening are still
  pending.
- The QEMU crash smoke kills QEMU at one point (after the workload). Finer
  crash points are covered by the host reordering-cache test, not under QEMU.
- `user/journal_smoke_helper.c` uses `mkdirat`: musl's `mkdir()` issues the
  legacy `mkdir` syscall, which the kernel does not implement (`ENOSYS`).
- Crash testing is host-side. The reordering-cache model treats individual
  block writes as atomic; torn sector writes inside one block are not
  modeled, and commit records carry no checksum (no `journal_checksum`) to
  detect them.
- A sealed transaction is only split if it exceeds the entire journal window
  (descriptors + data + commit must fit in the log). The credit planner keeps
  segments far smaller, so this only affects tiny synthetic test journals.
  There is no JBD-style cap of a quarter of the journal per transaction.

## Ordered Patch Checklist

The following list is ordered to move from the current partial implementation
to something materially closer to full ext3 parity.

1. Tighten advertised feature compatibility.
   - Audit every currently accepted feature bit.
   - Reject compat bits that are not actually implemented yet, or implement the
     missing behavior before continuing to advertise support.
   - This reduces the risk of mounting images whose on-disk structures the
     kernel does not truly understand.

2. Define and implement one concrete ext3 data mode first: `data=ordered`.
   - Ordered mode is the most pragmatic parity target.
   - Metadata commit ordering must ensure data blocks reach stable storage
     before the journal commits metadata that references them.
   - Document the intended semantics in `docs/vfs.md` and `README.md`.

3. Introduce a real JBD-style transaction/handle layer.
   - Add transaction handles with credit accounting.
   - Attach metadata updates to transactions at mutation time rather than only
     during dirty-block flush.
   - Split running, committing, and checkpointed transaction state explicitly.

4. Rework write paths around transaction ownership instead of flush-time
   journaling.
   - Metadata operations such as create, unlink, rename, link count updates,
     bitmap changes, inode updates, and directory entry updates should enter the
     journal as part of a transaction scope.
   - Flush should become a durability boundary, not the place where journal
     intent is first constructed.

5. Implement ext3 `data=ordered` guarantees end to end.
   - Track data blocks dirtied by a transaction.
   - Force ordered home-block writeback of those data blocks before the
     transaction commit record is considered durable.
   - Add tests that distinguish ordered mode from the current metadata-journal
     flush model.

6. Add ext3 mount/error policy plumbing.
   - Parse and store mount options relevant to journaling.
   - Add explicit behavior for abort/error policy, at minimum distinguishing
     fail-fast, continue, and remount-read-only style policies even if the first
     implementation is conservative.

7. Support external journals or explicitly document them as out of scope for
   parity.
   - Full ext3 parity should include `journal_dev` topology support.
   - If that is intentionally out of scope, the docs should say parity is
     limited to internal-journal ext3.

8. Add dedicated crash/recovery integration tests in QEMU.
   - Cover interrupted metadata-only transactions.
   - Cover ordered-mode data visibility across crash boundaries.
   - Cover revoke correctness, wrapped sequence behavior, and post-abort mount
     handling.

9. Integrate journal semantics with the VM/file page cache path.
   - Shared writable mappings and syscall writeback should obey the same
     journaling and ordering rules.
   - Avoid divergent durability behavior between buffered syscall writes and
     mapped-file writeback.

10. Expand the supported ext3 feature set deliberately.
   - Done for the stock `mke2fs -t ext3` set (`ext_attr`, `resize_inode`,
     `dir_index`, `large_file`), validated against a real mkfs image plus
     `e2fsck -fn` after kernel-side mutation.
   - Remaining: maintain htree indexes on insert, expose xattrs, 64-bit file
     I/O, and only then consider anything beyond the ext3 matrix.

11. Done: `INCOMPAT_RECOVER` tracks live journal contents and `s_sequence`
    follows JBD semantics (section 8); checkpoint-after-reuse and shared-block
    snapshot loss are fixed (section 9); journal writes are ordered by write
    barriers and transactions can span several descriptor blocks (section 10).
    Commit checksums, journal feature validation, and a QEMU crash/recovery
    smoke are done (section 11). Remaining hardening: maintaining htree
    indexes, exposing xattrs, 64-bit file I/O, and a JBD-style per-transaction
    size cap.

## Historical Comparison (`peetos_c`)

- `peetos_c` roadmap and kernel code are ext2-focused; no ext3 journal implementation exists there.
- Therefore ext3/journaling is currently a Rust-only extension rather than a legacy compatibility requirement.
- Practical implication: Rust currently includes this capability, but it is still below full ext3/JBD behavior.

## Limitations / Simplifications

- Single in-kernel implementation path with global VFS state; no full Linux-like journaling architecture.
- Focused on crash-recovery essentials and baseline durability, not full ext3 compatibility matrix.
- Abort handling is a baseline runtime guard (`EIO`-driven state transition) rather than a full ext3 remount-ro + operator-policy model.
- Test coverage is currently strong at host-unit level for replay, flush,
  checkpoint, revoke, and abort mechanics, but there is no dedicated QEMU
  integration suite focused on ext3 crash scenarios yet.
- On-disk compatibility for a stock ext3 image is cross-checked with
  `e2fsck` for a clean (checkpointed) state and for one committed-but-not-
  checkpointed crash state; torn writes are not modeled.
