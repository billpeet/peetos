//! ext4 metadata checksums (verify side), matching Linux `fs/ext4`.
//!
//! - crc32c (`metadata_csum`): superblock, group descriptors (low 16 bits),
//!   block/inode bitmaps (stored in the descriptor), inodes, extent tree
//!   blocks, and directory leaf blocks (12-byte tail dirent).
//! - crc16 (`gdt_csum` / `uninit_bg`): group descriptors only.
//!
//! All functions work on raw on-disk bytes so they can be checked against
//! images written by `mke2fs` (`user/sample_ext4*.img`) before any of them is
//! wired into the mount/read path (`docs/ext4_compat_plan.md`, M1/M3).
//! Linux's `crc32c_le`/`crc16` take and return the running value without
//! pre/post inversion; seeds below follow `ext4_chksum` exactly.

#![cfg_attr(not(test), allow(dead_code))]

use super::{read_u16_le, read_u32_le, EINVAL, EIO};

pub(super) const EXT4_FEATURE_RO_COMPAT_GDT_CSUM: u32 = 0x0010;
pub(super) const EXT4_FEATURE_RO_COMPAT_METADATA_CSUM: u32 = 0x0400;
pub(super) const EXT4_FEATURE_INCOMPAT_CSUM_SEED: u32 = 0x2000;
const EXT4_FEATURE_INCOMPAT_64BIT: u32 = 0x0080;

/// `bg_flags` bits.
pub(super) const EXT4_BG_INODE_UNINIT: u16 = 0x0001;
pub(super) const EXT4_BG_BLOCK_UNINIT: u16 = 0x0002;

/// Superblock field offsets (relative to the superblock).
const SB_BLOCKS_PER_GROUP: usize = 0x20;
const SB_INODES_PER_GROUP: usize = 0x28;
const SB_FEATURE_INCOMPAT: usize = 0x60;
const SB_FEATURE_RO_COMPAT: usize = 0x64;
const SB_UUID: usize = 0x68;
const SB_INODE_SIZE: usize = 0x58;
const SB_DESC_SIZE: usize = 0xFE;
const SB_CHECKSUM_SEED: usize = 0x270;
const SB_CHECKSUM: usize = 0x3FC;

/// Group descriptor field offsets.
const GD_BLOCK_BITMAP_CSUM_LO: usize = 0x18;
const GD_INODE_BITMAP_CSUM_LO: usize = 0x1A;
const GD_CHECKSUM: usize = 0x1E;
const GD_BLOCK_BITMAP_CSUM_HI: usize = 0x38;
const GD_INODE_BITMAP_CSUM_HI: usize = 0x3A;

/// Inode field offsets.
const INODE_GENERATION: usize = 0x64;
const INODE_CHECKSUM_LO: usize = 0x7C;
const INODE_EXTRA_ISIZE: usize = 0x80;
const INODE_CHECKSUM_HI: usize = 0x82;
const GOOD_OLD_INODE_SIZE: usize = 128;

/// Extent header magic.
pub(super) const EXT4_EXTENT_MAGIC: u16 = 0xF30A;
/// Directory leaf checksum tail: inode 0, rec_len 12, name_len 0, type 0xDE.
const DIR_TAIL_SIZE: usize = 12;
const DIR_TAIL_FILE_TYPE: u8 = 0xDE;

const fn build_crc32c_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if (crc & 1) != 0 { (crc >> 1) ^ 0x82F6_3B78 } else { crc >> 1 };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

const fn build_crc16_table() -> [u16; 256] {
    let mut table = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u16;
        let mut bit = 0;
        while bit < 8 {
            crc = if (crc & 1) != 0 { (crc >> 1) ^ 0xA001 } else { crc >> 1 };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

static CRC32C_TABLE: [u32; 256] = build_crc32c_table();
static CRC16_TABLE: [u16; 256] = build_crc16_table();

/// CRC-32C (Castagnoli), reflected, no pre/post inversion (Linux `crc32c_le`).
pub(super) fn crc32c(mut crc: u32, data: &[u8]) -> u32 {
    for &byte in data {
        crc = CRC32C_TABLE[((crc ^ byte as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc
}

/// CRC-16 (ANSI/IBM, reflected poly 0xA001), no inversion (Linux `crc16`).
pub(super) fn crc16(mut crc: u16, data: &[u8]) -> u16 {
    for &byte in data {
        crc = CRC16_TABLE[((crc ^ byte as u16) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc
}

/// Superblock checksum: crc32c(~0, superblock up to `s_checksum`).
pub(super) fn superblock_csum(sb: &[u8]) -> Result<u32, i64> {
    if sb.len() < 1024 {
        return Err(EIO);
    }
    Ok(crc32c(!0, &sb[..SB_CHECKSUM]))
}

pub(super) fn superblock_csum_ok(sb: &[u8]) -> Result<bool, i64> {
    Ok(superblock_csum(sb)? == read_u32_le(sb, SB_CHECKSUM)?)
}

/// Per-filesystem checksum parameters derived from the superblock.
#[derive(Clone, Copy, Debug)]
pub(super) struct Ext4Csum {
    /// crc32c seed: `s_checksum_seed` with `csum_seed`, else crc32c(~0, uuid).
    pub seed: u32,
    pub uuid: [u8; 16],
    pub metadata_csum: bool,
    pub gdt_csum: bool,
    pub desc_size: usize,
    pub inode_size: usize,
    pub inodes_per_group: u32,
    pub blocks_per_group: u32,
}

impl Ext4Csum {
    /// `sb` is the 1024-byte superblock.
    pub(super) fn from_superblock(sb: &[u8]) -> Result<Self, i64> {
        if sb.len() < 1024 {
            return Err(EIO);
        }
        let incompat = read_u32_le(sb, SB_FEATURE_INCOMPAT)?;
        let ro_compat = read_u32_le(sb, SB_FEATURE_RO_COMPAT)?;
        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(&sb[SB_UUID..SB_UUID + 16]);
        let seed = if (incompat & EXT4_FEATURE_INCOMPAT_CSUM_SEED) != 0 {
            read_u32_le(sb, SB_CHECKSUM_SEED)?
        } else {
            crc32c(!0, &uuid)
        };
        let desc_size = if (incompat & EXT4_FEATURE_INCOMPAT_64BIT) != 0 {
            read_u16_le(sb, SB_DESC_SIZE)? as usize
        } else {
            32
        };
        let inode_size = read_u16_le(sb, SB_INODE_SIZE)? as usize;
        if desc_size < 32 || inode_size < GOOD_OLD_INODE_SIZE {
            return Err(EINVAL);
        }
        Ok(Self {
            seed,
            uuid,
            metadata_csum: (ro_compat & EXT4_FEATURE_RO_COMPAT_METADATA_CSUM) != 0,
            gdt_csum: (ro_compat & EXT4_FEATURE_RO_COMPAT_GDT_CSUM) != 0,
            desc_size,
            inode_size,
            inodes_per_group: read_u32_le(sb, SB_INODES_PER_GROUP)?,
            blocks_per_group: read_u32_le(sb, SB_BLOCKS_PER_GROUP)?,
        })
    }

    /// Expected `bg_checksum`, or `None` when neither `metadata_csum` nor
    /// `gdt_csum` is enabled (descriptors are then unchecksummed).
    pub(super) fn group_desc(&self, group: u32, desc: &[u8]) -> Result<Option<u16>, i64> {
        if desc.len() < self.desc_size {
            return Err(EIO);
        }
        let group_le = group.to_le_bytes();
        if self.metadata_csum {
            // crc32c over the descriptor with bg_checksum taken as zero.
            let mut crc = crc32c(self.seed, &group_le);
            crc = crc32c(crc, &desc[..GD_CHECKSUM]);
            crc = crc32c(crc, &[0, 0]);
            if self.desc_size > GD_CHECKSUM + 2 {
                crc = crc32c(crc, &desc[GD_CHECKSUM + 2..self.desc_size]);
            }
            return Ok(Some((crc & 0xFFFF) as u16));
        }
        if self.gdt_csum {
            // crc16 skips bg_checksum entirely (no zero placeholder).
            let mut crc = crc16(!0, &self.uuid);
            crc = crc16(crc, &group_le);
            crc = crc16(crc, &desc[..GD_CHECKSUM]);
            if self.desc_size > GD_CHECKSUM + 2 {
                crc = crc16(crc, &desc[GD_CHECKSUM + 2..self.desc_size]);
            }
            return Ok(Some(crc));
        }
        Ok(None)
    }

    pub(super) fn group_desc_ok(&self, group: u32, desc: &[u8]) -> Result<bool, i64> {
        match self.group_desc(group, desc)? {
            Some(expected) => Ok(expected == read_u16_le(desc, GD_CHECKSUM)?),
            None => Ok(true),
        }
    }

    /// Per-inode seed: crc32c(crc32c(fs seed, ino), i_generation).
    pub(super) fn inode_seed(&self, ino: u32, raw: &[u8]) -> Result<u32, i64> {
        let generation = read_u32_le(raw, INODE_GENERATION)?;
        Ok(crc32c(crc32c(self.seed, &ino.to_le_bytes()), &generation.to_le_bytes()))
    }

    fn checksum_hi_fits(&self, raw: &[u8]) -> Result<bool, i64> {
        if self.inode_size <= GOOD_OLD_INODE_SIZE {
            return Ok(false);
        }
        let extra = read_u16_le(raw, INODE_EXTRA_ISIZE)? as usize;
        Ok(INODE_CHECKSUM_HI + 2 <= GOOD_OLD_INODE_SIZE + extra)
    }

    /// Full 32-bit inode checksum with both checksum fields taken as zero.
    pub(super) fn inode(&self, ino: u32, raw: &[u8]) -> Result<u32, i64> {
        if raw.len() < self.inode_size {
            return Err(EIO);
        }
        let mut crc = self.inode_seed(ino, raw)?;
        crc = crc32c(crc, &raw[..INODE_CHECKSUM_LO]);
        crc = crc32c(crc, &[0, 0]);
        crc = crc32c(crc, &raw[INODE_CHECKSUM_LO + 2..GOOD_OLD_INODE_SIZE]);
        if self.inode_size > GOOD_OLD_INODE_SIZE {
            crc = crc32c(crc, &raw[GOOD_OLD_INODE_SIZE..INODE_CHECKSUM_HI]);
            let mut offset = INODE_CHECKSUM_HI;
            if self.checksum_hi_fits(raw)? {
                crc = crc32c(crc, &[0, 0]);
                offset += 2;
            }
            crc = crc32c(crc, &raw[offset..self.inode_size]);
        }
        Ok(crc)
    }

    pub(super) fn inode_ok(&self, ino: u32, raw: &[u8]) -> Result<bool, i64> {
        let computed = self.inode(ino, raw)?;
        let lo = read_u16_le(raw, INODE_CHECKSUM_LO)? as u32;
        if self.checksum_hi_fits(raw)? {
            let hi = read_u16_le(raw, INODE_CHECKSUM_HI)? as u32;
            Ok(computed == (hi << 16) | lo)
        } else {
            Ok((computed & 0xFFFF) == lo)
        }
    }

    fn bitmap_ok(
        &self,
        desc: &[u8],
        bitmap: &[u8],
        bits: u32,
        lo_offset: usize,
        hi_offset: usize,
    ) -> Result<bool, i64> {
        let bytes = (bits / 8) as usize;
        if bitmap.len() < bytes {
            return Err(EIO);
        }
        let computed = crc32c(self.seed, &bitmap[..bytes]);
        let lo = read_u16_le(desc, lo_offset)? as u32;
        if self.desc_size >= hi_offset + 2 {
            let hi = read_u16_le(desc, hi_offset)? as u32;
            Ok(computed == (hi << 16) | lo)
        } else {
            Ok((computed & 0xFFFF) == lo)
        }
    }

    /// Block bitmap checksum (crc32c over blocks_per_group / 8 bytes).
    pub(super) fn block_bitmap_ok(&self, desc: &[u8], bitmap: &[u8]) -> Result<bool, i64> {
        self.bitmap_ok(
            desc,
            bitmap,
            self.blocks_per_group,
            GD_BLOCK_BITMAP_CSUM_LO,
            GD_BLOCK_BITMAP_CSUM_HI,
        )
    }

    /// Inode bitmap checksum (crc32c over inodes_per_group / 8 bytes).
    pub(super) fn inode_bitmap_ok(&self, desc: &[u8], bitmap: &[u8]) -> Result<bool, i64> {
        self.bitmap_ok(
            desc,
            bitmap,
            self.inodes_per_group,
            GD_INODE_BITMAP_CSUM_LO,
            GD_INODE_BITMAP_CSUM_HI,
        )
    }
}

/// Extent tree block (index or leaf, not the in-inode root): the
/// `ext4_extent_tail` checksum follows `eh_max` entries.
pub(super) fn extent_block_csum_ok(inode_seed: u32, block: &[u8]) -> Result<bool, i64> {
    if read_u16_le(block, 0)? != EXT4_EXTENT_MAGIC {
        return Err(EIO);
    }
    let max_entries = read_u16_le(block, 4)? as usize;
    let tail = 12 + 12 * max_entries;
    if tail + 4 > block.len() {
        return Err(EIO);
    }
    Ok(crc32c(inode_seed, &block[..tail]) == read_u32_le(block, tail)?)
}

/// Directory leaf block checksum tail. `Ok(None)` when the block has no
/// tail dirent (for example an htree interior/root block, which uses a
/// `dx_tail` instead).
pub(super) fn dir_leaf_csum_ok(inode_seed: u32, block: &[u8]) -> Result<Option<bool>, i64> {
    if block.len() < DIR_TAIL_SIZE {
        return Err(EIO);
    }
    let tail = block.len() - DIR_TAIL_SIZE;
    let is_tail = read_u32_le(block, tail)? == 0
        && read_u16_le(block, tail + 4)? as usize == DIR_TAIL_SIZE
        && block[tail + 6] == 0
        && block[tail + 7] == DIR_TAIL_FILE_TYPE;
    if !is_tail {
        return Ok(None);
    }
    Ok(Some(crc32c(inode_seed, &block[..tail]) == read_u32_le(block, tail + 8)?))
}
