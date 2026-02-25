# Ext3 Feature Parity Audit (2026-02-25)

## Scope

This audit covers ext3-style journaling behavior in the Rust kernel (`/home/bill/source/peetos`) and compares it against:

- The current Rust implementation goals and behavior.
- Stage 1-9 parity source of truth in `/home/bill/source/peetos_c` (which is ext2-focused and does not implement ext3/JBD).

## Current Status

Rust currently implements a partial ext3-compatible path on top of the ext2 mount flow:

1. Journal feature detection and mount-time setup.
2. Internal journal replay during mount when recovery is needed.
3. Runtime journal transaction emission for dirty block flushes.
4. Deferred checkpointing of committed transactions to home blocks.
5. Revoke parsing/emission, including wrapped transaction sequence ordering.

Key code paths:

- `crates/fs/src/ext2_journal.rs`
- `crates/fs/src/ext2_api.rs`
- `crates/fs/src/ext2_sync.rs`
- `kernel/src/vfs.rs`

Representative tests:

- `ext2_mount_replays_internal_journal_and_clears_recover_state`
- `ext2_mount_replay_revoke_handles_wrapped_transaction_sequences`
- `ext2_journaled_flush_emits_runtime_transaction`
- `ext2_journaled_flush_batches_multiple_blocks_in_one_transaction`
- `ext2_journaled_flush_rotates_runtime_head_between_transactions`
- `ext2_journaled_flush_wraps_runtime_transaction_layout`
- `ext2_journaled_flush_emits_revoke_block_from_pending_revoke_set`

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

## Gaps

### 1. Not full ext3/JBD parity

- No full JBD transaction-handle model (credits, richer transaction state, abort semantics).
- No ext3 mount option semantics (for example data mode selection).
- No separate ext3 filesystem type; behavior is ext2 + optional internal journal handling.

### 2. Journal topology limits

- External journal device is explicitly rejected.
- Internal journal only (`journal_inode` path).

### 3. Feature coverage limits

- Superblock feature acceptance is intentionally narrow; unknown incompat/ro-compat bits are rejected.
- This is safer than silent misbehavior, but means many ext3/ext4-era feature combinations are unsupported.

### 4. Broader filesystem integration still incomplete

- Current docs already note this is not complete ext3 mode parity.
- Full fs-wide VM page-cache integration and broader hardening are still pending.

## Parity Against `peetos_c` (Stage 1-9 Source of Truth)

- `peetos_c` roadmap and kernel code are ext2-focused; no ext3 journal implementation exists there.
- Therefore ext3/journaling is currently a Rust-only extension rather than a required stage 1-9 parity item.
- Practical implication: Rust is ahead of C baseline for this area, but still below full ext3/JBD behavior.

## Limitations / Simplifications

- Single in-kernel implementation path with global VFS state; no full Linux-like journaling architecture.
- Focused on crash-recovery essentials and baseline durability, not full ext3 compatibility matrix.
- Test coverage is currently strong at unit level for replay/flush/checkpoint/revoke mechanics, but there is no dedicated QEMU integration suite focused on ext3 crash scenarios yet.
