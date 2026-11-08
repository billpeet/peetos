//! Block-map seam: translates a file's logical block numbers to filesystem
//! blocks and manages the blocks that hold the mapping itself.
//!
//! Every caller in the fs crate goes through these dispatchers rather than
//! touching `i_block` directly, so additional mapping formats (ext4 extent
//! trees, `docs/ext4_compat_plan.md` M1/M2) can be added without changing
//! the file, directory, or journal code.
//!
//! Backends:
//! - classic ext2/ext3 direct/indirect maps (read and write);
//! - ext4 extent trees (read-only for now: lookups work, while allocation,
//!   detach, free and block counting return `EIO`, which read-only ext4
//!   mounts never reach; see docs/ext4_compat_plan.md M2).

use super::{
    ext2_block_slice_state, ext2_report, ext4_csum, indirect_detach_block,
    indirect_ensure_block, indirect_free_all_blocks, indirect_lookup_state,
    indirect_recalc_block_count_512_state, read_u16_le, read_u32_le, Ext2Inode,
    Ext2JournalHandle, VfsState, EIO,
};

/// Deepest extent tree accepted (Linux: EXT4_MAX_EXTENT_DEPTH).
const EXT4_MAX_EXTENT_DEPTH: u16 = 5;
/// `ee_len` above this marks an unwritten (preallocated) extent.
const EXT_INIT_MAX_LEN: u32 = 32768;
const EXTENT_HEADER_SIZE: usize = 12;
const EXTENT_ENTRY_SIZE: usize = 12;

enum ExtentStep {
    /// Physical block, or 0 for a hole / unwritten extent.
    Found(u32),
    /// Descend into this tree block, which must have this depth.
    Descend(u32, u16),
}

/// Parses one extent node (the 60-byte in-inode root or a tree block) and
/// takes one step towards logical block `lblk`.
fn extent_step(node: &[u8], lblk: u32, expected_depth: Option<u16>) -> Result<ExtentStep, i64> {
    if node.len() < EXTENT_HEADER_SIZE || read_u16_le(node, 0)? != ext4_csum::EXT4_EXTENT_MAGIC {
        return Err(EIO);
    }
    let entries = read_u16_le(node, 2)? as usize;
    let max = read_u16_le(node, 4)? as usize;
    let depth = read_u16_le(node, 6)?;
    if entries > max
        || EXTENT_HEADER_SIZE + max * EXTENT_ENTRY_SIZE > node.len()
        || depth > EXT4_MAX_EXTENT_DEPTH
        || expected_depth.map_or(false, |d| d != depth)
    {
        return Err(EIO);
    }
    let entry = |i: usize| EXTENT_HEADER_SIZE + i * EXTENT_ENTRY_SIZE;

    if depth > 0 {
        // Index entries are sorted by ei_block: take the last one <= lblk.
        let mut chosen = None;
        for i in 0..entries {
            if read_u32_le(node, entry(i))? <= lblk {
                chosen = Some(i);
            } else {
                break;
            }
        }
        let i = match chosen {
            Some(i) => i,
            None => return Ok(ExtentStep::Found(0)),
        };
        let leaf_lo = read_u32_le(node, entry(i) + 4)?;
        let leaf_hi = read_u16_le(node, entry(i) + 8)?;
        if leaf_hi != 0 || leaf_lo == 0 {
            return Err(EIO);
        }
        return Ok(ExtentStep::Descend(leaf_lo, depth - 1));
    }

    for i in 0..entries {
        let first = read_u32_le(node, entry(i))?;
        let mut len = read_u16_le(node, entry(i) + 4)? as u32;
        let unwritten = len > EXT_INIT_MAX_LEN;
        if unwritten {
            len -= EXT_INIT_MAX_LEN;
        }
        if lblk < first {
            break;
        }
        if lblk - first < len {
            if unwritten {
                // Allocated but never written: reads as zeros.
                return Ok(ExtentStep::Found(0));
            }
            let start_hi = read_u16_le(node, entry(i) + 6)?;
            let start_lo = read_u32_le(node, entry(i) + 8)?;
            if start_hi != 0 {
                return Err(EIO);
            }
            return start_lo.checked_add(lblk - first).map(ExtentStep::Found).ok_or(EIO);
        }
    }
    Ok(ExtentStep::Found(0))
}

/// Maps a logical block through the inode's extent tree, verifying the
/// checksum of every tree block on `metadata_csum` filesystems.
fn extent_lookup(state: &mut VfsState, inode: &Ext2Inode, block_idx: usize) -> Result<u32, i64> {
    let lblk = u32::try_from(block_idx).map_err(|_| EIO)?;
    let mut root = [0u8; 60];
    for (i, word) in inode.blocks.iter().enumerate() {
        root[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    let seed = state
        .ext4_csum
        .filter(|csum| csum.metadata_csum)
        .map(|csum| csum.inode_seed_for(inode.ino, inode.generation));

    let mut step = extent_step(&root, lblk, None)?;
    let mut levels = 0u16;
    loop {
        let (block, depth) = match step {
            ExtentStep::Found(physical) => return Ok(physical),
            ExtentStep::Descend(block, depth) => (block, depth),
        };
        levels += 1;
        if levels > EXT4_MAX_EXTENT_DEPTH {
            return Err(EIO);
        }
        let (csum_ok, next) = {
            let node = ext2_block_slice_state(state, block)?;
            let csum_ok = match seed {
                Some(seed) => ext4_csum::extent_block_csum_ok(seed, node)?,
                None => true,
            };
            (csum_ok, if csum_ok { Some(extent_step(node, lblk, Some(depth))?) } else { None })
        };
        if !csum_ok {
            ext2_report(state, "extent block checksum", block as u64);
            return Err(EIO);
        }
        step = next.ok_or(EIO)?;
    }
}

/// `i_flags` bit: the inode's `i_block` holds an ext4 extent tree.
pub(super) const EXT4_EXTENTS_FL: u32 = 0x0008_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BlockMapKind {
    /// Twelve direct pointers plus single/double/triple indirect tables.
    Indirect,
    /// ext4 extent tree (lookups only so far).
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
        BlockMapKind::Extents => extent_lookup(state, inode, block_idx),
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
