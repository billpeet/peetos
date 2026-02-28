use super::{
    ensure_dir_path_tmpfs, ensure_init, ext2_backend, ext2_backend_read_block,
    ext2_clear_dirty_tracking, ext2_journal, ext2_mark_resident_range, ext2_parse_meta_with_len,
    ext2_read_inode_from_state, ext2_reset_dirty_tracking,
    ext2_set_cache_policy as ext2_set_cache_policy_state, ext2_set_image_len, ext2_sync,
    mount_table_set, prune_root_overlays_after_ext2_mount, rollback_ext2_mount_import, with_state,
    Ext2BackendKind, Ext2BlockBackend, Ext2DirtyRun, FileType, NodeBackend, EBUSY, EEXIST, EINVAL,
    EIO, ENOENT, ENOSPC, ENOTDIR, EXT2_ROOT_INODE, ROOT_INODE,
};

pub fn mount_ext2_block_backend(
    mount_path: &str,
    image_len: usize,
    head: &[u8],
) -> Result<(), i64> {
    with_state(|state| {
        ensure_init(state);

        let mount_inode = ensure_dir_path_tmpfs(state, mount_path)?;
        if state.inodes[mount_inode as usize].file_type != FileType::Directory {
            return Err(ENOTDIR);
        }

        if mount_inode != ROOT_INODE && state.inodes[mount_inode as usize].child_count != 0 {
            return Err(EEXIST);
        }

        if image_len == 0 {
            return Err(ENOSPC);
        }
        if !matches!(state.ext2_backend, ext2_backend::Ext2Backend::BlockOps(_)) {
            return Err(EINVAL);
        }

        ext2_set_image_len(state, image_len);
        state.ext2_image.clear();
        let head_len = core::cmp::min(image_len, head.len());
        if head_len > 0 {
            state
                .ext2_image
                .try_reserve_exact(head_len)
                .map_err(|_| ENOSPC)?;
            state.ext2_image.resize(head_len, 0);
            state.ext2_image[..head_len].copy_from_slice(&head[..head_len]);
        }

        let meta = match ext2_parse_meta_with_len(&state.ext2_image[..head_len], image_len, false) {
            Ok(meta) => meta,
            Err(err) => {
                state.ext2_image.clear();
                state.ext2_image_len = 0;
                ext2_clear_dirty_tracking(state);
                return Err(err);
            }
        };
        if meta.blocks_per_group == 0 {
            state.ext2_image.clear();
            state.ext2_image_len = 0;
            ext2_clear_dirty_tracking(state);
            return Err(EINVAL);
        }

        state.ext2_meta = meta;
        state.ext2_mounted = true;
        state.ext2_mount_inode = mount_inode;
        ext2_reset_dirty_tracking(state)?;
        let resident_len = (head_len / meta.block_size) * meta.block_size;
        ext2_mark_resident_range(state, 0, resident_len)?;
        if let Err(err) = ext2_journal::prepare_mount(state) {
            rollback_ext2_mount_import(state, mount_inode);
            return Err(err);
        }

        let root = match ext2_read_inode_from_state(state, EXT2_ROOT_INODE) {
            Ok(root) => root,
            Err(err) => {
                rollback_ext2_mount_import(state, mount_inode);
                return Err(err);
            }
        };
        if super::ext2_mode_to_filetype(root.mode) != Some(FileType::Directory) {
            rollback_ext2_mount_import(state, mount_inode);
            return Err(EIO);
        }

        let mount = &mut state.inodes[mount_inode as usize];
        mount.backend = NodeBackend::Ext2;
        mount.ext2_ino = EXT2_ROOT_INODE;
        mount.ext2_children_loaded = false;
        mount.size = root.size;
        mount.mode = root.mode;
        mount.links_count = root.links_count;
        mount.uid = root.uid;
        mount.gid = root.gid;
        mount.atime = root.atime as u64;
        mount.mtime = root.mtime as u64;
        mount.ctime = root.ctime as u64;
        let fs_name = if state.ext2_journal.present {
            "ext3"
        } else {
            "ext2"
        };
        if let Err(err) = mount_table_set(state, mount_inode, mount_path, fs_name, false) {
            rollback_ext2_mount_import(state, mount_inode);
            return Err(err);
        }
        if mount_inode == ROOT_INODE {
            prune_root_overlays_after_ext2_mount(state);
        }
        Ok(())
    })
}

pub fn export_ext2_image(out: &mut [u8]) -> Result<usize, i64> {
    with_state(|state| {
        ensure_init(state);
        if !state.ext2_mounted {
            return Err(ENOENT);
        }
        let image_len = state.ext2_image_len;
        if out.len() < image_len {
            return Err(ENOSPC);
        }
        let block_size = state.ext2_meta.block_size;
        if block_size == 0 || (image_len % block_size) != 0 {
            return Err(EIO);
        }

        let block_count = image_len / block_size;
        let mut block = 0usize;
        while block < block_count {
            let start = block.checked_mul(block_size).ok_or(EIO)?;
            let end = start.checked_add(block_size).ok_or(EIO)?;
            ext2_backend_read_block(state, block as u32, &mut out[start..end])?;
            block += 1;
        }

        Ok(image_len)
    })
}

pub fn ext2_backend_kind() -> Ext2BackendKind {
    with_state(|state| {
        ensure_init(state);
        state.ext2_backend.kind()
    })
}

pub fn set_ext2_block_backend(backend: Ext2BlockBackend) -> Result<(), i64> {
    with_state(|state| {
        ensure_init(state);
        if state.ext2_mounted {
            return Err(EBUSY);
        }
        state.ext2_backend = ext2_backend::Ext2Backend::BlockOps(backend);
        Ok(())
    })
}

pub fn set_ext2_cache_policy(
    cache_max_blocks: usize,
    dirty_high_watermark_percent: u8,
) -> Result<(), i64> {
    with_state(|state| {
        ensure_init(state);
        ext2_set_cache_policy_state(state, cache_max_blocks, dirty_high_watermark_percent)
    })
}

pub fn clear_ext2_block_backend() -> Result<(), i64> {
    with_state(|state| {
        ensure_init(state);
        if state.ext2_mounted {
            return Err(EBUSY);
        }
        state.ext2_backend = ext2_backend::Ext2Backend::none();
        Ok(())
    })
}

pub fn ext2_next_dirty_run(start_block: u32) -> Result<Option<Ext2DirtyRun>, i64> {
    with_state(|state| {
        ensure_init(state);
        if !state.ext2_mounted {
            return Err(ENOENT);
        }
        ext2_sync::next_dirty_run(state, start_block)
    })
}

pub fn ext2_has_journal() -> bool {
    with_state(|state| {
        ensure_init(state);
        state.ext2_mounted && state.ext2_journal.present
    })
}

pub fn flush_ext2_dirty_blocks() -> Result<usize, i64> {
    with_state(|state| {
        ensure_init(state);
        if !state.ext2_mounted {
            return Err(ENOENT);
        }
        ext2_sync::flush_all_dirty(state)
    })
}

pub fn checkpoint_ext2_journal() -> Result<usize, i64> {
    with_state(|state| {
        ensure_init(state);
        if !state.ext2_mounted {
            return Err(ENOENT);
        }
        ext2_sync::checkpoint_journal(state)
    })
}

pub fn copy_ext2_blocks(start_block: u32, out: &mut [u8]) -> Result<usize, i64> {
    with_state(|state| {
        ensure_init(state);
        if !state.ext2_mounted {
            return Err(ENOENT);
        }
        ext2_sync::copy_blocks(state, start_block, out)
    })
}

pub fn mark_ext2_blocks_synced() -> Result<(), i64> {
    with_state(|state| {
        ensure_init(state);
        if !state.ext2_mounted {
            return Err(ENOENT);
        }
        ext2_sync::mark_synced(state)
    })
}
