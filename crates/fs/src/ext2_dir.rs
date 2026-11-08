use super::{
    ext2_block_slice_mut_state, ext2_clear_inode_flags_state, ext2_dir_add_missing_tails,
    ext2_dir_block_finish, ext2_dir_tail_len, EXT2_INDEX_FL, ext2_data_block_for_logical_index_state, ext2_ensure_data_block,
    ext2_for_each_dir_entry_state, ext2_logical_blocks_for_size, ext2_mark_dirty_block,
    ext2_max_addressable_file_size, ext2_mode_to_filetype, ext2_read_inode_from_state,
    ext2_write_inode_to_state, read_u16_le,
    read_u32_le, write_u16_le, write_u32_le, Ext2JournalHandle, FileType, VfsState, EINVAL, EIO,
    ENAMETOOLONG, ENOENT, ENOSPC, ENOTDIR, EXT2_FT_DIR, EXT2_FT_REG_FILE, EXT2_FT_SYMLINK,
    EXT2_FEATURE_COMPAT_DIR_INDEX, Ext2Inode,
};
use super::ext2_htree::{self, DxError};

fn ext2_dirent_file_type(file_type: FileType) -> u8 {
    match file_type {
        FileType::Directory => EXT2_FT_DIR,
        FileType::Regular => EXT2_FT_REG_FILE,
        FileType::Symlink => EXT2_FT_SYMLINK,
        _ => 0,
    }
}

fn ext2_dirent_len(name_len: usize) -> usize {
    (8 + name_len + 3) & !3
}

pub(super) fn ext2_write_dirent(
    block: &mut [u8],
    pos: usize,
    inode_no: u32,
    rec_len: usize,
    file_type: FileType,
    name: &[u8],
) -> Result<(), i64> {
    if rec_len < 8 || rec_len > u16::MAX as usize {
        return Err(EIO);
    }
    if name.len() > 255 || rec_len < ext2_dirent_len(name.len()) {
        return Err(EIO);
    }
    if pos
        .checked_add(rec_len)
        .filter(|end| *end <= block.len())
        .is_none()
    {
        return Err(EIO);
    }

    write_u32_le(block, pos, inode_no)?;
    write_u16_le(block, pos + 4, rec_len as u16)?;
    block[pos + 6] = name.len() as u8;
    block[pos + 7] = ext2_dirent_file_type(file_type);
    block[pos + 8..pos + 8 + name.len()].copy_from_slice(name);
    if rec_len > 8 + name.len() {
        block[pos + 8 + name.len()..pos + rec_len].fill(0);
    }
    Ok(())
}

pub(super) fn ext2_add_dir_entry(
    state: &mut VfsState,
    journal_handle: Option<Ext2JournalHandle>,
    parent_ino: u32,
    child_ino: u32,
    name: &[u8],
    child_type: FileType,
) -> Result<(), i64> {
    if name.is_empty() || name.len() > 255 {
        return Err(ENAMETOOLONG);
    }

    let meta = state.ext2_meta;
    let mut parent = ext2_read_inode_from_state(state, parent_ino)?;
    if ext2_mode_to_filetype(parent.mode) != Some(FileType::Directory) {
        return Err(ENOTDIR);
    }
    let ft = ext2_dirent_file_type(child_type);
    let mut dx_fallback = false;
    if (parent.flags & EXT2_INDEX_FL) != 0 {
        match ext2_htree::dx_add_entry(state, journal_handle, &mut parent, child_ino, name, ft) {
            Ok(()) => return Ok(()),
            Err(DxError::Io(err)) => return Err(err),
            Err(DxError::Unusable) => {}
        }
        // An index this code cannot use (unknown hash, deeper than two
        // levels, damaged): linear insertion could overwrite its interior
        // data (dirents with inode 0, the ".." record's slack), so drop the
        // index first, as Linux does when it falls back to linear mode.
        dx_fallback = true;
        ext2_clear_inode_flags_state(state, parent_ino, EXT2_INDEX_FL)?;
        parent.flags &= !EXT2_INDEX_FL;
        // metadata_csum: the former index blocks become leaf blocks and need
        // checksum tails.
        ext2_dir_add_missing_tails(state, &parent)?;
    }
    let tail = ext2_dir_tail_len(state);

    let needed = ext2_dirent_len(name.len());
    let scan_blocks = ext2_logical_blocks_for_size(&meta, parent.size as usize)?;
    let addressable_blocks = ext2_max_addressable_file_size(&meta)? / meta.block_size;
    let mut max_blocks = scan_blocks.saturating_add(1);
    if max_blocks > addressable_blocks {
        max_blocks = addressable_blocks;
    }
    if max_blocks == 0 {
        return Err(ENOSPC);
    }
    let mut idx = 0usize;
    while idx < max_blocks {
        let block_no = ext2_data_block_for_logical_index_state(state, &parent, idx)?;
        if block_no == 0
            && idx == 1
            && scan_blocks == 1
            && !dx_fallback
            && (meta.feature_compat & EXT2_FEATURE_COMPAT_DIR_INDEX) != 0
        {
            // A full one-block directory becomes an htree instead of growing
            // linearly (ext4_add_entry -> make_indexed_dir).
            if ext2_htree::make_indexed_dir(state, journal_handle, &mut parent)? {
                return match ext2_htree::dx_add_entry(
                    state,
                    journal_handle,
                    &mut parent,
                    child_ino,
                    name,
                    ft,
                ) {
                    Ok(()) => Ok(()),
                    Err(DxError::Io(err)) => Err(err),
                    Err(DxError::Unusable) => Err(EIO),
                };
            }
        }
        if block_no == 0 {
            let new_block = ext2_ensure_data_block(
                state,
                journal_handle,
                &mut parent,
                idx,
            )?;

            {
                let blk = ext2_block_slice_mut_state(state, new_block)?;
                blk.fill(0);
                ext2_write_dirent(blk, 0, child_ino, meta.block_size - tail, child_type, name)?;
            }
            ext2_dir_block_finish(state, &parent, new_block)?;

            let min_size = ((idx + 1) * meta.block_size) as u32;
            if parent.size < min_size {
                parent.size = min_size;
            }
            super::ext2_update_block_count(state, &mut parent)?;
            ext2_write_inode_to_state(state, parent_ino, &parent)?;
            return Ok(());
        }

        if idx < scan_blocks {
            let mut inserted = false;
            {
                let blk = ext2_block_slice_mut_state(state, block_no)?;
                let usable = blk.len() - tail;
                let mut pos = 0usize;
                while pos + 8 <= usable {
                    let entry_ino = read_u32_le(blk, pos)?;
                    let rec_len = read_u16_le(blk, pos + 4)? as usize;
                    if rec_len < 8 || (rec_len & 0x3) != 0 || pos + rec_len > usable {
                        return Err(EIO);
                    }

                    if entry_ino == 0 {
                        if rec_len >= needed {
                            ext2_write_dirent(blk, pos, child_ino, rec_len, child_type, name)?;
                            inserted = true;
                            break;
                        }
                    } else {
                        let entry_name_len = blk[pos + 6] as usize;
                        let used = ext2_dirent_len(entry_name_len);
                        if rec_len >= used + needed {
                            write_u16_le(blk, pos + 4, used as u16)?;
                            let new_pos = pos + used;
                            let new_len = rec_len - used;
                            ext2_write_dirent(blk, new_pos, child_ino, new_len, child_type, name)?;
                            inserted = true;
                            break;
                        }
                    }

                    pos += rec_len;
                }
            }

            if inserted {
                ext2_dir_block_finish(state, &parent, block_no)?;
                return Ok(());
            }
        }

        idx += 1;
    }

    Err(ENOSPC)
}

pub(super) fn ext2_remove_dir_entry(
    state: &mut VfsState,
    _journal_handle: Option<Ext2JournalHandle>,
    parent_ino: u32,
    name: &[u8],
) -> Result<u32, i64> {
    if name.is_empty() || name == b"." || name == b".." {
        return Err(EINVAL);
    }
    let meta = state.ext2_meta;
    let parent = ext2_read_inode_from_state(state, parent_ino)?;
    if ext2_mode_to_filetype(parent.mode) != Some(FileType::Directory) {
        return Err(ENOTDIR);
    }

    if (parent.flags & EXT2_INDEX_FL) != 0 {
        match ext2_htree::dx_find(state, &parent, name) {
            Ok(Some(hit)) => {
                return remove_in_block(state, &parent, hit.block, name)?.ok_or(EIO);
            }
            Ok(None) => return Err(ENOENT),
            Err(DxError::Io(err)) => return Err(err),
            Err(DxError::Unusable) => {}
        }
    }

    let block_count = ext2_logical_blocks_for_size(&meta, parent.size as usize)?;
    let mut idx = 0usize;
    while idx < block_count {
        let block_no = ext2_data_block_for_logical_index_state(state, &parent, idx)?;
        idx += 1;
        if block_no == 0 {
            continue;
        }
        if let Some(ino) = remove_in_block(state, &parent, block_no, name)? {
            return Ok(ino);
        }
    }

    Err(ENOENT)
}

/// Removes `name` from one directory block, if it is there.
fn remove_in_block(
    state: &mut VfsState,
    parent: &Ext2Inode,
    block_no: u32,
    name: &[u8],
) -> Result<Option<u32>, i64> {
    let tail = ext2_dir_tail_len(state);
    {
        let mut removed = None;
        let block_has_tail: bool;
        {
            let blk = ext2_block_slice_mut_state(state, block_no)?;
            // htree index blocks have no tail dirent; their single fake entry
            // spans the block, so scanning to the full end is safe there.
            let usable = if tail != 0 && read_u16_le(blk, blk.len() - 8)? as usize == 12 {
                blk.len() - tail
            } else {
                blk.len()
            };
            block_has_tail = usable != blk.len();
            let mut pos = 0usize;
            let mut prev_pos = None;
            let mut prev_rec_len = 0usize;

            while pos + 8 <= usable {
                let inode_no = read_u32_le(blk, pos)?;
                let rec_len = read_u16_le(blk, pos + 4)? as usize;
                let name_len = blk[pos + 6] as usize;

                if rec_len < 8 || (rec_len & 0x3) != 0 || pos + rec_len > usable {
                    return Err(EIO);
                }

                if inode_no != 0 && 8 + name_len <= rec_len {
                    let entry_name = &blk[pos + 8..pos + 8 + name_len];
                    if entry_name == name {
                        if let Some(prev) = prev_pos {
                            let new_rec_len = prev_rec_len.saturating_add(rec_len);
                            if new_rec_len > u16::MAX as usize {
                                return Err(EIO);
                            }
                            write_u16_le(blk, prev + 4, new_rec_len as u16)?;
                        } else {
                            write_u32_le(blk, pos, 0)?;
                        }
                        removed = Some(inode_no);
                        break;
                    }
                }

                prev_pos = Some(pos);
                prev_rec_len = rec_len;
                pos += rec_len;
            }
        }

        if let Some(ino) = removed {
            if block_has_tail {
                ext2_dir_block_finish(state, parent, block_no)?;
            } else {
                ext2_mark_dirty_block(state, block_no)?;
            }
            return Ok(Some(ino));
        }
    }
    Ok(None)
}

/// Finds `name` in directory `dir_ino`: `(inode, dirent file type)`. Uses
/// the htree index when the directory has a usable one, so only the leaf
/// for the name's hash is read; otherwise scans every block.
pub(super) fn ext2_dir_lookup(
    state: &mut VfsState,
    dir_ino: u32,
    name: &[u8],
) -> Result<Option<(u32, u8)>, i64> {
    let dir = ext2_read_inode_from_state(state, dir_ino)?;
    if ext2_mode_to_filetype(dir.mode) != Some(FileType::Directory) {
        return Err(ENOTDIR);
    }
    if (dir.flags & EXT2_INDEX_FL) != 0 {
        match ext2_htree::dx_find(state, &dir, name) {
            Ok(Some(hit)) => return Ok(Some((hit.ino, hit.file_type))),
            Ok(None) => return Ok(None),
            Err(DxError::Io(err)) => return Err(err),
            Err(DxError::Unusable) => {}
        }
    }
    let mut found = None;
    ext2_for_each_dir_entry_state(state, &dir, |ino, ft, entry| {
        if found.is_none() && entry == name {
            found = Some((ino, ft));
        }
        Ok(())
    })?;
    Ok(found)
}

pub(super) fn ext2_directory_is_empty(state: &mut VfsState, ino: u32) -> Result<bool, i64> {
    let inode = ext2_read_inode_from_state(state, ino)?;
    if ext2_mode_to_filetype(inode.mode) != Some(FileType::Directory) {
        return Err(ENOTDIR);
    }

    let mut non_dot = false;
    ext2_for_each_dir_entry_state(state, &inode, |_child_ino, _ft, child_name| {
        if child_name != b"." && child_name != b".." {
            non_dot = true;
        }
        Ok(())
    })?;
    Ok(!non_dot)
}
