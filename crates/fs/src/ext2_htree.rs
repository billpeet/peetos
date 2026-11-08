//! ext3/ext4 hashed directory indexes (htree, `EXT2_INDEX_FL`).
//!
//! An indexed directory keeps "." / ".." plus a `dx_root` in logical block
//! 0; the root's sorted `(hash, block)` entries point at leaf blocks (or, with
//! `indirect_levels` = 1, at `dx_node` blocks that point at leaves). Each leaf
//! holds ordinary dirents whose name hashes fall in `[entry hash, next entry
//! hash)`. This module implements the Linux/e2fsprogs name hashes, index
//! lookup, insertion with leaf and index splits and depth growth, conversion
//! of a full one-block directory into an index (`make_indexed_dir`), and the
//! `metadata_csum` `dx_tail` checksum.

use alloc::vec::Vec;

use super::{
    ext2_block_slice_mut_state, ext2_block_slice_state, ext2_data_block_for_logical_index_state,
    ext2_dir_block_finish, ext2_dir_tail_len, ext2_ensure_data_block, ext2_mark_dirty_block,
    ext2_recalc_inode_block_count_512_state, ext2_report, ext2_write_inode_to_state, ext4_csum,
    read_u16_le, read_u32_le, write_u16_le, write_u32_le, Ext2Inode, Ext2JournalHandle,
    VfsState, EIO, ENOSPC, EXT2_INDEX_FL,
};

pub(super) const DX_HASH_LEGACY: u8 = 0;
pub(super) const DX_HASH_HALF_MD4: u8 = 1;
pub(super) const DX_HASH_TEA: u8 = 2;
pub(super) const DX_HASH_LEGACY_UNSIGNED: u8 = 3;
pub(super) const DX_HASH_HALF_MD4_UNSIGNED: u8 = 4;
pub(super) const DX_HASH_TEA_UNSIGNED: u8 = 5;

/// `s_flags` bits selecting how name bytes enter the hash.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) const EXT2_FLAGS_SIGNED_HASH: u32 = 0x0001;
pub(super) const EXT2_FLAGS_UNSIGNED_HASH: u32 = 0x0002;

const DX_ROOT_COUNT_OFFSET: usize = 0x20;
const DX_NODE_COUNT_OFFSET: usize = 8;
const DX_TAIL_SIZE: usize = 8;

// ---------------------------------------------------------------------------
// Name hashes (lib/ext2fs/dirhash.c, fs/ext4/hash.c)
// ---------------------------------------------------------------------------

fn tea_transform(buf: &mut [u32; 4], input: &[u32]) {
    let mut sum = 0u32;
    let (mut b0, mut b1) = (buf[0], buf[1]);
    let (a, b, c, d) = (input[0], input[1], input[2], input[3]);
    for _ in 0..16 {
        sum = sum.wrapping_add(0x9E37_79B9);
        b0 = b0.wrapping_add(
            (b1 << 4).wrapping_add(a) ^ b1.wrapping_add(sum) ^ (b1 >> 5).wrapping_add(b),
        );
        b1 = b1.wrapping_add(
            (b0 << 4).wrapping_add(c) ^ b0.wrapping_add(sum) ^ (b0 >> 5).wrapping_add(d),
        );
    }
    buf[0] = buf[0].wrapping_add(b0);
    buf[1] = buf[1].wrapping_add(b1);
}

fn half_md4_transform(buf: &mut [u32; 4], input: &[u32]) {
    fn f(x: u32, y: u32, z: u32) -> u32 {
        z ^ (x & (y ^ z))
    }
    fn g(x: u32, y: u32, z: u32) -> u32 {
        (x & y).wrapping_add((x ^ y) & z)
    }
    fn h(x: u32, y: u32, z: u32) -> u32 {
        x ^ y ^ z
    }
    const K2: u32 = 0o13240474631;
    const K3: u32 = 0o15666365641;
    let (mut a, mut b, mut c, mut d) = (buf[0], buf[1], buf[2], buf[3]);
    macro_rules! round {
        ($f:ident, $a:ident, $b:ident, $c:ident, $d:ident, $x:expr, $s:expr) => {
            $a = $a.wrapping_add($f($b, $c, $d)).wrapping_add($x);
            $a = $a.rotate_left($s);
        };
    }
    let x = input;
    round!(f, a, b, c, d, x[0], 3);
    round!(f, d, a, b, c, x[1], 7);
    round!(f, c, d, a, b, x[2], 11);
    round!(f, b, c, d, a, x[3], 19);
    round!(f, a, b, c, d, x[4], 3);
    round!(f, d, a, b, c, x[5], 7);
    round!(f, c, d, a, b, x[6], 11);
    round!(f, b, c, d, a, x[7], 19);

    round!(g, a, b, c, d, x[1].wrapping_add(K2), 3);
    round!(g, d, a, b, c, x[3].wrapping_add(K2), 5);
    round!(g, c, d, a, b, x[5].wrapping_add(K2), 9);
    round!(g, b, c, d, a, x[7].wrapping_add(K2), 13);
    round!(g, a, b, c, d, x[0].wrapping_add(K2), 3);
    round!(g, d, a, b, c, x[2].wrapping_add(K2), 5);
    round!(g, c, d, a, b, x[4].wrapping_add(K2), 9);
    round!(g, b, c, d, a, x[6].wrapping_add(K2), 13);

    round!(h, a, b, c, d, x[3].wrapping_add(K3), 3);
    round!(h, d, a, b, c, x[7].wrapping_add(K3), 9);
    round!(h, c, d, a, b, x[2].wrapping_add(K3), 11);
    round!(h, b, c, d, a, x[6].wrapping_add(K3), 15);
    round!(h, a, b, c, d, x[1].wrapping_add(K3), 3);
    round!(h, d, a, b, c, x[5].wrapping_add(K3), 9);
    round!(h, c, d, a, b, x[0].wrapping_add(K3), 11);
    round!(h, b, c, d, a, x[4].wrapping_add(K3), 15);

    buf[0] = buf[0].wrapping_add(a);
    buf[1] = buf[1].wrapping_add(b);
    buf[2] = buf[2].wrapping_add(c);
    buf[3] = buf[3].wrapping_add(d);
}

fn name_byte(b: u8, unsigned: bool) -> u32 {
    if unsigned {
        b as u32
    } else {
        b as i8 as i32 as u32
    }
}

fn dx_hack_hash(name: &[u8], unsigned: bool) -> u32 {
    let (mut hash0, mut hash1) = (0x12a3_fe2du32, 0x37ab_e8f9u32);
    for &b in name {
        let c = name_byte(b, unsigned);
        let mut hash = hash1.wrapping_add(hash0 ^ c.wrapping_mul(7_152_373));
        if (hash & 0x8000_0000) != 0 {
            hash = hash.wrapping_sub(0x7fff_ffff);
        }
        hash1 = hash0;
        hash0 = hash;
    }
    hash0 << 1
}

fn str2hashbuf(msg: &[u8], out: &mut [u32], unsigned: bool) {
    let num = out.len();
    let mut pad = (msg.len() as u32) | ((msg.len() as u32) << 8);
    pad |= pad << 16;
    let mut val = pad;
    let len = core::cmp::min(msg.len(), num * 4);
    let mut written = 0usize;
    for (i, &b) in msg[..len].iter().enumerate() {
        val = name_byte(b, unsigned).wrapping_add(val << 8);
        if (i % 4) == 3 {
            out[written] = val;
            written += 1;
            val = pad;
        }
    }
    if written < num {
        out[written] = val;
        written += 1;
    }
    while written < num {
        out[written] = pad;
        written += 1;
    }
}

/// `(major, minor)` directory hash of `name`; `None` for an unknown version.
/// The major hash has bit 0 clear (bit 0 marks hash-collision continuation
/// in index entries).
pub(super) fn dx_hash(version: u8, seed: &[u32; 4], name: &[u8]) -> Option<(u32, u32)> {
    let mut buf = [0x6745_2301u32, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    if seed.iter().any(|&w| w != 0) {
        buf = *seed;
    }
    let (hash, minor) = match version {
        DX_HASH_LEGACY | DX_HASH_LEGACY_UNSIGNED => {
            (dx_hack_hash(name, version == DX_HASH_LEGACY_UNSIGNED), 0)
        }
        DX_HASH_HALF_MD4 | DX_HASH_HALF_MD4_UNSIGNED => {
            let unsigned = version == DX_HASH_HALF_MD4_UNSIGNED;
            let mut input = [0u32; 8];
            let mut p = name;
            loop {
                str2hashbuf(p, &mut input, unsigned);
                half_md4_transform(&mut buf, &input);
                if p.len() <= 32 {
                    break;
                }
                p = &p[32..];
            }
            (buf[1], buf[2])
        }
        DX_HASH_TEA | DX_HASH_TEA_UNSIGNED => {
            let unsigned = version == DX_HASH_TEA_UNSIGNED;
            let mut input = [0u32; 4];
            let mut p = name;
            loop {
                str2hashbuf(p, &mut input, unsigned);
                tea_transform(&mut buf, &input);
                if p.len() <= 16 {
                    break;
                }
                p = &p[16..];
            }
            (buf[0], buf[1])
        }
        _ => return None,
    };
    let mut hash = hash & !1;
    // fs/ext4/hash.c: the largest value is reserved as the 32-bit EOF cookie.
    if hash == (0x7fff_ffff << 1) {
        hash = (0x7fff_ffff - 1) << 1;
    }
    Some((hash, minor))
}

// ---------------------------------------------------------------------------
// On-disk index layout
// ---------------------------------------------------------------------------

fn csum_enabled(state: &VfsState) -> bool {
    state.ext4_csum.map_or(false, |c| c.metadata_csum)
}

fn root_limit(state: &VfsState) -> usize {
    let bs = state.ext2_meta.block_size;
    let tail = if csum_enabled(state) { DX_TAIL_SIZE } else { 0 };
    (bs - DX_ROOT_COUNT_OFFSET - tail) / 8
}

fn node_limit(state: &VfsState) -> usize {
    let bs = state.ext2_meta.block_size;
    let tail = if csum_enabled(state) { DX_TAIL_SIZE } else { 0 };
    (bs - DX_NODE_COUNT_OFFSET - tail) / 8
}

/// Index levels below the root that a directory may use (2 total levels,
/// as Linux without `largedir`).
const MAX_INDIRECT_LEVELS: u8 = 1;

/// Where a block's `dx_countlimit` sits if it is an index block: 0x20 for a
/// dx root, 8 for a dx node (`get_dx_countlimit` in fs/ext4/namei.c).
pub(super) fn dx_count_offset(block: &[u8]) -> Option<usize> {
    let bs = block.len();
    let rec_len = read_u16_le(block, 4).ok()? as usize;
    if rec_len == bs {
        return Some(DX_NODE_COUNT_OFFSET);
    }
    if rec_len != 12 {
        return None;
    }
    let dotdot_len = read_u16_le(block, 12 + 4).ok()? as usize;
    if dotdot_len != bs - 12 {
        return None;
    }
    if read_u32_le(block, 0x18).ok()? != 0 || block[0x1D] != 8 {
        return None;
    }
    Some(DX_ROOT_COUNT_OFFSET)
}

fn dx_csum_value(seed: u32, block: &[u8], count_offset: usize, count: usize, limit: usize) -> u32 {
    let size = count_offset + count * 8;
    let tail = count_offset + limit * 8;
    // ext4_dx_csum: entries, then dt_reserved, then a zeroed dt_checksum.
    let csum = ext4_csum::crc32c(seed, &block[..size]);
    let csum = ext4_csum::crc32c(csum, &block[tail..tail + 4]);
    ext4_csum::crc32c(csum, &[0u8; 4])
}

/// `metadata_csum` check of an index block; `None` when `block` is not one
/// (or its count/limit do not leave room for a `dx_tail`).
pub(super) fn dx_csum_ok(seed: u32, block: &[u8]) -> Option<bool> {
    let count_offset = dx_count_offset(block)?;
    let limit = read_u16_le(block, count_offset).ok()? as usize;
    let count = read_u16_le(block, count_offset + 2).ok()? as usize;
    if count > limit || count_offset + limit * 8 + DX_TAIL_SIZE > block.len() {
        return Some(false);
    }
    let tail = count_offset + limit * 8;
    let stored = read_u32_le(block, tail + 4).ok()?;
    Some(stored == dx_csum_value(seed, block, count_offset, count, limit))
}

/// After an index block changes: refreshes its `dx_tail` checksum
/// (metadata_csum) and marks it dirty.
fn dx_block_finish(
    state: &mut VfsState,
    dir: &Ext2Inode,
    block: u32,
    count_offset: usize,
) -> Result<(), i64> {
    if let Some(csum) = state.ext4_csum.filter(|c| c.metadata_csum) {
        let seed = csum.inode_seed_for(dir.ino, dir.generation);
        let blk = ext2_block_slice_mut_state(state, block)?;
        let limit = read_u16_le(blk, count_offset)? as usize;
        let count = read_u16_le(blk, count_offset + 2)? as usize;
        let tail = count_offset + limit * 8;
        if tail + DX_TAIL_SIZE > blk.len() || count > limit {
            return Err(EIO);
        }
        write_u32_le(blk, tail, 0)?;
        let value = dx_csum_value(seed, blk, count_offset, count, limit);
        write_u32_le(blk, tail + 4, value)?;
    }
    ext2_mark_dirty_block(state, block)
}

fn entry_hash(block: &[u8], count_offset: usize, idx: usize) -> Result<u32, i64> {
    if idx == 0 {
        return Ok(0);
    }
    read_u32_le(block, count_offset + idx * 8)
}

fn entry_block(block: &[u8], count_offset: usize, idx: usize) -> Result<u32, i64> {
    Ok(read_u32_le(block, count_offset + idx * 8 + 4)? & 0x0fff_ffff)
}

/// One level of an index walk.
#[derive(Clone, Copy)]
struct Frame {
    phys: u32,
    count_offset: usize,
    count: usize,
    limit: usize,
    /// Entry followed at this level.
    at: usize,
}

/// Why an index cannot be used. `Corrupt` is a checksum failure (`EIO`);
/// `Unusable` means "treat the directory as linear", as Linux does when
/// `dx_probe` rejects an index it does not understand.
pub(super) enum DxError {
    Unusable,
    Io(i64),
}

impl From<i64> for DxError {
    fn from(err: i64) -> Self {
        DxError::Io(err)
    }
}

struct Probe {
    version: u8,
    levels: u8,
    frames: [Frame; 3],
    depth: usize,
    leaf_lblk: u32,
}

fn dir_block_count(state: &VfsState, dir: &Ext2Inode) -> usize {
    (dir.size as usize) / state.ext2_meta.block_size
}

fn dir_seed(state: &VfsState, dir: &Ext2Inode) -> Option<u32> {
    state
        .ext4_csum
        .filter(|c| c.metadata_csum)
        .map(|c| c.inode_seed_for(dir.ino, dir.generation))
}

fn hash_version_for(state: &VfsState, root_version: u8) -> u8 {
    if root_version <= DX_HASH_TEA && state.ext2_meta.hash_unsigned {
        root_version + 3
    } else {
        root_version
    }
}

fn verify_index_block(state: &mut VfsState, dir: &Ext2Inode, phys: u32) -> Result<(), DxError> {
    if let Some(seed) = dir_seed(state, dir) {
        let ok = {
            let blk = ext2_block_slice_state(state, phys)?;
            dx_csum_ok(seed, blk)
        };
        match ok {
            Some(true) => {}
            Some(false) => {
                ext2_report(state, "htree index checksum", phys as u64);
                return Err(DxError::Io(EIO));
            }
            None => return Err(DxError::Unusable),
        }
    }
    Ok(())
}

/// Walks the index of `dir` to the leaf that should hold `hash`
/// (`dx_probe`). `hash` is `None` to only read the root's hash version.
fn probe(state: &mut VfsState, dir: &Ext2Inode, name: &[u8]) -> Result<(Probe, u32), DxError> {
    let nblocks = dir_block_count(state, dir);
    if (dir.flags & EXT2_INDEX_FL) == 0 || nblocks < 2 {
        return Err(DxError::Unusable);
    }
    let root_phys = ext2_data_block_for_logical_index_state(state, dir, 0)?;
    if root_phys == 0 {
        return Err(DxError::Unusable);
    }
    let (root_version, levels) = {
        let blk = ext2_block_slice_state(state, root_phys)?;
        if dx_count_offset(blk) != Some(DX_ROOT_COUNT_OFFSET) {
            return Err(DxError::Unusable);
        }
        (blk[0x1C], blk[0x1E])
    };
    if root_version > DX_HASH_TEA || levels > MAX_INDIRECT_LEVELS {
        return Err(DxError::Unusable);
    }
    let version = hash_version_for(state, root_version);
    let seed = state.ext2_meta.hash_seed;
    let (hash, _) = dx_hash(version, &seed, name).ok_or(DxError::Unusable)?;

    let mut probe = Probe {
        version,
        levels,
        frames: [Frame { phys: 0, count_offset: 0, count: 0, limit: 0, at: 0 }; 3],
        depth: 0,
        leaf_lblk: 0,
    };
    let mut phys = root_phys;
    let mut count_offset = DX_ROOT_COUNT_OFFSET;
    let mut expected_limit = root_limit(state);
    for level in 0..=levels as usize {
        verify_index_block(state, dir, phys)?;
        let (count, limit, at, next) = {
            let blk = ext2_block_slice_state(state, phys)?;
            if level > 0 && dx_count_offset(blk) != Some(DX_NODE_COUNT_OFFSET) {
                return Err(DxError::Unusable);
            }
            let limit = read_u16_le(blk, count_offset)? as usize;
            let count = read_u16_le(blk, count_offset + 2)? as usize;
            if limit != expected_limit || count == 0 || count > limit {
                return Err(DxError::Unusable);
            }
            // Last entry whose hash is <= `hash` (entry 0 covers from 0).
            let (mut lo, mut hi) = (1usize, count);
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                if entry_hash(blk, count_offset, mid)? > hash {
                    hi = mid;
                } else {
                    lo = mid + 1;
                }
            }
            let at = lo - 1;
            (count, limit, at, entry_block(blk, count_offset, at)?)
        };
        if next as usize >= nblocks || next == 0 {
            return Err(DxError::Unusable);
        }
        probe.frames[level] = Frame { phys, count_offset, count, limit, at };
        probe.depth = level + 1;
        if level == levels as usize {
            probe.leaf_lblk = next;
        } else {
            phys = ext2_data_block_for_logical_index_state(state, dir, next as usize)?;
            if phys == 0 {
                return Err(DxError::Unusable);
            }
            count_offset = DX_NODE_COUNT_OFFSET;
            expected_limit = node_limit(state);
        }
    }
    Ok((probe, hash))
}

/// Moves `probe` to the next leaf in hash order (`ext4_htree_next_block`);
/// returns the hash that starts it, or `None` at the end of the index.
fn next_leaf(state: &mut VfsState, dir: &Ext2Inode, probe: &mut Probe) -> Result<Option<u32>, DxError> {
    let mut level = probe.depth;
    loop {
        if level == 0 {
            return Ok(None);
        }
        level -= 1;
        let f = probe.frames[level];
        if f.at + 1 < f.count {
            probe.frames[level].at += 1;
            break;
        }
    }
    let start_hash = {
        let f = probe.frames[level];
        let blk = ext2_block_slice_state(state, f.phys)?;
        entry_hash(blk, f.count_offset, f.at)?
    };
    // Descend along the first entries of the lower levels.
    while level + 1 < probe.depth {
        let child = {
            let f = probe.frames[level];
            let blk = ext2_block_slice_state(state, f.phys)?;
            entry_block(blk, f.count_offset, f.at)?
        };
        let phys = ext2_data_block_for_logical_index_state(state, dir, child as usize)?;
        if phys == 0 {
            return Err(DxError::Unusable);
        }
        verify_index_block(state, dir, phys)?;
        // dx_countlimit = { limit: u16, count: u16 }.
        let (limit, count) = {
            let blk = ext2_block_slice_state(state, phys)?;
            (
                read_u16_le(blk, DX_NODE_COUNT_OFFSET)? as usize,
                read_u16_le(blk, DX_NODE_COUNT_OFFSET + 2)? as usize,
            )
        };
        if count == 0 || count > limit {
            return Err(DxError::Unusable);
        }
        level += 1;
        probe.frames[level] = Frame { phys, count_offset: DX_NODE_COUNT_OFFSET, count, limit, at: 0 };
    }
    let f = probe.frames[probe.depth - 1];
    probe.leaf_lblk = {
        let blk = ext2_block_slice_state(state, f.phys)?;
        entry_block(blk, f.count_offset, f.at)?
    };
    Ok(Some(start_hash))
}

fn leaf_phys_checked(state: &mut VfsState, dir: &Ext2Inode, lblk: u32) -> Result<u32, DxError> {
    let phys = ext2_data_block_for_logical_index_state(state, dir, lblk as usize)?;
    if phys == 0 {
        return Err(DxError::Unusable);
    }
    if let Some(seed) = dir_seed(state, dir) {
        let ok = {
            let blk = ext2_block_slice_state(state, phys)?;
            ext4_csum::dir_leaf_csum_ok(seed, blk)?
        };
        match ok {
            Some(true) => {}
            Some(false) => {
                ext2_report(state, "directory block checksum", phys as u64);
                return Err(DxError::Io(EIO));
            }
            None => return Err(DxError::Unusable),
        }
    }
    Ok(phys)
}

/// Searches one leaf block for `name`: `(inode, file type)`.
fn search_leaf(block: &[u8], tail: usize, name: &[u8]) -> Result<Option<(u32, u8)>, i64> {
    let usable = block.len() - tail;
    let mut pos = 0usize;
    while pos + 8 <= usable {
        let ino = read_u32_le(block, pos)?;
        let rec_len = read_u16_le(block, pos + 4)? as usize;
        let name_len = block[pos + 6] as usize;
        if rec_len < 8 || (rec_len & 3) != 0 || pos + rec_len > usable || 8 + name_len > rec_len {
            return Err(EIO);
        }
        if ino != 0 && &block[pos + 8..pos + 8 + name_len] == name {
            return Ok(Some((ino, block[pos + 7])));
        }
        pos += rec_len;
    }
    Ok(None)
}

/// Result of an indexed lookup: inode, dirent file type, and the physical
/// leaf block holding the entry.
pub(super) struct DxHit {
    pub ino: u32,
    pub file_type: u8,
    pub block: u32,
}

/// Finds `name` through the index (`ext4_dx_find_entry`): only the leaf
/// for its hash (and collision continuation leaves) are read.
pub(super) fn dx_find(
    state: &mut VfsState,
    dir: &Ext2Inode,
    name: &[u8],
) -> Result<Option<DxHit>, DxError> {
    let (mut probe, hash) = probe(state, dir, name)?;
    let tail = ext2_dir_tail_len(state);
    loop {
        let phys = leaf_phys_checked(state, dir, probe.leaf_lblk)?;
        #[cfg(test)]
        DX_LEAF_READS.with(|c| c.set(c.get() + 1));
        let found = {
            let blk = ext2_block_slice_state(state, phys)?;
            search_leaf(blk, tail, name)?
        };
        if let Some((ino, file_type)) = found {
            return Ok(Some(DxHit { ino, file_type, block: phys }));
        }
        match next_leaf(state, dir, &mut probe)? {
            Some(start) if (start & !1) == hash => continue,
            _ => return Ok(None),
        }
    }
}

#[cfg(test)]
std::thread_local! {
    /// Leaf blocks read by `dx_find` (tests check lookups use the index).
    pub(super) static DX_LEAF_READS: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}

// ---------------------------------------------------------------------------
// Insertion
// ---------------------------------------------------------------------------

fn dirent_len(name_len: usize) -> usize {
    (8 + name_len + 3) & !3
}

/// Inserts a dirent into a leaf if it fits; `false` when the block is full.
fn leaf_insert(
    block: &mut [u8],
    tail: usize,
    ino: u32,
    name: &[u8],
    ft: u8,
) -> Result<bool, i64> {
    let usable = block.len() - tail;
    let needed = dirent_len(name.len());
    let mut pos = 0usize;
    while pos + 8 <= usable {
        let entry_ino = read_u32_le(block, pos)?;
        let rec_len = read_u16_le(block, pos + 4)? as usize;
        if rec_len < 8 || (rec_len & 3) != 0 || pos + rec_len > usable {
            return Err(EIO);
        }
        let used = if entry_ino == 0 { 0 } else { dirent_len(block[pos + 6] as usize) };
        if rec_len >= used + needed {
            let at = if used == 0 {
                pos
            } else {
                write_u16_le(block, pos + 4, used as u16)?;
                pos + used
            };
            write_dirent_raw(block, at, ino, rec_len - used, ft, name)?;
            return Ok(true);
        }
        pos += rec_len;
    }
    Ok(false)
}

fn write_dirent_raw(
    block: &mut [u8],
    pos: usize,
    ino: u32,
    rec_len: usize,
    ft: u8,
    name: &[u8],
) -> Result<(), i64> {
    if rec_len < dirent_len(name.len()) || pos + rec_len > block.len() {
        return Err(EIO);
    }
    write_u32_le(block, pos, ino)?;
    write_u16_le(block, pos + 4, rec_len as u16)?;
    block[pos + 6] = name.len() as u8;
    block[pos + 7] = ft;
    block[pos + 8..pos + 8 + name.len()].copy_from_slice(name);
    block[pos + 8 + name.len()..pos + rec_len].fill(0);
    Ok(())
}

struct MapEntry {
    hash: u32,
    minor: u32,
    ino: u32,
    ft: u8,
    name: Vec<u8>,
}

fn collect_leaf(
    block: &[u8],
    tail: usize,
    version: u8,
    seed: &[u32; 4],
) -> Result<Vec<MapEntry>, i64> {
    let usable = block.len() - tail;
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 8 <= usable {
        let ino = read_u32_le(block, pos)?;
        let rec_len = read_u16_le(block, pos + 4)? as usize;
        let name_len = block[pos + 6] as usize;
        if rec_len < 8 || pos + rec_len > usable || 8 + name_len > rec_len {
            return Err(EIO);
        }
        if ino != 0 {
            let name = &block[pos + 8..pos + 8 + name_len];
            let (hash, minor) = dx_hash(version, seed, name).ok_or(EIO)?;
            let mut copy = Vec::new();
            copy.try_reserve_exact(name.len()).map_err(|_| ENOSPC)?;
            copy.extend_from_slice(name);
            out.try_reserve(1).map_err(|_| ENOSPC)?;
            out.push(MapEntry { hash, minor, ino, ft: block[pos + 7], name: copy });
        }
        pos += rec_len;
    }
    Ok(out)
}

/// Rewrites a leaf with `entries` packed from offset 0; the last entry's
/// rec_len reaches the tail.
fn write_leaf(block: &mut [u8], tail: usize, entries: &[MapEntry]) -> Result<(), i64> {
    let usable = block.len() - tail;
    block[..usable].fill(0);
    if entries.is_empty() {
        write_u32_le(block, 0, 0)?;
        write_u16_le(block, 4, usable as u16)?;
        return Ok(());
    }
    let mut pos = 0usize;
    for (i, e) in entries.iter().enumerate() {
        let len = dirent_len(e.name.len());
        let rec_len = if i + 1 == entries.len() { usable - pos } else { len };
        if pos + rec_len > usable {
            return Err(ENOSPC);
        }
        write_dirent_raw(block, pos, e.ino, rec_len, e.ft, &e.name)?;
        pos += len;
    }
    Ok(())
}

/// Inserts `(hash, lblk)` into an index block after entry `after`.
fn index_insert(
    state: &mut VfsState,
    dir: &Ext2Inode,
    f: &Frame,
    after: usize,
    hash: u32,
    lblk: u32,
) -> Result<(), i64> {
    {
        let blk = ext2_block_slice_mut_state(state, f.phys)?;
        let count = read_u16_le(blk, f.count_offset + 2)? as usize;
        if count >= f.limit {
            return Err(EIO);
        }
        let start = f.count_offset + (after + 1) * 8;
        let end = f.count_offset + count * 8;
        blk.copy_within(start..end, start + 8);
        write_u32_le(blk, start, hash)?;
        write_u32_le(blk, start + 4, lblk)?;
        write_u16_le(blk, f.count_offset + 2, (count + 1) as u16)?;
    }
    dx_block_finish(state, dir, f.phys, f.count_offset)
}

/// Appends a zeroed block to the directory; returns (logical, physical).
fn append_block(
    state: &mut VfsState,
    handle: Option<Ext2JournalHandle>,
    dir: &mut Ext2Inode,
) -> Result<(u32, u32), i64> {
    let bs = state.ext2_meta.block_size;
    let lblk = dir_block_count(state, dir);
    let phys = ext2_ensure_data_block(state, handle, dir, lblk)?;
    ext2_block_slice_mut_state(state, phys)?.fill(0);
    dir.size = ((lblk + 1) * bs) as u32;
    dir.block_count_512 = ext2_recalc_inode_block_count_512_state(state, dir)?;
    ext2_write_inode_to_state(state, dir.ino, dir)?;
    Ok((lblk as u32, phys))
}

/// Initializes an empty dx node block holding `entries` (raw 8-byte index
/// entries, the first one's hash slot is replaced by count/limit).
fn init_node(
    state: &mut VfsState,
    dir: &Ext2Inode,
    phys: u32,
    entries: &[u8],
) -> Result<(), i64> {
    let limit = node_limit(state);
    let count = entries.len() / 8;
    {
        let blk = ext2_block_slice_mut_state(state, phys)?;
        let bs = blk.len();
        blk.fill(0);
        write_u32_le(blk, 0, 0)?;
        write_u16_le(blk, 4, bs as u16)?;
        blk[DX_NODE_COUNT_OFFSET..DX_NODE_COUNT_OFFSET + entries.len()].copy_from_slice(entries);
        write_u16_le(blk, DX_NODE_COUNT_OFFSET, limit as u16)?;
        write_u16_le(blk, DX_NODE_COUNT_OFFSET + 2, count as u16)?;
    }
    dx_block_finish(state, dir, phys, DX_NODE_COUNT_OFFSET)
}

fn copy_entries(state: &mut VfsState, f: &Frame, from: usize, to: usize) -> Result<Vec<u8>, i64> {
    let blk = ext2_block_slice_state(state, f.phys)?;
    let mut raw = Vec::new();
    raw.try_reserve_exact((to - from) * 8).map_err(|_| ENOSPC)?;
    raw.extend_from_slice(&blk[f.count_offset + from * 8..f.count_offset + to * 8]);
    Ok(raw)
}

/// Adds a dirent to an indexed directory (`ext4_dx_add_entry`). The caller
/// has checked that `name` is not present. `Unusable` asks the caller to
/// fall back to linear mode.
pub(super) fn dx_add_entry(
    state: &mut VfsState,
    handle: Option<Ext2JournalHandle>,
    dir: &mut Ext2Inode,
    ino: u32,
    name: &[u8],
    ft: u8,
) -> Result<(), DxError> {
    let tail = ext2_dir_tail_len(state);
    let seed = state.ext2_meta.hash_seed;
    // Each pass either inserts or grows the index by one step.
    for _ in 0..8 {
        let (probe, hash) = probe(state, dir, name)?;
        let leaf = leaf_phys_checked(state, dir, probe.leaf_lblk)?;
        let inserted = {
            let blk = ext2_block_slice_mut_state(state, leaf)?;
            leaf_insert(blk, tail, ino, name, ft)?
        };
        if inserted {
            ext2_dir_block_finish(state, dir, leaf)?;
            return Ok(());
        }

        let bottom = probe.frames[probe.depth - 1];
        if bottom.count < bottom.limit {
            split_leaf(state, handle, dir, &probe, leaf, hash, tail, &seed, ino, name, ft)?;
            return Ok(());
        }

        // The bottom index block is full: split the lowest full block whose
        // parent has room, or add a level when every level is full.
        let mut k = probe.depth - 1;
        while k > 0 && probe.frames[k - 1].count == probe.frames[k - 1].limit {
            k -= 1;
        }
        if k == 0 {
            if probe.levels >= MAX_INDIRECT_LEVELS {
                return Err(DxError::Io(ENOSPC));
            }
            let root = probe.frames[0];
            let entries = copy_entries(state, &root, 0, root.count)?;
            let (node_lblk, node_phys) = append_block(state, handle, dir)?;
            init_node(state, dir, node_phys, &entries)?;
            {
                let blk = ext2_block_slice_mut_state(state, root.phys)?;
                write_u16_le(blk, root.count_offset + 2, 1)?;
                write_u32_le(blk, root.count_offset + 4, node_lblk)?;
                blk[0x1E] += 1;
            }
            dx_block_finish(state, dir, root.phys, root.count_offset)?;
        } else {
            let full = probe.frames[k];
            let parent = probe.frames[k - 1];
            let keep = full.count / 2;
            let hash2 = {
                let blk = ext2_block_slice_state(state, full.phys)?;
                entry_hash(blk, full.count_offset, keep)?
            };
            let moved = copy_entries(state, &full, keep, full.count)?;
            let (node_lblk, node_phys) = append_block(state, handle, dir)?;
            init_node(state, dir, node_phys, &moved)?;
            {
                let blk = ext2_block_slice_mut_state(state, full.phys)?;
                write_u16_le(blk, full.count_offset + 2, keep as u16)?;
            }
            dx_block_finish(state, dir, full.phys, full.count_offset)?;
            index_insert(state, dir, &parent, parent.at, hash2, node_lblk)?;
        }
    }
    Err(DxError::Io(EIO))
}

#[allow(clippy::too_many_arguments)]
fn split_leaf(
    state: &mut VfsState,
    handle: Option<Ext2JournalHandle>,
    dir: &mut Ext2Inode,
    probe: &Probe,
    leaf: u32,
    hash: u32,
    tail: usize,
    seed: &[u32; 4],
    ino: u32,
    name: &[u8],
    ft: u8,
) -> Result<(), DxError> {
    let bs = state.ext2_meta.block_size;
    let mut map = {
        let blk = ext2_block_slice_state(state, leaf)?;
        collect_leaf(blk, tail, probe.version, seed)?
    };
    if map.len() < 2 {
        return Err(DxError::Io(ENOSPC));
    }
    map.sort_by(|a, b| (a.hash, a.minor).cmp(&(b.hash, b.minor)));
    // do_split: move entries from the top until about half the block.
    let mut size = 0usize;
    let mut moved = 0usize;
    let mut i = map.len();
    while i > 0 {
        let len = dirent_len(map[i - 1].name.len());
        if size + len / 2 > bs / 2 {
            break;
        }
        size += len;
        moved += 1;
        i -= 1;
    }
    let split = if i > 0 { map.len() - moved } else { map.len() / 2 };
    let split = split.clamp(1, map.len() - 1);
    let hash2 = map[split].hash;
    let continued = hash2 == map[split - 1].hash;
    let upper = map.split_off(split);

    let (new_lblk, new_phys) = append_block(state, handle, dir)?;
    {
        let blk = ext2_block_slice_mut_state(state, new_phys)?;
        write_leaf(blk, tail, &upper)?;
    }
    {
        let blk = ext2_block_slice_mut_state(state, leaf)?;
        write_leaf(blk, tail, &map)?;
    }
    let bottom = probe.frames[probe.depth - 1];
    let index_hash = hash2 | (continued as u32);
    index_insert(state, dir, &bottom, bottom.at, index_hash, new_lblk)?;

    let target = if hash >= hash2 { new_phys } else { leaf };
    let inserted = {
        let blk = ext2_block_slice_mut_state(state, target)?;
        leaf_insert(blk, tail, ino, name, ft)?
    };
    ext2_dir_block_finish(state, dir, leaf)?;
    ext2_dir_block_finish(state, dir, new_phys)?;
    if !inserted {
        return Err(DxError::Io(ENOSPC));
    }
    Ok(())
}

/// Converts a full one-block linear directory into an index
/// (`make_indexed_dir`): block 0 becomes the dx root, its entries move to a
/// new leaf, and `INDEX_FL` is set. `Ok(false)` when block 0 does not have
/// the expected "." / ".." layout (the caller keeps it linear).
pub(super) fn make_indexed_dir(
    state: &mut VfsState,
    handle: Option<Ext2JournalHandle>,
    dir: &mut Ext2Inode,
) -> Result<bool, i64> {
    let bs = state.ext2_meta.block_size;
    let tail = ext2_dir_tail_len(state);
    let root_phys = ext2_data_block_for_logical_index_state(state, dir, 0)?;
    if root_phys == 0 || dir_block_count(state, dir) != 1 {
        return Ok(false);
    }
    let moved = {
        let blk = ext2_block_slice_state(state, root_phys)?;
        let dot_ok = read_u16_le(blk, 4)? == 12 && blk[6] == 1 && blk[8] == b'.';
        let dotdot_len = read_u16_le(blk, 16)? as usize;
        let dotdot_ok = blk[18] == 2 && &blk[20..22] == b"..";
        if !dot_ok || !dotdot_ok || dotdot_len < 12 || 12 + dotdot_len >= bs - tail {
            return Ok(false);
        }
        let start = 12 + dotdot_len;
        let mut data = Vec::new();
        data.try_reserve_exact(bs - tail - start).map_err(|_| ENOSPC)?;
        data.extend_from_slice(&blk[start..bs - tail]);
        data
    };
    let (leaf_lblk, leaf_phys) = append_block(state, handle, dir)?;
    {
        let blk = ext2_block_slice_mut_state(state, leaf_phys)?;
        blk[..moved.len()].copy_from_slice(&moved);
        // The last moved entry now reaches this block's tail.
        let usable = moved.len();
        let mut pos = 0usize;
        loop {
            let rec_len = read_u16_le(blk, pos + 4)? as usize;
            if rec_len < 8 || pos + rec_len > usable {
                return Err(EIO);
            }
            if pos + rec_len == usable {
                break;
            }
            pos += rec_len;
        }
        let last = (bs - tail) - pos;
        write_u16_le(blk, pos + 4, last as u16)?;
    }
    ext2_dir_block_finish(state, dir, leaf_phys)?;

    let def_version = state.ext2_meta.def_hash_version;
    let limit = root_limit(state);
    {
        let blk = ext2_block_slice_mut_state(state, root_phys)?;
        blk[24..].fill(0);
        write_u16_le(blk, 16, (bs - 12) as u16)?;
        // dx_root_info: reserved_zero, hash_version, info_length 8, levels 0.
        blk[0x1C] = def_version;
        blk[0x1D] = 8;
        write_u16_le(blk, DX_ROOT_COUNT_OFFSET, limit as u16)?;
        write_u16_le(blk, DX_ROOT_COUNT_OFFSET + 2, 1)?;
        write_u32_le(blk, DX_ROOT_COUNT_OFFSET + 4, leaf_lblk)?;
    }
    dir.flags |= EXT2_INDEX_FL;
    super::ext2_set_inode_flags_state(state, dir.ino, EXT2_INDEX_FL)?;
    dx_block_finish(state, dir, root_phys, DX_ROOT_COUNT_OFFSET)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_from_uuid(bytes: [u8; 16]) -> [u32; 4] {
        let mut s = [0u32; 4];
        for i in 0..4 {
            s[i] = u32::from_le_bytes([bytes[i * 4], bytes[i * 4 + 1], bytes[i * 4 + 2], bytes[i * 4 + 3]]);
        }
        s
    }

    /// Expected values from e2fsprogs 1.47 `debugfs -R "dx_hash -h <v> [-s
    /// seed] name"`.
    #[test]
    fn dx_hashes_match_e2fsprogs() {
        let seed = seed_from_uuid([
            0x0f, 0x1e, 0x2d, 0x3c, 0x4b, 0x5a, 0x69, 0x78, 0x87, 0x96, 0xa5, 0xb4, 0xc3, 0xd2,
            0xe1, 0xf0,
        ]);
        let none = [0u32; 4];
        let long = [b'n'; 70];
        let high = b"h\xe9llo\xff";
        let cases: &[(u8, &[u32; 4], &[u8], u32, u32)] = &[
            (DX_HASH_LEGACY, &none, b"a", 0xe74b53e2, 0),
            (DX_HASH_LEGACY, &none, b"entry_with_a_fairly_long_name_001", 0x46220eda, 0),
            (DX_HASH_LEGACY, &none, &long, 0xa1d474aa, 0),
            (DX_HASH_LEGACY, &seed, high, 0xbdbc7c5e, 0),
            (DX_HASH_LEGACY_UNSIGNED, &seed, high, 0x729c6054, 0),
            (DX_HASH_HALF_MD4, &none, b"a", 0xd5fa7d7a, 0xacb48187),
            (DX_HASH_HALF_MD4, &seed, b"a", 0x3e64fde2, 0x42280a57),
            (DX_HASH_HALF_MD4, &seed, b"hello", 0x232245ac, 0xe1acc8de),
            (DX_HASH_HALF_MD4, &none, b"entry_with_a_fairly_long_name_001", 0xfb65514c, 0xa987a7d4),
            (DX_HASH_HALF_MD4, &seed, b"entry_with_a_fairly_long_name_001", 0x096419bc, 0xddaf278a),
            (DX_HASH_HALF_MD4, &none, &long, 0x23dd9658, 0xe6e8583a),
            (DX_HASH_HALF_MD4, &seed, &long, 0x76c4fed4, 0x3d318076),
            (DX_HASH_HALF_MD4, &seed, high, 0x529a7c60, 0x690eb7b1),
            (DX_HASH_HALF_MD4_UNSIGNED, &seed, high, 0x00074d30, 0xb0029bfb),
            (DX_HASH_TEA, &none, b"a", 0x6d0ea4c0, 0xc18922df),
            (DX_HASH_TEA, &seed, b"hello", 0xf9a4dcd0, 0xd0d3e919),
            (DX_HASH_TEA, &none, b"entry_with_a_fairly_long_name_001", 0x1e18667e, 0xa4557ba3),
            (DX_HASH_TEA, &seed, &long, 0xa2865340, 0xea7cda0b),
            (DX_HASH_TEA, &none, high, 0x71b17e08, 0xf811ad18),
            (DX_HASH_TEA_UNSIGNED, &seed, high, 0x626714a8, 0x48059242),
        ];
        for &(version, seed, name, major, minor) in cases {
            assert_eq!(
                dx_hash(version, seed, name),
                Some((major, minor)),
                "version {version} name {:?}",
                core::str::from_utf8(name)
            );
        }
        assert_eq!(dx_hash(6, &none, b"a"), None);
    }
}
