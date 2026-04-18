use super::{
    ext2_adjust_group_counters_state, ext2_adjust_super_counters_state, ext2_bitmap_get,
    ext2_bitmap_set, ext2_block_group_and_bit, ext2_block_slice_mut_state, ext2_block_slice_state,
    ext2_ensure_range_resident, ext2_group_block_bitmap_block_state, ext2_group_desc_offset,
    ext2_group_inode_bitmap_block_state, ext2_indirect_entries_per_block, ext2_inode_group_and_bit,
    ext2_inode_offset_in_state, ext2_mark_dirty_block, ext2_mark_dirty_range,
    ext2_read_indirect_entry_from_state, ext2_write_indirect_entry_from_state,
    ext2_write_u16_state, ext2_write_u32_state, ext2_zero_range_state, Ext2Inode,
    Ext2JournalHandle, VfsState, EINVAL, EIO, ENOSPC, EXT2_GROUP_DESC_SIZE,
    EXT2_INODE_DIRECT_COUNT, EXT2_SUPERBLOCK_OFFSET,
};

pub(super) fn ext2_alloc_block(
    state: &mut VfsState,
    journal_handle: Option<Ext2JournalHandle>,
) -> Result<u32, i64> {
    if !state.ext2_mounted {
        return Err(EINVAL);
    }

    let meta = state.ext2_meta;

    let mut group = 0u32;
    while group < meta.group_count {
        let desc_offset = ext2_group_desc_offset(&meta, group)?;
        ext2_ensure_range_resident(state, desc_offset, EXT2_GROUP_DESC_SIZE)?;
        let bitmap_block = ext2_group_block_bitmap_block_state(state, &meta, group)?;
        let mut found = None;

        {
            let bitmap = ext2_block_slice_state(state, bitmap_block)?;
            let mut bit = 0u32;
            while bit < meta.blocks_per_group {
                let block_num = meta
                    .first_data_block
                    .saturating_add(group.saturating_mul(meta.blocks_per_group))
                    .saturating_add(bit);
                if block_num >= meta.total_blocks {
                    break;
                }

                if !ext2_bitmap_get(bitmap, bit)? {
                    found = Some(block_num);
                    break;
                }
                bit += 1;
            }
        }

        if let Some(block_num) = found {
            super::ext2_journal::with_handle_restartable_phase(
                state,
                journal_handle,
                super::Ext2JournalPhaseKind::AllocBlockBookkeeping,
                super::EXT2_JOURNAL_CREDITS_HELPER_ALLOC_BLOCK_BOOKKEEPING,
                |state| {
                    {
                        let bitmap = ext2_block_slice_mut_state(state, bitmap_block)?;
                        let (_, bit) = ext2_block_group_and_bit(&meta, block_num)?;
                        ext2_bitmap_set(bitmap, bit, true)?;
                    }
                    ext2_mark_dirty_block(state, bitmap_block)?;
                    ext2_ensure_range_resident(state, EXT2_SUPERBLOCK_OFFSET, 1024)?;
                    ext2_adjust_super_counters_state(state, -1, 0)?;
                    ext2_adjust_group_counters_state(state, &meta, group, -1, 0, 0)?;
                    ext2_mark_dirty_range(state, EXT2_SUPERBLOCK_OFFSET, 1024)?;
                    let desc_offset = ext2_group_desc_offset(&meta, group)?;
                    ext2_mark_dirty_range(state, desc_offset, EXT2_GROUP_DESC_SIZE)?;
                    Ok(())
                },
            )?;
            super::ext2_journal::with_handle_restartable_phase(
                state,
                journal_handle,
                super::Ext2JournalPhaseKind::AllocBlockInit,
                super::EXT2_JOURNAL_CREDITS_HELPER_ALLOC_BLOCK_INIT,
                |state| {
                    {
                        let blk = ext2_block_slice_mut_state(state, block_num)?;
                        blk.fill(0);
                    }
                    ext2_mark_dirty_block(state, block_num)?;
                    Ok(())
                },
            )?;
            return Ok(block_num);
        }

        group += 1;
    }

    Err(ENOSPC)
}

pub(super) fn ext2_free_block(
    state: &mut VfsState,
    journal_handle: Option<Ext2JournalHandle>,
    block_num: u32,
) -> Result<(), i64> {
    if !state.ext2_mounted {
        return Err(EINVAL);
    }

    let meta = state.ext2_meta;
    let (group, bit) = ext2_block_group_and_bit(&meta, block_num)?;
    let desc_offset = ext2_group_desc_offset(&meta, group)?;
    ext2_ensure_range_resident(state, desc_offset, EXT2_GROUP_DESC_SIZE)?;
    let bitmap_block = ext2_group_block_bitmap_block_state(state, &meta, group)?;
    let mut was_set = false;

    {
        let bitmap = ext2_block_slice_mut_state(state, bitmap_block)?;
        if ext2_bitmap_get(bitmap, bit)? {
            ext2_bitmap_set(bitmap, bit, false)?;
            was_set = true;
        }
    }

    if was_set {
        super::ext2_journal::with_handle_restartable_phase(
            state,
            journal_handle,
            super::Ext2JournalPhaseKind::FreeBlockBookkeeping,
            super::EXT2_JOURNAL_CREDITS_HELPER_FREE_BLOCK_BOOKKEEPING,
            |state| {
                ext2_mark_dirty_block(state, bitmap_block)?;
                ext2_ensure_range_resident(state, EXT2_SUPERBLOCK_OFFSET, 1024)?;
                ext2_adjust_super_counters_state(state, 1, 0)?;
                ext2_adjust_group_counters_state(state, &meta, group, 1, 0, 0)?;
                ext2_mark_dirty_range(state, EXT2_SUPERBLOCK_OFFSET, 1024)?;
                let desc_offset = ext2_group_desc_offset(&meta, group)?;
                ext2_mark_dirty_range(state, desc_offset, EXT2_GROUP_DESC_SIZE)?;
                Ok(())
            },
        )?;
        super::ext2_journal::with_handle_restartable_phase(
            state,
            journal_handle,
            super::Ext2JournalPhaseKind::FreeBlockRevoke,
            super::EXT2_JOURNAL_CREDITS_HELPER_FREE_BLOCK_REVOKE,
            |state| super::ext2_journal::note_revoke(state, block_num),
        )?;
    }

    Ok(())
}

pub(super) fn ext2_alloc_inode(
    state: &mut VfsState,
    journal_handle: Option<Ext2JournalHandle>,
    is_dir: bool,
) -> Result<u32, i64> {
    if !state.ext2_mounted {
        return Err(EINVAL);
    }

    let meta = state.ext2_meta;
    let mut group = 0u32;

    while group < meta.group_count {
        let desc_offset = ext2_group_desc_offset(&meta, group)?;
        ext2_ensure_range_resident(state, desc_offset, EXT2_GROUP_DESC_SIZE)?;
        let bitmap_block = ext2_group_inode_bitmap_block_state(state, &meta, group)?;
        let mut found = None;

        {
            let bitmap = ext2_block_slice_state(state, bitmap_block)?;
            let mut bit = 0u32;
            while bit < meta.inodes_per_group {
                let ino = group
                    .saturating_mul(meta.inodes_per_group)
                    .saturating_add(bit)
                    .saturating_add(1);
                if ino > meta.total_inodes {
                    break;
                }

                if !ext2_bitmap_get(bitmap, bit)? {
                    found = Some(ino);
                    break;
                }
                bit += 1;
            }
        }

        if let Some(ino) = found {
            super::ext2_journal::with_handle_restartable_phase(
                state,
                journal_handle,
                super::Ext2JournalPhaseKind::AllocInodeBookkeeping,
                super::EXT2_JOURNAL_CREDITS_HELPER_ALLOC_INODE_BOOKKEEPING,
                |state| {
                    {
                        let bitmap = ext2_block_slice_mut_state(state, bitmap_block)?;
                        let (_, bit) = ext2_inode_group_and_bit(&meta, ino)?;
                        ext2_bitmap_set(bitmap, bit, true)?;
                    }
                    ext2_mark_dirty_block(state, bitmap_block)?;
                    ext2_ensure_range_resident(state, EXT2_SUPERBLOCK_OFFSET, 1024)?;
                    ext2_adjust_super_counters_state(state, 0, -1)?;
                    ext2_adjust_group_counters_state(
                        state,
                        &meta,
                        group,
                        0,
                        -1,
                        if is_dir { 1 } else { 0 },
                    )?;
                    ext2_mark_dirty_range(state, EXT2_SUPERBLOCK_OFFSET, 1024)?;
                    let desc_offset = ext2_group_desc_offset(&meta, group)?;
                    ext2_mark_dirty_range(state, desc_offset, EXT2_GROUP_DESC_SIZE)?;
                    Ok(())
                },
            )?;

            let inode_offset = ext2_inode_offset_in_state(state, &meta, ino)?;
            ext2_ensure_range_resident(state, inode_offset, meta.inode_size)?;
            if inode_offset
                .checked_add(meta.inode_size)
                .filter(|end| *end <= state.ext2_image_len)
                .is_none()
            {
                return Err(EIO);
            }
            super::ext2_journal::with_handle_restartable_phase(
                state,
                journal_handle,
                super::Ext2JournalPhaseKind::AllocInodeInit,
                super::EXT2_JOURNAL_CREDITS_HELPER_ALLOC_INODE_INIT,
                |state| ext2_zero_range_state(state, inode_offset, meta.inode_size),
            )?;

            return Ok(ino);
        }

        group += 1;
    }

    Err(ENOSPC)
}

pub(super) fn ext2_free_inode(
    state: &mut VfsState,
    journal_handle: Option<Ext2JournalHandle>,
    ino: u32,
    is_dir: bool,
) -> Result<(), i64> {
    if !state.ext2_mounted {
        return Err(EINVAL);
    }
    let meta = state.ext2_meta;
    let (group, bit) = ext2_inode_group_and_bit(&meta, ino)?;
    let desc_offset = ext2_group_desc_offset(&meta, group)?;
    ext2_ensure_range_resident(state, desc_offset, EXT2_GROUP_DESC_SIZE)?;
    let bitmap_block = ext2_group_inode_bitmap_block_state(state, &meta, group)?;
    let mut was_set = false;

    {
        let bitmap = ext2_block_slice_mut_state(state, bitmap_block)?;
        if ext2_bitmap_get(bitmap, bit)? {
            ext2_bitmap_set(bitmap, bit, false)?;
            was_set = true;
        }
    }

    if was_set {
        super::ext2_journal::with_handle_restartable_phase(
            state,
            journal_handle,
            super::Ext2JournalPhaseKind::FreeInodeBookkeeping,
            super::EXT2_JOURNAL_CREDITS_HELPER_FREE_INODE_BOOKKEEPING,
            |state| {
                ext2_mark_dirty_block(state, bitmap_block)?;
                ext2_ensure_range_resident(state, EXT2_SUPERBLOCK_OFFSET, 1024)?;
                ext2_adjust_super_counters_state(state, 0, 1)?;
                ext2_adjust_group_counters_state(
                    state,
                    &meta,
                    group,
                    0,
                    1,
                    if is_dir { -1 } else { 0 },
                )?;
                ext2_mark_dirty_range(state, EXT2_SUPERBLOCK_OFFSET, 1024)?;
                let desc_offset = ext2_group_desc_offset(&meta, group)?;
                ext2_mark_dirty_range(state, desc_offset, EXT2_GROUP_DESC_SIZE)?;
                Ok(())
            },
        )?;
    }

    let inode_offset = ext2_inode_offset_in_state(state, &meta, ino)?;
    ext2_ensure_range_resident(state, inode_offset, meta.inode_size)?;
    if inode_offset
        .checked_add(meta.inode_size)
        .filter(|end| *end <= state.ext2_image_len)
        .is_none()
    {
        return Err(EIO);
    }
    super::ext2_journal::with_handle_restartable_phase(
        state,
        journal_handle,
        super::Ext2JournalPhaseKind::FreeInodeClear,
        super::EXT2_JOURNAL_CREDITS_HELPER_FREE_INODE_CLEAR,
        |state| ext2_zero_range_state(state, inode_offset, meta.inode_size),
    )?;
    Ok(())
}

pub(super) fn ext2_write_inode_to_state(
    state: &mut VfsState,
    ino: u32,
    inode: &Ext2Inode,
) -> Result<(), i64> {
    if !state.ext2_mounted {
        return Err(EINVAL);
    }
    let meta = state.ext2_meta;
    let inode_offset = ext2_inode_offset_in_state(state, &meta, ino)?;
    ext2_ensure_range_resident(state, inode_offset, meta.inode_size)?;
    if inode_offset
        .checked_add(meta.inode_size)
        .filter(|end| *end <= state.ext2_image_len)
        .is_none()
    {
        return Err(EIO);
    }
    ext2_write_u16_state(state, inode_offset, inode.mode)?;
    ext2_write_u16_state(state, inode_offset + 2, inode.uid)?;
    ext2_write_u32_state(state, inode_offset + 4, inode.size)?;
    ext2_write_u32_state(state, inode_offset + 8, inode.atime)?;
    ext2_write_u32_state(state, inode_offset + 12, inode.ctime)?;
    ext2_write_u32_state(state, inode_offset + 16, inode.mtime)?;
    ext2_write_u16_state(state, inode_offset + 24, inode.gid)?;
    ext2_write_u16_state(state, inode_offset + 26, inode.links_count)?;
    ext2_write_u32_state(state, inode_offset + 28, inode.block_count_512)?;

    let mut i = 0usize;
    while i < 15 {
        ext2_write_u32_state(state, inode_offset + 40 + i * 4, inode.blocks[i])?;
        i += 1;
    }

    Ok(())
}

pub(super) fn ext2_ensure_data_block(
    state: &mut VfsState,
    journal_handle: Option<Ext2JournalHandle>,
    inode: &mut Ext2Inode,
    block_idx: usize,
) -> Result<u32, i64> {
    fn ensure_inode_table_root(
        state: &mut VfsState,
        journal_handle: Option<Ext2JournalHandle>,
        root: &mut u32,
    ) -> Result<u32, i64> {
        if *root != 0 {
            return Ok(*root);
        }
        let block = ext2_alloc_block(state, journal_handle)?;
        *root = block;
        Ok(block)
    }

    fn ensure_table_entry(
        state: &mut VfsState,
        journal_handle: Option<Ext2JournalHandle>,
        table_block: u32,
        idx: usize,
    ) -> Result<u32, i64> {
        let existing = ext2_read_indirect_entry_from_state(state, table_block, idx)?;
        if existing != 0 {
            return Ok(existing);
        }

        let block = ext2_alloc_block(state, journal_handle)?;
        let write_result = super::ext2_journal::with_handle_restartable_phase(
            state,
            journal_handle,
            super::Ext2JournalPhaseKind::IndirectLink,
            super::EXT2_JOURNAL_CREDITS_HELPER_INDIRECT_LINK,
            |state| ext2_write_indirect_entry_from_state(state, table_block, idx, block),
        );
        if let Err(err) = write_result {
            let _ = ext2_free_block(state, journal_handle, block);
            return Err(err);
        }
        Ok(block)
    }

    if block_idx < EXT2_INODE_DIRECT_COUNT {
        if inode.blocks[block_idx] != 0 {
            return Ok(inode.blocks[block_idx]);
        }
        let block = ext2_alloc_block(state, journal_handle)?;
        inode.blocks[block_idx] = block;
        return Ok(block);
    }

    let meta = state.ext2_meta;
    let entries = ext2_indirect_entries_per_block(&meta)?;
    let mut remaining = block_idx - EXT2_INODE_DIRECT_COUNT;

    if remaining < entries {
        let single = ensure_inode_table_root(state, journal_handle, &mut inode.blocks[12])?;
        return ensure_table_entry(state, journal_handle, single, remaining);
    }
    remaining -= entries;

    let double_span = entries.checked_mul(entries).ok_or(EIO)?;
    if remaining < double_span {
        let double = ensure_inode_table_root(state, journal_handle, &mut inode.blocks[13])?;
        let l1_idx = remaining / entries;
        let l2_idx = remaining % entries;
        let l1 = ensure_table_entry(state, journal_handle, double, l1_idx)?;
        return ensure_table_entry(state, journal_handle, l1, l2_idx);
    }
    remaining -= double_span;

    let triple_span = double_span.checked_mul(entries).ok_or(EIO)?;
    if remaining < triple_span {
        let triple = ensure_inode_table_root(state, journal_handle, &mut inode.blocks[14])?;
        let l1_idx = remaining / double_span;
        let l2_idx = (remaining % double_span) / entries;
        let l3_idx = remaining % entries;
        let l1 = ensure_table_entry(state, journal_handle, triple, l1_idx)?;
        let l2 = ensure_table_entry(state, journal_handle, l1, l2_idx)?;
        return ensure_table_entry(state, journal_handle, l2, l3_idx);
    }

    Err(ENOSPC)
}

pub(super) fn ext2_write_zero_range(
    state: &mut VfsState,
    journal_handle: Option<Ext2JournalHandle>,
    inode: &mut Ext2Inode,
    mut start: usize,
    end: usize,
) -> Result<(), i64> {
    if start >= end {
        return Ok(());
    }

    let meta = state.ext2_meta;
    while start < end {
        let block_idx = start / meta.block_size;
        let step_credits = super::ext2_estimate_data_write_step_credits(state, inode, block_idx)?;
        let chunk = super::ext2_journal::with_handle_restartable_phase(
            state,
            journal_handle,
            super::Ext2JournalPhaseKind::DataZero,
            step_credits,
            |state| {
                let block_no = ext2_ensure_data_block(state, journal_handle, inode, block_idx)?;
                let block_off = start % meta.block_size;
                let chunk = core::cmp::min(end - start, meta.block_size - block_off);

                {
                    let blk = ext2_block_slice_mut_state(state, block_no)?;
                    blk[block_off..block_off + chunk].fill(0);
                }
                super::ext2_journal::note_data_block(state, block_no)?;
                ext2_mark_dirty_block(state, block_no)?;
                Ok(chunk)
            },
        )?;
        start += chunk;
    }

    Ok(())
}
