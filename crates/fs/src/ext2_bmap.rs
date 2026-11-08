//! Block-map seam: translates a file's logical block numbers to filesystem
//! blocks and manages the blocks that hold the mapping itself.
//!
//! Every caller in the fs crate goes through these dispatchers rather than
//! touching `i_block` directly, so additional mapping formats (ext4 extent
//! trees, `docs/ext4_compat_plan.md` M1/M2) can be added without changing
//! the file, directory, or journal code.
//!
//! Today only the classic ext2/ext3 direct/indirect scheme is implemented.
//! An inode flagged `EXT4_EXTENTS_FL` is refused with `EIO`: on a filesystem
//! without the `extent` feature (the only kind that mounts so far) such an
//! inode is corrupt.

use super::{
    indirect_detach_block, indirect_ensure_block, indirect_free_all_blocks,
    indirect_lookup_state, indirect_recalc_block_count_512_state, Ext2Inode, Ext2JournalHandle,
    VfsState, EIO,
};

/// `i_flags` bit: the inode's `i_block` holds an ext4 extent tree.
pub(super) const EXT4_EXTENTS_FL: u32 = 0x0008_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BlockMapKind {
    /// Twelve direct pointers plus single/double/triple indirect tables.
    Indirect,
    /// ext4 extent tree (not supported yet).
    Extents,
}

pub(super) fn block_map_kind(inode: &Ext2Inode) -> BlockMapKind {
    if (inode.flags & EXT4_EXTENTS_FL) != 0 {
        BlockMapKind::Extents
    } else {
        BlockMapKind::Indirect
    }
}

/// Filesystem block backing logical block `block_idx`, or 0 for a hole.
pub(super) fn ext2_data_block_for_logical_index_state(
    state: &mut VfsState,
    inode: &Ext2Inode,
    block_idx: usize,
) -> Result<u32, i64> {
    match block_map_kind(inode) {
        BlockMapKind::Indirect => indirect_lookup_state(state, inode, block_idx),
        BlockMapKind::Extents => Err(EIO),
    }
}

/// Filesystem block backing logical block `block_idx`, allocating it (and
/// any mapping blocks it needs) when it is a hole.
pub(super) fn ext2_ensure_data_block(
    state: &mut VfsState,
    journal_handle: Option<Ext2JournalHandle>,
    inode: &mut Ext2Inode,
    block_idx: usize,
) -> Result<u32, i64> {
    match block_map_kind(inode) {
        BlockMapKind::Indirect => indirect_ensure_block(state, journal_handle, inode, block_idx),
        BlockMapKind::Extents => Err(EIO),
    }
}

/// Unmaps logical block `block_idx` and returns the filesystem block that
/// backed it (0 for a hole). Mapping blocks that become empty are freed; the
/// returned data block is left for the caller to free.
pub(super) fn ext2_detach_data_block(
    state: &mut VfsState,
    journal_handle: Option<Ext2JournalHandle>,
    inode: &mut Ext2Inode,
    block_idx: usize,
) -> Result<u32, i64> {
    match block_map_kind(inode) {
        BlockMapKind::Indirect => indirect_detach_block(state, journal_handle, inode, block_idx),
        BlockMapKind::Extents => Err(EIO),
    }
}

/// Frees every data and mapping block of `inode` and clears its map.
/// `phased` wraps each direct-block free in its own restartable journal
/// phase (the truncate path) instead of freeing them back to back (the
/// final-unlink path).
pub(super) fn bmap_free_all_blocks(
    state: &mut VfsState,
    journal_handle: Option<Ext2JournalHandle>,
    inode: &mut Ext2Inode,
    phased: bool,
) -> Result<(), i64> {
    match block_map_kind(inode) {
        BlockMapKind::Indirect => indirect_free_all_blocks(state, journal_handle, inode, phased),
        BlockMapKind::Extents => Err(EIO),
    }
}

/// `i_blocks` (512-byte sectors) implied by the inode's map, including
/// mapping blocks and any external xattr block.
pub(super) fn ext2_recalc_inode_block_count_512_state(
    state: &mut VfsState,
    inode: &Ext2Inode,
) -> Result<u32, i64> {
    match block_map_kind(inode) {
        BlockMapKind::Indirect => indirect_recalc_block_count_512_state(state, inode),
        BlockMapKind::Extents => Err(EIO),
    }
}
