//! Block-map seam: translates a file's logical block numbers to filesystem
//! blocks and manages the blocks that hold the mapping itself.
//!
//! Every caller in the fs crate goes through these dispatchers rather than
//! touching `i_block` directly, so additional mapping formats (ext4 extent
//! trees, `docs/ext4_compat_plan.md` M1/M2) can be added without changing
//! the file, directory, or journal code.
//!
//! Backends:
//! - classic ext2/ext3 direct/indirect maps;
//! - ext4 extent trees. Lookups walk the tree directly. Changes load every
//!   leaf extent into a sorted in-memory list, edit the list (insert, split,
//!   convert unwritten, remove), merge neighbours, and write the tree back:
//!   up to four extents live in the inode, more get leaf (and if needed
//!   index) blocks, reusing the existing tree blocks first. That is simpler
//!   and easier to verify than Linux's in-place tree surgery, at the cost of
//!   rewriting the (small) tree on each change.

use alloc::vec::Vec;

use super::{
    ext2_alloc_block_goal, ext2_block_slice_mut_state, ext2_block_slice_state, ext2_free_block,
    ext2_inode_acl_sectors, ext2_journal, ext2_mark_dirty_block, ext2_report, ext4_csum,
    indirect_detach_block, indirect_ensure_block, indirect_free_all_blocks,
    indirect_lookup_state, indirect_recalc_block_count_512_state, read_u16_le, read_u32_le,
    write_u16_le, write_u32_le, Ext2Inode, Ext2JournalHandle, Ext2JournalPhaseKind, VfsState,
    EIO, ENOSPC, EXT2_JOURNAL_CREDITS_STEP_FREE_DIRECT,
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
/// Extents per in-inode root (60-byte `i_block`).
const EXTENT_ROOT_MAX: usize = 4;
/// Longest initialized / unwritten extent (Linux EXT_INIT_MAX_LEN /
/// EXT_UNWRITTEN_MAX_LEN).
const EXT_WRITTEN_MAX_LEN: u32 = 32768;
const EXT_UNWRITTEN_MAX_LEN: u32 = 32767;

/// One leaf extent: `len` blocks from logical `lblk` at physical `pblk`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Extent {
    lblk: u32,
    len: u32,
    pblk: u32,
    unwritten: bool,
}

fn inode_root(inode: &Ext2Inode) -> [u8; 60] {
    let mut root = [0u8; 60];
    for (i, word) in inode.blocks.iter().enumerate() {
        root[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    root
}

fn store_inode_root(inode: &mut Ext2Inode, root: &[u8; 60]) {
    for i in 0..15 {
        inode.blocks[i] =
            u32::from_le_bytes([root[i * 4], root[i * 4 + 1], root[i * 4 + 2], root[i * 4 + 3]]);
    }
}

/// `i_block` contents of an inode with an empty extent tree.
pub(super) fn empty_extent_root() -> [u32; 15] {
    let mut root = [0u8; 60];
    write_extent_header(&mut root, 0, EXTENT_ROOT_MAX, 0);
    let mut blocks = [0u32; 15];
    for i in 0..15 {
        blocks[i] =
            u32::from_le_bytes([root[i * 4], root[i * 4 + 1], root[i * 4 + 2], root[i * 4 + 3]]);
    }
    blocks
}

fn write_extent_header(node: &mut [u8], entries: usize, max: usize, depth: u16) {
    node[0..2].copy_from_slice(&ext4_csum::EXT4_EXTENT_MAGIC.to_le_bytes());
    node[2..4].copy_from_slice(&(entries as u16).to_le_bytes());
    node[4..6].copy_from_slice(&(max as u16).to_le_bytes());
    node[6..8].copy_from_slice(&depth.to_le_bytes());
    node[8..12].copy_from_slice(&0u32.to_le_bytes()); // eh_generation
}

fn write_leaf_entry(node: &mut [u8], slot: usize, ext: &Extent) -> Result<(), i64> {
    let at = EXTENT_HEADER_SIZE + slot * EXTENT_ENTRY_SIZE;
    let raw_len = if ext.unwritten { ext.len + EXT_INIT_MAX_LEN } else { ext.len };
    write_u32_le(node, at, ext.lblk)?;
    write_u16_le(node, at + 4, raw_len as u16)?;
    write_u16_le(node, at + 6, 0)?; // ee_start_hi
    write_u32_le(node, at + 8, ext.pblk)
}

fn write_index_entry(node: &mut [u8], slot: usize, first_lblk: u32, child: u32) -> Result<(), i64> {
    let at = EXTENT_HEADER_SIZE + slot * EXTENT_ENTRY_SIZE;
    write_u32_le(node, at, first_lblk)?;
    write_u32_le(node, at + 4, child)?; // ei_leaf_lo
    write_u16_le(node, at + 8, 0)?; // ei_leaf_hi
    write_u16_le(node, at + 10, 0) // ei_unused
}

/// Collects every leaf extent (in logical order) and every tree block.
fn load_extents(state: &mut VfsState, inode: &Ext2Inode) -> Result<(Vec<Extent>, Vec<u32>), i64> {
    let seed = state
        .ext4_csum
        .filter(|csum| csum.metadata_csum)
        .map(|csum| csum.inode_seed_for(inode.ino, inode.generation));
    let mut extents = Vec::new();
    let mut tree = Vec::new();
    // (node bytes, expected depth); the root is checked by parse below.
    let root = inode_root(inode);
    let mut pending: Vec<(Option<u32>, Option<u16>)> = Vec::new();
    pending.push((None, None));
    while let Some((block, expected_depth)) = pending.pop() {
        let mut children: Vec<(u32, u16)> = Vec::new();
        let mut csum_ok = true;
        {
            let node: &[u8] = match block {
                None => &root,
                Some(b) => ext2_block_slice_state(state, b)?,
            };
            if let (Some(seed), Some(_)) = (seed, block) {
                csum_ok = ext4_csum::extent_block_csum_ok(seed, node)?;
            }
            if csum_ok {
                if node.len() < EXTENT_HEADER_SIZE
                    || read_u16_le(node, 0)? != ext4_csum::EXT4_EXTENT_MAGIC
                {
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
                for i in 0..entries {
                    let at = EXTENT_HEADER_SIZE + i * EXTENT_ENTRY_SIZE;
                    if depth > 0 {
                        let child = read_u32_le(node, at + 4)?;
                        if read_u16_le(node, at + 8)? != 0 || child == 0 || depth == 0 {
                            return Err(EIO);
                        }
                        children.push((child, depth - 1));
                    } else {
                        let lblk = read_u32_le(node, at)?;
                        let mut len = read_u16_le(node, at + 4)? as u32;
                        let unwritten = len > EXT_INIT_MAX_LEN;
                        if unwritten {
                            len -= EXT_INIT_MAX_LEN;
                        }
                        if read_u16_le(node, at + 6)? != 0 || len == 0 {
                            return Err(EIO);
                        }
                        let pblk = read_u32_le(node, at + 8)?;
                        extents.try_reserve(1).map_err(|_| ENOSPC)?;
                        extents.push(Extent { lblk, len, pblk, unwritten });
                    }
                }
            }
        }
        if !csum_ok {
            ext2_report(state, "extent block checksum", block.unwrap_or(0) as u64);
            return Err(EIO);
        }
        if let Some(b) = block {
            tree.try_reserve(1).map_err(|_| ENOSPC)?;
            tree.push(b);
        }
        // Depth-first, keeping logical order: push children in reverse.
        for &(child, depth) in children.iter().rev() {
            if tree.len() + pending.len() > 4096 {
                return Err(EIO);
            }
            pending.try_reserve(1).map_err(|_| ENOSPC)?;
            pending.push((Some(child), Some(depth)));
        }
    }
    extents.sort_unstable_by_key(|e| e.lblk);
    for pair in extents.windows(2) {
        if pair[0].lblk.checked_add(pair[0].len).map_or(true, |end| end > pair[1].lblk) {
            return Err(EIO);
        }
    }
    Ok((extents, tree))
}

/// Merges logically and physically adjacent extents of the same kind.
fn merge_extents(extents: &mut Vec<Extent>) {
    let mut out: Vec<Extent> = Vec::with_capacity(extents.len());
    for ext in extents.iter() {
        if let Some(last) = out.last_mut() {
            let limit = if ext.unwritten { EXT_UNWRITTEN_MAX_LEN } else { EXT_WRITTEN_MAX_LEN };
            if last.unwritten == ext.unwritten
                && last.lblk + last.len == ext.lblk
                && last.pblk + last.len == ext.pblk
                && last.len + ext.len <= limit
            {
                last.len += ext.len;
                continue;
            }
        }
        out.push(*ext);
    }
    *extents = out;
}

/// Tree blocks a list of `n` extents needs (0 when it fits in the inode).
fn tree_blocks_for(n: usize, per_block: usize) -> usize {
    if n <= EXTENT_ROOT_MAX {
        return 0;
    }
    let mut total = 0usize;
    let mut nodes = (n + per_block - 1) / per_block;
    loop {
        total += nodes;
        if nodes <= EXTENT_ROOT_MAX {
            return total;
        }
        nodes = (nodes + per_block - 1) / per_block;
    }
}

/// Writes `extents` back as a tree, reusing `old_tree` blocks first,
/// allocating any extra tree blocks, and freeing leftovers.
fn store_extents(
    state: &mut VfsState,
    handle: Option<Ext2JournalHandle>,
    inode: &mut Ext2Inode,
    extents: &[Extent],
    old_tree: Vec<u32>,
) -> Result<(), i64> {
    let block_size = state.ext2_meta.block_size;
    let per_block = (block_size - EXTENT_HEADER_SIZE) / EXTENT_ENTRY_SIZE;
    let mut root = [0u8; 60];
    let mut pool = old_tree;
    pool.reverse(); // pop() hands blocks back in their original order
    let goal = extents.first().map_or(0, |e| e.pblk);

    if extents.len() <= EXTENT_ROOT_MAX {
        write_extent_header(&mut root, extents.len(), EXTENT_ROOT_MAX, 0);
        for (slot, ext) in extents.iter().enumerate() {
            write_leaf_entry(&mut root, slot, ext)?;
        }
    } else {
        let seed = state
            .ext4_csum
            .filter(|csum| csum.metadata_csum)
            .map(|csum| csum.inode_seed_for(inode.ino, inode.generation));
        // (first logical block, physical block) of each node on the level.
        let mut level: Vec<(u32, u32)> = Vec::new();
        for chunk in extents.chunks(per_block) {
            let block = match pool.pop() {
                Some(b) => b,
                None => ext2_alloc_block_goal(state, handle, goal)?,
            };
            {
                let node = ext2_block_slice_mut_state(state, block)?;
                node.fill(0);
                write_extent_header(node, chunk.len(), per_block, 0);
                for (slot, ext) in chunk.iter().enumerate() {
                    write_leaf_entry(node, slot, ext)?;
                }
                if let Some(seed) = seed {
                    set_extent_tail(node, per_block, seed);
                }
            }
            ext2_mark_dirty_block(state, block)?;
            level.push((chunk[0].lblk, block));
        }
        let mut depth = 1u16;
        while level.len() > EXTENT_ROOT_MAX {
            let mut next = Vec::new();
            for chunk in level.chunks(per_block) {
                let block = match pool.pop() {
                    Some(b) => b,
                    None => ext2_alloc_block_goal(state, handle, goal)?,
                };
                {
                    let node = ext2_block_slice_mut_state(state, block)?;
                    node.fill(0);
                    write_extent_header(node, chunk.len(), per_block, depth);
                    for (slot, &(first, child)) in chunk.iter().enumerate() {
                        write_index_entry(node, slot, first, child)?;
                    }
                    if let Some(seed) = seed {
                        set_extent_tail(node, per_block, seed);
                    }
                }
                ext2_mark_dirty_block(state, block)?;
                next.push((chunk[0].0, block));
            }
            level = next;
            depth += 1;
            if depth > EXT4_MAX_EXTENT_DEPTH {
                return Err(ENOSPC);
            }
        }
        write_extent_header(&mut root, level.len(), EXTENT_ROOT_MAX, depth);
        for (slot, &(first, child)) in level.iter().enumerate() {
            write_index_entry(&mut root, slot, first, child)?;
        }
    }
    for block in pool {
        ext2_free_block(state, handle, block)?;
    }
    store_inode_root(inode, &root);
    Ok(())
}

/// metadata_csum: `ext4_extent_tail` after `max` entries.
fn set_extent_tail(node: &mut [u8], max: usize, seed: u32) {
    let tail = EXTENT_HEADER_SIZE + max * EXTENT_ENTRY_SIZE;
    if tail + 4 <= node.len() {
        let csum = ext4_csum::crc32c(seed, &node[..tail]);
        node[tail..tail + 4].copy_from_slice(&csum.to_le_bytes());
    }
}

/// Index of the extent containing `lblk`, if any.
fn find_extent(extents: &[Extent], lblk: u32) -> Option<usize> {
    extents.iter().position(|e| lblk >= e.lblk && lblk - e.lblk < e.len)
}

/// Splits `extents[idx]` around `lblk`, returning the one-block piece.
fn split_out(extents: &mut Vec<Extent>, idx: usize, lblk: u32) -> Extent {
    let ext = extents.remove(idx);
    let offset = lblk - ext.lblk;
    let mut insert_at = idx;
    if offset > 0 {
        extents.insert(insert_at, Extent { len: offset, ..ext });
        insert_at += 1;
    }
    let piece = Extent { lblk, len: 1, pblk: ext.pblk + offset, unwritten: ext.unwritten };
    if offset + 1 < ext.len {
        extents.insert(
            insert_at,
            Extent {
                lblk: lblk + 1,
                len: ext.len - offset - 1,
                pblk: ext.pblk + offset + 1,
                unwritten: ext.unwritten,
            },
        );
    }
    piece
}

fn extent_ensure(
    state: &mut VfsState,
    handle: Option<Ext2JournalHandle>,
    inode: &mut Ext2Inode,
    block_idx: usize,
) -> Result<u32, i64> {
    let lblk = u32::try_from(block_idx).map_err(|_| EIO)?;
    let (mut extents, tree) = load_extents(state, inode)?;
    if let Some(idx) = find_extent(&extents, lblk) {
        let ext = extents[idx];
        let pblk = ext.pblk + (lblk - ext.lblk);
        if !ext.unwritten {
            return Ok(pblk);
        }
        // Writing into a preallocated (unwritten) extent: the block becomes
        // initialized, and its stale on-disk contents must read as zeros.
        let mut piece = split_out(&mut extents, idx, lblk);
        piece.unwritten = false;
        let at = extents.iter().position(|e| e.lblk > lblk).unwrap_or(extents.len());
        extents.insert(at, piece);
        merge_extents(&mut extents);
        {
            let block = ext2_block_slice_mut_state(state, pblk)?;
            block.fill(0);
        }
        ext2_mark_dirty_block(state, pblk)?;
        store_extents(state, handle, inode, &extents, tree)?;
        return Ok(pblk);
    }
    // Allocate right after the logically preceding extent when possible so
    // sequential writes extend one extent instead of creating new ones.
    let goal = extents
        .iter()
        .rev()
        .find(|e| e.lblk < lblk)
        .map_or(0, |e| e.pblk.saturating_add(lblk - e.lblk));
    let pblk = ext2_alloc_block_goal(state, handle, goal)?;
    let at = extents.iter().position(|e| e.lblk > lblk).unwrap_or(extents.len());
    extents.insert(at, Extent { lblk, len: 1, pblk, unwritten: false });
    merge_extents(&mut extents);
    store_extents(state, handle, inode, &extents, tree)?;
    Ok(pblk)
}

fn extent_detach(
    state: &mut VfsState,
    handle: Option<Ext2JournalHandle>,
    inode: &mut Ext2Inode,
    block_idx: usize,
) -> Result<u32, i64> {
    let lblk = u32::try_from(block_idx).map_err(|_| EIO)?;
    let (mut extents, tree) = load_extents(state, inode)?;
    let idx = match find_extent(&extents, lblk) {
        Some(idx) => idx,
        None => return Ok(0),
    };
    let piece = split_out(&mut extents, idx, lblk);
    store_extents(state, handle, inode, &extents, tree)?;
    Ok(piece.pblk)
}

fn extent_free_all(
    state: &mut VfsState,
    handle: Option<Ext2JournalHandle>,
    inode: &mut Ext2Inode,
    phased: bool,
) -> Result<(), i64> {
    let (extents, tree) = load_extents(state, inode)?;
    for ext in &extents {
        for i in 0..ext.len {
            let block = ext.pblk + i;
            if phased {
                ext2_journal::with_handle_restartable_phase(
                    state,
                    handle,
                    Ext2JournalPhaseKind::FreeBlockBookkeeping,
                    EXT2_JOURNAL_CREDITS_STEP_FREE_DIRECT,
                    |state| ext2_free_block(state, handle, block),
                )?;
            } else {
                ext2_free_block(state, handle, block)?;
            }
        }
    }
    for block in tree {
        ext2_free_block(state, handle, block)?;
    }
    inode.blocks = empty_extent_root();
    Ok(())
}

fn extent_recalc_block_count_512(state: &mut VfsState, inode: &Ext2Inode) -> Result<u64, i64> {
    let (extents, tree) = load_extents(state, inode)?;
    let mut blocks = tree.len() as u64;
    for ext in &extents {
        blocks += ext.len as u64;
    }
    Ok(blocks * (state.ext2_meta.block_size as u64 / 512)
        + ext2_inode_acl_sectors(&state.ext2_meta, inode) as u64)
}

/// Journal credits one extent mapping change may use: rewriting every tree
/// block plus up to one new level, each new block's allocation bookkeeping
/// (bitmap, descriptor, superblock), the data block, and the inode.
pub(super) fn extent_step_credits(state: &mut VfsState, inode: &Ext2Inode) -> Result<usize, i64> {
    let (extents, tree) = load_extents(state, inode)?;
    let per_block = (state.ext2_meta.block_size - EXTENT_HEADER_SIZE) / EXTENT_ENTRY_SIZE;
    let after = tree_blocks_for(extents.len() + 2, per_block);
    let tree_blocks = core::cmp::max(tree.len(), after);
    Ok(tree_blocks * 5 + 10)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BlockMapKind {
    /// Twelve direct pointers plus single/double/triple indirect tables.
    Indirect,
    /// ext4 extent tree.
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
        BlockMapKind::Extents => extent_ensure(state, journal_handle, inode, block_idx),
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
        BlockMapKind::Extents => extent_detach(state, journal_handle, inode, block_idx),
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
        BlockMapKind::Extents => extent_free_all(state, journal_handle, inode, phased),
    }
}

/// First mapped logical block at or after `from` (holes are skipped
/// without visiting each block), or `None` past the last mapped block.
pub(super) fn ext2_next_mapped_block(
    state: &mut VfsState,
    inode: &Ext2Inode,
    from: usize,
) -> Result<Option<usize>, i64> {
    match block_map_kind(inode) {
        BlockMapKind::Indirect => super::indirect_next_mapped(state, inode, from),
        BlockMapKind::Extents => {
            let (extents, _) = load_extents(state, inode)?;
            let mut best: Option<usize> = None;
            for ext in &extents {
                let start = ext.lblk as usize;
                let end = start + ext.len as usize;
                if end > from {
                    let cand = core::cmp::max(start, from);
                    best = Some(best.map_or(cand, |b| core::cmp::min(b, cand)));
                }
            }
            Ok(best)
        }
    }
}

/// `i_blocks` (512-byte sectors) implied by the inode's map, including
/// mapping blocks and any external xattr block.
pub(super) fn ext2_recalc_inode_block_count_512_state(
    state: &mut VfsState,
    inode: &Ext2Inode,
) -> Result<u64, i64> {
    match block_map_kind(inode) {
        BlockMapKind::Indirect => indirect_recalc_block_count_512_state(state, inode),
        BlockMapKind::Extents => extent_recalc_block_count_512(state, inode),
    }
}
