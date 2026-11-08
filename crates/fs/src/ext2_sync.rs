use super::{ext2_backend_read_block, Ext2DirtyRun, VfsState, EINVAL, EIO};

pub(crate) fn next_dirty_run(
    state: &VfsState,
    start_block: u32,
) -> Result<Option<Ext2DirtyRun>, i64> {
    if state.ext2_dirty_blocks.is_empty() {
        return Ok(None);
    }

    let start_idx = match state.ext2_dirty_blocks.binary_search(&start_block) {
        Ok(idx) => idx,
        Err(idx) => idx,
    };
    if start_idx >= state.ext2_dirty_blocks.len() {
        return Ok(None);
    }

    let run_start = state.ext2_dirty_blocks[start_idx];
    let mut run_end = run_start;
    let mut idx = start_idx + 1;
    while idx < state.ext2_dirty_blocks.len() {
        let block = state.ext2_dirty_blocks[idx];
        if block != run_end.saturating_add(1) {
            break;
        }
        run_end = block;
        idx += 1;
    }
    Ok(Some(Ext2DirtyRun {
        start_block: run_start,
        block_count: run_end.saturating_sub(run_start).saturating_add(1),
    }))
}

pub(crate) fn copy_blocks(
    state: &mut VfsState,
    start_block: u32,
    out: &mut [u8],
) -> Result<usize, i64> {
    let block_size = state.ext2_meta.block_size;
    if block_size == 0 {
        return Err(EIO);
    }
    if (out.len() % block_size) != 0 {
        return Err(EINVAL);
    }

    let start = start_block as usize;
    if start >= state.ext2_tracked_block_count {
        return Ok(0);
    }
    let max_blocks = core::cmp::min(
        out.len() / block_size,
        state.ext2_tracked_block_count.saturating_sub(start),
    );

    let mut idx = 0usize;
    while idx < max_blocks {
        let dst_off = idx.checked_mul(block_size).ok_or(EIO)?;
        let dst_end = dst_off.checked_add(block_size).ok_or(EIO)?;
        let block = (start + idx) as u32;
        ext2_backend_read_block(state, block, &mut out[dst_off..dst_end])?;
        idx += 1;
    }

    max_blocks.checked_mul(block_size).ok_or(EIO)
}

pub(crate) fn mark_synced(state: &mut VfsState) -> Result<(), i64> {
    state.ext2_dirty_blocks.clear();
    for entry in state.ext2_cached_blocks.iter_mut() {
        entry.dirty = false;
    }
    Ok(())
}

pub(crate) fn flush_all_dirty(state: &mut VfsState) -> Result<usize, i64> {
    let mut flushed = 0usize;
    while !state.ext2_dirty_blocks.is_empty() {
        if state.ext2_journal.present && !state.ext2_journal.replaying {
            let batch_flushed = super::ext2_journal::flush_dirty_batch(state, 64)?;
            if batch_flushed > 0 {
                flushed = flushed.saturating_add(batch_flushed);
                continue;
            }
        }
        let block = state.ext2_dirty_blocks[0];
        super::ext2_cache_flush_block(state, block)?;
        flushed = flushed.saturating_add(1);
    }
    if state.ext2_journal.present && !state.ext2_journal.replaying {
        // An explicit flush promises durability of everything it committed.
        super::ext2_journal::settle_commit_barrier(state)?;
    }
    Ok(flushed)
}

pub(crate) fn checkpoint_journal(state: &mut VfsState) -> Result<usize, i64> {
    if !state.ext2_journal.present || state.ext2_journal.replaying {
        return Ok(0);
    }
    super::ext2_journal::checkpoint_all_committed(state)
}
