# Ext3 Journal Status Audit (2026-04-12)

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
8. Narrow feature-bit acceptance with explicit rejection of unknown incompat/ro-compat combinations.
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

### 6. Conservative feature acceptance

- The mount parser accepts a small, explicit set of ext2/ext3-era feature bits
  and rejects unknown incompat/ro-compat combinations instead of attempting a
  best-effort mount.
- The allowlist is now intentionally tightened further:
  - compat: `HAS_JOURNAL` only
  - incompat: `FILETYPE`, `RECOVER`, `JOURNAL_DEV`
  - ro-compat: `SPARSE_SUPER` only
- Journaled mounts require an internal journal inode and reject `journal_dev`
  / external-journal topology.

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

- Superblock feature acceptance is intentionally narrow; unknown incompat and
  ro-compat bits are rejected.
- Several ext2/ext3-era feature bits are now rejected on purpose until their
  operational semantics are implemented:
  - `EXT_ATTR`
  - `DIR_INDEX`
  - `RESIZE_INO`
  - `IMAGIC_INODES`
  - `DIR_PREALLOC`
  - `LARGE_FILE`
- This is safer than silent misbehavior, but it means many ext3 and ext4-era
  images remain unsupported or only partially compatible.

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
- There is no dedicated QEMU crash/recovery suite exercising ext3 replay across
  actual reboot/failure scenarios.

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
   - Once the core journaling model is correct, add or validate operational
     support for accepted ext3-era feature bits.
   - Only then consider broader compatibility beyond the current narrow matrix.

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
