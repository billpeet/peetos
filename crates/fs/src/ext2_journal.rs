use alloc::vec::Vec;

use super::{
    ext2_backend, ext2_backend_read_block, ext2_block_slice_mut_state, ext2_cache_find,
    ext2_cache_flush_block, ext2_data_block_for_logical_index_state, ext2_mark_dirty_block,
    ext2_read_inode_from_state, ext2_read_u32_state, ext2_unmark_dirty_block,
    ext2_write_u32_state, Ext2Inode,
    Ext2JournalCommittedTx, Ext2JournalCommittedUpdate, Ext2JournalMode, Ext2JournalState,
    VfsState, EINVAL, EIO, ENOSPC, EXT2_FEATURE_COMPAT_HAS_JOURNAL,
    EXT2_FEATURE_INCOMPAT_JOURNAL_DEV, EXT2_FEATURE_INCOMPAT_RECOVER, EXT2_SUPERBLOCK_OFFSET,
};

const JBD_MAGIC: u32 = 0xC03B_3998;
const JBD_DESCRIPTOR_BLOCK: u32 = 1;
const JBD_COMMIT_BLOCK: u32 = 2;
const JBD_SUPERBLOCK_V1: u32 = 3;
const JBD_SUPERBLOCK_V2: u32 = 4;
const JBD_REVOKE_BLOCK: u32 = 5;

/// JBD2 superblock feature bits (v2 superblocks only).
const JBD2_FEATURE_COMPAT_CHECKSUM: u32 = 0x0000_0001;
const JBD2_FEATURE_INCOMPAT_REVOKE: u32 = 0x0000_0001;
/// Incompat/ro-compat bits this implementation understands. Anything else
/// (64BIT tags, ASYNC_COMMIT, CSUM_V2/V3, FAST_COMMIT, ...) changes the
/// on-disk log format, so such journals are refused rather than misparsed.
const JBD2_KNOWN_INCOMPAT: u32 = JBD2_FEATURE_INCOMPAT_REVOKE;
const JBD2_KNOWN_RO_COMPAT: u32 = 0;
/// Commit-block checksum type/size for COMPAT_CHECKSUM (crc32_be).
const JBD2_CRC32_CHKSUM: u8 = 1;
const JBD2_CRC32_CHKSUM_SIZE: u8 = 4;

const JBD_FLAG_ESCAPE: u32 = 0x0000_0001;
const JBD_FLAG_SAME_UUID: u32 = 0x0000_0002;
const JBD_FLAG_DELETED: u32 = 0x0000_0004;
const JBD_FLAG_LAST_TAG: u32 = 0x0000_0008;

#[derive(Clone, Copy)]
struct JournalHeader {
    block_type: u32,
    sequence: u32,
}

#[derive(Clone, Copy)]
struct JournalSuper {
    block_size: u32,
    max_len: u32,
    first: u32,
    sequence: u32,
    start: u32,
    /// v2 superblock (has feature fields).
    v2: bool,
    feature_compat: u32,
}

/// CRC-32 as used by JBD2 COMPAT_CHECKSUM (Linux `crc32_be`): polynomial
/// 0x04C11DB7, MSB first, caller-supplied seed, no final inversion.
fn crc32_be(mut crc: u32, data: &[u8]) -> u32 {
    for &byte in data {
        crc ^= (byte as u32) << 24;
        let mut bit = 0;
        while bit < 8 {
            crc = if (crc & 0x8000_0000) != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
            bit += 1;
        }
    }
    crc
}

#[cfg(test)]
pub(super) fn crc32_be_for_test(crc: u32, data: &[u8]) -> u32 {
    crc32_be(crc, data)
}

#[cfg(test)]
pub(super) fn journal_fs_block_for_test(state: &mut VfsState, jblock: u32) -> Result<u32, i64> {
    journal_block_to_fs_from_state(state, jblock)
}

#[cfg(test)]
pub(super) fn checksum_enabled_for_test(state: &VfsState) -> bool {
    state.ext2_journal.checksum
}

#[derive(Clone, Copy)]
struct DescriptorTag {
    home_block: u32,
    flags: u32,
}

#[derive(Clone, Copy)]
struct PendingUpdate {
    home_block: u32,
    journal_block: u32,
    escaped: bool,
}

fn read_u32_be(buf: &[u8], offset: usize) -> Result<u32, i64> {
    if offset + 4 > buf.len() {
        return Err(EIO);
    }
    Ok(u32::from_be_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
    ]))
}

fn write_u32_be(buf: &mut [u8], offset: usize, value: u32) -> Result<(), i64> {
    if offset + 4 > buf.len() {
        return Err(EIO);
    }
    let bytes = value.to_be_bytes();
    buf[offset] = bytes[0];
    buf[offset + 1] = bytes[1];
    buf[offset + 2] = bytes[2];
    buf[offset + 3] = bytes[3];
    Ok(())
}

fn parse_header(block: &[u8]) -> Result<Option<JournalHeader>, i64> {
    if block.len() < 12 {
        return Err(EIO);
    }
    let magic = read_u32_be(block, 0)?;
    if magic != JBD_MAGIC {
        return Ok(None);
    }
    Ok(Some(JournalHeader {
        block_type: read_u32_be(block, 4)?,
        sequence: read_u32_be(block, 8)?,
    }))
}

fn parse_superblock(block: &[u8], fs_block_size: usize) -> Result<JournalSuper, i64> {
    let header = parse_header(block)?.ok_or(EINVAL)?;
    if header.block_type != JBD_SUPERBLOCK_V1 && header.block_type != JBD_SUPERBLOCK_V2 {
        return Err(EINVAL);
    }

    let block_size = read_u32_be(block, 12)?;
    let max_len = read_u32_be(block, 16)?;
    let first = read_u32_be(block, 20)?;
    let sequence = read_u32_be(block, 24)?;
    let start = read_u32_be(block, 28)?;

    if block_size as usize != fs_block_size || max_len < 2 {
        return Err(EINVAL);
    }
    if first == 0 || first >= max_len {
        return Err(EINVAL);
    }
    if start != 0 && (start < first || start >= max_len) {
        return Err(EINVAL);
    }

    let v2 = header.block_type == JBD_SUPERBLOCK_V2;
    let (feature_compat, feature_incompat, feature_ro_compat) = if v2 {
        (
            read_u32_be(block, 36)?,
            read_u32_be(block, 40)?,
            read_u32_be(block, 44)?,
        )
    } else {
        (0, 0, 0)
    };
    if (feature_incompat & !JBD2_KNOWN_INCOMPAT) != 0
        || (feature_ro_compat & !JBD2_KNOWN_RO_COMPAT) != 0
    {
        return Err(EINVAL);
    }

    Ok(JournalSuper {
        block_size,
        max_len,
        first,
        sequence,
        start,
        v2,
        feature_compat,
    })
}

fn journal_next_block(superblock: &JournalSuper, current: u32) -> Result<u32, i64> {
    if current < superblock.first || current >= superblock.max_len {
        return Err(EIO);
    }
    let next = current.saturating_add(1);
    if next >= superblock.max_len {
        Ok(superblock.first)
    } else {
        Ok(next)
    }
}

fn tx_sequence_after(left: u32, right: u32) -> bool {
    (left.wrapping_sub(right) as i32) > 0
}

fn tx_sequence_after_or_eq(left: u32, right: u32) -> bool {
    left == right || tx_sequence_after(left, right)
}

fn journal_abort_errno(state: &VfsState) -> i64 {
    if state.ext2_journal.abort_errno == 0 {
        EIO
    } else {
        state.ext2_journal.abort_errno
    }
}

fn ensure_journal_not_aborted(state: &VfsState) -> Result<(), i64> {
    if state.ext2_journal.aborted {
        return Err(journal_abort_errno(state));
    }
    Ok(())
}

fn mark_journal_aborted(state: &mut VfsState, err: i64) {
    if !state.ext2_journal.present || state.ext2_journal.replaying || state.ext2_journal.aborted {
        return;
    }
    state.ext2_journal.aborted = true;
    state.ext2_journal.abort_errno = if err == 0 { EIO } else { err };
}

fn abort_on_runtime_io_error<T>(state: &mut VfsState, result: Result<T, i64>) -> Result<T, i64> {
    match result {
        Ok(value) => Ok(value),
        Err(err) => {
            if err == EIO {
                mark_journal_aborted(state, err);
            }
            Err(err)
        }
    }
}

fn backend_write_block_raw(state: &mut VfsState, block: u32, data: &[u8]) -> Result<(), i64> {
    let backend = match state.ext2_backend {
        ext2_backend::Ext2Backend::None => return Err(EIO),
        ext2_backend::Ext2Backend::BlockOps(backend) => backend,
    };
    let (sb_block, sb_offset) = fs_superblock_location(state);
    if state.ext2_journal.present && !state.ext2_journal.replaying && block == sb_block {
        // Every home write of the fs superblock block (checkpointed journal
        // snapshots, plain cache writeback) carries the *current* recovery
        // marker, so an older copy can never silently clear or set it.
        let mut patched = Vec::new();
        patched.try_reserve_exact(data.len()).map_err(|_| ENOSPC)?;
        patched.extend_from_slice(data);
        patch_recover_bit(&mut patched, sb_offset, state.ext2_journal.recover_marked)?;
        return (backend.write_block)(backend.ctx, block, patched.as_slice());
    }
    (backend.write_block)(backend.ctx, block, data)
}

/// Write barrier: everything written to the backend so far must be on
/// stable storage before anything written afterwards. Backends without a
/// flush hook are treated as write-through.
fn backend_barrier(state: &mut VfsState) -> Result<(), i64> {
    let backend = match state.ext2_backend {
        ext2_backend::Ext2Backend::None => return Err(EIO),
        ext2_backend::Ext2Backend::BlockOps(backend) => backend,
    };
    match backend.flush {
        Some(flush) => flush(backend.ctx)?,
        None => {}
    }
    // Every earlier write, including any not-yet-settled commit record, is
    // now durable.
    state.ext2_journal.commit_barrier_pending = false;
    Ok(())
}

/// Makes the most recent commit record durable if no barrier has done so
/// yet. Called before checkpoint home writes (which must not overtake their
/// commit) and when an explicit flush/sync finishes. Deferring the
/// post-commit barrier this way costs one device flush per batch of commits
/// instead of two per commit; the next commit's pre-commit barrier also
/// settles it.
pub(super) fn settle_commit_barrier(state: &mut VfsState) -> Result<(), i64> {
    if state.ext2_journal.commit_barrier_pending {
        backend_barrier(state)?;
    }
    Ok(())
}

fn backend_read_block_raw(state: &mut VfsState, block: u32, out: &mut [u8]) -> Result<(), i64> {
    let backend = match state.ext2_backend {
        ext2_backend::Ext2Backend::None => return Err(EIO),
        ext2_backend::Ext2Backend::BlockOps(backend) => backend,
    };
    (backend.read_block)(backend.ctx, block, out)
}

/// (block index, byte offset within that block) of the fs superblock.
fn fs_superblock_location(state: &VfsState) -> (u32, usize) {
    let block_size = state.ext2_meta.block_size;
    if block_size == 0 {
        return (u32::MAX, 0);
    }
    (
        (EXT2_SUPERBLOCK_OFFSET / block_size) as u32,
        EXT2_SUPERBLOCK_OFFSET % block_size,
    )
}

fn patch_recover_bit(block: &mut [u8], sb_offset: usize, on: bool) -> Result<(), i64> {
    let off = sb_offset + 96;
    if off + 4 > block.len() {
        return Err(EIO);
    }
    let mut incompat = u32::from_le_bytes([block[off], block[off + 1], block[off + 2], block[off + 3]]);
    if on {
        incompat |= EXT2_FEATURE_INCOMPAT_RECOVER;
    } else {
        incompat &= !EXT2_FEATURE_INCOMPAT_RECOVER;
    }
    block[off..off + 4].copy_from_slice(&incompat.to_le_bytes());
    Ok(())
}

/// Persists INCOMPAT_RECOVER = `on` in the on-disk superblock. Only that bit
/// changes on disk: the rest comes from the backend copy, because the cached
/// superblock may hold uncommitted counter updates. The cached copy gets the
/// same bit (without touching its dirty state) so reads and exports agree.
fn set_fs_recover_flag(state: &mut VfsState, on: bool) -> Result<(), i64> {
    if state.ext2_journal.recover_marked == on {
        return Ok(());
    }
    let (sb_block, sb_offset) = fs_superblock_location(state);
    let block_size = state.ext2_meta.block_size;
    let mut buf = Vec::new();
    buf.try_reserve_exact(block_size).map_err(|_| ENOSPC)?;
    buf.resize(block_size, 0);
    backend_read_block_raw(state, sb_block, &mut buf)?;
    state.ext2_journal.recover_marked = on;
    backend_write_block_raw(state, sb_block, buf.as_slice())?;
    if let Ok(idx) = ext2_cache_find(state, sb_block) {
        patch_recover_bit(&mut state.ext2_cached_blocks[idx].data, sb_offset, on)?;
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn recover_marked(state: &VfsState) -> bool {
    state.ext2_journal.recover_marked
}

fn refresh_cache_after_backend_write(state: &mut VfsState, block: u32, data: &[u8]) {
    if let Ok(idx) = ext2_cache_find(state, block) {
        let entry = &mut state.ext2_cached_blocks[idx];
        if entry.data.len() == data.len() {
            entry.data.copy_from_slice(data);
        }
        entry.referenced = true;
        entry.dirty = false;
    }
    let _ = ext2_unmark_dirty_block(state, block);
}

fn write_backend_and_refresh_cache(
    state: &mut VfsState,
    block: u32,
    data: &[u8],
) -> Result<(), i64> {
    backend_write_block_raw(state, block, data)?;
    refresh_cache_after_backend_write(state, block, data);
    Ok(())
}

/// Writes a committed (or ordered-data) snapshot to its home location.
///
/// The cached block is never touched: every snapshot was taken from the cache,
/// so the cache is always at least as new. Copying an older transaction's
/// snapshot back into a clean cache entry (which this used to do) rolled the
/// block back whenever a newer transaction had already committed it; with
/// checkpoints lagging behind commits, a rolled-back bitmap made the
/// allocator hand out blocks and inodes that were already in use.
fn write_checkpoint_home_block(state: &mut VfsState, block: u32, data: &[u8]) -> Result<(), i64> {
    backend_write_block_raw(state, block, data)?;
    if let Ok(idx) = ext2_cache_find(state, block) {
        state.ext2_cached_blocks[idx].referenced = true;
        if state.ext2_cached_blocks[idx].dirty {
            return Ok(());
        }
    }
    let _ = ext2_unmark_dirty_block(state, block);
    Ok(())
}

/// A clean cached block whose newest contents exist only in committed but
/// uncheckpointed transactions must not simply be dropped: its home location
/// is still older, so re-reading it would resurrect stale metadata. JBD pins
/// such buffers until checkpoint; here eviction checkpoints through the last
/// transaction that journaled the block first.
pub(super) fn checkpoint_through_block(state: &mut VfsState, block: u32) -> Result<(), i64> {
    if !state.ext2_journal.present || state.ext2_journal.replaying {
        return Ok(());
    }
    let last = state
        .ext2_journal_committed_txs
        .iter()
        .rposition(|tx| tx.updates.iter().any(|update| update.home_block == block));
    if let Some(idx) = last {
        checkpoint_committed(state, idx + 1)?;
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn checkpoint_oldest_for_test(state: &mut VfsState, count: usize) -> Result<usize, i64> {
    checkpoint_committed(state, count)
}

fn journal_cached_inode(state: &VfsState) -> Ext2Inode {
    Ext2Inode {
        mode: 0,
        uid: 0,
        size: 0,
        atime: 0,
        ctime: 0,
        mtime: 0,
        links_count: 0,
        gid: 0,
        block_count_512: 0,
        blocks: state.ext2_journal.inode_blocks,
        flags: 0,
        file_acl: 0,
        size_high: 0,
    }
}

fn journal_block_to_fs_from_state(state: &mut VfsState, journal_block: u32) -> Result<u32, i64> {
    let journal_inode = journal_cached_inode(state);
    let fs_block =
        ext2_data_block_for_logical_index_state(state, &journal_inode, journal_block as usize)?;
    if fs_block == 0 {
        return Err(EIO);
    }
    Ok(fs_block)
}

/// Tags that fit in one descriptor block (12-byte header, 8-byte tags; every
/// tag uses JBD_FLAG_SAME_UUID so no UUID follows).
fn descriptor_tag_capacity(state: &VfsState) -> usize {
    state.ext2_meta.block_size.saturating_sub(12).saturating_div(8)
}

fn descriptor_blocks_for(state: &VfsState, data_blocks: usize) -> usize {
    let tags = descriptor_tag_capacity(state);
    if tags == 0 || data_blocks == 0 {
        return 0;
    }
    (data_blocks + tags - 1) / tags
}

fn journal_window(state: &VfsState) -> usize {
    state
        .ext2_journal
        .max_len
        .saturating_sub(state.ext2_journal.first) as usize
}

/// Largest number of journaled blocks one transaction can carry: a
/// transaction may use as many descriptor blocks as it needs (replay and
/// e2fsck accept any number before the commit record), so the limit is the
/// journal window itself minus descriptors and the commit block.
fn runtime_tx_data_capacity(state: &VfsState) -> usize {
    let window = journal_window(state);
    if window < 3 || descriptor_tag_capacity(state) == 0 {
        return 0;
    }
    let mut data = window - 2;
    while data > 0 && data + descriptor_blocks_for(state, data) + 1 > window {
        data -= 1;
    }
    data
}

fn runtime_journal_super(state: &VfsState) -> JournalSuper {
    JournalSuper {
        block_size: state.ext2_journal.block_size,
        max_len: state.ext2_journal.max_len,
        first: state.ext2_journal.first,
        sequence: state.ext2_journal.sequence,
        start: state.ext2_journal.start,
        v2: true,
        feature_compat: if state.ext2_journal.checksum {
            JBD2_FEATURE_COMPAT_CHECKSUM
        } else {
            0
        },
    }
}

fn runtime_head_jblock(state: &VfsState) -> u32 {
    let head = state.ext2_journal.runtime_head_jblock;
    if head < state.ext2_journal.first || head >= state.ext2_journal.max_len {
        state.ext2_journal.first
    } else {
        head
    }
}

/// Journal placement for one transaction:
/// `[desc, data..., desc, data..., revoke..., commit]`.
struct TxLayout {
    descriptor_jblocks: Vec<u32>,
    data_jblocks: Vec<u32>,
    revoke_jblocks: Vec<u32>,
    commit_jblock: u32,
}

impl TxLayout {
    fn log_block_count(&self) -> usize {
        self.descriptor_jblocks.len() + self.data_jblocks.len() + self.revoke_jblocks.len() + 1
    }
}

fn runtime_tx_layout(
    state: &VfsState,
    data_blocks: usize,
    revoke_blocks: usize,
) -> Result<TxLayout, i64> {
    if data_blocks == 0 {
        return Err(EINVAL);
    }
    if data_blocks > runtime_tx_data_capacity(state) {
        return Err(EIO);
    }
    let descriptor_count = descriptor_blocks_for(state, data_blocks);
    if data_blocks + descriptor_count + revoke_blocks + 1 > journal_window(state) {
        return Err(EIO);
    }
    let tags_per_descriptor = descriptor_tag_capacity(state);

    let superblock = runtime_journal_super(state);
    let first_descriptor = runtime_head_jblock(state);
    let mut descriptor_jblocks = Vec::new();
    descriptor_jblocks
        .try_reserve_exact(descriptor_count)
        .map_err(|_| ENOSPC)?;
    let mut data_jblocks = Vec::new();
    data_jblocks
        .try_reserve_exact(data_blocks)
        .map_err(|_| ENOSPC)?;

    let mut cursor = first_descriptor;
    descriptor_jblocks.push(cursor);
    let mut idx = 0usize;
    while idx < data_blocks {
        if idx > 0 && idx % tags_per_descriptor == 0 {
            cursor = journal_next_block(&superblock, cursor)?;
            descriptor_jblocks.push(cursor);
        }
        cursor = journal_next_block(&superblock, cursor)?;
        data_jblocks.push(cursor);
        idx += 1;
    }
    let mut revoke_jblocks = Vec::new();
    revoke_jblocks
        .try_reserve_exact(revoke_blocks)
        .map_err(|_| ENOSPC)?;
    idx = 0;
    while idx < revoke_blocks {
        cursor = journal_next_block(&superblock, cursor)?;
        revoke_jblocks.push(cursor);
        idx += 1;
    }
    let commit_jblock = journal_next_block(&superblock, cursor)?;
    let mut conflict = false;
    for &jblock in descriptor_jblocks.iter() {
        if pending_tx_conflicts(state, jblock, &[], &[], commit_jblock) {
            conflict = true;
            break;
        }
    }
    if conflict
        || pending_tx_conflicts(
            state,
            first_descriptor,
            data_jblocks.as_slice(),
            revoke_jblocks.as_slice(),
            commit_jblock,
        )
    {
        return Err(ENOSPC);
    }
    Ok(TxLayout {
        descriptor_jblocks,
        data_jblocks,
        revoke_jblocks,
        commit_jblock,
    })
}

/// JBD `s_sequence` is the ID of the first transaction expected at `s_start`:
/// the oldest committed-but-uncheckpointed transaction while the log is live,
/// or the next ID to be issued once it is empty. Writing the *next* ID while
/// `s_start` still points at an older transaction makes recovery (ours and
/// Linux's) see a sequence mismatch at the first descriptor and replay nothing.
fn journal_super_sequence(state: &VfsState) -> u32 {
    match state.ext2_journal_committed_txs.first() {
        Some(tx) => tx.sequence,
        None => state.ext2_journal.sequence,
    }
}

fn write_journal_super_fields(state: &mut VfsState, sequence: u32, start: u32) -> Result<(), i64> {
    let block_size = state.ext2_meta.block_size;
    let mut super_block = Vec::new();
    super_block
        .try_reserve_exact(block_size)
        .map_err(|_| ENOSPC)?;
    super_block.resize(block_size, 0);
    ext2_backend_read_block(state, state.ext2_journal.superblock_block, &mut super_block)?;
    write_u32_be(&mut super_block, 24, sequence)?;
    write_u32_be(&mut super_block, 28, start)?;
    write_backend_and_refresh_cache(state, state.ext2_journal.superblock_block, &super_block)
}

fn insert_revoked(revoked: &mut Vec<u32>, block: u32) -> Result<(), i64> {
    match revoked.binary_search(&block) {
        Ok(_) => Ok(()),
        Err(insert_at) => {
            revoked.try_reserve(1).map_err(|_| ENOSPC)?;
            revoked.insert(insert_at, block);
            Ok(())
        }
    }
}

fn revoke_entries_per_block(state: &VfsState) -> usize {
    state
        .ext2_meta
        .block_size
        .saturating_sub(16)
        .saturating_div(4)
}

fn remove_sorted_u32(values: &mut Vec<u32>, value: u32) -> bool {
    match values.binary_search(&value) {
        Ok(idx) => {
            values.remove(idx);
            true
        }
        Err(_) => false,
    }
}

fn insert_sorted_unique_u32(values: &mut Vec<u32>, value: u32) -> Result<(), i64> {
    match values.binary_search(&value) {
        Ok(_) => Ok(()),
        Err(insert_at) => {
            values.try_reserve(1).map_err(|_| ENOSPC)?;
            values.insert(insert_at, value);
            Ok(())
        }
    }
}

fn reserve_running_tx_credits(state: &mut VfsState, credits: usize) -> Result<(), i64> {
    if credits == 0 {
        return Ok(());
    }
    state.ext2_journal_running_tx.reserved_credits = state
        .ext2_journal_running_tx
        .reserved_credits
        .checked_add(credits)
        .ok_or(ENOSPC)?;
    Ok(())
}

fn next_journal_tx_id(state: &mut VfsState) -> u64 {
    let tx_id = state.ext2_journal_next_tx_id;
    state.ext2_journal_next_tx_id = state.ext2_journal_next_tx_id.saturating_add(1);
    tx_id
}

fn populate_pending_from_running_tx(
    state: &mut VfsState,
    pending: &mut super::Ext2JournalPendingTx,
) -> Result<(), i64> {
    pending.op_id = state.ext2_journal_running_tx.op_id;
    pending.tx_id = state.ext2_journal_running_tx.tx_id;
    pending.segment_index = state.ext2_journal_running_tx.segment_index;
    pending.phase_entries = state.ext2_journal_running_tx.phase_entries;
    pending.last_phase_kind = state.ext2_journal_running_tx.last_phase_kind;
    pending.restart_count = state.ext2_journal_running_tx.restart_count;
    pending.reserved_credits = state.ext2_journal_running_tx.reserved_credits;
    pending.used_credits = state.ext2_journal_running_tx.used_credits;
    pending
        .metadata_blocks
        .try_reserve_exact(state.ext2_journal_running_tx.tracked_blocks.len())
        .map_err(|_| ENOSPC)?;
    pending
        .metadata_snapshots
        .try_reserve_exact(state.ext2_journal_running_tx.tracked_blocks.len())
        .map_err(|_| ENOSPC)?;
    pending
        .data_blocks
        .try_reserve_exact(state.ext2_journal_running_tx.data_blocks.len())
        .map_err(|_| ENOSPC)?;
    pending
        .data_snapshots
        .try_reserve_exact(state.ext2_journal_running_tx.data_blocks.len())
        .map_err(|_| ENOSPC)?;
    pending
        .revoke_blocks
        .try_reserve_exact(state.ext2_journal_running_tx.revoke_blocks.len())
        .map_err(|_| ENOSPC)?;

    let mut idx = 0usize;
    while idx < state.ext2_journal_running_tx.tracked_blocks.len() {
        let block = state.ext2_journal_running_tx.tracked_blocks[idx];
        if !running_tx_marks_data(state, block) {
            pending.metadata_blocks.push(block);
            snapshot_tx_block(state, &mut pending.metadata_snapshots, block)?;
        }
        idx += 1;
    }
    idx = 0;
    while idx < state.ext2_journal_running_tx.data_blocks.len() {
        let block = state.ext2_journal_running_tx.data_blocks[idx];
        pending.data_blocks.push(block);
        snapshot_tx_block(state, &mut pending.data_snapshots, block)?;
        idx += 1;
    }
    pending
        .revoke_blocks
        .extend_from_slice(state.ext2_journal_running_tx.revoke_blocks.as_slice());
    Ok(())
}

fn seal_running_tx_into_pending(state: &mut VfsState) -> Result<bool, i64> {
    if !state.ext2_journal_running_tx.active {
        return Ok(false);
    }
    let mut pending = super::Ext2JournalPendingTx::empty();
    populate_pending_from_running_tx(state, &mut pending)?;
    state.ext2_journal_pending_txs.try_reserve(1).map_err(|_| ENOSPC)?;
    state.ext2_journal_pending_txs.push(pending);
    Ok(true)
}

fn restart_running_tx_segment(state: &mut VfsState) -> Result<(), i64> {
    if !state.ext2_journal_running_tx.restartable {
        return Err(ENOSPC);
    }
    let growth = core::cmp::max(1usize, state.ext2_journal_running_tx.credit_growth_chunk);
    let op_id = state.ext2_journal_running_tx.op_id;
    let next_segment_index = state.ext2_journal_running_tx.segment_index.saturating_add(1);
    let scope_depth = state.ext2_journal_running_tx.scope_depth;
    let handle_count = state.ext2_journal_running_tx.handle_count;
    let restart_count = state.ext2_journal_running_tx.restart_count.saturating_add(1);
    seal_running_tx_into_pending(state)?;
    clear_running_tx_tracking(state);
    state.ext2_journal_running_tx.op_id = op_id;
    state.ext2_journal_running_tx.tx_id = next_journal_tx_id(state);
    state.ext2_journal_running_tx.segment_index = next_segment_index;
    state.ext2_journal_running_tx.phase_entries = 0;
    state.ext2_journal_running_tx.last_phase_kind = super::Ext2JournalPhaseKind::None;
    state.ext2_journal_running_tx.active = false;
    state.ext2_journal_running_tx.scope_depth = scope_depth;
    state.ext2_journal_running_tx.handle_count = handle_count;
    state.ext2_journal_running_tx.restartable = true;
    state.ext2_journal_running_tx.credit_growth_chunk = growth;
    state.ext2_journal_running_tx.restart_count = restart_count;
    state.ext2_journal_running_tx.reserved_credits = 0;
    state.ext2_journal_running_tx.used_credits = 0;
    reserve_running_tx_credits(state, growth)?;
    Ok(())
}

fn consume_running_tx_credit(state: &mut VfsState) -> Result<(), i64> {
    if state.ext2_journal_running_tx.reserved_credits == 0 {
        state.ext2_journal_running_tx.used_credits = state
            .ext2_journal_running_tx
            .used_credits
            .saturating_add(1);
        return Ok(());
    }
    if state.ext2_journal_running_tx.used_credits >= state.ext2_journal_running_tx.reserved_credits {
        restart_running_tx_segment(state)?;
    }
    state.ext2_journal_running_tx.used_credits += 1;
    Ok(())
}

pub(super) fn prepare_running_tx_credits(
    state: &mut VfsState,
    needed_credits: usize,
) -> Result<(), i64> {
    if needed_credits == 0
        || !state.ext2_journal.present
        || state.ext2_journal.replaying
        || state.ext2_journal.aborted
        || !state.ext2_journal_running_tx.restartable
        || state.ext2_journal_running_tx.scope_depth == 0
        || !state.ext2_journal_running_tx.active
        || state.ext2_journal_running_tx.reserved_credits == 0
    {
        return Ok(());
    }
    if state
        .ext2_journal_running_tx
        .used_credits
        .saturating_add(needed_credits)
        > state.ext2_journal_running_tx.reserved_credits
    {
        restart_running_tx_segment(state)?;
    }
    Ok(())
}

pub(super) fn begin_restartable_phase(
    state: &mut VfsState,
    phase_kind: super::Ext2JournalPhaseKind,
    needed_credits: usize,
) -> Result<(), i64> {
    prepare_running_tx_credits(state, needed_credits)?;
    if phase_kind == super::Ext2JournalPhaseKind::None
        || !state.ext2_journal.present
        || state.ext2_journal.replaying
        || state.ext2_journal.aborted
        || state.ext2_journal_running_tx.scope_depth == 0
    {
        return Ok(());
    }
    if state.ext2_journal_running_tx.last_phase_kind != phase_kind {
        state.ext2_journal_running_tx.phase_entries =
            state.ext2_journal_running_tx.phase_entries.saturating_add(1);
        state.ext2_journal_running_tx.last_phase_kind = phase_kind;
    }
    Ok(())
}

pub(super) fn with_restartable_phase<R>(
    state: &mut VfsState,
    phase_kind: super::Ext2JournalPhaseKind,
    needed_credits: usize,
    f: impl FnOnce(&mut VfsState) -> Result<R, i64>,
) -> Result<R, i64> {
    begin_restartable_phase(state, phase_kind, needed_credits)?;
    f(state)
}

pub(super) fn current_handle(state: &VfsState) -> Option<super::Ext2JournalHandle> {
    if !state.ext2_journal.present
        || state.ext2_journal.replaying
        || state.ext2_journal.aborted
        || state.ext2_journal_running_tx.scope_depth == 0
        || state.ext2_journal_running_tx.op_id == 0
    {
        return None;
    }
    Some(super::Ext2JournalHandle {
        op_id: state.ext2_journal_running_tx.op_id,
    })
}

fn handle_matches_running_tx(state: &VfsState, handle: super::Ext2JournalHandle) -> bool {
    state.ext2_journal_running_tx.scope_depth > 0
        && state.ext2_journal_running_tx.op_id != 0
        && state.ext2_journal_running_tx.op_id == handle.op_id
}

pub(super) fn with_handle_restartable_phase<R>(
    state: &mut VfsState,
    handle: Option<super::Ext2JournalHandle>,
    phase_kind: super::Ext2JournalPhaseKind,
    needed_credits: usize,
    f: impl FnOnce(&mut VfsState) -> Result<R, i64>,
) -> Result<R, i64> {
    if let Some(handle) = handle {
        if !handle_matches_running_tx(state, handle) {
            return Err(EINVAL);
        }
    }
    with_restartable_phase(state, phase_kind, needed_credits, f)
}

fn note_running_tx_block(state: &mut VfsState, block: u32, is_data: bool) -> Result<(), i64> {
    if !state.ext2_journal.present || state.ext2_journal.replaying || state.ext2_journal.aborted {
        return Ok(());
    }
    if state.ext2_journal_running_tx.scope_depth == 0 {
        return Ok(());
    }
    state.ext2_journal_running_tx.active = true;
    let tracked_new = state
        .ext2_journal_running_tx
        .tracked_blocks
        .binary_search(&block)
        .is_err();
    let data_new = is_data
        && state
            .ext2_journal_running_tx
            .data_blocks
            .binary_search(&block)
            .is_err();
    if tracked_new || data_new {
        consume_running_tx_credit(state)?;
    }
    insert_sorted_unique_u32(&mut state.ext2_journal_running_tx.tracked_blocks, block)?;
    if is_data {
        insert_sorted_unique_u32(&mut state.ext2_journal_running_tx.data_blocks, block)?;
    }
    // JBD "cancel revoke": a block freed earlier in this transaction is being
    // reused, so its new contents must be journaled/written rather than
    // revoked. (Revokes sealed into older transactions stay put; replay only
    // applies them to copies from transactions up to their own sequence.)
    let _ = remove_sorted_u32(&mut state.ext2_journal_running_tx.revoke_blocks, block);
    // Global revokes are emitted under a *later* sequence than this reuse may
    // commit with, where they would suppress the reuse's own journal copy.
    let _ = remove_sorted_u32(&mut state.ext2_journal_pending_revokes, block);
    Ok(())
}

fn note_running_tx_revoke(state: &mut VfsState, block: u32) -> Result<(), i64> {
    if !state.ext2_journal.present || state.ext2_journal.replaying || state.ext2_journal.aborted {
        return Ok(());
    }
    if state.ext2_journal_running_tx.scope_depth == 0 {
        return Ok(());
    }
    state.ext2_journal_running_tx.active = true;
    if state
        .ext2_journal_running_tx
        .revoke_blocks
        .binary_search(&block)
        .is_err()
    {
        consume_running_tx_credit(state)?;
    }
    insert_sorted_unique_u32(&mut state.ext2_journal_running_tx.revoke_blocks, block)?;

    // JBD "forget": a block dirtied earlier in this transaction (for example
    // an indirect table emptied during truncate) and now freed must not be
    // journaled. Its stale contents would otherwise replay over whatever
    // reuses the block, and the revoke would be dropped as superseded.
    let was_tracked = remove_sorted_u32(&mut state.ext2_journal_running_tx.tracked_blocks, block);
    let _ = remove_sorted_u32(&mut state.ext2_journal_running_tx.data_blocks, block);
    if was_tracked && pending_tx_owner_index(state, block).is_none() {
        if let Ok(cache_idx) = ext2_cache_find(state, block) {
            state.ext2_cached_blocks[cache_idx].dirty = false;
        }
        let _ = ext2_unmark_dirty_block(state, block);
    }
    Ok(())
}

pub(super) fn note_metadata_block(state: &mut VfsState, block: u32) -> Result<(), i64> {
    note_running_tx_block(state, block, false)
}

pub(super) fn note_data_block(state: &mut VfsState, block: u32) -> Result<(), i64> {
    note_running_tx_block(state, block, true)
}

pub(super) fn begin_mutation_scope_with_credits_options(
    state: &mut VfsState,
    credits: usize,
    restartable: bool,
    credit_growth_chunk: usize,
) -> Result<u64, i64> {
    if !state.ext2_journal.present || state.ext2_journal.replaying {
        return Ok(0);
    }
    ensure_journal_not_aborted(state)?;
    let opening_new_tx = state.ext2_journal_running_tx.handle_count == 0;
    if opening_new_tx {
        let tx_id = next_journal_tx_id(state);
        state.ext2_journal_running_tx.op_id = tx_id;
        state.ext2_journal_running_tx.tx_id = tx_id;
        state.ext2_journal_running_tx.segment_index = 0;
        state.ext2_journal_running_tx.phase_entries = 0;
        state.ext2_journal_running_tx.last_phase_kind = super::Ext2JournalPhaseKind::None;
        state.ext2_journal_running_tx.restartable = restartable;
        state.ext2_journal_running_tx.credit_growth_chunk = credit_growth_chunk;
        state.ext2_journal_running_tx.restart_count = 0;
        state.ext2_journal_running_tx.reserved_credits = 0;
        state.ext2_journal_running_tx.used_credits = 0;
    } else if restartable {
        state.ext2_journal_running_tx.restartable = true;
        state.ext2_journal_running_tx.credit_growth_chunk = core::cmp::max(
            state.ext2_journal_running_tx.credit_growth_chunk,
            credit_growth_chunk,
        );
    }
    if let Err(err) = reserve_running_tx_credits(state, credits) {
        if opening_new_tx {
            state.ext2_journal_running_tx.op_id = 0;
            state.ext2_journal_running_tx.tx_id = 0;
            state.ext2_journal_running_tx.segment_index = 0;
            state.ext2_journal_running_tx.phase_entries = 0;
            state.ext2_journal_running_tx.last_phase_kind = super::Ext2JournalPhaseKind::None;
            state.ext2_journal_running_tx.restartable = false;
            state.ext2_journal_running_tx.credit_growth_chunk = 0;
            state.ext2_journal_running_tx.restart_count = 0;
            state.ext2_journal_running_tx.reserved_credits = 0;
            state.ext2_journal_running_tx.used_credits = 0;
            state.ext2_journal_next_tx_id = state.ext2_journal_next_tx_id.saturating_sub(1);
        }
        return Err(err);
    }
    state.ext2_journal_running_tx.handle_count =
        state.ext2_journal_running_tx.handle_count.saturating_add(1);
    state.ext2_journal_running_tx.scope_depth =
        state.ext2_journal_running_tx.scope_depth.saturating_add(1);
    Ok(state.ext2_journal_running_tx.tx_id)
}

#[cfg(test)]
pub(super) fn begin_mutation_scope_with_credits(
    state: &mut VfsState,
    credits: usize,
) -> Result<u64, i64> {
    begin_mutation_scope_with_credits_options(state, credits, false, 0)
}

pub(super) fn end_mutation_scope(state: &mut VfsState) -> Result<(), i64> {
    if !state.ext2_journal.present || state.ext2_journal.replaying {
        return Ok(());
    }
    if state.ext2_journal_running_tx.scope_depth == 0 {
        return Err(EINVAL);
    }
    state.ext2_journal_running_tx.scope_depth -= 1;
    if state.ext2_journal_running_tx.handle_count > 0 {
        state.ext2_journal_running_tx.handle_count -= 1;
    }
    if state.ext2_journal_running_tx.scope_depth == 0 && state.ext2_journal_running_tx.active {
        seal_running_tx_into_pending(state)?;
        clear_running_tx_tracking(state);
    } else if state.ext2_journal_running_tx.scope_depth == 0 {
        clear_running_tx_tracking(state);
    }
    Ok(())
}

fn clear_running_tx_tracking(state: &mut VfsState) {
    state.ext2_journal_running_tx.op_id = 0;
    state.ext2_journal_running_tx.tx_id = 0;
    state.ext2_journal_running_tx.segment_index = 0;
    state.ext2_journal_running_tx.phase_entries = 0;
    state.ext2_journal_running_tx.last_phase_kind = super::Ext2JournalPhaseKind::None;
    state.ext2_journal_running_tx.active = false;
    state.ext2_journal_running_tx.scope_depth = 0;
    state.ext2_journal_running_tx.handle_count = 0;
    state.ext2_journal_running_tx.restartable = false;
    state.ext2_journal_running_tx.credit_growth_chunk = 0;
    state.ext2_journal_running_tx.restart_count = 0;
    state.ext2_journal_running_tx.reserved_credits = 0;
    state.ext2_journal_running_tx.used_credits = 0;
    state.ext2_journal_running_tx.tracked_blocks.clear();
    state.ext2_journal_running_tx.data_blocks.clear();
    state.ext2_journal_running_tx.revoke_blocks.clear();
}

fn snapshot_tx_block(
    state: &mut VfsState,
    snapshots: &mut Vec<super::Ext2JournalBlockSnapshot>,
    block: u32,
) -> Result<(), i64> {
    let cache_idx = ext2_cache_find(state, block).map_err(|_| EIO)?;
    let cache = &state.ext2_cached_blocks[cache_idx];
    let mut data = Vec::new();
    data.try_reserve_exact(cache.data.len()).map_err(|_| ENOSPC)?;
    data.extend_from_slice(cache.data.as_slice());
    snapshots.try_reserve(1).map_err(|_| ENOSPC)?;
    snapshots.push(super::Ext2JournalBlockSnapshot {
        home_block: block,
        escaped: data.len() >= 4 && data[..4] == JBD_MAGIC.to_be_bytes(),
        data,
    });
    Ok(())
}

fn pending_tx_snapshot_for_block(
    snapshots: &[super::Ext2JournalBlockSnapshot],
    block: u32,
) -> Option<&super::Ext2JournalBlockSnapshot> {
    let mut idx = 0usize;
    while idx < snapshots.len() {
        if snapshots[idx].home_block == block {
            return Some(&snapshots[idx]);
        }
        idx += 1;
    }
    None
}

fn pending_tx_contains_all_metadata_blocks(
    tx: &super::Ext2JournalPendingTx,
    blocks: &[u32],
) -> bool {
    let mut idx = 0usize;
    while idx < blocks.len() {
        if tx.metadata_blocks.binary_search(&blocks[idx]).is_err() {
            return false;
        }
        idx += 1;
    }
    true
}

fn pending_tx_owns_block(tx: &super::Ext2JournalPendingTx, block: u32) -> bool {
    tx.metadata_blocks.binary_search(&block).is_ok() || tx.data_blocks.binary_search(&block).is_ok()
}

fn pending_tx_owner_index(state: &VfsState, block: u32) -> Option<usize> {
    let mut idx = 0usize;
    while idx < state.ext2_journal_pending_txs.len() {
        if pending_tx_owns_block(&state.ext2_journal_pending_txs[idx], block) {
            return Some(idx);
        }
        idx += 1;
    }
    None
}

fn remove_pending_tx_snapshot(
    snapshots: &mut Vec<super::Ext2JournalBlockSnapshot>,
    block: u32,
) {
    let mut idx = 0usize;
    while idx < snapshots.len() {
        if snapshots[idx].home_block == block {
            snapshots.remove(idx);
            return;
        }
        idx += 1;
    }
}

fn block_has_newer_ordered_owner(state: &VfsState, block: u32) -> bool {
    let mut tx_idx = 1usize;
    while tx_idx < state.ext2_journal_pending_txs.len() {
        let tx = &state.ext2_journal_pending_txs[tx_idx];
        if tx.metadata_blocks.binary_search(&block).is_ok() || tx.data_blocks.binary_search(&block).is_ok()
        {
            return true;
        }
        tx_idx += 1;
    }
    state
        .ext2_journal_running_tx
        .tracked_blocks
        .binary_search(&block)
        .is_ok()
}

fn clear_cache_if_snapshot_matches(
    state: &mut VfsState,
    block: u32,
    snapshot: &[u8],
) -> Result<(), i64> {
    if block_has_newer_ordered_owner(state, block) {
        return Ok(());
    }
    if let Ok(cache_idx) = ext2_cache_find(state, block) {
        let cache = &mut state.ext2_cached_blocks[cache_idx];
        if cache.dirty && cache.data.as_slice() == snapshot {
            cache.dirty = false;
            cache.referenced = true;
            let _ = ext2_unmark_dirty_block(state, block);
        }
    }
    Ok(())
}

fn drop_running_tx_blocks(state: &mut VfsState, blocks: &[u32]) {
    let mut idx = 0usize;
    while idx < blocks.len() {
        let block = blocks[idx];
        let _ = remove_sorted_u32(&mut state.ext2_journal_running_tx.tracked_blocks, block);
        let _ = remove_sorted_u32(&mut state.ext2_journal_running_tx.data_blocks, block);
        idx += 1;
    }
    state.ext2_journal_running_tx.active = !state.ext2_journal_running_tx.tracked_blocks.is_empty();
}

/// Which in-memory transaction a home write or commit consumed blocks from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TxSource {
    /// The oldest sealed pending transaction (`ext2_journal_pending_txs[0]`).
    PendingHead,
    /// The running transaction / generic dirty-block path.
    Running,
}

/// Releases `blocks` from the oldest pending transaction only. Newer pending
/// transactions (and the running one) keep their own snapshots of the same
/// blocks: they changed them too, and those changes must commit atomically
/// with the rest of their own transaction. Stripping shared blocks (bitmaps,
/// group descriptors, superblock, directory blocks) from every pending
/// transaction used to let a newer transaction commit without its changes to
/// them, so a crash right after that commit replayed an inconsistent tree.
fn drop_pending_head_blocks(state: &mut VfsState, blocks: &[u32]) {
    if let Some(tx) = state.ext2_journal_pending_txs.first_mut() {
        for &block in blocks {
            let _ = remove_sorted_u32(&mut tx.metadata_blocks, block);
            let _ = remove_sorted_u32(&mut tx.data_blocks, block);
            remove_pending_tx_snapshot(&mut tx.metadata_snapshots, block);
            remove_pending_tx_snapshot(&mut tx.data_snapshots, block);
        }
    }
    retain_nonempty_pending_txs(state);
}

fn retain_nonempty_pending_txs(state: &mut VfsState) {
    state.ext2_journal_pending_txs.retain(|tx| {
        !tx.metadata_blocks.is_empty()
            || !tx.metadata_snapshots.is_empty()
            || !tx.data_blocks.is_empty()
            || !tx.data_snapshots.is_empty()
            || !tx.revoke_blocks.is_empty()
    });
}

fn drop_released_blocks(state: &mut VfsState, source: TxSource, blocks: &[u32]) {
    match source {
        TxSource::PendingHead => drop_pending_head_blocks(state, blocks),
        TxSource::Running => {
            drop_running_tx_blocks(state, blocks);
            // Defensive: callers only use this path for blocks no pending
            // transaction owns, so this is normally a no-op.
            drop_pending_tx_blocks(state, blocks);
        }
    }
}

/// JBD drops a freed buffer from older transactions' checkpoint lists once
/// the transaction carrying its revoke commits. Without that, checkpointing
/// an older transaction writes its stale journaled copy home over whatever
/// reused the block since (for example ordered file data written directly
/// home). Only called after the revoke's commit record is on disk: dropping
/// earlier would let a crash leave checkpointed metadata pointing at a block
/// that never received its journaled contents.
fn forget_revoked_checkpoint_updates(state: &mut VfsState, revoked: &[u32]) {
    if revoked.is_empty() {
        return;
    }
    for tx in state.ext2_journal_committed_txs.iter_mut() {
        tx.updates.retain(|update| !revoked.contains(&update.home_block));
    }
}

fn drop_pending_tx_blocks(state: &mut VfsState, blocks: &[u32]) {
    let mut tx_idx = 0usize;
    while tx_idx < state.ext2_journal_pending_txs.len() {
        let tx = &mut state.ext2_journal_pending_txs[tx_idx];
        let mut idx = 0usize;
        while idx < blocks.len() {
            let block = blocks[idx];
            let _ = remove_sorted_u32(&mut tx.metadata_blocks, block);
            let _ = remove_sorted_u32(&mut tx.data_blocks, block);
            remove_pending_tx_snapshot(&mut tx.metadata_snapshots, block);
            remove_pending_tx_snapshot(&mut tx.data_snapshots, block);
            idx += 1;
        }
        tx_idx += 1;
    }
    state.ext2_journal_pending_txs.retain(|tx| {
        !tx.metadata_blocks.is_empty()
            || !tx.metadata_snapshots.is_empty()
            || !tx.data_blocks.is_empty()
            || !tx.data_snapshots.is_empty()
            || !tx.revoke_blocks.is_empty()
    });
}

fn tx_uses_journal_block(
    superblock: &JournalSuper,
    tx: &Ext2JournalCommittedTx,
    jblock: u32,
) -> bool {
    let mut cursor = tx.descriptor_jblock;
    let mut remaining = tx.log_block_count;
    while remaining > 0 {
        if cursor == jblock {
            return true;
        }
        match journal_next_block(superblock, cursor) {
            Ok(next) => cursor = next,
            Err(_) => return false,
        }
        remaining -= 1;
    }
    false
}

fn pending_tx_conflicts(
    state: &VfsState,
    descriptor_jblock: u32,
    data_jblocks: &[u32],
    revoke_jblocks: &[u32],
    commit_jblock: u32,
) -> bool {
    if state.ext2_journal_committed_txs.is_empty() {
        return false;
    }
    let superblock = runtime_journal_super(state);
    let mut idx = 0usize;
    while idx < state.ext2_journal_committed_txs.len() {
        let tx = &state.ext2_journal_committed_txs[idx];
        if tx_uses_journal_block(&superblock, tx, descriptor_jblock)
            || tx_uses_journal_block(&superblock, tx, commit_jblock)
        {
            return true;
        }
        let mut j = 0usize;
        while j < data_jblocks.len() {
            if tx_uses_journal_block(&superblock, tx, data_jblocks[j]) {
                return true;
            }
            j += 1;
        }
        j = 0;
        while j < revoke_jblocks.len() {
            if tx_uses_journal_block(&superblock, tx, revoke_jblocks[j]) {
                return true;
            }
            j += 1;
        }
        idx += 1;
    }
    false
}

fn running_tx_marks_data(state: &VfsState, block: u32) -> bool {
    state
        .ext2_journal_running_tx
        .data_blocks
        .binary_search(&block)
        .is_ok()
}

fn collect_dirty_running_data_blocks(state: &mut VfsState) -> Result<Vec<u32>, i64> {
    let mut blocks = Vec::new();
    blocks
        .try_reserve_exact(state.ext2_journal_running_tx.data_blocks.len())
        .map_err(|_| ENOSPC)?;
    let mut idx = 0usize;
    while idx < state.ext2_journal_running_tx.data_blocks.len() {
        let block = state.ext2_journal_running_tx.data_blocks[idx];
        if block != state.ext2_journal.superblock_block {
            if let Ok(cache_idx) = ext2_cache_find(state, block) {
                if state.ext2_cached_blocks[cache_idx].dirty {
                    blocks.push(block);
                }
            }
        }
        idx += 1;
    }
    Ok(blocks)
}

fn collect_dirty_pending_data_blocks(state: &mut VfsState) -> Result<Vec<u32>, i64> {
    if state.ext2_journal_pending_txs.is_empty() {
        return Ok(Vec::new());
    }
    let tx = &state.ext2_journal_pending_txs[0];
    let mut blocks = Vec::new();
    blocks
        .try_reserve_exact(tx.data_blocks.len())
        .map_err(|_| ENOSPC)?;
    blocks.extend_from_slice(tx.data_blocks.as_slice());
    Ok(blocks)
}

fn collect_dirty_running_metadata_blocks(state: &mut VfsState) -> Result<Vec<u32>, i64> {
    let mut blocks = Vec::new();
    blocks
        .try_reserve_exact(state.ext2_journal_running_tx.tracked_blocks.len())
        .map_err(|_| ENOSPC)?;
    let mut idx = 0usize;
    while idx < state.ext2_journal_running_tx.tracked_blocks.len() {
        let block = state.ext2_journal_running_tx.tracked_blocks[idx];
        if block != state.ext2_journal.superblock_block && !running_tx_marks_data(state, block) {
            if let Ok(cache_idx) = ext2_cache_find(state, block) {
                if state.ext2_cached_blocks[cache_idx].dirty {
                    blocks.push(block);
                }
            }
        }
        idx += 1;
    }
    Ok(blocks)
}

fn collect_dirty_pending_metadata_blocks(state: &mut VfsState) -> Result<Vec<u32>, i64> {
    if state.ext2_journal_pending_txs.is_empty() {
        return Ok(Vec::new());
    }
    let tx = &state.ext2_journal_pending_txs[0];
    let mut blocks = Vec::new();
    blocks
        .try_reserve_exact(tx.metadata_blocks.len())
        .map_err(|_| ENOSPC)?;
    blocks.extend_from_slice(tx.metadata_blocks.as_slice());
    Ok(blocks)
}

fn collect_pending_revoke_blocks(state: &VfsState) -> Result<Vec<u32>, i64> {
    let mut blocks = Vec::new();
    if !state.ext2_journal_pending_txs.is_empty() {
        let tx = &state.ext2_journal_pending_txs[0];
        blocks
            .try_reserve_exact(tx.revoke_blocks.len())
            .map_err(|_| ENOSPC)?;
        blocks.extend_from_slice(tx.revoke_blocks.as_slice());
        return Ok(blocks);
    }
    blocks
        .try_reserve_exact(state.ext2_journal_pending_revokes.len())
        .map_err(|_| ENOSPC)?;
    blocks.extend_from_slice(state.ext2_journal_pending_revokes.as_slice());
    Ok(blocks)
}

fn flush_ordered_data_home(
    state: &mut VfsState,
    blocks: &[u32],
    source: TxSource,
) -> Result<usize, i64> {
    let mut idx = 0usize;
    while idx < blocks.len() {
        let block = blocks[idx];
        let mut snapshot: Option<Vec<u8>> = None;
        if source == TxSource::PendingHead && !state.ext2_journal_pending_txs.is_empty() {
            if let Some(entry) = pending_tx_snapshot_for_block(
                state.ext2_journal_pending_txs[0].data_snapshots.as_slice(),
                block,
            ) {
                snapshot = Some(entry.data.clone());
            }
        }
        let data = match snapshot {
            Some(data) => data,
            None => {
                let cache_idx = ext2_cache_find(state, block).map_err(|_| EIO)?;
                state.ext2_cached_blocks[cache_idx].data.clone()
            }
        };
        write_checkpoint_home_block(state, block, data.as_slice())?;
        clear_cache_if_snapshot_matches(state, block, data.as_slice())?;
        idx += 1;
    }
    drop_released_blocks(state, source, blocks);
    Ok(blocks.len())
}

pub(super) fn note_revoke(state: &mut VfsState, block: u32) -> Result<(), i64> {
    if !state.ext2_journal.present || state.ext2_journal.replaying {
        return Ok(());
    }
    ensure_journal_not_aborted(state)?;
    if state.ext2_journal.mode == Ext2JournalMode::Ordered && state.ext2_journal_running_tx.scope_depth > 0
    {
        return note_running_tx_revoke(state, block);
    }
    insert_revoked(&mut state.ext2_journal_pending_revokes, block)
}

fn upsert_pending(
    pending: &mut Vec<PendingUpdate>,
    home_block: u32,
    journal_block: u32,
    escaped: bool,
) -> Result<(), i64> {
    let mut idx = 0usize;
    while idx < pending.len() {
        if pending[idx].home_block == home_block {
            pending[idx].journal_block = journal_block;
            pending[idx].escaped = escaped;
            return Ok(());
        }
        idx += 1;
    }
    pending.try_reserve(1).map_err(|_| ENOSPC)?;
    pending.push(PendingUpdate {
        home_block,
        journal_block,
        escaped,
    });
    Ok(())
}

fn parse_descriptor_tags(block: &[u8]) -> Result<Vec<DescriptorTag>, i64> {
    let mut tags = Vec::new();
    let mut offset = 12usize;
    loop {
        if offset + 8 > block.len() {
            return Err(EIO);
        }
        let home_block = read_u32_be(block, offset)?;
        let flags = read_u32_be(block, offset + 4)?;
        offset += 8;

        if (flags & JBD_FLAG_SAME_UUID) == 0 {
            if offset + 16 > block.len() {
                return Err(EIO);
            }
            offset += 16;
        }

        tags.try_reserve(1).map_err(|_| ENOSPC)?;
        tags.push(DescriptorTag { home_block, flags });

        if (flags & JBD_FLAG_LAST_TAG) != 0 {
            break;
        }
    }
    Ok(tags)
}

fn parse_revoke_block(block: &[u8], revoked: &mut Vec<u32>) -> Result<(), i64> {
    let used_bytes = read_u32_be(block, 12)? as usize;
    if used_bytes < 16 || used_bytes > block.len() || ((used_bytes - 16) % 4) != 0 {
        return Err(EIO);
    }

    let mut offset = 16usize;
    while offset < used_bytes {
        insert_revoked(revoked, read_u32_be(block, offset)?)?;
        offset += 4;
    }
    Ok(())
}

fn insert_revoke_sequence(
    revoke_map: &mut Vec<(u32, u32)>,
    block: u32,
    sequence: u32,
) -> Result<(), i64> {
    match revoke_map.binary_search_by_key(&block, |(entry_block, _)| *entry_block) {
        Ok(idx) => {
            if tx_sequence_after(sequence, revoke_map[idx].1) {
                revoke_map[idx].1 = sequence;
            }
            Ok(())
        }
        Err(insert_at) => {
            revoke_map.try_reserve(1).map_err(|_| ENOSPC)?;
            revoke_map.insert(insert_at, (block, sequence));
            Ok(())
        }
    }
}

fn journal_block_to_fs(
    state: &mut VfsState,
    journal_inode: &Ext2Inode,
    journal_block: u32,
) -> Result<u32, i64> {
    let fs_block =
        ext2_data_block_for_logical_index_state(state, journal_inode, journal_block as usize)?;
    if fs_block == 0 {
        return Err(EIO);
    }
    Ok(fs_block)
}

fn read_journal_block(
    state: &mut VfsState,
    journal_inode: &Ext2Inode,
    journal_block: u32,
    out: &mut [u8],
) -> Result<u32, i64> {
    let fs_block = journal_block_to_fs(state, journal_inode, journal_block)?;
    ext2_backend_read_block(state, fs_block, out)?;
    Ok(fs_block)
}

fn apply_committed_transaction(
    state: &mut VfsState,
    journal_inode: &Ext2Inode,
    pending: &[PendingUpdate],
    sequence: u32,
    revoke_map: &[(u32, u32)],
    scratch: &mut [u8],
) -> Result<(), i64> {
    let magic = JBD_MAGIC.to_be_bytes();
    let mut idx = 0usize;
    while idx < pending.len() {
        let entry = pending[idx];
        let revoked = match revoke_map.binary_search_by_key(&entry.home_block, |(block, _)| *block)
        {
            Ok(map_idx) => tx_sequence_after_or_eq(revoke_map[map_idx].1, sequence),
            Err(_) => false,
        };
        if revoked {
            idx += 1;
            continue;
        }

        read_journal_block(state, journal_inode, entry.journal_block, scratch)?;
        if entry.escaped {
            if scratch.len() < 4 {
                return Err(EIO);
            }
            scratch[..4].copy_from_slice(&magic);
        }

        {
            let dst = ext2_block_slice_mut_state(state, entry.home_block)?;
            dst.copy_from_slice(scratch);
        }
        ext2_mark_dirty_block(state, entry.home_block)?;
        idx += 1;
    }
    Ok(())
}

fn flush_tx_blocks(state: &mut VfsState, blocks: &[u32]) -> Result<usize, i64> {
    if blocks.is_empty() {
        return Ok(0);
    }
    if !state.ext2_journal.present || state.ext2_journal.replaying {
        return Err(EINVAL);
    }
    ensure_journal_not_aborted(state)?;

    let result = (|| -> Result<usize, i64> {
        let block_size = state.ext2_meta.block_size;
        let sequence = state.ext2_journal.sequence;
        let revoke_capacity = revoke_entries_per_block(state);
        if revoke_capacity == 0 {
            return Err(EIO);
        }
        let using_pending_tx = state.ext2_journal.mode == Ext2JournalMode::Ordered
            && !state.ext2_journal_pending_txs.is_empty()
            && pending_tx_contains_all_metadata_blocks(&state.ext2_journal_pending_txs[0], blocks);

        let mut escaped = Vec::new();
        escaped
            .try_reserve_exact(blocks.len())
            .map_err(|_| ENOSPC)?;
        let mut idx = 0usize;
        while idx < blocks.len() {
            let block = blocks[idx];
            if using_pending_tx {
                let snapshot = pending_tx_snapshot_for_block(
                    state.ext2_journal_pending_txs[0].metadata_snapshots.as_slice(),
                    block,
                )
                .ok_or(EIO)?;
                escaped.push(snapshot.escaped);
                idx += 1;
                continue;
            }
            let cache_idx = match ext2_cache_find(state, block) {
                Ok(v) => v,
                Err(_) => return Err(EIO),
            };
            if !state.ext2_cached_blocks[cache_idx].dirty {
                return Err(EIO);
            }
            escaped.push(
                state.ext2_cached_blocks[cache_idx].data.len() >= 4
                    && state.ext2_cached_blocks[cache_idx].data[..4] == JBD_MAGIC.to_be_bytes(),
            );
            idx += 1;
        }
        let ordered_data_blocks = if state.ext2_journal.mode == Ext2JournalMode::Ordered {
            if using_pending_tx {
                collect_dirty_pending_data_blocks(state)?
            } else {
                collect_dirty_running_data_blocks(state)?
            }
        } else {
            Vec::new()
        };

        if blocks.len() > runtime_tx_data_capacity(state) {
            return Err(EIO);
        }
        let fixed_blocks = blocks.len() + descriptor_blocks_for(state, blocks.len()) + 1;

        let revoke_source = if using_pending_tx {
            collect_pending_revoke_blocks(state)?
        } else {
            let mut blocks = Vec::new();
            blocks
                .try_reserve_exact(state.ext2_journal_pending_revokes.len())
                .map_err(|_| ENOSPC)?;
            blocks.extend_from_slice(state.ext2_journal_pending_revokes.as_slice());
            blocks
        };
        let mut revoke_candidates = Vec::new();
        revoke_candidates
            .try_reserve_exact(revoke_source.len())
            .map_err(|_| ENOSPC)?;
        idx = 0;
        while idx < revoke_source.len() {
            let revoke = revoke_source[idx];
            if !blocks.contains(&revoke) {
                revoke_candidates.push(revoke);
            }
            idx += 1;
        }
        let max_revoke_blocks = journal_window(state).saturating_sub(fixed_blocks);
        let max_revoke_entries = max_revoke_blocks.saturating_mul(revoke_capacity);
        let emitted_revoke_count = core::cmp::min(revoke_candidates.len(), max_revoke_entries);
        let revoke_block_count = if emitted_revoke_count == 0 {
            0
        } else {
            emitted_revoke_count
                .saturating_add(revoke_capacity.saturating_sub(1))
                .saturating_div(revoke_capacity)
        };

        let layout = loop {
            match runtime_tx_layout(state, blocks.len(), revoke_block_count) {
                Ok(layout) => break layout,
                Err(ENOSPC) if !state.ext2_journal_committed_txs.is_empty() => {
                    checkpoint_committed(state, 1)?;
                }
                Err(err) => return Err(err),
            }
        };
        let log_block_count = layout.log_block_count();
        let TxLayout {
            descriptor_jblocks,
            data_jblocks,
            revoke_jblocks,
            commit_jblock,
        } = layout;
        let descriptor_jblock = descriptor_jblocks[0];

        let descriptor_fs_block = journal_block_to_fs_from_state(state, descriptor_jblock)?;
        let commit_fs_block = journal_block_to_fs_from_state(state, commit_jblock)?;
        let first_data_jblock = data_jblocks[0];
        let first_data_fs_block = journal_block_to_fs_from_state(state, first_data_jblock)?;

        state.ext2_journal.tx_descriptor_jblock = descriptor_jblock;
        state.ext2_journal.tx_data_jblock = first_data_jblock;
        state.ext2_journal.tx_commit_jblock = commit_jblock;
        state.ext2_journal.tx_descriptor_fs_block = descriptor_fs_block;
        state.ext2_journal.tx_data_fs_block = first_data_fs_block;
        state.ext2_journal.tx_commit_fs_block = commit_fs_block;

        let tags_per_descriptor = descriptor_tag_capacity(state);
        let mut descriptors: Vec<Vec<u8>> = Vec::new();
        descriptors
            .try_reserve_exact(descriptor_jblocks.len())
            .map_err(|_| ENOSPC)?;
        for (chunk_idx, chunk) in blocks.chunks(tags_per_descriptor).enumerate() {
            let mut descriptor = Vec::new();
            descriptor
                .try_reserve_exact(block_size)
                .map_err(|_| ENOSPC)?;
            descriptor.resize(block_size, 0);
            write_u32_be(&mut descriptor, 0, JBD_MAGIC)?;
            write_u32_be(&mut descriptor, 4, JBD_DESCRIPTOR_BLOCK)?;
            write_u32_be(&mut descriptor, 8, sequence)?;
            let mut desc_off = 12usize;
            for (tag_idx, &home_block) in chunk.iter().enumerate() {
                let block_idx = chunk_idx * tags_per_descriptor + tag_idx;
                write_u32_be(&mut descriptor, desc_off, home_block)?;
                let mut flags = JBD_FLAG_SAME_UUID;
                if escaped[block_idx] {
                    flags |= JBD_FLAG_ESCAPE;
                }
                if tag_idx + 1 == chunk.len() {
                    flags |= JBD_FLAG_LAST_TAG;
                }
                write_u32_be(&mut descriptor, desc_off + 4, flags)?;
                desc_off = desc_off.saturating_add(8);
            }
            descriptors.push(descriptor);
        }
        if descriptors.len() != descriptor_jblocks.len() {
            return Err(EIO);
        }

        let checksum_enabled = state.ext2_journal.checksum;
        // JBD2 seeds the per-transaction checksum with ~0 and folds in every
        // descriptor and journaled block in log order (not revoke blocks).
        let mut tx_crc = !0u32;
        let mut commit = Vec::new();
        commit.try_reserve_exact(block_size).map_err(|_| ENOSPC)?;
        commit.resize(block_size, 0);
        write_u32_be(&mut commit, 0, JBD_MAGIC)?;
        write_u32_be(&mut commit, 4, JBD_COMMIT_BLOCK)?;
        write_u32_be(&mut commit, 8, sequence)?;

        let start_before_commit = if state.ext2_journal_committed_txs.is_empty() {
            descriptor_jblock
        } else {
            state.ext2_journal_committed_txs[0].descriptor_jblock
        };
        // s_sequence must name the transaction at s_start: the oldest live one
        // if any, otherwise this one (journal_super_sequence covers both).
        let super_sequence = journal_super_sequence(state);
        write_journal_super_fields(state, super_sequence, start_before_commit)?;
        for (desc_idx, descriptor) in descriptors.iter().enumerate() {
            let fs_block = journal_block_to_fs_from_state(state, descriptor_jblocks[desc_idx])?;
            write_backend_and_refresh_cache(state, fs_block, descriptor.as_slice())?;
        }

        idx = 0;
        while idx < blocks.len() {
            let mut journal_data = Vec::new();
            journal_data
                .try_reserve_exact(block_size)
                .map_err(|_| ENOSPC)?;
            journal_data.resize(block_size, 0);
            let block = blocks[idx];
            if using_pending_tx {
                let snapshot = pending_tx_snapshot_for_block(
                    state.ext2_journal_pending_txs[0].metadata_snapshots.as_slice(),
                    block,
                )
                .ok_or(EIO)?;
                journal_data.copy_from_slice(snapshot.data.as_slice());
            } else {
                let cache_idx = ext2_cache_find(state, block).map_err(|_| EIO)?;
                journal_data.copy_from_slice(state.ext2_cached_blocks[cache_idx].data.as_slice());
            }
            if escaped[idx] {
                journal_data[..4].fill(0);
            }
            if checksum_enabled {
                if idx % tags_per_descriptor == 0 {
                    tx_crc = crc32_be(tx_crc, descriptors[idx / tags_per_descriptor].as_slice());
                }
                tx_crc = crc32_be(tx_crc, journal_data.as_slice());
            }
            let jblock = data_jblocks[idx];
            let fs_block = journal_block_to_fs_from_state(state, jblock)?;
            write_backend_and_refresh_cache(state, fs_block, journal_data.as_slice())?;
            idx += 1;
        }
        if checksum_enabled {
            // struct commit_header: h_chksum_type @12, h_chksum_size @13,
            // h_chksum[0] @16 (big endian).
            commit[12] = JBD2_CRC32_CHKSUM;
            commit[13] = JBD2_CRC32_CHKSUM_SIZE;
            write_u32_be(&mut commit, 16, tx_crc)?;
        }

        idx = 0;
        let mut revoke_written = 0usize;
        while idx < revoke_jblocks.len() {
            let remaining = emitted_revoke_count.saturating_sub(revoke_written);
            let chunk = core::cmp::min(remaining, revoke_capacity);
            let mut revoke_block = Vec::new();
            revoke_block
                .try_reserve_exact(block_size)
                .map_err(|_| ENOSPC)?;
            revoke_block.resize(block_size, 0);
            write_u32_be(&mut revoke_block, 0, JBD_MAGIC)?;
            write_u32_be(&mut revoke_block, 4, JBD_REVOKE_BLOCK)?;
            write_u32_be(&mut revoke_block, 8, sequence)?;
            let used_bytes = 16usize.saturating_add(chunk.saturating_mul(4));
            write_u32_be(&mut revoke_block, 12, used_bytes as u32)?;
            let mut revoke_off = 16usize;
            let mut chunk_idx = 0usize;
            while chunk_idx < chunk {
                write_u32_be(
                    &mut revoke_block,
                    revoke_off,
                    revoke_candidates[revoke_written + chunk_idx],
                )?;
                revoke_off = revoke_off.saturating_add(4);
                chunk_idx += 1;
            }
            let revoke_fs_block = journal_block_to_fs_from_state(state, revoke_jblocks[idx])?;
            write_backend_and_refresh_cache(state, revoke_fs_block, revoke_block.as_slice())?;
            revoke_written = revoke_written.saturating_add(chunk);
            idx += 1;
        }

        if !ordered_data_blocks.is_empty() {
            let source = if using_pending_tx {
                TxSource::PendingHead
            } else {
                TxSource::Running
            };
            flush_ordered_data_home(state, ordered_data_blocks.as_slice(), source)?;
        }

        // The commit record makes this transaction replayable, so the
        // superblock must already say "needs recovery" when it lands.
        set_fs_recover_flag(state, true)?;
        // Barrier 1: descriptors, journaled copies, revoke blocks, ordered
        // file data, and the recovery marker are durable before the commit
        // record can be; otherwise a reordering write cache could persist a
        // commit whose payload never reached the disk.
        backend_barrier(state)?;
        write_backend_and_refresh_cache(state, commit_fs_block, commit.as_slice())?;
        // Barrier 2 (deferred): the commit must be durable before any
        // checkpoint write of this transaction's blocks and before a
        // flush/sync returns; settle_commit_barrier enforces both.
        state.ext2_journal.commit_barrier_pending = true;

        idx = 0;
        while idx < emitted_revoke_count {
            if using_pending_tx {
                let _ = remove_sorted_u32(
                    &mut state.ext2_journal_pending_txs[0].revoke_blocks,
                    revoke_candidates[idx],
                );
            } else {
                let _ = remove_sorted_u32(
                    &mut state.ext2_journal_pending_revokes,
                    revoke_candidates[idx],
                );
            }
            idx += 1;
        }
        idx = 0;
        while idx < blocks.len() {
            let _ = remove_sorted_u32(&mut state.ext2_journal_pending_revokes, blocks[idx]);
            // Revokes for blocks journaled in this same commit were skipped as
            // superseded above; retire them too, or the pending transaction
            // keeps a revoke-only remainder that can never be flushed.
            if using_pending_tx {
                let _ = remove_sorted_u32(
                    &mut state.ext2_journal_pending_txs[0].revoke_blocks,
                    blocks[idx],
                );
            }
            idx += 1;
        }
        if using_pending_tx {
            retain_nonempty_pending_txs(state);
        }
        // The revokes are durable now: older committed transactions must not
        // checkpoint their stale copies of those blocks.
        forget_revoked_checkpoint_updates(state, &revoke_candidates[..emitted_revoke_count]);

        let mut committed_updates = Vec::new();
        committed_updates
            .try_reserve_exact(blocks.len())
            .map_err(|_| ENOSPC)?;
        idx = 0;
        while idx < blocks.len() {
            let block = blocks[idx];
            let snapshot_data = if using_pending_tx {
                pending_tx_snapshot_for_block(
                    state.ext2_journal_pending_txs[0].metadata_snapshots.as_slice(),
                    block,
                )
                .map(|entry| entry.data.clone())
            } else {
                None
            };
            committed_updates.push(Ext2JournalCommittedUpdate {
                home_block: block,
                journal_block: data_jblocks[idx],
                escaped: escaped[idx],
            });
            if let Some(snapshot_data) = snapshot_data {
                clear_cache_if_snapshot_matches(state, block, snapshot_data.as_slice())?;
            } else {
                if let Ok(cache_idx) = ext2_cache_find(state, block) {
                    state.ext2_cached_blocks[cache_idx].dirty = false;
                    state.ext2_cached_blocks[cache_idx].referenced = true;
                }
                let _ = ext2_unmark_dirty_block(state, block);
            }
            idx += 1;
        }
        drop_released_blocks(
            state,
            if using_pending_tx {
                TxSource::PendingHead
            } else {
                TxSource::Running
            },
            blocks,
        );

        state
            .ext2_journal_committed_txs
            .try_reserve(1)
            .map_err(|_| ENOSPC)?;
        state
            .ext2_journal_committed_txs
            .push(Ext2JournalCommittedTx {
                sequence,
                descriptor_jblock,
                log_block_count: log_block_count as u32,
                updates: committed_updates,
            });

        let next_start = if state.ext2_journal_committed_txs.is_empty() {
            0
        } else {
            state.ext2_journal_committed_txs[0].descriptor_jblock
        };
        state.ext2_journal.sequence = sequence.wrapping_add(1);
        let super_sequence = journal_super_sequence(state);
        write_journal_super_fields(state, super_sequence, next_start)?;
        state.ext2_journal.start = next_start;
        let journal_super = runtime_journal_super(state);
        state.ext2_journal.runtime_head_jblock = journal_next_block(&journal_super, commit_jblock)?;
        Ok(blocks.len())
    })();

    abort_on_runtime_io_error(state, result)
}

fn checkpoint_committed(state: &mut VfsState, max_txs: usize) -> Result<usize, i64> {
    if max_txs == 0 || state.ext2_journal_committed_txs.is_empty() {
        return Ok(0);
    }
    ensure_journal_not_aborted(state)?;

    let result = (|| -> Result<usize, i64> {
        settle_commit_barrier(state)?;
        let block_size = state.ext2_meta.block_size;
        let mut scratch = Vec::new();
        scratch.try_reserve_exact(block_size).map_err(|_| ENOSPC)?;
        scratch.resize(block_size, 0);
        let magic = JBD_MAGIC.to_be_bytes();

        let mut completed_txs = 0usize;
        let mut checkpointed_blocks = 0usize;
        while !state.ext2_journal_committed_txs.is_empty() && completed_txs < max_txs {
            let _sequence = state.ext2_journal_committed_txs[0].sequence;
            let updates_len = state.ext2_journal_committed_txs[0].updates.len();
            let mut idx = 0usize;
            while idx < updates_len {
                let update = state.ext2_journal_committed_txs[0].updates[idx];
                let journal_fs_block = journal_block_to_fs_from_state(state, update.journal_block)?;
                ext2_backend_read_block(state, journal_fs_block, &mut scratch)?;
                if update.escaped {
                    if scratch.len() < 4 {
                        return Err(EIO);
                    }
                    scratch[..4].copy_from_slice(&magic);
                }
                write_checkpoint_home_block(state, update.home_block, scratch.as_slice())?;
                checkpointed_blocks = checkpointed_blocks.saturating_add(1);
                idx += 1;
            }
            state.ext2_journal_committed_txs.remove(0);
            completed_txs = completed_txs.saturating_add(1);
        }

        let start = if state.ext2_journal_committed_txs.is_empty() {
            0
        } else {
            state.ext2_journal_committed_txs[0].descriptor_jblock
        };
        // Barrier 3: checkpointed home blocks are durable before the log tail
        // moves past the transactions that still describe them.
        backend_barrier(state)?;
        let super_sequence = journal_super_sequence(state);
        write_journal_super_fields(state, super_sequence, start)?;
        state.ext2_journal.start = start;
        if start == 0 {
            // Journal is empty on disk: nothing left to replay. The empty tail
            // must be durable before needs_recovery is cleared.
            backend_barrier(state)?;
            set_fs_recover_flag(state, false)?;
        }
        backend_barrier(state)?;
        Ok(checkpointed_blocks)
    })();

    abort_on_runtime_io_error(state, result)
}

pub(super) fn checkpoint_all_committed(state: &mut VfsState) -> Result<usize, i64> {
    checkpoint_committed(state, usize::MAX)
}

pub(super) fn flush_oldest_ordered_pending_tx(
    state: &mut VfsState,
    max_blocks: usize,
) -> Result<usize, i64> {
    if max_blocks == 0 {
        return Ok(0);
    }
    if state.ext2_journal.mode != Ext2JournalMode::Ordered || state.ext2_journal_pending_txs.is_empty()
    {
        return Ok(0);
    }
    let tx_capacity = runtime_tx_data_capacity(state);
    if tx_capacity == 0 {
        return Err(EIO);
    }
    let batch_cap = core::cmp::min(max_blocks, tx_capacity);
    let carried_revokes = carry_forward_revoke_only_pending_txs(state)?;
    if state.ext2_journal_pending_txs.is_empty() {
        // Report progress so callers waiting on the queue to drain move on.
        return Ok(if carried_revokes { 1 } else { 0 });
    }
    let metadata_blocks = collect_dirty_pending_metadata_blocks(state)?;
    if !metadata_blocks.is_empty() {
        // Commit the whole sealed transaction in one journal transaction:
        // `max_blocks` is only a soft writeback budget, and honoring it here
        // used to tear one filesystem operation across several commits, so a
        // crash between them replayed half of it. Splitting only happens when
        // the transaction cannot fit in a single journal transaction at all.
        let take = core::cmp::min(tx_capacity, metadata_blocks.len());
        return flush_tx_blocks(state, &metadata_blocks[..take]);
    }

    let ordered_data_blocks = collect_dirty_pending_data_blocks(state)?;
    if !ordered_data_blocks.is_empty() {
        let count = core::cmp::min(batch_cap, ordered_data_blocks.len());
        return flush_ordered_data_home(state, &ordered_data_blocks[..count], TxSource::PendingHead);
    }
    Ok(if carried_revokes { 1 } else { 0 })
}

/// A sealed transaction can lose all of its metadata/data blocks before it
/// commits: when an older transaction commits a shared block (bitmap, group
/// descriptor, superblock), `drop_pending_tx_blocks` removes it from every
/// pending transaction. What remains is a revoke-only transaction that no
/// flush path can emit, which used to wedge the queue (EIO). Fold those
/// revokes into the next pending transaction (or the global revoke set when
/// none is left) so they are still committed, just under a later sequence.
/// A revoke for a block the receiving commit journals is dropped as
/// superseded, matching the same-commit rule in `flush_tx_blocks`.
fn carry_forward_revoke_only_pending_txs(state: &mut VfsState) -> Result<bool, i64> {
    let mut carried = false;
    while let Some(tx) = state.ext2_journal_pending_txs.first() {
        let revoke_only = tx.metadata_blocks.is_empty()
            && tx.metadata_snapshots.is_empty()
            && tx.data_blocks.is_empty()
            && tx.data_snapshots.is_empty();
        if !revoke_only {
            break;
        }
        let tx = state.ext2_journal_pending_txs.remove(0);
        if let Some(next) = state.ext2_journal_pending_txs.first_mut() {
            for &block in tx.revoke_blocks.iter() {
                if !pending_tx_owns_block(next, block) {
                    insert_revoked(&mut next.revoke_blocks, block)?;
                }
            }
        } else {
            for &block in tx.revoke_blocks.iter() {
                insert_revoked(&mut state.ext2_journal_pending_revokes, block)?;
            }
        }
        carried = true;
    }
    Ok(carried)
}

pub(super) fn flush_dirty_block(state: &mut VfsState, block: u32) -> Result<(), i64> {
    if state.ext2_journal.mode == Ext2JournalMode::Ordered {
        let tx_capacity = runtime_tx_data_capacity(state);
        if tx_capacity == 0 {
            return Err(EIO);
        }
        while pending_tx_owner_index(state, block).is_some() {
            let flushed = flush_oldest_ordered_pending_tx(state, tx_capacity)?;
            if flushed == 0 {
                return Err(EIO);
            }
        }
    }
    let cache_idx = match ext2_cache_find(state, block) {
        Ok(idx) => idx,
        Err(_) => {
            ext2_unmark_dirty_block(state, block);
            return Ok(());
        }
    };
    if !state.ext2_cached_blocks[cache_idx].dirty {
        ext2_unmark_dirty_block(state, block);
        return Ok(());
    }
    let flushed = flush_tx_blocks(state, &[block])?;
    if flushed != 1 {
        return Err(EIO);
    }
    Ok(())
}

pub(super) fn flush_dirty_batch(state: &mut VfsState, max_blocks: usize) -> Result<usize, i64> {
    if max_blocks == 0 || !state.ext2_journal.present {
        return Ok(0);
    }
    let tx_capacity = runtime_tx_data_capacity(state);
    if tx_capacity == 0 {
        return Err(EIO);
    }
    let batch_cap = core::cmp::min(max_blocks, tx_capacity);
    let mut picked = Vec::new();
    picked.try_reserve_exact(batch_cap).map_err(|_| ENOSPC)?;

    if state.ext2_journal.mode == Ext2JournalMode::Ordered && !state.ext2_journal_pending_txs.is_empty() {
        return flush_oldest_ordered_pending_tx(state, batch_cap);
    } else if state.ext2_journal.mode == Ext2JournalMode::Ordered
        && state.ext2_journal_running_tx.active
    {
        let metadata_blocks = collect_dirty_running_metadata_blocks(state)?;
        let take = core::cmp::min(batch_cap, metadata_blocks.len());
        picked.extend_from_slice(&metadata_blocks[..take]);
    }

    if picked.is_empty() {
        let mut idx = 0usize;
        while idx < state.ext2_dirty_blocks.len() && picked.len() < batch_cap {
            let block = state.ext2_dirty_blocks[idx];
            if block == state.ext2_journal.superblock_block {
                idx += 1;
                continue;
            }
            if state.ext2_journal.mode == Ext2JournalMode::Ordered && running_tx_marks_data(state, block) {
                idx += 1;
                continue;
            }
            if let Ok(cache_idx) = ext2_cache_find(state, block) {
                if state.ext2_cached_blocks[cache_idx].dirty {
                    picked.push(block);
                }
            }
            idx += 1;
        }
    }
    if picked.is_empty() {
        if state.ext2_journal.mode == Ext2JournalMode::Ordered {
            let (ordered_data_blocks, source) = if !state.ext2_journal_pending_txs.is_empty() {
                (collect_dirty_pending_data_blocks(state)?, TxSource::PendingHead)
            } else {
                (collect_dirty_running_data_blocks(state)?, TxSource::Running)
            };
            if !ordered_data_blocks.is_empty() {
                let count = core::cmp::min(batch_cap, ordered_data_blocks.len());
                return flush_ordered_data_home(state, &ordered_data_blocks[..count], source);
            }
        }
        return Ok(0);
    }
    flush_tx_blocks(state, picked.as_slice())
}

fn replay_journal(
    state: &mut VfsState,
    journal_inode: &Ext2Inode,
    superblock: &JournalSuper,
) -> Result<u32, i64> {
    if superblock.start == 0 {
        return Ok(superblock.sequence);
    }

    let block_size = state.ext2_meta.block_size;
    let mut block = Vec::new();
    block.try_reserve_exact(block_size).map_err(|_| ENOSPC)?;
    block.resize(block_size, 0);

    let mut next_sequence = superblock.sequence;
    let mut cursor = superblock.start;
    let mut tx_budget = superblock.max_len as usize;
    struct ReplayTx {
        sequence: u32,
        updates: Vec<PendingUpdate>,
    }
    let mut replay_txs = Vec::<ReplayTx>::new();
    let mut revoke_map = Vec::<(u32, u32)>::new();
    let verify_checksums = (superblock.feature_compat & JBD2_FEATURE_COMPAT_CHECKSUM) != 0;
    let mut checksum_seen = false;
    let mut data_block = Vec::new();
    if verify_checksums {
        data_block.try_reserve_exact(block_size).map_err(|_| ENOSPC)?;
        data_block.resize(block_size, 0);
    }

    while tx_budget > 0 {
        read_journal_block(state, journal_inode, cursor, &mut block)?;
        let header = match parse_header(&block)? {
            Some(h) => h,
            None => break,
        };
        if header.block_type != JBD_DESCRIPTOR_BLOCK || header.sequence != next_sequence {
            break;
        }

        let mut pending = Vec::new();
        let mut position = cursor;
        let mut scan_budget = superblock.max_len as usize;
        let mut committed = false;
        let mut tx_crc = !0u32;

        while scan_budget > 0 {
            read_journal_block(state, journal_inode, position, &mut block)?;
            let header = match parse_header(&block)? {
                Some(h) => h,
                None => break,
            };
            if header.sequence != next_sequence {
                break;
            }

            match header.block_type {
                JBD_DESCRIPTOR_BLOCK => {
                    let tags = parse_descriptor_tags(&block)?;
                    if tags.is_empty() {
                        return Err(EIO);
                    }
                    if verify_checksums {
                        tx_crc = crc32_be(tx_crc, &block);
                    }
                    let mut tag_idx = 0usize;
                    while tag_idx < tags.len() {
                        let tag = tags[tag_idx];
                        position = journal_next_block(superblock, position)?;
                        scan_budget = scan_budget.saturating_sub(1);
                        if verify_checksums {
                            read_journal_block(state, journal_inode, position, &mut data_block)?;
                            tx_crc = crc32_be(tx_crc, &data_block);
                        }
                        if (tag.flags & JBD_FLAG_DELETED) == 0 {
                            upsert_pending(
                                &mut pending,
                                tag.home_block,
                                position,
                                (tag.flags & JBD_FLAG_ESCAPE) != 0,
                            )?;
                        }
                        tag_idx += 1;
                    }
                    position = journal_next_block(superblock, position)?;
                    scan_budget = scan_budget.saturating_sub(1);
                }
                JBD_REVOKE_BLOCK => {
                    let mut revoked = Vec::new();
                    parse_revoke_block(&block, &mut revoked)?;
                    let mut revoked_idx = 0usize;
                    while revoked_idx < revoked.len() {
                        insert_revoke_sequence(
                            &mut revoke_map,
                            revoked[revoked_idx],
                            next_sequence,
                        )?;
                        revoked_idx += 1;
                    }
                    position = journal_next_block(superblock, position)?;
                    scan_budget = scan_budget.saturating_sub(1);
                }
                JBD_COMMIT_BLOCK => {
                    if verify_checksums {
                        // Same acceptance rule as e2fsck/jbd2 recovery: a
                        // matching CRC32 commits the transaction; an all-zero
                        // checksum field is tolerated only before the first
                        // checksummed commit (written before the feature was
                        // enabled); anything else ends the log here.
                        let chksum_type = block[12];
                        let chksum_size = block[13];
                        let found = read_u32_be(&block, 16)?;
                        if chksum_type == JBD2_CRC32_CHKSUM
                            && chksum_size == JBD2_CRC32_CHKSUM_SIZE
                            && found == tx_crc
                        {
                            checksum_seen = true;
                        } else if !(chksum_type == 0
                            && chksum_size == 0
                            && found == 0
                            && !checksum_seen)
                        {
                            break;
                        }
                    }
                    committed = true;
                    break;
                }
                _ => break,
            }
        }

        if !committed {
            break;
        }

        replay_txs.try_reserve(1).map_err(|_| ENOSPC)?;
        replay_txs.push(ReplayTx {
            sequence: next_sequence,
            updates: pending,
        });
        next_sequence = next_sequence.wrapping_add(1);
        cursor = journal_next_block(superblock, position)?;
        tx_budget -= 1;
        if cursor == superblock.start {
            break;
        }
    }

    let mut tx_idx = 0usize;
    while tx_idx < replay_txs.len() {
        apply_committed_transaction(
            state,
            journal_inode,
            replay_txs[tx_idx].updates.as_slice(),
            replay_txs[tx_idx].sequence,
            revoke_map.as_slice(),
            &mut block,
        )?;
        tx_idx += 1;
    }

    Ok(next_sequence)
}

fn flush_all_dirty_blocks(state: &mut VfsState) -> Result<(), i64> {
    while !state.ext2_dirty_blocks.is_empty() {
        let block = state.ext2_dirty_blocks[0];
        ext2_cache_flush_block(state, block)?;
    }
    Ok(())
}

/// Finishes mount-time recovery in crash-safe order. Replay only dirtied the
/// home blocks in the cache, so they must reach stable storage *before* the
/// log is emptied; a crash anywhere in here simply replays again.
fn clear_recovery_state(state: &mut VfsState, next_sequence: u32) -> Result<(), i64> {
    let incompat_off = EXT2_SUPERBLOCK_OFFSET + 96;
    // A replayed superblock image may predate the recovery marker; keep it set
    // in whatever superblock gets written while the log still holds data.
    let incompat = ext2_read_u32_state(state, incompat_off)?;
    if (incompat & EXT2_FEATURE_INCOMPAT_RECOVER) == 0 {
        ext2_write_u32_state(state, incompat_off, incompat | EXT2_FEATURE_INCOMPAT_RECOVER)?;
    }
    flush_all_dirty_blocks(state)?;
    backend_barrier(state)?;

    write_journal_super_fields(state, next_sequence, 0)?;
    backend_barrier(state)?;

    state.ext2_journal.recover_marked = false;
    let incompat = ext2_read_u32_state(state, incompat_off)? & !EXT2_FEATURE_INCOMPAT_RECOVER;
    ext2_write_u32_state(state, incompat_off, incompat)?;
    state.ext2_meta.feature_incompat &= !EXT2_FEATURE_INCOMPAT_RECOVER;
    flush_all_dirty_blocks(state)?;
    backend_barrier(state)
}

pub(super) fn prepare_mount(state: &mut VfsState) -> Result<(), i64> {
    state.ext2_journal = Ext2JournalState::empty();
    clear_running_tx_tracking(state);
    state.ext2_journal_pending_txs.clear();
    state.ext2_journal_committed_txs.clear();
    state.ext2_journal_pending_revokes.clear();
    state.ext2_journal_next_tx_id = 1;

    if (state.ext2_meta.feature_compat & EXT2_FEATURE_COMPAT_HAS_JOURNAL) == 0 {
        return Ok(());
    }

    if (state.ext2_meta.feature_incompat & EXT2_FEATURE_INCOMPAT_JOURNAL_DEV) != 0
        || state.ext2_meta.journal_dev != 0
    {
        return Err(EINVAL);
    }
    if state.ext2_meta.journal_inode == 0 {
        return Err(EINVAL);
    }

    let journal_inode_num = state.ext2_meta.journal_inode;
    let journal_inode = ext2_read_inode_from_state(state, journal_inode_num)?;
    let journal_sb_fs_block = journal_block_to_fs(state, &journal_inode, 0)?;

    let block_size = state.ext2_meta.block_size;
    let mut block = Vec::new();
    block.try_reserve_exact(block_size).map_err(|_| ENOSPC)?;
    block.resize(block_size, 0);
    ext2_backend_read_block(state, journal_sb_fs_block, &mut block)?;
    let superblock = parse_superblock(&block, block_size)?;

    let descriptor_jblock = superblock.first;
    let data_jblock = journal_next_block(&superblock, descriptor_jblock)?;
    let commit_jblock = journal_next_block(&superblock, data_jblock)?;
    let descriptor_fs_block = journal_block_to_fs(state, &journal_inode, descriptor_jblock)?;
    let data_fs_block = journal_block_to_fs(state, &journal_inode, data_jblock)?;
    let commit_fs_block = journal_block_to_fs(state, &journal_inode, commit_jblock)?;

    state.ext2_journal = Ext2JournalState {
        present: true,
        replaying: false,
        aborted: false,
        abort_errno: 0,
        mode: Ext2JournalMode::Ordered,
        inode: journal_inode_num,
        inode_blocks: journal_inode.blocks,
        superblock_block: journal_sb_fs_block,
        block_size: superblock.block_size,
        max_len: superblock.max_len,
        first: superblock.first,
        sequence: superblock.sequence,
        start: superblock.start,
        runtime_head_jblock: if superblock.start != 0 {
            superblock.start
        } else {
            superblock.first
        },
        tx_descriptor_jblock: descriptor_jblock,
        tx_data_jblock: data_jblock,
        tx_commit_jblock: commit_jblock,
        tx_descriptor_fs_block: descriptor_fs_block,
        tx_data_fs_block: data_fs_block,
        tx_commit_fs_block: commit_fs_block,
        recover_marked: (state.ext2_meta.feature_incompat & EXT2_FEATURE_INCOMPAT_RECOVER) != 0,
        // Enabled by enable_journal_checksums once the log is known empty.
        checksum: false,
        commit_barrier_pending: false,
    };

    let needs_replay = (state.ext2_meta.feature_incompat & EXT2_FEATURE_INCOMPAT_RECOVER) != 0
        || superblock.start != 0;
    if !needs_replay {
        return enable_journal_checksums(state, &superblock);
    }

    state.ext2_journal.replaying = true;
    let next_sequence = match replay_journal(state, &journal_inode, &superblock) {
        Ok(sequence) => sequence,
        Err(err) => {
            state.ext2_journal.replaying = false;
            return Err(err);
        }
    };
    if let Err(err) = clear_recovery_state(state, next_sequence) {
        state.ext2_journal.replaying = false;
        return Err(err);
    }
    state.ext2_journal.replaying = false;
    state.ext2_journal.sequence = next_sequence;
    state.ext2_journal.start = 0;
    state.ext2_journal.runtime_head_jblock = state.ext2_journal.first;
    enable_journal_checksums(state, &superblock)
}

/// Turns on JBD2 COMPAT_CHECKSUM for this journal (v2 superblocks only).
/// Called only with an empty log, so no older commit lacks a checksum. It is
/// a compat feature: readers that do not know it still replay the journal,
/// and e2fsck/Linux verify the commit CRCs.
fn enable_journal_checksums(state: &mut VfsState, superblock: &JournalSuper) -> Result<(), i64> {
    if !superblock.v2 {
        return Ok(());
    }
    if (superblock.feature_compat & JBD2_FEATURE_COMPAT_CHECKSUM) == 0 {
        let block_size = state.ext2_meta.block_size;
        let mut super_block = Vec::new();
        super_block
            .try_reserve_exact(block_size)
            .map_err(|_| ENOSPC)?;
        super_block.resize(block_size, 0);
        let jsb = state.ext2_journal.superblock_block;
        ext2_backend_read_block(state, jsb, &mut super_block)?;
        let compat = read_u32_be(&super_block, 36)? | JBD2_FEATURE_COMPAT_CHECKSUM;
        write_u32_be(&mut super_block, 36, compat)?;
        write_backend_and_refresh_cache(state, jsb, &super_block)?;
        backend_barrier(state)?;
    }
    state.ext2_journal.checksum = true;
    Ok(())
}
