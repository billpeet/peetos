use alloc::vec::Vec;

use super::{
    ext2_backend, ext2_backend_read_block, ext2_block_slice_mut_state, ext2_cache_find,
    ext2_cache_flush_block, ext2_data_block_for_logical_index_state, ext2_mark_dirty_block,
    ext2_read_inode_from_state, ext2_unmark_dirty_block, ext2_write_u32_state,
    Ext2Inode, Ext2JournalCommittedTx, Ext2JournalCommittedUpdate, Ext2JournalState, VfsState,
    EINVAL, EIO, ENOSPC, EXT2_FEATURE_COMPAT_HAS_JOURNAL, EXT2_FEATURE_INCOMPAT_JOURNAL_DEV,
    EXT2_FEATURE_INCOMPAT_RECOVER, EXT2_SUPERBLOCK_OFFSET,
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
    if data_blocks
        .saturating_add(revoke_blocks)
        .saturating_add(2)
        > journal_window
    {
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
    Ok((descriptor_jblock, data_jblocks, revoke_jblocks, commit_jblock))
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
    state.ext2_meta.block_size.saturating_sub(16).saturating_div(4)
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

fn tx_uses_journal_block(superblock: &JournalSuper, tx: &Ext2JournalCommittedTx, jblock: u32) -> bool {
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

pub(super) fn note_revoke(state: &mut VfsState, block: u32) -> Result<(), i64> {
    if !state.ext2_journal.present || state.ext2_journal.replaying {
        return Ok(());
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
        let revoked = match revoke_map.binary_search_by_key(&entry.home_block, |(block, _)| *block) {
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

    let block_size = state.ext2_meta.block_size;
    let sequence = state.ext2_journal.sequence;
    let revoke_capacity = revoke_entries_per_block(state);
    if revoke_capacity == 0 {
        return Err(EIO);
    }

    let mut escaped = Vec::new();
    escaped
        .try_reserve_exact(blocks.len())
        .map_err(|_| ENOSPC)?;
    let mut idx = 0usize;
    while idx < blocks.len() {
        let block = blocks[idx];
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

    let journal_window = state
        .ext2_journal
        .max_len
        .saturating_sub(state.ext2_journal.first) as usize;
    let max_payload_blocks = journal_window.saturating_sub(2);
    if blocks.len() > max_payload_blocks {
        return Err(EIO);
    }

    let mut revoke_candidates = Vec::new();
    revoke_candidates
        .try_reserve_exact(state.ext2_journal_pending_revokes.len())
        .map_err(|_| ENOSPC)?;
    idx = 0;
    while idx < state.ext2_journal_pending_revokes.len() {
        let revoke = state.ext2_journal_pending_revokes[idx];
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
        let cache_idx = ext2_cache_find(state, blocks[idx]).map_err(|_| EIO)?;
        journal_data.copy_from_slice(state.ext2_cached_blocks[cache_idx].data.as_slice());
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

    write_backend_and_refresh_cache(state, commit_fs_block, commit.as_slice())?;

    idx = 0;
    while idx < emitted_revoke_count {
        let _ = remove_sorted_u32(&mut state.ext2_journal_pending_revokes, revoke_candidates[idx]);
        idx += 1;
    }
    idx = 0;
    while idx < blocks.len() {
        let _ = remove_sorted_u32(&mut state.ext2_journal_pending_revokes, blocks[idx]);
        idx += 1;
    }

    let mut committed_updates = Vec::new();
    committed_updates
        .try_reserve_exact(blocks.len())
        .map_err(|_| ENOSPC)?;
    idx = 0;
    while idx < blocks.len() {
        let block = blocks[idx];
        committed_updates.push(Ext2JournalCommittedUpdate {
            home_block: block,
            journal_block: data_jblocks[idx],
            escaped: escaped[idx],
        });
        if let Ok(cache_idx) = ext2_cache_find(state, block) {
            state.ext2_cached_blocks[cache_idx].dirty = false;
            state.ext2_cached_blocks[cache_idx].referenced = true;
        }
        ext2_unmark_dirty_block(state, block);
        idx += 1;
    }

    state
        .ext2_journal_committed_txs
        .try_reserve(1)
        .map_err(|_| ENOSPC)?;
    state.ext2_journal_committed_txs.push(Ext2JournalCommittedTx {
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
}

fn checkpoint_committed(state: &mut VfsState, max_txs: usize) -> Result<usize, i64> {
    if max_txs == 0 || state.ext2_journal_committed_txs.is_empty() {
        return Ok(0);
    }

    let block_size = state.ext2_meta.block_size;
    let mut scratch = Vec::new();
    scratch.try_reserve_exact(block_size).map_err(|_| ENOSPC)?;
    scratch.resize(block_size, 0);
    let magic = JBD_MAGIC.to_be_bytes();

    let mut completed_txs = 0usize;
    let mut checkpointed_blocks = 0usize;
    while !state.ext2_journal_committed_txs.is_empty() && completed_txs < max_txs {
        let tx = state.ext2_journal_committed_txs.remove(0);
        let _sequence = tx.sequence;
        let mut idx = 0usize;
        while idx < tx.updates.len() {
            let update = tx.updates[idx];
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
}

pub(super) fn checkpoint_all_committed(state: &mut VfsState) -> Result<usize, i64> {
    checkpoint_committed(state, usize::MAX)
}

pub(super) fn flush_dirty_block(state: &mut VfsState, block: u32) -> Result<(), i64> {
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

    let mut idx = 0usize;
    while idx < state.ext2_dirty_blocks.len() && picked.len() < batch_cap {
        let block = state.ext2_dirty_blocks[idx];
        if block == state.ext2_journal.superblock_block {
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
    if picked.is_empty() {
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
    state.ext2_journal_committed_txs.clear();
    state.ext2_journal_pending_revokes.clear();

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
