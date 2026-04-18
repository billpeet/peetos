use alloc::vec::Vec;

use super::{
    ext2_backend, ext2_backend_read_block, ext2_block_slice_mut_state, ext2_cache_find,
    ext2_cache_flush_block, ext2_data_block_for_logical_index_state, ext2_mark_dirty_block,
    ext2_read_inode_from_state, ext2_unmark_dirty_block, ext2_write_u32_state, Ext2Inode,
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

    Ok(JournalSuper {
        block_size,
        max_len,
        first,
        sequence,
        start,
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
    (backend.write_block)(backend.ctx, block, data)
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

fn write_checkpoint_home_block(state: &mut VfsState, block: u32, data: &[u8]) -> Result<(), i64> {
    backend_write_block_raw(state, block, data)?;
    if let Ok(idx) = ext2_cache_find(state, block) {
        let entry = &mut state.ext2_cached_blocks[idx];
        if entry.dirty {
            entry.referenced = true;
            return Ok(());
        }
        if entry.data.len() == data.len() {
            entry.data.copy_from_slice(data);
        }
        entry.referenced = true;
        entry.dirty = false;
    }
    let _ = ext2_unmark_dirty_block(state, block);
    Ok(())
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

fn runtime_tx_data_capacity(state: &VfsState) -> usize {
    if state.ext2_journal.max_len <= state.ext2_journal.first.saturating_add(1) {
        return 0;
    }
    let journal_window = state
        .ext2_journal
        .max_len
        .saturating_sub(state.ext2_journal.first) as usize;
    let max_data_by_blocks = journal_window.saturating_sub(2);
    let max_data_by_tags = state
        .ext2_meta
        .block_size
        .saturating_sub(12)
        .saturating_div(8);
    core::cmp::min(max_data_by_blocks, max_data_by_tags)
}

fn runtime_journal_super(state: &VfsState) -> JournalSuper {
    JournalSuper {
        block_size: state.ext2_journal.block_size,
        max_len: state.ext2_journal.max_len,
        first: state.ext2_journal.first,
        sequence: state.ext2_journal.sequence,
        start: state.ext2_journal.start,
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

fn runtime_tx_layout(
    state: &VfsState,
    data_blocks: usize,
    revoke_blocks: usize,
) -> Result<(u32, Vec<u32>, Vec<u32>, u32), i64> {
    if data_blocks == 0 {
        return Err(EINVAL);
    }
    if data_blocks > runtime_tx_data_capacity(state) {
        return Err(EIO);
    }
    let journal_window = state
        .ext2_journal
        .max_len
        .saturating_sub(state.ext2_journal.first) as usize;
    if data_blocks.saturating_add(revoke_blocks).saturating_add(2) > journal_window {
        return Err(EIO);
    }

    let superblock = runtime_journal_super(state);
    let descriptor_jblock = runtime_head_jblock(state);
    let mut cursor = descriptor_jblock;
    let mut data_jblocks = Vec::new();
    data_jblocks
        .try_reserve_exact(data_blocks)
        .map_err(|_| ENOSPC)?;

    let mut idx = 0usize;
    while idx < data_blocks {
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
    if pending_tx_conflicts(
        state,
        descriptor_jblock,
        data_jblocks.as_slice(),
        revoke_jblocks.as_slice(),
        commit_jblock,
    ) {
        return Err(ENOSPC);
    }
    Ok((
        descriptor_jblock,
        data_jblocks,
        revoke_jblocks,
        commit_jblock,
    ))
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
    insert_sorted_unique_u32(&mut state.ext2_journal_running_tx.revoke_blocks, block)
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

fn flush_ordered_data_home(state: &mut VfsState, blocks: &[u32]) -> Result<usize, i64> {
    let mut idx = 0usize;
    while idx < blocks.len() {
        let block = blocks[idx];
        let mut snapshot: Option<Vec<u8>> = None;
        if !state.ext2_journal_pending_txs.is_empty() {
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
    drop_running_tx_blocks(state, blocks);
    drop_pending_tx_blocks(state, blocks);
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

        let journal_window = state
            .ext2_journal
            .max_len
            .saturating_sub(state.ext2_journal.first) as usize;
        let max_payload_blocks = journal_window.saturating_sub(2);
        if blocks.len() > max_payload_blocks {
            return Err(EIO);
        }

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
        let max_revoke_blocks = max_payload_blocks.saturating_sub(blocks.len());
        let max_revoke_entries = max_revoke_blocks.saturating_mul(revoke_capacity);
        let emitted_revoke_count = core::cmp::min(revoke_candidates.len(), max_revoke_entries);
        let revoke_block_count = if emitted_revoke_count == 0 {
            0
        } else {
            emitted_revoke_count
                .saturating_add(revoke_capacity.saturating_sub(1))
                .saturating_div(revoke_capacity)
        };

        let (descriptor_jblock, data_jblocks, revoke_jblocks, commit_jblock) = loop {
            match runtime_tx_layout(state, blocks.len(), revoke_block_count) {
                Ok(layout) => break layout,
                Err(ENOSPC) if !state.ext2_journal_committed_txs.is_empty() => {
                    checkpoint_committed(state, 1)?;
                }
                Err(err) => return Err(err),
            }
        };

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

        let mut descriptor = Vec::new();
        descriptor
            .try_reserve_exact(block_size)
            .map_err(|_| ENOSPC)?;
        descriptor.resize(block_size, 0);
        write_u32_be(&mut descriptor, 0, JBD_MAGIC)?;
        write_u32_be(&mut descriptor, 4, JBD_DESCRIPTOR_BLOCK)?;
        write_u32_be(&mut descriptor, 8, sequence)?;
        let mut desc_off = 12usize;
        idx = 0;
        while idx < blocks.len() {
            write_u32_be(&mut descriptor, desc_off, blocks[idx])?;
            let mut flags = JBD_FLAG_SAME_UUID;
            if escaped[idx] {
                flags |= JBD_FLAG_ESCAPE;
            }
            if idx + 1 == blocks.len() {
                flags |= JBD_FLAG_LAST_TAG;
            }
            write_u32_be(&mut descriptor, desc_off + 4, flags)?;
            desc_off = desc_off.saturating_add(8);
            idx += 1;
        }

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
        write_journal_super_fields(state, sequence, start_before_commit)?;
        write_backend_and_refresh_cache(state, descriptor_fs_block, descriptor.as_slice())?;

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
            let jblock = data_jblocks[idx];
            let fs_block = journal_block_to_fs_from_state(state, jblock)?;
            write_backend_and_refresh_cache(state, fs_block, journal_data.as_slice())?;
            idx += 1;
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
            flush_ordered_data_home(state, ordered_data_blocks.as_slice())?;
        }

        write_backend_and_refresh_cache(state, commit_fs_block, commit.as_slice())?;

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
            idx += 1;
        }
        if using_pending_tx {
            state.ext2_journal_pending_txs.retain(|tx| {
                !tx.metadata_blocks.is_empty()
                    || !tx.metadata_snapshots.is_empty()
                    || !tx.data_blocks.is_empty()
                    || !tx.data_snapshots.is_empty()
                    || !tx.revoke_blocks.is_empty()
            });
        }

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
        drop_running_tx_blocks(state, blocks);
        if using_pending_tx {
            drop_pending_tx_blocks(state, blocks);
        }

        state
            .ext2_journal_committed_txs
            .try_reserve(1)
            .map_err(|_| ENOSPC)?;
        state
            .ext2_journal_committed_txs
            .push(Ext2JournalCommittedTx {
                sequence,
                descriptor_jblock,
                log_block_count: blocks
                    .len()
                    .saturating_add(revoke_jblocks.len())
                    .saturating_add(2) as u32,
                updates: committed_updates,
            });

        let next_start = if state.ext2_journal_committed_txs.is_empty() {
            0
        } else {
            state.ext2_journal_committed_txs[0].descriptor_jblock
        };
        write_journal_super_fields(state, sequence.wrapping_add(1), next_start)?;
        state.ext2_journal.sequence = sequence.wrapping_add(1);
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
        write_journal_super_fields(state, state.ext2_journal.sequence, start)?;
        state.ext2_journal.start = start;
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
    let metadata_blocks = collect_dirty_pending_metadata_blocks(state)?;
    if !metadata_blocks.is_empty() {
        let take = core::cmp::min(batch_cap, metadata_blocks.len());
        return flush_tx_blocks(state, &metadata_blocks[..take]);
    }

    let ordered_data_blocks = collect_dirty_pending_data_blocks(state)?;
    if !ordered_data_blocks.is_empty() {
        let count = core::cmp::min(batch_cap, ordered_data_blocks.len());
        return flush_ordered_data_home(state, &ordered_data_blocks[..count]);
    }
    Ok(0)
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
            let ordered_data_blocks = if !state.ext2_journal_pending_txs.is_empty() {
                collect_dirty_pending_data_blocks(state)?
            } else {
                collect_dirty_running_data_blocks(state)?
            };
            if !ordered_data_blocks.is_empty() {
                let count = core::cmp::min(batch_cap, ordered_data_blocks.len());
                return flush_ordered_data_home(state, &ordered_data_blocks[..count]);
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
                    let mut tag_idx = 0usize;
                    while tag_idx < tags.len() {
                        let tag = tags[tag_idx];
                        position = journal_next_block(superblock, position)?;
                        scan_budget = scan_budget.saturating_sub(1);
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

fn clear_recovery_state(state: &mut VfsState, next_sequence: u32) -> Result<(), i64> {
    write_journal_super_fields(state, next_sequence, 0)?;

    if (state.ext2_meta.feature_incompat & EXT2_FEATURE_INCOMPAT_RECOVER) != 0 {
        let incompat = state.ext2_meta.feature_incompat & !EXT2_FEATURE_INCOMPAT_RECOVER;
        ext2_write_u32_state(state, EXT2_SUPERBLOCK_OFFSET + 96, incompat)?;
        state.ext2_meta.feature_incompat = incompat;
    }
    flush_all_dirty_blocks(state)
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
    };

    let needs_replay = (state.ext2_meta.feature_incompat & EXT2_FEATURE_INCOMPAT_RECOVER) != 0
        || superblock.start != 0;
    if !needs_replay {
        return Ok(());
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
    Ok(())
}
