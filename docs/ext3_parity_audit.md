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
  - no credit accounting / reservation model
  - no richer transaction lifecycle or per-handle attachment of metadata updates
- No ext3 mount option semantics:
  - no `data=journal`
  - no `data=ordered`
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
