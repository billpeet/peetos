#![no_std]

extern crate alloc;

mod ext2_alloc;
mod ext2_api;
mod ext2_bmap;
mod ext2_backend;
mod ext2_dir;
mod ext2_htree;
mod ext2_journal;
mod ext2_sync;
mod ext4_csum;

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::hint::spin_loop;
use core::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use ext2_alloc::ext2_alloc_block;
use ext2_alloc::{
    ext2_alloc_block_goal, ext2_alloc_inode, ext2_free_block, ext2_free_inode,
    ext2_write_inode_to_state, ext2_write_zero_range, indirect_ensure_block,
};
use ext2_bmap::{
    bmap_free_all_blocks, ext2_data_block_for_logical_index_state, ext2_detach_data_block,
    ext2_ensure_data_block, ext2_recalc_inode_block_count_512_state,
};
use ext2_dir::{
    ext2_add_dir_entry, ext2_directory_is_empty, ext2_remove_dir_entry, ext2_write_dirent,
};

pub use ext2_api::{
    ext2_fs_name, ext2_mount_read_only, mount_ext2_block_backend_with_options, set_ext2_diag_hook,
    checkpoint_ext2_journal, clear_ext2_block_backend, copy_ext2_blocks, export_ext2_image,
    ext2_backend_kind, ext2_has_journal, ext2_next_dirty_run, flush_ext2_dirty_blocks,
    mark_ext2_blocks_synced, mount_ext2_block_backend, set_ext2_block_backend,
    set_ext2_cache_policy,
};
pub use ext2_backend::{
    Ext2BackendKind, Ext2BlockBackend, Ext2BlockFlushFn, Ext2BlockReadFn, Ext2BlockWriteFn,
};

pub const VFS_MAX_PATH: usize = 256;
/// Name buffer size: names up to 255 bytes, as ext2/ext3/ext4 allow.
pub const VFS_MAX_NAME: usize = 256;
/// Upper bound on live VFS nodes (the table grows on demand; handles are
/// `u16`). ext2-family directories are imported lazily, so this bounds the
/// files touched since mount, not the size of the filesystem.
pub const VFS_MAX_INODES: usize = 32768;
pub const TMPFS_MAX_FILE_SIZE: usize = 32 * 1024;

pub const O_RDONLY: u32 = 0x0000;
pub const O_WRONLY: u32 = 0x0001;
pub const O_RDWR: u32 = 0x0002;
pub const O_ACCMODE: u32 = 0x0003;
pub const O_CREAT: u32 = 0x0040;
pub const O_EXCL: u32 = 0x0080;
pub const O_TRUNC: u32 = 0x0200;
pub const O_APPEND: u32 = 0x0400;
pub const O_DIRECTORY: u32 = 0x10000;

pub const SEEK_SET: u32 = 0;
pub const SEEK_CUR: u32 = 1;
pub const SEEK_END: u32 = 2;

pub const AT_FDCWD: i32 = -100;

pub const EPERM: i64 = 1;
pub const ENOENT: i64 = 2;
pub const EIO: i64 = 5;
pub const EEXIST: i64 = 17;
pub const ENOTDIR: i64 = 20;
pub const EISDIR: i64 = 21;
pub const EINVAL: i64 = 22;
pub const ENOSPC: i64 = 28;
pub const EROFS: i64 = 30;
pub const ENAMETOOLONG: i64 = 36;
pub const EBUSY: i64 = 16;
pub const ENOTEMPTY: i64 = 39;
pub const EFBIG: i64 = 27;
pub const EOVERFLOW: i64 = 75;

const ROOT_INODE: u16 = 0;
const PROCFS_BUFFER_MAX: usize = 512;
const PROC_TICK_HZ: u64 = 100;
const MOUNT_MAX: usize = 8;
const LEGACY_EXT2_MOUNT_PREFIX: &str = "/mnt/ext2";
const ROOT_PSEUDO_MOUNT_NAMES: [&[u8]; 4] = [b"dev", b"proc", b"temp", b"boot"];

const STAT_S_IFMT: u32 = 0xF000;
const STAT_S_IFDIR: u32 = 0x4000;
const STAT_S_IFREG: u32 = 0x8000;
const STAT_S_IFLNK: u32 = 0xA000;
const STAT_S_IRUSR: u32 = 0o400;
const STAT_S_IWUSR: u32 = 0o200;
const STAT_S_IXUSR: u32 = 0o100;
const STAT_S_IRGRP: u32 = 0o040;
const STAT_S_IWGRP: u32 = 0o020;
const STAT_S_IXGRP: u32 = 0o010;
const STAT_S_IROTH: u32 = 0o004;
const STAT_S_IWOTH: u32 = 0o002;
const STAT_S_IXOTH: u32 = 0o001;

pub const DT_UNKNOWN: u8 = 0;
pub const DT_DIR: u8 = 4;
pub const DT_REG: u8 = 8;
pub const DT_LNK: u8 = 10;

const EXT2_SUPERBLOCK_OFFSET: usize = 1024;
const EXT2_SUPER_MAGIC: u16 = 0xEF53;
const EXT2_ROOT_INODE: u32 = 2;
/// Group descriptor size without the ext4 `64bit` feature.
const EXT2_GROUP_DESC_SIZE: usize = 32;
/// Smallest descriptor size the `64bit` feature allows (`s_desc_size`).
const EXT4_MIN_DESC_SIZE_64BIT: usize = 64;
/// Upper bound accepted for `s_desc_size` (Linux: EXT4_MAX_DESC_SIZE).
const EXT4_MAX_DESC_SIZE: usize = 1024;
/// Largest inode size checksummed in place (ext4 allows up to the block size;
/// e2fsprogs creates at most 1024).
const EXT4_MAX_INODE_SIZE: usize = 1024;
/// Offset of a descriptor field's high half (`bg_*_hi`) relative to its low
/// half, for the fields that have one, when descriptors are >= 64 bytes.
const EXT4_GD_HI_DELTA: usize = 0x20;
const EXT2_INODE_DIRECT_COUNT: usize = 12;
const EXT2_CACHE_MAX_BLOCKS_DEFAULT: usize = 2048;
const EXT2_CACHE_DIRTY_HIGH_WATERMARK_PERCENT_DEFAULT: u8 = 30;

const EXT2_FEATURE_COMPAT_HAS_JOURNAL: u32 = 0x0004;
#[cfg(test)]
const EXT2_FEATURE_COMPAT_DIR_PREALLOC: u32 = 0x0001;
#[cfg(test)]
const EXT2_FEATURE_COMPAT_IMAGIC_INODES: u32 = 0x0002;
const EXT2_FEATURE_COMPAT_EXT_ATTR: u32 = 0x0008;
const EXT2_FEATURE_COMPAT_RESIZE_INO: u32 = 0x0010;
const EXT2_FEATURE_COMPAT_DIR_INDEX: u32 = 0x0020;
/// ext4 `orphan_file`: orphan inode numbers live in a dedicated file
/// (`s_orphan_file_inum`) instead of only the `s_last_orphan` list.
const EXT4_FEATURE_COMPAT_ORPHAN_FILE: u32 = 0x1000;
// The compat allowlist covers the stock `mke2fs -t ext3` feature set. Each bit
// here has explicit handling (see docs/ext3_parity_audit.md):
// - EXT_ATTR: xattr blocks are counted in i_blocks and released on delete.
// - RESIZE_INO: reserved GDT blocks are already marked in-use in the bitmaps,
//   and the kernel never resizes, so the reservation is preserved untouched.
// - DIR_INDEX: htree indexes are used for lookups and maintained on insert
//   (ext2_htree.rs); a full one-block directory is converted to an index.
//   An index this code cannot use falls back to linear mode (INDEX_FL is
//   cleared first, as Linux does).
// DIR_PREALLOC and IMAGIC_INODES remain rejected.
const EXT2_FEATURE_COMPAT_SUPPORTED: u32 = EXT2_FEATURE_COMPAT_HAS_JOURNAL
    | EXT2_FEATURE_COMPAT_EXT_ATTR
    | EXT2_FEATURE_COMPAT_RESIZE_INO
    | EXT2_FEATURE_COMPAT_DIR_INDEX
    | EXT4_FEATURE_COMPAT_ORPHAN_FILE;

const EXT2_FEATURE_INCOMPAT_FILETYPE: u32 = 0x0002;
const EXT2_FEATURE_INCOMPAT_RECOVER: u32 = 0x0004;
const EXT2_FEATURE_INCOMPAT_JOURNAL_DEV: u32 = 0x0008;
/// ext4 `64bit`: 64-bit block numbers and `s_desc_size`-byte descriptors.
const EXT4_FEATURE_INCOMPAT_64BIT: u32 = 0x0080;
const EXT4_FEATURE_INCOMPAT_EXTENTS: u32 = 0x0040;
const EXT4_FEATURE_INCOMPAT_FLEX_BG: u32 = 0x0200;
const EXT4_FEATURE_INCOMPAT_CSUM_SEED: u32 = 0x2000;
/// ext4 incompat features understood by the mount path (fs type "ext4").
const EXT4_FEATURE_INCOMPAT_ALL: u32 = EXT4_FEATURE_INCOMPAT_EXTENTS
    | EXT4_FEATURE_INCOMPAT_64BIT
    | EXT4_FEATURE_INCOMPAT_FLEX_BG
    | EXT4_FEATURE_INCOMPAT_CSUM_SEED;
/// ...of which these would force a read-only mount. Since M3 (checksummed
/// writes) none do; the mechanism stays for future read-only-only features.
const EXT4_FEATURE_INCOMPAT_READ_ONLY: u32 = 0;
const EXT4_FEATURE_RO_COMPAT_HUGE_FILE: u32 = 0x0008;
const EXT4_FEATURE_RO_COMPAT_GDT_CSUM: u32 = 0x0010;
const EXT4_FEATURE_RO_COMPAT_DIR_NLINK: u32 = 0x0020;
const EXT4_FEATURE_RO_COMPAT_EXTRA_ISIZE: u32 = 0x0040;
const EXT4_FEATURE_RO_COMPAT_METADATA_CSUM: u32 = 0x0400;
/// ext4 ro-compat features understood by the mount path.
/// The orphan file holds entries (set while orphans are recorded there).
const EXT4_FEATURE_RO_COMPAT_ORPHAN_PRESENT: u32 = 0x0001_0000;
const EXT4_FEATURE_RO_COMPAT_ALL: u32 = EXT4_FEATURE_RO_COMPAT_ORPHAN_PRESENT
    | EXT4_FEATURE_RO_COMPAT_HUGE_FILE
    | EXT4_FEATURE_RO_COMPAT_GDT_CSUM
    | EXT4_FEATURE_RO_COMPAT_DIR_NLINK
    | EXT4_FEATURE_RO_COMPAT_EXTRA_ISIZE
    | EXT4_FEATURE_RO_COMPAT_METADATA_CSUM;
/// ...of which these would force a read-only mount (none since M3).
const EXT4_FEATURE_RO_COMPAT_READ_ONLY: u32 = 0;
/// `i_flags`: `i_blocks` counts filesystem blocks instead of 512-byte sectors.
const EXT4_HUGE_FILE_FL: u32 = 0x0004_0000;
const EXT2_FEATURE_INCOMPAT_SUPPORTED: u32 = EXT2_FEATURE_INCOMPAT_FILETYPE
    | EXT2_FEATURE_INCOMPAT_RECOVER
    | EXT2_FEATURE_INCOMPAT_JOURNAL_DEV;

const EXT2_FEATURE_RO_COMPAT_SPARSE_SUPER: u32 = 0x0001;
// LARGE_FILE: regular files carry i_size_high; sizes are 64-bit end to end,
// and the feature is turned on when the first file grows past 2 GiB.
const EXT2_FEATURE_RO_COMPAT_LARGE_FILE: u32 = 0x0002;
#[cfg(test)]
const EXT2_FEATURE_RO_COMPAT_BTREE_DIR: u32 = 0x0004;
const EXT2_FEATURE_RO_COMPAT_SUPPORTED: u32 =
    EXT2_FEATURE_RO_COMPAT_SPARSE_SUPER | EXT2_FEATURE_RO_COMPAT_LARGE_FILE;

/// Inode flag: directory uses a hashed b-tree (htree) index.
const EXT2_INDEX_FL: u32 = 0x0000_1000;
/// Magic at the start of an external extended-attribute block.
const EXT2_XATTR_MAGIC: u32 = 0xEA02_0000;
/// Symlink targets shorter than this may live inline in `i_block`.
const EXT2_FAST_SYMLINK_MAX: usize = 60;

const EXT2_FT_REG_FILE: u8 = 1;
const EXT2_FT_DIR: u8 = 2;
const EXT2_FT_SYMLINK: u8 = 7;

const EXT2_S_IFMT: u16 = 0xF000;
const EXT2_S_IFDIR: u16 = 0x4000;
const EXT2_S_IFREG: u16 = 0x8000;
const EXT2_S_IFLNK: u16 = 0xA000;
const EXT2_DEF_FILE_MODE: u16 = EXT2_S_IFREG | 0o644;
const EXT2_DEF_DIR_MODE: u16 = EXT2_S_IFDIR | 0o755;
const EXT2_DEF_SYMLINK_MODE: u16 = EXT2_S_IFLNK | 0o777;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FileType {
    None = 0,
    Regular = 1,
    Directory = 2,
    Symlink = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum NodeBackend {
    Tmpfs = 0,
    Ext2 = 1,
    DevNull = 2,
    DevZero = 3,
    DevConsole = 4,
    ProcUptime = 5,
    ProcMeminfo = 6,
    ProcMounts = 7,
    ProcVersion = 8,
    DevUrandom = 9,
    DevTty = 10,
    DevPtmx = 11,
    DevPtsSlave = 12,
    DevFramebuffer = 13,
    DevInputEvent = 14,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenResult {
    pub inode: u16,
    pub file_type: FileType,
    pub is_console: bool,
    pub is_tty: bool,
    pub is_ptmx: bool,
    pub is_pts_slave: bool,
    pub is_framebuffer: bool,
    pub pts_id: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriteResult {
    pub written: usize,
    pub new_offset: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStat {
    pub ino: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u32,
    pub size: u64,
    pub blksize: u32,
    pub blocks: u64,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub file_type: FileType,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub ino: u64,
    pub next_offset: u64,
    pub d_type: u8,
    pub name_len: usize,
    pub name: [u8; VFS_MAX_NAME],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ext2DirtyRun {
    pub start_block: u32,
    pub block_count: u32,
}

impl DirEntry {
    const fn empty() -> Self {
        Self {
            ino: 0,
            next_offset: 0,
            d_type: DT_UNKNOWN,
            name_len: 0,
            name: [0; VFS_MAX_NAME],
        }
    }
}

struct Inode {
    used: bool,
    file_type: FileType,
    backend: NodeBackend,
    parent: u16,
    name: [u8; VFS_MAX_NAME],
    name_len: u8,
    size: u64,
    /// Child nodes in creation order (readdir offsets index this list).
    children: Vec<u16>,
    /// tmpfs file contents / symlink target (at least `size` bytes).
    data: Vec<u8>,
    ext2_ino: u32,
    dev_id: u16,
    ext2_children_loaded: bool,
    mode: u16,
    uid: u16,
    gid: u16,
    links_count: u16,
    atime: u64,
    mtime: u64,
    ctime: u64,
    /// References held by the kernel (open fds, working directories):
    /// `inode_open_retain` / `inode_open_release`.
    open_count: u32,
    /// Removed from its directory while still referenced: freed (and for
    /// ext2, its on-disk inode and blocks released) on the last release.
    unlinked: bool,
}

impl Inode {
    const fn empty() -> Self {
        Self {
            used: false,
            file_type: FileType::None,
            backend: NodeBackend::Tmpfs,
            parent: ROOT_INODE,
            name: [0; VFS_MAX_NAME],
            name_len: 0,
            size: 0,
            children: Vec::new(),
            data: Vec::new(),
            ext2_ino: 0,
            dev_id: 0,
            ext2_children_loaded: false,
            mode: 0,
            uid: 0,
            gid: 0,
            links_count: 0,
            atime: 0,
            mtime: 0,
            ctime: 0,
            open_count: 0,
            unlinked: false,
        }
    }
}

#[derive(Clone, Copy)]
struct MountEntry {
    used: bool,
    mount_inode: u16,
    path_len: u16,
    path: [u8; VFS_MAX_PATH],
    fs_name_len: u8,
    fs_name: [u8; 8],
    readonly: bool,
}

impl MountEntry {
    const fn empty() -> Self {
        Self {
            used: false,
            mount_inode: ROOT_INODE,
            path_len: 0,
            path: [0; VFS_MAX_PATH],
            fs_name_len: 0,
            fs_name: [0; 8],
            readonly: false,
        }
    }
}

struct Ext2CachedBlock {
    block_index: u32,
    referenced: bool,
    dirty: bool,
    data: Vec<u8>,
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
struct Ext2JournalState {
    present: bool,
    replaying: bool,
    aborted: bool,
    abort_errno: i64,
    mode: Ext2JournalMode,
    inode: u32,
    inode_blocks: [u32; 15],
    inode_flags: u32,
    inode_generation: u32,
    superblock_block: u32,
    block_size: u32,
    max_len: u32,
    first: u32,
    sequence: u32,
    start: u32,
    runtime_head_jblock: u32,
    tx_descriptor_jblock: u32,
    tx_data_jblock: u32,
    tx_commit_jblock: u32,
    tx_descriptor_fs_block: u32,
    tx_data_fs_block: u32,
    tx_commit_fs_block: u32,
    /// Whether the on-disk superblock currently carries INCOMPAT_RECOVER.
    /// Set before the first commit lands in an empty journal and cleared once
    /// a checkpoint empties it again, so e2fsck/Linux see "needs recovery"
    /// exactly while committed-but-uncheckpointed transactions exist.
    recover_marked: bool,
    /// JBD2 COMPAT_CHECKSUM: commit blocks carry a CRC32 over the
    /// transaction's descriptor and journaled blocks, verified on replay.
    checksum: bool,
    /// The journal uses JBD2 CSUM_V2/V3: journal superblock writes must keep
    /// `s_checksum` valid, and transactions carry v2/v3 checksums.
    csum_v2_or_v3: bool,
    /// JBD2 incompat features of the journal (REVOKE/64BIT/CSUM_V2/CSUM_V3)
    /// and its checksum seed; the runtime writer emits this format.
    feature_incompat: u32,
    csum_seed: u32,
    /// A commit record was written but no barrier has made it durable yet.
    /// Settled before checkpoint home writes and before flush/sync returns.
    commit_barrier_pending: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ext2JournalMode {
    Disabled,
    Ordered,
}

impl Ext2JournalState {
    const fn empty() -> Self {
        Self {
            present: false,
            replaying: false,
            aborted: false,
            abort_errno: 0,
            mode: Ext2JournalMode::Disabled,
            inode: 0,
            inode_blocks: [0; 15],
            inode_flags: 0,
            inode_generation: 0,
            superblock_block: 0,
            block_size: 0,
            max_len: 0,
            first: 0,
            sequence: 0,
            start: 0,
            runtime_head_jblock: 0,
            tx_descriptor_jblock: 0,
            tx_data_jblock: 0,
            tx_commit_jblock: 0,
            tx_descriptor_fs_block: 0,
            tx_data_fs_block: 0,
            tx_commit_fs_block: 0,
            recover_marked: false,
            checksum: false,
            csum_v2_or_v3: false,
            feature_incompat: 0,
            csum_seed: 0,
            commit_barrier_pending: false,
        }
    }
}

struct Ext2JournalRunningTx {
    op_id: u64,
    tx_id: u64,
    segment_index: usize,
    phase_entries: usize,
    last_phase_kind: Ext2JournalPhaseKind,
    active: bool,
    scope_depth: usize,
    handle_count: usize,
    restartable: bool,
    credit_growth_chunk: usize,
    restart_count: usize,
    reserved_credits: usize,
    used_credits: usize,
    tracked_blocks: Vec<u32>,
    data_blocks: Vec<u32>,
    revoke_blocks: Vec<u32>,
}

impl Ext2JournalRunningTx {
    const fn empty() -> Self {
        Self {
            op_id: 0,
            tx_id: 0,
            segment_index: 0,
            phase_entries: 0,
            last_phase_kind: Ext2JournalPhaseKind::None,
            active: false,
            scope_depth: 0,
            handle_count: 0,
            restartable: false,
            credit_growth_chunk: 0,
            restart_count: 0,
            reserved_credits: 0,
            used_credits: 0,
            tracked_blocks: Vec::new(),
            data_blocks: Vec::new(),
            revoke_blocks: Vec::new(),
        }
    }
}

struct Ext2JournalBlockSnapshot {
    home_block: u32,
    data: Vec<u8>,
    escaped: bool,
}

struct Ext2JournalPendingTx {
    op_id: u64,
    tx_id: u64,
    segment_index: usize,
    phase_entries: usize,
    last_phase_kind: Ext2JournalPhaseKind,
    restart_count: usize,
    reserved_credits: usize,
    used_credits: usize,
    metadata_blocks: Vec<u32>,
    metadata_snapshots: Vec<Ext2JournalBlockSnapshot>,
    data_blocks: Vec<u32>,
    data_snapshots: Vec<Ext2JournalBlockSnapshot>,
    revoke_blocks: Vec<u32>,
}

impl Ext2JournalPendingTx {
    const fn empty() -> Self {
        Self {
            op_id: 0,
            tx_id: 0,
            segment_index: 0,
            phase_entries: 0,
            last_phase_kind: Ext2JournalPhaseKind::None,
            restart_count: 0,
            reserved_credits: 0,
            used_credits: 0,
            metadata_blocks: Vec::new(),
            metadata_snapshots: Vec::new(),
            data_blocks: Vec::new(),
            data_snapshots: Vec::new(),
            revoke_blocks: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Ext2JournalHandle {
    op_id: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ext2JournalPhaseKind {
    None,
    DataWrite,
    DataZero,
    InodeCloseout,
    AllocBlockBookkeeping,
    AllocBlockInit,
    FreeBlockBookkeeping,
    FreeBlockRevoke,
    AllocInodeBookkeeping,
    AllocInodeInit,
    FreeInodeBookkeeping,
    FreeInodeClear,
    IndirectLink,
    IndirectUnlink,
    XattrRelease,
}

#[derive(Clone, Copy)]
struct Ext2JournalCommittedUpdate {
    home_block: u32,
    journal_block: u32,
    escaped: bool,
}

struct Ext2JournalCommittedTx {
    sequence: u32,
    descriptor_jblock: u32,
    log_block_count: u32,
    updates: Vec<Ext2JournalCommittedUpdate>,
}

struct VfsState {
    initialized: bool,
    inodes: Vec<Inode>,
    proc_uptime_ticks: u64,
    proc_total_bytes: u64,
    proc_free_bytes: u64,
    proc_used_bytes: u64,
    clock_ticks: u64,
    mounts: [MountEntry; MOUNT_MAX],
    ext2_mounted: bool,
    ext2_mount_inode: u16,
    ext2_meta: Ext2Meta,
    ext2_image_len: usize,
    ext2_image: Vec<u8>,
    ext2_backend: ext2_backend::Ext2Backend,
    ext2_tracked_block_count: usize,
    ext2_dirty_blocks: Vec<u32>,
    ext2_cached_blocks: Vec<Ext2CachedBlock>,
    ext2_cache_max_blocks: usize,
    ext2_cache_clock_hand: usize,
    ext2_cache_dirty_high_watermark_percent: u8,
    ext2_journal: Ext2JournalState,
    ext2_journal_running_tx: Ext2JournalRunningTx,
    ext2_journal_pending_txs: Vec<Ext2JournalPendingTx>,
    ext2_journal_committed_txs: Vec<Ext2JournalCommittedTx>,
    ext2_journal_pending_revokes: Vec<u32>,
    ext2_journal_next_tx_id: u64,
    /// The mounted ext2-family filesystem refuses VFS mutations (`EROFS`).
    ext2_read_only: bool,
    /// ext4 `metadata_csum` parameters when the mounted image has it.
    ext4_csum: Option<ext4_csum::Ext4Csum>,
    /// Optional diagnostics sink (checksum failures).
    ext2_diag: Option<Ext2DiagFn>,
    prng_state: u64,
}

impl VfsState {
    const fn new() -> Self {
        Self {
            initialized: false,
            inodes: Vec::new(),
            proc_uptime_ticks: 0,
            proc_total_bytes: 0,
            proc_free_bytes: 0,
            proc_used_bytes: 0,
            clock_ticks: 0,
            mounts: [MountEntry::empty(); MOUNT_MAX],
            ext2_mounted: false,
            ext2_mount_inode: ROOT_INODE,
            ext2_meta: Ext2Meta::empty(),
            ext2_image_len: 0,
            ext2_image: Vec::new(),
            ext2_backend: ext2_backend::Ext2Backend::none(),
            ext2_tracked_block_count: 0,
            ext2_dirty_blocks: Vec::new(),
            ext2_cached_blocks: Vec::new(),
            ext2_cache_max_blocks: EXT2_CACHE_MAX_BLOCKS_DEFAULT,
            ext2_cache_clock_hand: 0,
            ext2_cache_dirty_high_watermark_percent:
                EXT2_CACHE_DIRTY_HIGH_WATERMARK_PERCENT_DEFAULT,
            ext2_journal: Ext2JournalState::empty(),
            ext2_journal_running_tx: Ext2JournalRunningTx::empty(),
            ext2_journal_pending_txs: Vec::new(),
            ext2_journal_committed_txs: Vec::new(),
            ext2_journal_pending_revokes: Vec::new(),
            ext2_journal_next_tx_id: 1,
            ext2_read_only: false,
            ext4_csum: None,
            ext2_diag: None,
            prng_state: 0,
        }
    }
}

struct LockedVfsState {
    lock: AtomicBool,
    state: UnsafeCell<VfsState>,
}

impl LockedVfsState {
    const fn new() -> Self {
        Self {
            lock: AtomicBool::new(false),
            state: UnsafeCell::new(VfsState::new()),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut VfsState) -> R) -> R {
        while self
            .lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            spin_loop();
        }
        let result = f(unsafe { &mut *self.state.get() });
        self.lock.store(false, Ordering::Release);
        result
    }
}

unsafe impl Sync for LockedVfsState {}

static VFS_STATE: LockedVfsState = LockedVfsState::new();

#[derive(Clone, Copy)]
struct Ext2Meta {
    block_size: usize,
    inode_size: usize,
    inodes_per_group: u32,
    blocks_per_group: u32,
    total_blocks: u32,
    first_data_block: u32,
    total_inodes: u32,
    group_count: u32,
    gdt_offset: usize,
    feature_compat: u32,
    feature_incompat: u32,
    /// On-disk group descriptor size: 32, or `s_desc_size` with `64bit`.
    desc_size: usize,
    feature_ro_compat: u32,
    /// The image uses ext4 features that only mount read-only so far.
    ext4_read_only: bool,
    journal_inode: u32,
    journal_dev: u32,
    /// `s_hash_seed` (htree name hash seed, four LE words).
    hash_seed: [u32; 4],
    /// `s_def_hash_version` for newly indexed directories.
    def_hash_version: u8,
    /// `s_flags` selects unsigned-char name hashing.
    hash_unsigned: bool,
}

impl Ext2Meta {
    const fn empty() -> Self {
        Self {
            block_size: 0,
            inode_size: 0,
            inodes_per_group: 0,
            blocks_per_group: 0,
            total_blocks: 0,
            first_data_block: 0,
            total_inodes: 0,
            group_count: 0,
            gdt_offset: 0,
            feature_compat: 0,
            feature_incompat: 0,
            desc_size: EXT2_GROUP_DESC_SIZE,
            feature_ro_compat: 0,
            ext4_read_only: false,
            journal_inode: 0,
            journal_dev: 0,
            hash_seed: [0; 4],
            def_hash_version: 0,
            hash_unsigned: false,
        }
    }
}

#[derive(Clone, Copy)]
struct Ext2Inode {
    mode: u16,
    uid: u16,
    size: u32,
    /// Seconds since the epoch, decoded with the ext4 extra epoch bits
    /// (`i_*time_extra`) when the inode has room for them.
    atime: u64,
    ctime: u64,
    mtime: u64,
    links_count: u16,
    gid: u16,
    block_count_512: u32,
    blocks: [u32; 15],
    /// `i_flags` (offset 32). Read-only here: only dedicated helpers mutate it.
    flags: u32,
    /// `i_file_acl` (offset 104): external xattr block, or 0.
    file_acl: u32,
    /// `i_size_high` / `i_dir_acl` (offset 108). Read-only here.
    size_high: u32,
    /// Inode number this copy was read from (0 for synthetic inodes). With
    /// `i_generation` it seeds ext4 per-inode checksums (extent and
    /// directory blocks).
    ino: u32,
    /// `i_generation` (offset 0x64).
    generation: u32,
    /// `l_i_blocks_high` (offset 0x74): bits 32..47 of `i_blocks` (huge_file).
    blocks_hi: u16,
    /// Raw `i_{a,c,m}time_extra` as read, and the decoded times they went
    /// with: nanoseconds survive an inode write while a time is unchanged.
    time_extra: [u32; 3],
    time_orig: [u64; 3],
}

/// Byte offset and the `i_extra_isize` an inode needs for the
/// `i_{a,c,m}time_extra` fields.
const EXT4_TIME_EXTRA: [(usize, u16); 3] = [(0x8C, 16), (0x84, 8), (0x88, 12)];

/// Decodes an ext4 timestamp: the base is signed 32-bit seconds; the low two
/// bits of the extra field extend it past 2038 (fs/ext4/ext4.h).
fn ext4_decode_time(base: u32, extra: Option<u32>) -> u64 {
    let mut secs = base as i32 as i64;
    if let Some(extra) = extra {
        secs += ((extra & 3) as i64) << 32;
    }
    secs as u64
}

/// Splits `secs` into the 32-bit base and the epoch bits, clamping to what
/// fits (0x7FFF_FFFF without extra fields, year 2446 with them).
fn ext4_encode_time(secs: u64, has_extra: bool) -> (u32, u32) {
    let max = if has_extra { 0x3_7FFF_FFFFi64 } else { 0x7FFF_FFFF };
    let secs = core::cmp::min(secs as i64, max);
    let base = secs as i32;
    let epoch = ((secs - base as i64) >> 32) as u32 & 3;
    (base as u32, epoch)
}

/// `i_extra_isize` of the inode at `inode_offset` (0 for 128-byte inodes).
fn ext2_inode_extra_isize(state: &mut VfsState, inode_offset: usize) -> Result<u16, i64> {
    if state.ext2_meta.inode_size <= 128 {
        return Ok(0);
    }
    let extra = ext2_read_u16_state(state, inode_offset + 0x80)?;
    if (extra as usize) > state.ext2_meta.inode_size - 128 {
        return Ok(0);
    }
    Ok(extra)
}

fn with_state<R>(f: impl FnOnce(&mut VfsState) -> R) -> R {
    VFS_STATE.with(f)
}

fn fs_time_tick(state: &mut VfsState) -> u64 {
    state.clock_ticks = state.clock_ticks.saturating_add(1);
    state.clock_ticks
}

fn touch_inode_metadata(
    state: &mut VfsState,
    inode: u16,
    access: bool,
    modify: bool,
    change: bool,
) {
    if (inode as usize) >= state.inodes.len() {
        return;
    }
    let now = fs_time_tick(state);
    let node = &mut state.inodes[inode as usize];
    if !node.used {
        return;
    }
    if access {
        node.atime = now;
    }
    if modify {
        node.mtime = now;
    }
    if change {
        node.ctime = now;
    }
}

fn fill_xorshift_bytes(state: &mut VfsState, buf: &mut [u8]) {
    let mut s = state.prng_state;
    if s == 0 {
        s = 0x9E37_79B9_7F4A_7C15 ^ state.clock_ticks.wrapping_mul(0xA076_1D64_78BD_642F);
        if s == 0 {
            s = 1;
        }
    }
    for chunk in buf.chunks_mut(8) {
        s ^= s >> 12;
        s ^= s << 25;
        s ^= s >> 27;
        let sample = s.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes();
        chunk.copy_from_slice(&sample[..chunk.len()]);
    }
    state.prng_state = s;
}

fn mount_table_reset(state: &mut VfsState) {
    state.mounts.fill(MountEntry::empty());
}

fn mount_table_set(
    state: &mut VfsState,
    mount_inode: u16,
    path: &str,
    fs_name: &str,
    readonly: bool,
) -> Result<(), i64> {
    if path.is_empty() || path.len() >= VFS_MAX_PATH || fs_name.is_empty() || fs_name.len() > 8 {
        return Err(EINVAL);
    }

    let mut free_slot = None;
    let mut idx = 0usize;
    while idx < MOUNT_MAX {
        let entry = &state.mounts[idx];
        if entry.used
            && entry.mount_inode == mount_inode
            && entry.path_len as usize == path.len()
            && entry.path[..path.len()] == path.as_bytes()[..]
        {
            free_slot = Some(idx);
            break;
        }
        if free_slot.is_none() && !entry.used {
            free_slot = Some(idx);
        }
        idx += 1;
    }

    let slot = free_slot.ok_or(ENOSPC)?;
    let entry = &mut state.mounts[slot];
    *entry = MountEntry::empty();
    entry.used = true;
    entry.mount_inode = mount_inode;
    entry.path_len = path.len() as u16;
    entry.path[..path.len()].copy_from_slice(path.as_bytes());
    entry.fs_name_len = fs_name.len() as u8;
    entry.fs_name[..fs_name.len()].copy_from_slice(fs_name.as_bytes());
    entry.readonly = readonly;
    Ok(())
}

fn mount_table_remove_by_inode(state: &mut VfsState, mount_inode: u16) {
    let mut idx = 0usize;
    while idx < MOUNT_MAX {
        if state.mounts[idx].used && state.mounts[idx].mount_inode == mount_inode {
            state.mounts[idx] = MountEntry::empty();
        }
        idx += 1;
    }
}

fn inode_is_mount_root(state: &VfsState, inode: u16) -> bool {
    let mut idx = 0usize;
    while idx < MOUNT_MAX {
        let entry = &state.mounts[idx];
        if entry.used && entry.mount_inode == inode {
            return true;
        }
        idx += 1;
    }
    false
}

fn clear_inode(inode: &mut Inode) {
    inode.used = false;
    inode.file_type = FileType::None;
    inode.backend = NodeBackend::Tmpfs;
    inode.parent = ROOT_INODE;
    inode.name.fill(0);
    inode.name_len = 0;
    inode.size = 0;
    inode.children = Vec::new();
    inode.data = Vec::new();
    inode.ext2_ino = 0;
    inode.dev_id = 0;
    inode.ext2_children_loaded = false;
    inode.mode = 0;
    inode.uid = 0;
    inode.gid = 0;
    inode.links_count = 0;
    inode.atime = 0;
    inode.mtime = 0;
    inode.ctime = 0;
    inode.open_count = 0;
    inode.unlinked = false;
}

fn reset(state: &mut VfsState) {
    state.inodes = Vec::new();
    state.proc_uptime_ticks = 0;
    state.proc_total_bytes = 0;
    state.proc_free_bytes = 0;
    state.proc_used_bytes = 0;
    state.clock_ticks = 0;
    mount_table_reset(state);
    state.ext2_mounted = false;
    state.ext2_mount_inode = ROOT_INODE;
    state.ext2_meta = Ext2Meta::empty();
    state.ext2_journal = Ext2JournalState::empty();
    state.ext2_journal_running_tx = Ext2JournalRunningTx::empty();
    state.ext2_journal_pending_txs.clear();
    state.ext2_image_len = 0;
    state.ext2_image.clear();
    state.ext2_backend = ext2_backend::Ext2Backend::none();
    ext2_clear_dirty_tracking(state);
    state.ext2_cache_max_blocks = EXT2_CACHE_MAX_BLOCKS_DEFAULT;
    state.ext2_cache_dirty_high_watermark_percent = EXT2_CACHE_DIRTY_HIGH_WATERMARK_PERCENT_DEFAULT;
    state.initialized = false;
}

fn setup_root(state: &mut VfsState) {
    if state.inodes.is_empty() {
        state.inodes.push(Inode::empty());
    }
    let root = &mut state.inodes[ROOT_INODE as usize];
    clear_inode(root);
    root.used = true;
    root.file_type = FileType::Directory;
    root.backend = NodeBackend::Tmpfs;
    root.parent = ROOT_INODE;
    root.name_len = 0;
    root.mode = (STAT_S_IFDIR | 0o755) as u16;
    root.links_count = 2;
    root.atime = 1;
    root.mtime = 1;
    root.ctime = 1;
}

/// Format "/dev/pts/N" into buf, return length written.
fn format_pts_path(idx: u16, buf: &mut [u8; 16]) -> usize {
    let prefix = b"/dev/pts/";
    let plen = prefix.len();
    buf[..plen].copy_from_slice(prefix);
    if idx >= 10 {
        buf[plen] = b'0' + (idx / 10) as u8;
        buf[plen + 1] = b'0' + (idx % 10) as u8;
        plen + 2
    } else {
        buf[plen] = b'0' + idx as u8;
        plen + 1
    }
}

fn ensure_init(state: &mut VfsState) {
    if state.initialized {
        return;
    }

    reset(state);
    setup_root(state);
    state.initialized = true;

    let _ = mkdir_inner(state, "/tmp");
    let _ = mkdir_inner(state, "/temp");
    let _ = mkdir_inner(state, "/boot");
    let _ = mkdir_inner(state, "/etc");

    let _ = mkdir_virtual_inner(state, "/dev");
    let _ = create_virtual_file_inner(state, "/dev/null", NodeBackend::DevNull);
    let _ = create_virtual_file_inner(state, "/dev/zero", NodeBackend::DevZero);
    let _ = create_virtual_file_inner(state, "/dev/console", NodeBackend::DevConsole);
    let _ = create_virtual_file_inner(state, "/dev/urandom", NodeBackend::DevUrandom);
    let _ = create_virtual_file_inner(state, "/dev/tty", NodeBackend::DevTty);
    let _ = create_virtual_file_inner(state, "/dev/ptmx", NodeBackend::DevPtmx);
    let _ = create_virtual_file_inner(state, "/dev/fb0", NodeBackend::DevFramebuffer);
    // Listing is synthesized per process by the kernel (getdents64).
    let _ = mkdir_virtual_inner(state, "/dev/fd");
    let _ = mkdir_virtual_inner(state, "/dev/input");
    if let Ok(inode) =
        create_virtual_file_inner(state, "/dev/input/event0", NodeBackend::DevInputEvent)
    {
        state.inodes[inode as usize].dev_id = 0;
    }
    if let Ok(inode) =
        create_virtual_file_inner(state, "/dev/input/event1", NodeBackend::DevInputEvent)
    {
        state.inodes[inode as usize].dev_id = 1;
    }

    let _ = mkdir_virtual_inner(state, "/dev/pts");
    {
        let mut pty_idx: u16 = 0;
        while (pty_idx as usize) < 4 {
            let mut path_buf = [0u8; 16];
            let path_len = format_pts_path(pty_idx, &mut path_buf);
            // Safety: path_buf contains only ASCII digits prefixed by /dev/pts/
            let path = unsafe { core::str::from_utf8_unchecked(&path_buf[..path_len]) };
            if let Ok(inode) = create_virtual_file_inner(state, path, NodeBackend::DevPtsSlave) {
                state.inodes[inode as usize].dev_id = pty_idx;
            }
            pty_idx += 1;
        }
    }

    let _ = mkdir_virtual_inner(state, "/proc");
    let _ = create_virtual_file_inner(state, "/proc/uptime", NodeBackend::ProcUptime);
    let _ = create_virtual_file_inner(state, "/proc/meminfo", NodeBackend::ProcMeminfo);
    let _ = create_virtual_file_inner(state, "/proc/mounts", NodeBackend::ProcMounts);
    let _ = create_virtual_file_inner(state, "/proc/version", NodeBackend::ProcVersion);

    let _ = mount_table_set(state, ROOT_INODE, "/", "tmpfs", false);
    if let Ok(temp_inode) = lookup_path_inner(state, "/temp") {
        let _ = mount_table_set(state, temp_inode, "/temp", "tmpfs", false);
    }
    if let Ok(boot_inode) = lookup_path_inner(state, "/boot") {
        let _ = mount_table_set(state, boot_inode, "/boot", "bootfs", false);
    }
    if let Ok(dev_inode) = lookup_path_inner(state, "/dev") {
        let _ = mount_table_set(state, dev_inode, "/dev", "devfs", false);
    }
    if let Ok(proc_inode) = lookup_path_inner(state, "/proc") {
        let _ = mount_table_set(state, proc_inode, "/proc", "procfs", true);
    }
}

fn name_eq(inode: &Inode, name: &[u8]) -> bool {
    if inode.name_len as usize != name.len() {
        return false;
    }
    inode.name[..name.len()] == *name
}

fn is_root_pseudo_mount_name(name: &[u8]) -> bool {
    ROOT_PSEUDO_MOUNT_NAMES
        .iter()
        .any(|candidate| *candidate == name)
}

fn find_child(state: &VfsState, dir: u16, name: &[u8]) -> Option<u16> {
    let inode = &state.inodes[dir as usize];
    let mut idx = 0usize;
    while idx < inode.children.len() {
        let child = inode.children[idx];
        let child_inode = &state.inodes[child as usize];
        if child_inode.used && name_eq(child_inode, name) {
            return Some(child);
        }
        idx += 1;
    }
    None
}

/// Child `name` of `dir`. For an ext2-family directory whose children are
/// not all imported yet, only that name is looked up on disk (through the
/// htree index when the directory has one) and imported.
fn find_child_loaded(state: &mut VfsState, dir: u16, name: &[u8]) -> Result<Option<u16>, i64> {
    if let Some(child) = find_child(state, dir, name) {
        return Ok(Some(child));
    }
    let (backend, file_type, loaded, dir_ino) = {
        let node = &state.inodes[dir as usize];
        (node.backend, node.file_type, node.ext2_children_loaded, node.ext2_ino)
    };
    if !state.ext2_mounted
        || backend != NodeBackend::Ext2
        || file_type != FileType::Directory
        || loaded
    {
        return Ok(None);
    }
    if dir_ino == 0 {
        return Err(EIO);
    }
    if name.is_empty() || name == b"." || name == b".." || name.len() >= VFS_MAX_NAME {
        return Ok(None);
    }
    match ext2_dir::ext2_dir_lookup(state, dir_ino, name)? {
        Some((ino, _)) => {
            ext2_import_node(state, dir, name, ino, false)?;
            Ok(find_child(state, dir, name))
        }
        None => Ok(None),
    }
}

fn add_child(state: &mut VfsState, dir: u16, child: u16) -> Result<(), i64> {
    let parent = &mut state.inodes[dir as usize];
    if parent.file_type != FileType::Directory {
        return Err(ENOTDIR);
    }
    parent.children.try_reserve(1).map_err(|_| ENOSPC)?;
    parent.children.push(child);
    Ok(())
}

fn two_inodes_mut(state: &mut VfsState, left: u16, right: u16) -> Option<(&mut Inode, &mut Inode)> {
    let left = left as usize;
    let right = right as usize;
    if left >= state.inodes.len() || right >= state.inodes.len() || left == right {
        return None;
    }

    if left < right {
        let (head, tail) = state.inodes.split_at_mut(right);
        Some((&mut head[left], &mut tail[0]))
    } else {
        let (head, tail) = state.inodes.split_at_mut(left);
        Some((&mut tail[0], &mut head[right]))
    }
}

fn remove_child(state: &mut VfsState, dir: u16, child: u16) -> Result<(), i64> {
    if (dir as usize) >= state.inodes.len() {
        return Err(ENOENT);
    }
    let parent = &mut state.inodes[dir as usize];
    if parent.file_type != FileType::Directory {
        return Err(ENOTDIR);
    }
    match parent.children.iter().position(|&c| c == child) {
        Some(idx) => {
            parent.children.remove(idx);
            Ok(())
        }
        None => Err(ENOENT),
    }
}

fn alloc_inode(
    state: &mut VfsState,
    file_type: FileType,
    backend: NodeBackend,
    parent: u16,
    name: &[u8],
) -> Result<u16, i64> {
    if name.is_empty() || name.len() >= VFS_MAX_NAME {
        return Err(ENAMETOOLONG);
    }

    let mut slot = 1usize;
    while slot < state.inodes.len() && state.inodes[slot].used {
        slot += 1;
    }
    if slot == state.inodes.len() {
        if slot >= VFS_MAX_INODES {
            return Err(ENOSPC);
        }
        state.inodes.try_reserve(1).map_err(|_| ENOSPC)?;
        state.inodes.push(Inode::empty());
    }
    {
        {
            let now = fs_time_tick(state);
            let inode = &mut state.inodes[slot];
            clear_inode(inode);
            inode.used = true;
            inode.file_type = file_type;
            inode.backend = backend;
            inode.parent = parent;
            inode.name_len = name.len() as u8;
            inode.name[..name.len()].copy_from_slice(name);
            inode.dev_id = 0;
            inode.ext2_children_loaded =
                !(backend == NodeBackend::Ext2 && file_type == FileType::Directory);
            inode.mode = match file_type {
                FileType::Directory => (STAT_S_IFDIR | 0o755) as u16,
                FileType::Regular => (STAT_S_IFREG | 0o644) as u16,
                FileType::Symlink => (STAT_S_IFLNK | 0o777) as u16,
                FileType::None => 0,
            };
            inode.uid = 0;
            inode.gid = 0;
            inode.links_count = if file_type == FileType::Directory {
                2
            } else {
                1
            };
            inode.atime = now;
            inode.mtime = now;
            inode.ctime = now;
            Ok(slot as u16)
        }
    }
}

fn has_free_vfs_inode(state: &VfsState) -> bool {
    state.inodes.len() < VFS_MAX_INODES || state.inodes.iter().skip(1).any(|n| !n.used)
}

fn next_component(
    path: &[u8],
    cursor: &mut usize,
    out: &mut [u8; VFS_MAX_NAME],
) -> Result<Option<usize>, i64> {
    while *cursor < path.len() && path[*cursor] == b'/' {
        *cursor += 1;
    }

    if *cursor >= path.len() {
        return Ok(None);
    }

    let start = *cursor;
    while *cursor < path.len() && path[*cursor] != b'/' {
        *cursor += 1;
    }

    let len = *cursor - start;
    if len == 0 {
        return Ok(None);
    }
    if len >= VFS_MAX_NAME {
        return Err(ENAMETOOLONG);
    }

    out[..len].copy_from_slice(&path[start..*cursor]);
    Ok(Some(len))
}

fn has_remaining_component(path: &[u8], mut cursor: usize) -> bool {
    while cursor < path.len() {
        if path[cursor] != b'/' {
            return true;
        }
        cursor += 1;
    }
    false
}

fn remap_legacy_ext2_root_path<'a>(state: &VfsState, path: &'a str) -> &'a str {
    if !state.ext2_mounted || state.ext2_mount_inode != ROOT_INODE {
        return path;
    }
    if !path.starts_with(LEGACY_EXT2_MOUNT_PREFIX) {
        return path;
    }
    if path.len() == LEGACY_EXT2_MOUNT_PREFIX.len() {
        return "/";
    }
    if path.as_bytes()[LEGACY_EXT2_MOUNT_PREFIX.len()] != b'/' {
        return path;
    }
    &path[LEGACY_EXT2_MOUNT_PREFIX.len()..]
}

fn lookup_path_from_inner(state: &mut VfsState, start: u16, path: &str) -> Result<u16, i64> {
    let path = remap_legacy_ext2_root_path(state, path);
    if path.is_empty() {
        return Err(EINVAL);
    }

    if path == "/" {
        return Ok(ROOT_INODE);
    }

    let bytes = path.as_bytes();
    let absolute = bytes[0] == b'/';
    let mut cursor = if absolute { 1usize } else { 0usize };
    let mut current = if absolute { ROOT_INODE } else { start };
    if current as usize >= state.inodes.len() || !state.inodes[current as usize].used {
        return Err(ENOENT);
    }
    let mut name = [0u8; VFS_MAX_NAME];

    loop {
        let len = match next_component(bytes, &mut cursor, &mut name)? {
            Some(v) => v,
            None => break,
        };

        if len == 1 && name[0] == b'.' {
            continue;
        }

        if len == 2 && name[0] == b'.' && name[1] == b'.' {
            current = if current == ROOT_INODE {
                ROOT_INODE
            } else {
                state.inodes[current as usize].parent
            };
            continue;
        }

        if state.inodes[current as usize].file_type != FileType::Directory {
            return Err(ENOTDIR);
        }

        let child = find_child_loaded(state, current, &name[..len])?.ok_or(ENOENT)?;
        current = child;
    }

    Ok(current)
}

fn lookup_path_inner(state: &mut VfsState, path: &str) -> Result<u16, i64> {
    lookup_path_from_inner(state, ROOT_INODE, path)
}

fn resolve_parent_from_inner(
    state: &mut VfsState,
    start: u16,
    path: &str,
    out_name: &mut [u8; VFS_MAX_NAME],
) -> Result<(u16, usize), i64> {
    let path = remap_legacy_ext2_root_path(state, path);
    if path.is_empty() || path == "/" {
        return Err(EINVAL);
    }

    let bytes = path.as_bytes();
    let absolute = bytes[0] == b'/';
    let mut cursor = if absolute { 1usize } else { 0usize };
    let mut current = if absolute { ROOT_INODE } else { start };
    if current as usize >= state.inodes.len() || !state.inodes[current as usize].used {
        return Err(ENOENT);
    }
    let mut component = [0u8; VFS_MAX_NAME];

    loop {
        let len = match next_component(bytes, &mut cursor, &mut component)? {
            Some(v) => v,
            None => return Err(EINVAL),
        };

        let is_last = !has_remaining_component(bytes, cursor);
        if is_last {
            if len == 1 && component[0] == b'.' {
                return Err(EINVAL);
            }
            if len == 2 && component[0] == b'.' && component[1] == b'.' {
                return Err(EINVAL);
            }
            if state.inodes[current as usize].unlinked {
                // A removed (but still open) directory takes no new entries.
                return Err(ENOENT);
            }
            out_name[..len].copy_from_slice(&component[..len]);
            return Ok((current, len));
        }

        if len == 1 && component[0] == b'.' {
            continue;
        }

        if len == 2 && component[0] == b'.' && component[1] == b'.' {
            current = if current == ROOT_INODE {
                ROOT_INODE
            } else {
                state.inodes[current as usize].parent
            };
            continue;
        }

        if state.inodes[current as usize].file_type != FileType::Directory {
            return Err(ENOTDIR);
        }

        let child = find_child_loaded(state, current, &component[..len])?.ok_or(ENOENT)?;
        if state.inodes[child as usize].file_type != FileType::Directory {
            return Err(ENOTDIR);
        }
        current = child;
    }
}

fn resolve_parent_inner(
    state: &mut VfsState,
    path: &str,
    out_name: &mut [u8; VFS_MAX_NAME],
) -> Result<(u16, usize), i64> {
    resolve_parent_from_inner(state, ROOT_INODE, path, out_name)
}

fn create_regular_at_inner(state: &mut VfsState, start: u16, path: &str) -> Result<u16, i64> {
    let mut name = [0u8; VFS_MAX_NAME];
    let (parent, name_len) = resolve_parent_from_inner(state, start, path, &mut name)?;

    let parent_backend = state.inodes[parent as usize].backend;
    if !backend_mutable(state, parent_backend) {
        return Err(EROFS);
    }

    if find_child_loaded(state, parent, &name[..name_len])?.is_some() {
        return Err(EEXIST);
    }

    if parent_backend == NodeBackend::Ext2 {
        return with_ext2_journal_scope_credits(state, true, EXT2_JOURNAL_CREDITS_CREATE, |state| {
            if state.inodes[parent as usize].ext2_ino == 0 {
                return Err(EIO);
            }
            if !has_free_vfs_inode(state) {
                return Err(ENOSPC);
            }

            let (ext2_ino, ext2_size) = ext2_create_child_on_disk(
                state,
                ext2_journal::current_handle(state),
                state.inodes[parent as usize].ext2_ino,
                &name[..name_len],
                FileType::Regular,
            )?;

            let inode = alloc_inode(
                state,
                FileType::Regular,
                NodeBackend::Ext2,
                parent,
                &name[..name_len],
            )?;
            let node = &mut state.inodes[inode as usize];
            node.ext2_ino = ext2_ino;
            node.size = ext2_size as u64;
            add_child(state, parent, inode)?;
            touch_inode_metadata(state, parent, true, true, true);
            Ok(inode)
        });
    }

    let inode = alloc_inode(
        state,
        FileType::Regular,
        NodeBackend::Tmpfs,
        parent,
        &name[..name_len],
    )?;
    add_child(state, parent, inode)?;
    touch_inode_metadata(state, parent, true, true, true);
    Ok(inode)
}

fn mkdir_at_inner(state: &mut VfsState, start: u16, path: &str) -> Result<(), i64> {
    let mut name = [0u8; VFS_MAX_NAME];
    let (parent, name_len) = resolve_parent_from_inner(state, start, path, &mut name)?;

    let parent_backend = state.inodes[parent as usize].backend;
    if !backend_mutable(state, parent_backend) {
        return Err(EROFS);
    }

    if find_child_loaded(state, parent, &name[..name_len])?.is_some() {
        return Err(EEXIST);
    }

    if parent_backend == NodeBackend::Ext2 {
        return with_ext2_journal_scope_credits(state, true, EXT2_JOURNAL_CREDITS_MKDIR, |state| {
            if state.inodes[parent as usize].ext2_ino == 0 {
                return Err(EIO);
            }
            if !has_free_vfs_inode(state) {
                return Err(ENOSPC);
            }

            let (ext2_ino, ext2_size) = ext2_create_child_on_disk(
                state,
                ext2_journal::current_handle(state),
                state.inodes[parent as usize].ext2_ino,
                &name[..name_len],
                FileType::Directory,
            )?;

            let inode = alloc_inode(
                state,
                FileType::Directory,
                NodeBackend::Ext2,
                parent,
                &name[..name_len],
            )?;
            let node = &mut state.inodes[inode as usize];
            node.ext2_ino = ext2_ino;
            node.size = ext2_size as u64;
            node.ext2_children_loaded = true;
            add_child(state, parent, inode)?;
            touch_inode_metadata(state, parent, true, true, true);
            Ok(())
        });
    }

    let inode = alloc_inode(
        state,
        FileType::Directory,
        NodeBackend::Tmpfs,
        parent,
        &name[..name_len],
    )?;
    add_child(state, parent, inode)?;
    touch_inode_metadata(state, parent, true, true, true);
    Ok(())
}

fn mkdir_inner(state: &mut VfsState, path: &str) -> Result<(), i64> {
    mkdir_at_inner(state, ROOT_INODE, path)
}

fn mkdir_virtual_inner(state: &mut VfsState, path: &str) -> Result<(), i64> {
    let mut name = [0u8; VFS_MAX_NAME];
    let (parent, name_len) = resolve_parent_inner(state, path, &mut name)?;

    if find_child_loaded(state, parent, &name[..name_len])?.is_some() {
        return Err(EEXIST);
    }

    let inode = alloc_inode(
        state,
        FileType::Directory,
        NodeBackend::Tmpfs,
        parent,
        &name[..name_len],
    )?;
    add_child(state, parent, inode)?;
    Ok(())
}

fn create_virtual_file_inner(
    state: &mut VfsState,
    path: &str,
    backend: NodeBackend,
) -> Result<u16, i64> {
    let mut name = [0u8; VFS_MAX_NAME];
    let (parent, name_len) = resolve_parent_inner(state, path, &mut name)?;

    if find_child_loaded(state, parent, &name[..name_len])?.is_some() {
        return Err(EEXIST);
    }

    let inode = alloc_inode(state, FileType::Regular, backend, parent, &name[..name_len])?;
    {
        let node = &mut state.inodes[inode as usize];
        node.mode = match backend {
            NodeBackend::ProcUptime
            | NodeBackend::ProcMeminfo
            | NodeBackend::ProcMounts
            | NodeBackend::ProcVersion => (STAT_S_IFREG | 0o444) as u16,
            NodeBackend::DevNull
            | NodeBackend::DevZero
            | NodeBackend::DevConsole
            | NodeBackend::DevUrandom
            | NodeBackend::DevTty
            | NodeBackend::DevPtmx
            | NodeBackend::DevPtsSlave
            | NodeBackend::DevFramebuffer => (STAT_S_IFREG | 0o666) as u16,
            NodeBackend::DevInputEvent => (STAT_S_IFREG | 0o444) as u16,
            _ => node.mode,
        };
    }
    add_child(state, parent, inode)?;
    Ok(inode)
}

fn append_bytes(out: &mut [u8], cursor: &mut usize, src: &[u8]) {
    let remaining = out.len().saturating_sub(*cursor);
    if remaining == 0 {
        return;
    }
    let take = core::cmp::min(remaining, src.len());
    out[*cursor..*cursor + take].copy_from_slice(&src[..take]);
    *cursor += take;
}

fn append_str(out: &mut [u8], cursor: &mut usize, s: &str) {
    append_bytes(out, cursor, s.as_bytes());
}

fn append_u64(out: &mut [u8], cursor: &mut usize, value: u64) {
    let mut tmp = [0u8; 20];
    let mut len = 0usize;
    let mut n = value;

    if n == 0 {
        tmp[0] = b'0';
        len = 1;
    } else {
        while n > 0 && len < tmp.len() {
            tmp[len] = b'0' + (n % 10) as u8;
            n /= 10;
            len += 1;
        }
    }

    let mut idx = len;
    while idx > 0 {
        idx -= 1;
        append_bytes(out, cursor, &tmp[idx..idx + 1]);
    }
}

fn append_u64_fixed_2(out: &mut [u8], cursor: &mut usize, value: u64) {
    let tens = ((value / 10) % 10) as u8;
    let ones = (value % 10) as u8;
    append_bytes(out, cursor, &[b'0' + tens, b'0' + ones]);
}

fn append_mount_line(out: &mut [u8], cursor: &mut usize, entry: &MountEntry) {
    if !entry.used || entry.path_len == 0 || entry.fs_name_len == 0 {
        return;
    }
    append_bytes(out, cursor, &entry.fs_name[..entry.fs_name_len as usize]);
    append_str(out, cursor, " ");
    append_bytes(out, cursor, &entry.path[..entry.path_len as usize]);
    append_str(out, cursor, " ");
    append_bytes(out, cursor, &entry.fs_name[..entry.fs_name_len as usize]);
    append_str(out, cursor, " ");
    if entry.readonly {
        append_str(out, cursor, "ro");
    } else {
        append_str(out, cursor, "rw");
    }
    append_str(out, cursor, " 0 0\n");
}

fn append_mounts(state: &VfsState, out: &mut [u8], cursor: &mut usize) {
    let mut idx = 0usize;
    while idx < MOUNT_MAX {
        let entry = &state.mounts[idx];
        if entry.used {
            append_mount_line(out, cursor, entry);
        }
        idx += 1;
    }
}

fn format_proc_payload(state: &VfsState, backend: NodeBackend, out: &mut [u8]) -> usize {
    let mut cursor = 0usize;
    match backend {
        NodeBackend::ProcVersion => {
            append_str(out, &mut cursor, "PeetOS-Rust 0.1\n");
        }
        NodeBackend::ProcMounts => {
            append_mounts(state, out, &mut cursor);
        }
        NodeBackend::ProcUptime => {
            let secs = state.proc_uptime_ticks / PROC_TICK_HZ;
            let frac = state.proc_uptime_ticks % PROC_TICK_HZ;
            append_u64(out, &mut cursor, secs);
            append_str(out, &mut cursor, ".");
            append_u64_fixed_2(out, &mut cursor, frac);
            append_str(out, &mut cursor, " ");
            append_u64(out, &mut cursor, secs);
            append_str(out, &mut cursor, ".");
            append_u64_fixed_2(out, &mut cursor, frac);
            append_str(out, &mut cursor, "\n");
        }
        NodeBackend::ProcMeminfo => {
            let total = state.proc_total_bytes / 1024;
            let free = state.proc_free_bytes / 1024;
            let used = if state.proc_used_bytes == 0 {
                total.saturating_sub(free)
            } else {
                state.proc_used_bytes / 1024
            };
            append_str(out, &mut cursor, "MemTotal: ");
            append_u64(out, &mut cursor, total);
            append_str(out, &mut cursor, " kB\n");
            append_str(out, &mut cursor, "MemFree: ");
            append_u64(out, &mut cursor, free);
            append_str(out, &mut cursor, " kB\n");
            append_str(out, &mut cursor, "MemUsed: ");
            append_u64(out, &mut cursor, used);
            append_str(out, &mut cursor, " kB\n");
        }
        _ => {}
    }
    cursor
}

fn read_proc(node: &Inode, state: &VfsState, offset: u64, dst: &mut [u8]) -> Result<usize, i64> {
    let mut buf = [0u8; PROCFS_BUFFER_MAX];
    let len = format_proc_payload(state, node.backend, &mut buf);

    if offset as usize >= len || dst.is_empty() {
        return Ok(0);
    }

    let start = offset as usize;
    let available = len - start;
    let count = core::cmp::min(dst.len(), available);
    dst[..count].copy_from_slice(&buf[start..start + count]);
    Ok(count)
}

fn read_u16_le(buf: &[u8], offset: usize) -> Result<u16, i64> {
    if offset + 2 > buf.len() {
        return Err(EIO);
    }
    Ok(u16::from_le_bytes([buf[offset], buf[offset + 1]]))
}

fn read_u32_le(buf: &[u8], offset: usize) -> Result<u32, i64> {
    if offset + 4 > buf.len() {
        return Err(EIO);
    }
    Ok(u32::from_le_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
    ]))
}

fn write_u16_le(buf: &mut [u8], offset: usize, value: u16) -> Result<(), i64> {
    if offset + 2 > buf.len() {
        return Err(EIO);
    }
    let bytes = value.to_le_bytes();
    buf[offset] = bytes[0];
    buf[offset + 1] = bytes[1];
    Ok(())
}

fn write_u32_le(buf: &mut [u8], offset: usize, value: u32) -> Result<(), i64> {
    if offset + 4 > buf.len() {
        return Err(EIO);
    }
    let bytes = value.to_le_bytes();
    buf[offset] = bytes[0];
    buf[offset + 1] = bytes[1];
    buf[offset + 2] = bytes[2];
    buf[offset + 3] = bytes[3];
    Ok(())
}

/// Parses and validates a superblock for mounting: the layout must be sane
/// and every feature bit must be one this implementation supports.
fn ext2_parse_meta_with_len(
    image: &[u8],
    logical_image_len: usize,
    require_gdt_resident: bool,
) -> Result<Ext2Meta, i64> {
    let mut meta = ext2_parse_layout_with_len(image, logical_image_len, require_gdt_resident)?;
    if (meta.feature_compat & !EXT2_FEATURE_COMPAT_SUPPORTED) != 0 {
        return Err(EINVAL);
    }
    let incompat_ok = EXT2_FEATURE_INCOMPAT_SUPPORTED | EXT4_FEATURE_INCOMPAT_ALL;
    if (meta.feature_incompat & !incompat_ok) != 0 {
        return Err(EINVAL);
    }
    let ro_compat_ok = EXT2_FEATURE_RO_COMPAT_SUPPORTED | EXT4_FEATURE_RO_COMPAT_ALL;
    if (meta.feature_ro_compat & !ro_compat_ok) != 0 {
        return Err(EINVAL);
    }
    // Two checksum schemes for group descriptors cannot both be active.
    if (meta.feature_ro_compat & EXT4_FEATURE_RO_COMPAT_GDT_CSUM) != 0
        && (meta.feature_ro_compat & EXT4_FEATURE_RO_COMPAT_METADATA_CSUM) != 0
    {
        return Err(EINVAL);
    }
    meta.ext4_read_only = (meta.feature_incompat & EXT4_FEATURE_INCOMPAT_READ_ONLY) != 0
        || (meta.feature_ro_compat & EXT4_FEATURE_RO_COMPAT_READ_ONLY) != 0;
    Ok(meta)
}

/// Parses the on-disk layout (geometry, descriptor size, journal fields)
/// without judging feature bits, so images that do not mount yet (ext4) can
/// still be inspected by tests and diagnostics.
fn ext2_parse_layout_with_len(
    image: &[u8],
    logical_image_len: usize,
    require_gdt_resident: bool,
) -> Result<Ext2Meta, i64> {
    // Parse only the superblock fields we actually consume.
    if image.len() < EXT2_SUPERBLOCK_OFFSET + 236 {
        return Err(EIO);
    }

    let sb = EXT2_SUPERBLOCK_OFFSET;
    let magic = read_u16_le(image, sb + 56)?;
    if magic != EXT2_SUPER_MAGIC {
        return Err(EINVAL);
    }

    let total_inodes = read_u32_le(image, sb + 0)?;
    let total_blocks = read_u32_le(image, sb + 4)?;
    let first_data_block = read_u32_le(image, sb + 20)?;
    let log_block_size = read_u32_le(image, sb + 24)?;
    let blocks_per_group = read_u32_le(image, sb + 32)?;
    let inodes_per_group = read_u32_le(image, sb + 40)?;
    let rev_level = read_u32_le(image, sb + 76)?;
    let feature_compat = read_u32_le(image, sb + 92)?;
    let feature_incompat = read_u32_le(image, sb + 96)?;
    let journal_inode = read_u32_le(image, sb + 224)?;
    let journal_dev = read_u32_le(image, sb + 228)?;

    if total_inodes == 0 || total_blocks == 0 || blocks_per_group == 0 || inodes_per_group == 0 {
        return Err(EINVAL);
    }

    let block_size = 1024usize.checked_shl(log_block_size).ok_or(EINVAL)?;
    if block_size < 1024 || block_size > 4096 {
        return Err(EINVAL);
    }

    let inode_size = if rev_level == 0 {
        128usize
    } else {
        let size = read_u16_le(image, sb + 88)? as usize;
        if size < 128 || (size & 0x3) != 0 {
            return Err(EINVAL);
        }
        size
    };

    let group_count = (total_blocks + blocks_per_group - 1) / blocks_per_group;
    if group_count == 0 {
        return Err(EINVAL);
    }

    let gdt_offset = if block_size == 1024 {
        2 * 1024
    } else {
        block_size
    };

    let desc_size = ext2_desc_size_from_superblock(image, feature_incompat)?;
    let mut hash_seed = [0u32; 4];
    let mut def_hash_version = ext2_htree::DX_HASH_HALF_MD4;
    let mut hash_unsigned = false;
    if image.len() >= sb + 0x164 {
        for (i, word) in hash_seed.iter_mut().enumerate() {
            *word = read_u32_le(image, sb + 0xEC + i * 4)?;
        }
        def_hash_version = image[sb + 0xFC];
        let flags = read_u32_le(image, sb + 0x160)?;
        hash_unsigned = (flags & ext2_htree::EXT2_FLAGS_UNSIGNED_HASH) != 0;
    }
    let gdt_size = (group_count as usize)
        .checked_mul(desc_size)
        .ok_or(EIO)?;
    let gdt_end = gdt_offset.checked_add(gdt_size).ok_or(EIO)?;
    if gdt_end > logical_image_len {
        return Err(EIO);
    }
    if require_gdt_resident && gdt_end > image.len() {
        return Err(EIO);
    }

    Ok(Ext2Meta {
        block_size,
        inode_size,
        inodes_per_group,
        blocks_per_group,
        total_blocks,
        first_data_block,
        total_inodes,
        group_count,
        gdt_offset,
        feature_compat,
        feature_incompat,
        desc_size,
        feature_ro_compat: read_u32_le(image, sb + 100)?,
        ext4_read_only: false,
        journal_inode,
        journal_dev,
        hash_seed,
        def_hash_version,
        hash_unsigned,
    })
}

fn ext2_tracked_block_count(meta: &Ext2Meta, image_len: usize) -> Result<usize, i64> {
    if meta.block_size == 0 || (image_len % meta.block_size) != 0 {
        return Err(EIO);
    }
    Ok(image_len / meta.block_size)
}

fn ext2_set_image_len(state: &mut VfsState, image_len: usize) {
    state.ext2_image_len = image_len;
}

fn ext2_set_cache_policy(
    state: &mut VfsState,
    cache_max_blocks: usize,
    dirty_high_watermark_percent: u8,
) -> Result<(), i64> {
    if cache_max_blocks == 0 {
        return Err(EINVAL);
    }
    if dirty_high_watermark_percent == 0 || dirty_high_watermark_percent > 100 {
        return Err(EINVAL);
    }

    state.ext2_cache_max_blocks = cache_max_blocks;
    state.ext2_cache_dirty_high_watermark_percent = dirty_high_watermark_percent;
    while state.ext2_cached_blocks.len() > state.ext2_cache_max_blocks {
        ext2_cache_evict_one_clock(state)?;
    }
    if state.ext2_cache_clock_hand >= state.ext2_cached_blocks.len() {
        state.ext2_cache_clock_hand = 0;
    }
    Ok(())
}

fn ext2_cache_find(state: &VfsState, block: u32) -> Result<usize, usize> {
    state
        .ext2_cached_blocks
        .binary_search_by_key(&block, |entry| entry.block_index)
}

fn ext2_unmark_dirty_block(state: &mut VfsState, block: u32) -> bool {
    match state.ext2_dirty_blocks.binary_search(&block) {
        Ok(idx) => {
            state.ext2_dirty_blocks.remove(idx);
            true
        }
        Err(_) => false,
    }
}

fn ext2_cache_flush_block(state: &mut VfsState, block: u32) -> Result<(), i64> {
    let idx = match ext2_cache_find(state, block) {
        Ok(idx) => idx,
        Err(_) => {
            ext2_unmark_dirty_block(state, block);
            return Ok(());
        }
    };

    if !state.ext2_cached_blocks[idx].dirty {
        ext2_unmark_dirty_block(state, block);
        return Ok(());
    }

    if state.ext2_journal.present
        && !state.ext2_journal.replaying
        && block != state.ext2_journal.superblock_block
        && block != state.ext2_journal.tx_descriptor_fs_block
        && block != state.ext2_journal.tx_data_fs_block
        && block != state.ext2_journal.tx_commit_fs_block
    {
        return ext2_journal::flush_dirty_block(state, block);
    }

    // Same raw write path as the journal: superblock-block writes get the
    // recovery-marker and metadata_csum checksum fixups there.
    let data = state.ext2_cached_blocks[idx].data.clone();
    ext2_journal::backend_write_block_raw(state, block, data.as_slice())?;
    state.ext2_cached_blocks[idx].dirty = false;
    ext2_unmark_dirty_block(state, block);
    Ok(())
}

fn ext2_cache_evict_one_clock(state: &mut VfsState) -> Result<(), i64> {
    if state.ext2_cached_blocks.is_empty() {
        state.ext2_cache_clock_hand = 0;
        return Ok(());
    }

    let max_attempts = core::cmp::max(1usize, state.ext2_cached_blocks.len().saturating_mul(2));
    let mut attempts = 0usize;
    loop {
        if attempts >= max_attempts {
            return Err(ENOSPC);
        }
        attempts += 1;
        if state.ext2_cache_clock_hand >= state.ext2_cached_blocks.len() {
            state.ext2_cache_clock_hand = 0;
        }
        let idx = state.ext2_cache_clock_hand;
        if state.ext2_cached_blocks[idx].referenced {
            state.ext2_cached_blocks[idx].referenced = false;
            state.ext2_cache_clock_hand = (idx.saturating_add(1)) % state.ext2_cached_blocks.len();
            continue;
        }

        let block = state.ext2_cached_blocks[idx].block_index;
        if state.ext2_cached_blocks[idx].dirty {
            if state.ext2_journal.present
                && !state.ext2_journal.replaying
                && state.ext2_journal.mode == Ext2JournalMode::Ordered
                && state.ext2_journal_running_tx.scope_depth > 0
                && state
                    .ext2_journal_running_tx
                    .tracked_blocks
                    .binary_search(&block)
                    .is_ok()
            {
                state.ext2_cache_clock_hand =
                    (idx.saturating_add(1)) % state.ext2_cached_blocks.len();
                continue;
            }
            if state.ext2_journal.present
                && !state.ext2_journal.replaying
                && state.ext2_journal.mode == Ext2JournalMode::Ordered
                && !state.ext2_journal_pending_txs.is_empty()
            {
                let flushed = ext2_journal::flush_oldest_ordered_pending_tx(state, 64)?;
                if flushed == 0 {
                    return Err(EIO);
                }
                continue;
            }
            ext2_cache_flush_block(state, block)?;
        }
        // Clean now, but possibly newer than its home copy (committed, not yet
        // checkpointed): make the home copy current before dropping it.
        ext2_journal::checkpoint_through_block(state, block)?;
        if idx >= state.ext2_cached_blocks.len()
            || state.ext2_cached_blocks[idx].block_index != block
            || state.ext2_cached_blocks[idx].dirty
        {
            continue;
        }

        state.ext2_cached_blocks.remove(idx);
        if state.ext2_cached_blocks.is_empty() || idx >= state.ext2_cached_blocks.len() {
            state.ext2_cache_clock_hand = 0;
        } else {
            state.ext2_cache_clock_hand = idx;
        }
        return Ok(());
    }
}

fn ext2_cache_ensure_capacity(state: &mut VfsState, incoming: usize) -> Result<(), i64> {
    while state.ext2_cached_blocks.len().saturating_add(incoming) > state.ext2_cache_max_blocks {
        ext2_cache_evict_one_clock(state)?;
    }
    Ok(())
}

fn ext2_cache_insert(state: &mut VfsState, block: u32, data: &[u8]) -> Result<(), i64> {
    let block_size = state.ext2_meta.block_size;
    if data.len() != block_size {
        return Err(EINVAL);
    }
    if let Ok(idx) = ext2_cache_find(state, block) {
        state.ext2_cached_blocks[idx].referenced = true;
        return Ok(());
    }
    ext2_cache_ensure_capacity(state, 1)?;
    state
        .ext2_cached_blocks
        .try_reserve(1)
        .map_err(|_| ENOSPC)?;
    let mut owned = Vec::new();
    owned.try_reserve_exact(block_size).map_err(|_| ENOSPC)?;
    owned.resize(block_size, 0);
    owned.copy_from_slice(data);
    let insert_at = match ext2_cache_find(state, block) {
        Ok(idx) => idx,
        Err(idx) => idx,
    };
    if insert_at <= state.ext2_cache_clock_hand && !state.ext2_cached_blocks.is_empty() {
        state.ext2_cache_clock_hand = state.ext2_cache_clock_hand.saturating_add(1);
    }
    state.ext2_cached_blocks.insert(
        insert_at,
        Ext2CachedBlock {
            block_index: block,
            referenced: true,
            dirty: false,
            data: owned,
        },
    );
    if state.ext2_cache_clock_hand >= state.ext2_cached_blocks.len() {
        state.ext2_cache_clock_hand = 0;
    }
    Ok(())
}

fn ext2_cache_has_block(state: &VfsState, block: u32) -> bool {
    ext2_cache_find(state, block).is_ok()
}

fn ext2_cache_block_mut(state: &mut VfsState, block: u32) -> Result<&mut [u8], i64> {
    let idx = ext2_cache_find(state, block).map_err(|_| EIO)?;
    state.ext2_cached_blocks[idx].referenced = true;
    Ok(state.ext2_cached_blocks[idx].data.as_mut_slice())
}

fn ext2_cache_block_ref(state: &mut VfsState, block: u32) -> Result<&[u8], i64> {
    let idx = ext2_cache_find(state, block).map_err(|_| EIO)?;
    state.ext2_cached_blocks[idx].referenced = true;
    Ok(state.ext2_cached_blocks[idx].data.as_slice())
}

fn ext2_cache_dirty_pressure_exceeded(state: &VfsState) -> bool {
    if state.ext2_cached_blocks.is_empty() {
        return false;
    }
    let dirty = state.ext2_dirty_blocks.len();
    let cached = state.ext2_cached_blocks.len();
    dirty.saturating_mul(100)
        > cached.saturating_mul(state.ext2_cache_dirty_high_watermark_percent as usize)
}

fn ext2_cache_maybe_flush_dirty_pressure(state: &mut VfsState) -> Result<(), i64> {
    if !ext2_cache_dirty_pressure_exceeded(state) {
        return Ok(());
    }
    if state.ext2_journal.present
        && !state.ext2_journal.replaying
        && state.ext2_journal.mode == Ext2JournalMode::Ordered
        && state.ext2_journal_running_tx.scope_depth > 0
        && state.ext2_journal_pending_txs.is_empty()
    {
        return Ok(());
    }
    let target_dirty = state
        .ext2_cached_blocks
        .len()
        .saturating_mul(state.ext2_cache_dirty_high_watermark_percent as usize)
        / 100;
    let mut budget = core::cmp::max(1usize, state.ext2_cached_blocks.len() / 8);
    while !state.ext2_dirty_blocks.is_empty()
        && state.ext2_dirty_blocks.len() > target_dirty
        && budget > 0
    {
        if state.ext2_journal.present
            && !state.ext2_journal.replaying
            && state.ext2_journal.mode == Ext2JournalMode::Ordered
            && !state.ext2_journal_pending_txs.is_empty()
        {
            let flushed = ext2_journal::flush_oldest_ordered_pending_tx(state, budget)?;
            if flushed == 0 {
                break;
            }
            budget = budget.saturating_sub(core::cmp::max(1, flushed));
            continue;
        }
        let block = state.ext2_dirty_blocks[0];
        if !ext2_cache_has_block(state, block) {
            ext2_unmark_dirty_block(state, block);
            continue;
        }
        ext2_cache_flush_block(state, block)?;
        budget -= 1;
    }
    Ok(())
}

fn ext2_clear_dirty_tracking(state: &mut VfsState) {
    state.ext2_tracked_block_count = 0;
    state.ext2_dirty_blocks.clear();
    state.ext2_cached_blocks.clear();
    state.ext2_cache_clock_hand = 0;
    state.ext2_journal_running_tx = Ext2JournalRunningTx::empty();
    state.ext2_journal_pending_txs.clear();
    state.ext2_journal_committed_txs.clear();
    state.ext2_journal_pending_revokes.clear();
    state.ext2_journal_next_tx_id = 1;
}

fn ext2_reset_dirty_tracking(state: &mut VfsState) -> Result<(), i64> {
    if !state.ext2_mounted {
        return Err(ENOENT);
    }
    let block_count = ext2_tracked_block_count(&state.ext2_meta, state.ext2_image_len)?;
    state.ext2_tracked_block_count = block_count;
    state.ext2_dirty_blocks.clear();
    state.ext2_cached_blocks.clear();
    state.ext2_cache_clock_hand = 0;
    state.ext2_journal_running_tx = Ext2JournalRunningTx::empty();
    state.ext2_journal_pending_txs.clear();
    state.ext2_journal_committed_txs.clear();
    state.ext2_journal_pending_revokes.clear();
    state.ext2_journal_next_tx_id = 1;
    Ok(())
}

fn ext2_mark_resident_range(state: &mut VfsState, start: usize, len: usize) -> Result<(), i64> {
    if len == 0 {
        return Ok(());
    }
    let block_size = state.ext2_meta.block_size;
    if block_size == 0 {
        return Err(EIO);
    }
    let end = start.checked_add(len).ok_or(EIO)?;
    if end > state.ext2_image_len {
        return Err(EIO);
    }
    let first = start / block_size;
    let last = (end - 1) / block_size;
    if last >= state.ext2_tracked_block_count || first > last {
        return Err(EIO);
    }
    let mut block = first;
    while block <= last {
        if ext2_cache_has_block(state, block as u32) {
            block += 1;
            continue;
        }
        let block_start = block.checked_mul(block_size).ok_or(EIO)?;
        let block_end = block_start.checked_add(block_size).ok_or(EIO)?;
        if block_end <= state.ext2_image.len() {
            let mut seed = Vec::new();
            seed.try_reserve_exact(block_size).map_err(|_| ENOSPC)?;
            seed.resize(block_size, 0);
            seed.copy_from_slice(&state.ext2_image[block_start..block_end]);
            ext2_cache_insert(state, block as u32, &seed)?;
        }
        block += 1;
    }
    Ok(())
}

fn ext2_ensure_block_resident(state: &mut VfsState, block: u32) -> Result<(), i64> {
    let idx = block as usize;
    if idx >= state.ext2_tracked_block_count {
        return Err(EIO);
    }
    let block_size = state.ext2_meta.block_size;
    if block_size == 0 {
        return Err(EIO);
    }
    match state.ext2_backend {
        ext2_backend::Ext2Backend::None => Err(EIO),
        ext2_backend::Ext2Backend::BlockOps(backend) => {
            if ext2_cache_has_block(state, block) {
                return Ok(());
            }
            let mut loaded = Vec::new();
            loaded.try_reserve_exact(block_size).map_err(|_| ENOSPC)?;
            loaded.resize(block_size, 0);
            (backend.read_block)(backend.ctx, block, &mut loaded)?;
            ext2_cache_insert(state, block, &loaded)?;
            Ok(())
        }
    }
}

fn ext2_ensure_range_resident(state: &mut VfsState, start: usize, len: usize) -> Result<(), i64> {
    if len == 0 {
        return Ok(());
    }
    let block_size = state.ext2_meta.block_size;
    if block_size == 0 {
        return Err(EIO);
    }
    let end = start.checked_add(len).ok_or(EIO)?;
    if end > state.ext2_image_len {
        return Err(EIO);
    }
    let first = start / block_size;
    let last = (end - 1) / block_size;
    let mut idx = first;
    while idx <= last {
        ext2_ensure_block_resident(state, idx as u32)?;
        idx += 1;
    }
    Ok(())
}

fn ext2_mark_dirty_block(state: &mut VfsState, block: u32) -> Result<(), i64> {
    let idx = block as usize;
    if idx >= state.ext2_tracked_block_count {
        return Err(EIO);
    }
    match state.ext2_dirty_blocks.binary_search(&block) {
        Ok(_) => {}
        Err(insert_at) => {
            state.ext2_dirty_blocks.try_reserve(1).map_err(|_| ENOSPC)?;
            state.ext2_dirty_blocks.insert(insert_at, block);
        }
    }
    if let Ok(cache_idx) = ext2_cache_find(state, block) {
        state.ext2_cached_blocks[cache_idx].dirty = true;
        state.ext2_cached_blocks[cache_idx].referenced = true;
    }
    ext2_journal::note_metadata_block(state, block)?;
    ext2_cache_maybe_flush_dirty_pressure(state)?;
    Ok(())
}

fn ext2_mark_dirty_range(state: &mut VfsState, start: usize, len: usize) -> Result<(), i64> {
    if len == 0 {
        return Ok(());
    }
    let block_size = state.ext2_meta.block_size;
    if block_size == 0 {
        return Err(EIO);
    }

    let end = start.checked_add(len).ok_or(EIO)?;
    if end > state.ext2_image_len {
        return Err(EIO);
    }

    let first = start / block_size;
    let last = (end - 1) / block_size;
    if last >= state.ext2_tracked_block_count {
        return Err(EIO);
    }

    let mut idx = first;
    while idx <= last {
        ext2_mark_dirty_block(state, idx as u32)?;
        idx += 1;
    }
    Ok(())
}

fn ext2_backend_read_block(
    state: &mut VfsState,
    block_index: u32,
    out: &mut [u8],
) -> Result<(), i64> {
    let block_size = state.ext2_meta.block_size;
    if block_size == 0 || out.len() != block_size {
        return Err(EINVAL);
    }
    if (block_index as usize) >= state.ext2_tracked_block_count {
        return Err(EIO);
    }
    if ext2_cache_has_block(state, block_index) {
        out.copy_from_slice(ext2_cache_block_ref(state, block_index)?);
        return Ok(());
    }

    match state.ext2_backend {
        ext2_backend::Ext2Backend::None => Err(EIO),
        ext2_backend::Ext2Backend::BlockOps(backend) => {
            (backend.read_block)(backend.ctx, block_index, out)
        }
    }
}

fn ext2_group_inode_table_block(image: &[u8], meta: &Ext2Meta, group: u32) -> Result<u32, i64> {
    if group >= meta.group_count {
        return Err(EIO);
    }

    ext2_gd_location(image, meta, group, 8)
}

#[allow(dead_code)]
/// Block 0 is a valid metadata block (it holds the superblock) when
/// `s_first_data_block` is 0, i.e. with blocks larger than 1 KiB; with 1 KiB
/// blocks it is the boot block and never part of the filesystem. Callers that
/// use 0 to mean "no block" (holes, unset pointers) check before slicing.
fn ext2_block_is_null(meta: &Ext2Meta, block: u32) -> bool {
    block == 0 && meta.first_data_block != 0
}

fn ext2_block_slice<'a>(image: &'a [u8], meta: &Ext2Meta, block: u32) -> Result<&'a [u8], i64> {
    if ext2_block_is_null(meta, block) {
        return Err(EIO);
    }
    let start = (block as usize).checked_mul(meta.block_size).ok_or(EIO)?;
    let end = start.checked_add(meta.block_size).ok_or(EIO)?;
    if end > image.len() {
        return Err(EIO);
    }
    Ok(&image[start..end])
}

#[allow(dead_code)]
fn ext2_block_slice_mut<'a>(
    image: &'a mut [u8],
    meta: &Ext2Meta,
    block: u32,
) -> Result<&'a mut [u8], i64> {
    if ext2_block_is_null(meta, block) {
        return Err(EIO);
    }
    let start = (block as usize).checked_mul(meta.block_size).ok_or(EIO)?;
    let end = start.checked_add(meta.block_size).ok_or(EIO)?;
    if end > image.len() {
        return Err(EIO);
    }
    Ok(&mut image[start..end])
}

fn ext2_block_slice_state<'a>(state: &'a mut VfsState, block: u32) -> Result<&'a [u8], i64> {
    let meta = state.ext2_meta;
    if ext2_block_is_null(&meta, block) {
        return Err(EIO);
    }
    let start = (block as usize).checked_mul(meta.block_size).ok_or(EIO)?;
    let end = start.checked_add(meta.block_size).ok_or(EIO)?;
    if end > state.ext2_image_len {
        return Err(EIO);
    }
    ext2_ensure_block_resident(state, block)?;
    match state.ext2_backend {
        ext2_backend::Ext2Backend::BlockOps(_) => ext2_cache_block_ref(state, block),
        ext2_backend::Ext2Backend::None => Err(EIO),
    }
}

fn ext2_block_slice_mut_state<'a>(
    state: &'a mut VfsState,
    block: u32,
) -> Result<&'a mut [u8], i64> {
    let meta = state.ext2_meta;
    if ext2_block_is_null(&meta, block) {
        return Err(EIO);
    }
    let start = (block as usize).checked_mul(meta.block_size).ok_or(EIO)?;
    let end = start.checked_add(meta.block_size).ok_or(EIO)?;
    if end > state.ext2_image_len {
        return Err(EIO);
    }
    ext2_ensure_block_resident(state, block)?;
    match state.ext2_backend {
        ext2_backend::Ext2Backend::BlockOps(_) => ext2_cache_block_mut(state, block),
        ext2_backend::Ext2Backend::None => Err(EIO),
    }
}

fn ext2_read_range_from_state(
    state: &mut VfsState,
    offset: usize,
    out: &mut [u8],
) -> Result<(), i64> {
    if out.is_empty() {
        return Ok(());
    }
    let end = offset.checked_add(out.len()).ok_or(EIO)?;
    if end > state.ext2_image_len {
        return Err(EIO);
    }
    match state.ext2_backend {
        ext2_backend::Ext2Backend::BlockOps(_) => {
            let block_size = state.ext2_meta.block_size;
            if block_size == 0 {
                return Err(EIO);
            }
            let mut copied = 0usize;
            while copied < out.len() {
                let abs = offset + copied;
                let block = abs / block_size;
                let block_off = abs % block_size;
                let chunk = core::cmp::min(block_size - block_off, out.len() - copied);
                let blk = ext2_block_slice_state(state, block as u32)?;
                out[copied..copied + chunk].copy_from_slice(&blk[block_off..block_off + chunk]);
                copied += chunk;
            }
            Ok(())
        }
        ext2_backend::Ext2Backend::None => Err(EIO),
    }
}

fn ext2_write_range_to_state(state: &mut VfsState, offset: usize, data: &[u8]) -> Result<(), i64> {
    if data.is_empty() {
        return Ok(());
    }
    let end = offset.checked_add(data.len()).ok_or(EIO)?;
    if end > state.ext2_image_len {
        return Err(EIO);
    }
    match state.ext2_backend {
        ext2_backend::Ext2Backend::BlockOps(_) => {
            let block_size = state.ext2_meta.block_size;
            if block_size == 0 {
                return Err(EIO);
            }
            let mut copied = 0usize;
            while copied < data.len() {
                let abs = offset + copied;
                let block = abs / block_size;
                let block_off = abs % block_size;
                let chunk = core::cmp::min(block_size - block_off, data.len() - copied);
                {
                    let blk = ext2_block_slice_mut_state(state, block as u32)?;
                    blk[block_off..block_off + chunk]
                        .copy_from_slice(&data[copied..copied + chunk]);
                }
                ext2_mark_dirty_block(state, block as u32)?;
                copied += chunk;
            }
            Ok(())
        }
        ext2_backend::Ext2Backend::None => Err(EIO),
    }
}

fn ext2_zero_range_state(state: &mut VfsState, offset: usize, len: usize) -> Result<(), i64> {
    if len == 0 {
        return Ok(());
    }
    let end = offset.checked_add(len).ok_or(EIO)?;
    if end > state.ext2_image_len {
        return Err(EIO);
    }
    match state.ext2_backend {
        ext2_backend::Ext2Backend::BlockOps(_) => {
            let block_size = state.ext2_meta.block_size;
            if block_size == 0 {
                return Err(EIO);
            }
            let mut done = 0usize;
            while done < len {
                let abs = offset + done;
                let block = abs / block_size;
                let block_off = abs % block_size;
                let chunk = core::cmp::min(block_size - block_off, len - done);
                {
                    let blk = ext2_block_slice_mut_state(state, block as u32)?;
                    blk[block_off..block_off + chunk].fill(0);
                }
                ext2_mark_dirty_block(state, block as u32)?;
                done += chunk;
            }
            Ok(())
        }
        ext2_backend::Ext2Backend::None => Err(EIO),
    }
}

fn ext2_read_u16_state(state: &mut VfsState, offset: usize) -> Result<u16, i64> {
    let mut buf = [0u8; 2];
    ext2_read_range_from_state(state, offset, &mut buf)?;
    Ok(u16::from_le_bytes(buf))
}

fn ext2_read_u32_state(state: &mut VfsState, offset: usize) -> Result<u32, i64> {
    let mut buf = [0u8; 4];
    ext2_read_range_from_state(state, offset, &mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn ext2_write_u16_state(state: &mut VfsState, offset: usize, value: u16) -> Result<(), i64> {
    ext2_write_range_to_state(state, offset, &value.to_le_bytes())
}

fn ext2_write_u32_state(state: &mut VfsState, offset: usize, value: u32) -> Result<(), i64> {
    ext2_write_range_to_state(state, offset, &value.to_le_bytes())
}

#[cfg(test)]
fn ext2_block_cached(state: &VfsState, block: u32) -> bool {
    ext2_cache_has_block(state, block)
}

/// Group descriptor size for this superblock: 32 bytes, or `s_desc_size`
/// (offset 0xFE) when the ext4 `64bit` feature is set.
fn ext2_desc_size_from_superblock(image: &[u8], feature_incompat: u32) -> Result<usize, i64> {
    if (feature_incompat & EXT4_FEATURE_INCOMPAT_64BIT) == 0 {
        return Ok(EXT2_GROUP_DESC_SIZE);
    }
    let size = read_u16_le(image, EXT2_SUPERBLOCK_OFFSET + 0xFE)? as usize;
    if size < EXT4_MIN_DESC_SIZE_64BIT || size > EXT4_MAX_DESC_SIZE || !size.is_power_of_two() {
        return Err(EINVAL);
    }
    Ok(size)
}

fn ext2_group_desc_offset(meta: &Ext2Meta, group: u32) -> Result<usize, i64> {
    if group >= meta.group_count {
        return Err(EIO);
    }
    meta.gdt_offset
        .checked_add((group as usize) * meta.desc_size)
        .ok_or(EIO)
}

/// Reads a 32-bit descriptor location (block/inode bitmap, inode table)
/// whose high half lives `EXT4_GD_HI_DELTA` bytes later in >= 64-byte
/// descriptors. Block numbers are 32-bit internally, so a non-zero high half
/// (a filesystem above 2^32 blocks) is refused with `EIO`.
fn ext2_gd_location_state(
    state: &mut VfsState,
    meta: &Ext2Meta,
    group: u32,
    lo_offset: usize,
) -> Result<u32, i64> {
    let desc_offset = ext2_group_desc_offset(meta, group)?;
    let lo = ext2_read_u32_state(state, desc_offset + lo_offset)?;
    if meta.desc_size >= EXT4_MIN_DESC_SIZE_64BIT {
        let hi = ext2_read_u32_state(state, desc_offset + lo_offset + EXT4_GD_HI_DELTA)?;
        if hi != 0 {
            return Err(EIO);
        }
    }
    Ok(lo)
}

/// `bg_flags` bits (ext4 lazy group initialization).
const EXT4_BG_INODE_UNINIT: u16 = 0x0001;
const EXT4_BG_BLOCK_UNINIT: u16 = 0x0002;
const GD_FLAGS: usize = 0x12;
const GD_ITABLE_UNUSED: usize = 0x1C;
const EXT2_FEATURE_COMPAT_RESIZE_INODE_BIT: u32 = 0x0010;

/// Whether this filesystem maintains group descriptor checksums (and with
/// them the lazy-init `bg_flags` / `bg_itable_unused` fields).
fn ext2_has_group_csum(meta: &Ext2Meta) -> bool {
    (meta.feature_ro_compat
        & (EXT4_FEATURE_RO_COMPAT_GDT_CSUM | EXT4_FEATURE_RO_COMPAT_METADATA_CSUM))
        != 0
}

fn ext2_gd_flags_state(state: &mut VfsState, meta: &Ext2Meta, group: u32) -> Result<u16, i64> {
    let desc = ext2_group_desc_offset(meta, group)?;
    ext2_read_u16_state(state, desc + GD_FLAGS)
}

fn ext2_gd_block_bitmap_uninit(state: &mut VfsState, meta: &Ext2Meta, group: u32) -> Result<bool, i64> {
    Ok(ext2_has_group_csum(meta)
        && (ext2_gd_flags_state(state, meta, group)? & EXT4_BG_BLOCK_UNINIT) != 0)
}

fn ext2_gd_inode_bitmap_uninit(state: &mut VfsState, meta: &Ext2Meta, group: u32) -> Result<bool, i64> {
    Ok(ext2_has_group_csum(meta)
        && (ext2_gd_flags_state(state, meta, group)? & EXT4_BG_INODE_UNINIT) != 0)
}

/// Recomputes `bg_checksum` (crc16 `gdt_csum` or crc32c `metadata_csum`)
/// after any change to the descriptor.
fn ext2_gd_refresh_csum(state: &mut VfsState, meta: &Ext2Meta, group: u32) -> Result<(), i64> {
    let csum = match state.ext4_csum {
        Some(csum) => csum,
        None => return Ok(()),
    };
    let offset = ext2_group_desc_offset(meta, group)?;
    if csum.metadata_csum {
        // metadata_csum: the descriptor carries both bitmaps' checksums, so
        // they are refreshed first (skipped while a bitmap is uninitialized).
        let flags = ext2_read_u16_state(state, offset + GD_FLAGS)?;
        let wide = meta.desc_size >= EXT4_MIN_DESC_SIZE_64BIT;
        if (flags & EXT4_BG_BLOCK_UNINIT) == 0 {
            let bitmap_block = ext2_group_block_bitmap_block_state(state, meta, group)?;
            let value = {
                let bitmap = ext2_block_slice_state(state, bitmap_block)?;
                csum.block_bitmap_csum(bitmap)?
            };
            ext2_write_u16_state(state, offset + 0x18, (value & 0xFFFF) as u16)?;
            if wide {
                ext2_write_u16_state(state, offset + 0x38, (value >> 16) as u16)?;
            }
        }
        if (flags & EXT4_BG_INODE_UNINIT) == 0 {
            let bitmap_block = ext2_group_inode_bitmap_block_state(state, meta, group)?;
            let value = {
                let bitmap = ext2_block_slice_state(state, bitmap_block)?;
                csum.inode_bitmap_csum(bitmap)?
            };
            ext2_write_u16_state(state, offset + 0x1A, (value & 0xFFFF) as u16)?;
            if wide {
                ext2_write_u16_state(state, offset + 0x3A, (value >> 16) as u16)?;
            }
        }
    }
    let mut desc = [0u8; EXT4_MAX_DESC_SIZE];
    ext2_read_range_from_state(state, offset, &mut desc[..meta.desc_size])?;
    if let Some(value) = csum.group_desc(group, &desc[..meta.desc_size])? {
        ext2_write_u16_state(state, offset + 0x1E, value)?;
    }
    Ok(())
}

/// metadata_csum: checks a group's initialized bitmaps against the
/// checksums in its descriptor before the allocator trusts them.
fn ext2_verify_group_bitmaps(state: &mut VfsState, meta: &Ext2Meta, group: u32) -> Result<(), i64> {
    let csum = match state.ext4_csum {
        Some(csum) if csum.metadata_csum => csum,
        _ => return Ok(()),
    };
    let offset = ext2_group_desc_offset(meta, group)?;
    let mut desc = [0u8; EXT4_MAX_DESC_SIZE];
    ext2_read_range_from_state(state, offset, &mut desc[..meta.desc_size])?;
    let flags = read_u16_le(&desc, GD_FLAGS)?;
    if (flags & EXT4_BG_BLOCK_UNINIT) == 0 {
        let block = ext2_group_block_bitmap_block_state(state, meta, group)?;
        let ok = {
            let bitmap = ext2_block_slice_state(state, block)?;
            csum.block_bitmap_ok(&desc[..meta.desc_size], bitmap)?
        };
        if !ok {
            ext2_report(state, "block bitmap checksum", group as u64);
            return Err(EIO);
        }
    }
    if (flags & EXT4_BG_INODE_UNINIT) == 0 {
        let block = ext2_group_inode_bitmap_block_state(state, meta, group)?;
        let ok = {
            let bitmap = ext2_block_slice_state(state, block)?;
            csum.inode_bitmap_ok(&desc[..meta.desc_size], bitmap)?
        };
        if !ok {
            ext2_report(state, "inode bitmap checksum", group as u64);
            return Err(EIO);
        }
    }
    Ok(())
}

/// Recomputes an inode's `metadata_csum` checksum from its on-disk bytes;
/// called after every inode write.
fn ext2_update_inode_csum(state: &mut VfsState, ino: u32) -> Result<(), i64> {
    let csum = match state.ext4_csum {
        Some(csum) if csum.inode_csums => csum,
        _ => return Ok(()),
    };
    let meta = state.ext2_meta;
    let offset = ext2_inode_offset_in_state(state, &meta, ino)?;
    let size = meta.inode_size;
    let mut raw = [0u8; EXT4_MAX_INODE_SIZE];
    if size > raw.len() {
        return Err(EIO);
    }
    ext2_read_range_from_state(state, offset, &mut raw[..size])?;
    let value = csum.inode(ino, &raw[..size])?;
    ext2_write_u16_state(state, offset + 0x7C, (value & 0xFFFF) as u16)?;
    if csum.inode_has_checksum_hi(&raw[..size])? {
        ext2_write_u16_state(state, offset + 0x82, (value >> 16) as u16)?;
    }
    Ok(())
}

/// Bytes reserved at the end of every directory block for the
/// `metadata_csum` tail dirent (0 without metadata_csum).
fn ext2_dir_tail_len(state: &VfsState) -> usize {
    if state.ext4_csum.map_or(false, |csum| csum.metadata_csum) {
        12
    } else {
        0
    }
}

/// After a directory block changes: rewrites its tail dirent (inode 0,
/// rec_len 12, name_len 0, type 0xDE) and checksum on metadata_csum
/// filesystems, and marks the block dirty.
fn ext2_dir_block_finish(state: &mut VfsState, dir: &Ext2Inode, block: u32) -> Result<(), i64> {
    if let Some(csum) = state.ext4_csum.filter(|csum| csum.metadata_csum) {
        let seed = csum.inode_seed_for(dir.ino, dir.generation);
        let blk = ext2_block_slice_mut_state(state, block)?;
        let tail = blk.len() - 12;
        write_u32_le(blk, tail, 0)?;
        write_u16_le(blk, tail + 4, 12)?;
        blk[tail + 6] = 0;
        blk[tail + 7] = 0xDE;
        let value = ext4_csum::crc32c(seed, &blk[..tail]);
        write_u32_le(blk, tail + 8, value)?;
    }
    ext2_mark_dirty_block(state, block)
}

/// metadata_csum: once a directory stops being an htree, its former index
/// blocks (the dx root and dx nodes) are ordinary leaf blocks and need tail
/// dirents. Carves the 12 bytes out of the slack of the entry that reaches
/// the end of each such block.
fn ext2_dir_add_missing_tails(state: &mut VfsState, dir: &Ext2Inode) -> Result<(), i64> {
    let csum = match state.ext4_csum {
        Some(csum) if csum.metadata_csum => csum,
        _ => return Ok(()),
    };
    let seed = csum.inode_seed_for(dir.ino, dir.generation);
    let meta = state.ext2_meta;
    let count = ext2_logical_blocks_for_size(&meta, dir.size as usize)?;
    for idx in 0..count {
        let block = ext2_data_block_for_logical_index_state(state, dir, idx)?;
        if block == 0 {
            continue;
        }
        let has_tail = {
            let blk = ext2_block_slice_state(state, block)?;
            ext4_csum::dir_leaf_csum_ok(seed, blk)?.is_some()
        };
        if has_tail {
            continue;
        }
        {
            let blk = ext2_block_slice_mut_state(state, block)?;
            let end = blk.len();
            let mut pos = 0usize;
            let mut shrunk = false;
            while pos + 8 <= end {
                let rec_len = read_u16_le(blk, pos + 4)? as usize;
                if rec_len < 8 || pos + rec_len > end {
                    return Err(EIO);
                }
                if pos + rec_len == end {
                    let name_len = blk[pos + 6] as usize;
                    let used = if read_u32_le(blk, pos)? == 0 { 8 } else { (8 + name_len + 3) & !3 };
                    if rec_len < used + 12 {
                        return Err(EIO);
                    }
                    write_u16_le(blk, pos + 4, (rec_len - 12) as u16)?;
                    shrunk = true;
                    break;
                }
                pos += rec_len;
            }
            if !shrunk {
                return Err(EIO);
            }
        }
        ext2_dir_block_finish(state, dir, block)?;
    }
    Ok(())
}

/// sparse_super: the superblock and GDT are backed up in groups 0, 1 and
/// powers of 3, 5 and 7.
fn ext2_group_has_super(meta: &Ext2Meta, group: u32) -> bool {
    if group <= 1 || (meta.feature_ro_compat & EXT2_FEATURE_RO_COMPAT_SPARSE_SUPER) == 0 {
        return true;
    }
    for base in [3u32, 5, 7] {
        let mut n = base;
        while n < group {
            n = match n.checked_mul(base) {
                Some(v) => v,
                None => break,
            };
        }
        if n == group {
            return true;
        }
    }
    false
}

/// Blocks in `group` (the last group may be short).
fn ext2_blocks_in_group(meta: &Ext2Meta, group: u32) -> u32 {
    let first = meta.first_data_block + group * meta.blocks_per_group;
    core::cmp::min(meta.blocks_per_group, meta.total_blocks.saturating_sub(first))
}

fn ext2_bitmap_set_tail(bitmap: &mut [u8], from_bit: u32) -> Result<(), i64> {
    let bits = (bitmap.len() * 8) as u32;
    let mut bit = from_bit;
    while bit < bits {
        ext2_bitmap_set(bitmap, bit, true)?;
        bit += 1;
    }
    Ok(())
}

/// Builds the block bitmap of a `BLOCK_UNINIT` group the way Linux's
/// `ext4_init_block_bitmap` does: superblock/GDT backups (and reserved GDT
/// blocks), the group's own bitmaps and inode table when they live inside
/// it (not the case with flex_bg packing), and every bit past the group's
/// last block. The result must agree with `bg_free_blocks_count`.
fn ext2_init_block_bitmap(state: &mut VfsState, meta: &Ext2Meta, group: u32) -> Result<(), i64> {
    let group_first = meta.first_data_block + group * meta.blocks_per_group;
    let in_group = ext2_blocks_in_group(meta, group);
    let mut used: Vec<(u32, u32)> = Vec::new(); // (first block, count)
    if ext2_group_has_super(meta, group) {
        let gdt_blocks = ((meta.group_count as usize * meta.desc_size + meta.block_size - 1)
            / meta.block_size) as u32;
        let reserved = if (meta.feature_compat & EXT2_FEATURE_COMPAT_RESIZE_INODE_BIT) != 0 {
            ext2_read_u16_state(state, EXT2_SUPERBLOCK_OFFSET + 0xCE)? as u32
        } else {
            0
        };
        used.push((group_first, 1 + gdt_blocks + reserved));
    }
    let inode_table_blocks =
        ((meta.inodes_per_group as usize * meta.inode_size + meta.block_size - 1) / meta.block_size)
            as u32;
    used.push((ext2_group_block_bitmap_block_state(state, meta, group)?, 1));
    used.push((ext2_group_inode_bitmap_block_state(state, meta, group)?, 1));
    used.push((ext2_group_inode_table_block_state(state, meta, group)?, inode_table_blocks));

    let bitmap_block = ext2_group_block_bitmap_block_state(state, meta, group)?;
    let free = {
        let bitmap = ext2_block_slice_mut_state(state, bitmap_block)?;
        bitmap.fill(0);
        for &(first, count) in &used {
            for block in first..first.saturating_add(count) {
                if block >= group_first && block - group_first < in_group {
                    ext2_bitmap_set(bitmap, block - group_first, true)?;
                }
            }
        }
        ext2_bitmap_set_tail(bitmap, in_group)?;
        let mut free = 0u32;
        for bit in 0..in_group {
            if !ext2_bitmap_get(bitmap, bit)? {
                free += 1;
            }
        }
        free
    };
    let desc = ext2_group_desc_offset(meta, group)?;
    let recorded = ext2_read_u16_state(state, desc + 12)? as u32
        | if meta.desc_size >= EXT4_MIN_DESC_SIZE_64BIT {
            (ext2_read_u16_state(state, desc + 12 + EXT4_GD_HI_DELTA)? as u32) << 16
        } else {
            0
        };
    if free != recorded {
        ext2_report(state, "uninit block bitmap free count", group as u64);
        return Err(EIO);
    }
    ext2_mark_dirty_block(state, bitmap_block)?;
    let flags = ext2_read_u16_state(state, desc + GD_FLAGS)? & !EXT4_BG_BLOCK_UNINIT;
    ext2_write_u16_state(state, desc + GD_FLAGS, flags)?;
    ext2_gd_refresh_csum(state, meta, group)?;
    ext2_mark_dirty_range(state, desc, meta.desc_size)
}

/// Builds the inode bitmap of an `INODE_UNINIT` group: no inode in use, and
/// every bit past `inodes_per_group` set (Linux `ext4_mark_bitmap_end`).
fn ext2_init_inode_bitmap(state: &mut VfsState, meta: &Ext2Meta, group: u32) -> Result<(), i64> {
    let bitmap_block = ext2_group_inode_bitmap_block_state(state, meta, group)?;
    {
        let bitmap = ext2_block_slice_mut_state(state, bitmap_block)?;
        bitmap.fill(0);
        ext2_bitmap_set_tail(bitmap, meta.inodes_per_group)?;
    }
    ext2_mark_dirty_block(state, bitmap_block)?;
    let desc = ext2_group_desc_offset(meta, group)?;
    let flags = ext2_read_u16_state(state, desc + GD_FLAGS)? & !EXT4_BG_INODE_UNINIT;
    ext2_write_u16_state(state, desc + GD_FLAGS, flags)?;
    ext2_gd_refresh_csum(state, meta, group)?;
    ext2_mark_dirty_range(state, desc, meta.desc_size)
}

/// Shrinks `bg_itable_unused` when an inode beyond the initialized part of
/// the group's inode table is allocated (lazy itable init).
fn ext2_gd_note_inode_used(state: &mut VfsState, meta: &Ext2Meta, ino: u32) -> Result<(), i64> {
    if !ext2_has_group_csum(meta) {
        return Ok(());
    }
    let (group, bit) = ext2_inode_group_and_bit(meta, ino)?;
    let desc = ext2_group_desc_offset(meta, group)?;
    let wide = meta.desc_size >= EXT4_MIN_DESC_SIZE_64BIT;
    let lo = ext2_read_u16_state(state, desc + GD_ITABLE_UNUSED)? as u32;
    let hi = if wide {
        ext2_read_u16_state(state, desc + GD_ITABLE_UNUSED + 0x16)? as u32
    } else {
        0
    };
    let unused = (hi << 16) | lo;
    let initialized = meta.inodes_per_group.saturating_sub(unused);
    if bit < initialized {
        return Ok(());
    }
    let new_unused = meta.inodes_per_group - (bit + 1);
    ext2_write_u16_state(state, desc + GD_ITABLE_UNUSED, (new_unused & 0xFFFF) as u16)?;
    if wide {
        ext2_write_u16_state(state, desc + GD_ITABLE_UNUSED + 0x16, (new_unused >> 16) as u16)?;
    }
    ext2_gd_refresh_csum(state, meta, group)?;
    ext2_mark_dirty_range(state, desc, meta.desc_size)
}

/// Image-buffer variant of `ext2_gd_location_state`.
fn ext2_gd_location(image: &[u8], meta: &Ext2Meta, group: u32, lo_offset: usize) -> Result<u32, i64> {
    let desc_offset = ext2_group_desc_offset(meta, group)?;
    let lo = read_u32_le(image, desc_offset + lo_offset)?;
    if meta.desc_size >= EXT4_MIN_DESC_SIZE_64BIT
        && read_u32_le(image, desc_offset + lo_offset + EXT4_GD_HI_DELTA)? != 0
    {
        return Err(EIO);
    }
    Ok(lo)
}

/// Adjusts a 16-bit descriptor counter (free blocks/inodes, used dirs) that
/// gains a high 16-bit half `EXT4_GD_HI_DELTA` bytes later in >= 64-byte
/// descriptors.
fn ext2_gd_adjust_counter_state(
    state: &mut VfsState,
    meta: &Ext2Meta,
    group: u32,
    lo_offset: usize,
    delta: i32,
) -> Result<(), i64> {
    if delta == 0 {
        return Ok(());
    }
    let desc_offset = ext2_group_desc_offset(meta, group)?;
    if meta.desc_size < EXT4_MIN_DESC_SIZE_64BIT {
        return ext2_adjust_counter_u16_state(state, desc_offset + lo_offset, delta);
    }
    let lo = ext2_read_u16_state(state, desc_offset + lo_offset)? as i64;
    let hi = ext2_read_u16_state(state, desc_offset + lo_offset + EXT4_GD_HI_DELTA)? as i64;
    let next = ((hi << 16) | lo) + delta as i64;
    if next < 0 || next > u32::MAX as i64 {
        return Err(EIO);
    }
    ext2_write_u16_state(state, desc_offset + lo_offset, (next & 0xFFFF) as u16)?;
    ext2_write_u16_state(
        state,
        desc_offset + lo_offset + EXT4_GD_HI_DELTA,
        ((next >> 16) & 0xFFFF) as u16,
    )
}

#[allow(dead_code)]
fn ext2_group_bitmap_block(
    image: &[u8],
    meta: &Ext2Meta,
    group: u32,
    field_offset: usize,
) -> Result<u32, i64> {
    ext2_gd_location(image, meta, group, field_offset)
}

#[allow(dead_code)]
fn ext2_group_block_bitmap_block(image: &[u8], meta: &Ext2Meta, group: u32) -> Result<u32, i64> {
    ext2_group_bitmap_block(image, meta, group, 0)
}

#[allow(dead_code)]
fn ext2_group_inode_bitmap_block(image: &[u8], meta: &Ext2Meta, group: u32) -> Result<u32, i64> {
    ext2_group_bitmap_block(image, meta, group, 4)
}

fn ext2_group_bitmap_block_state(
    state: &mut VfsState,
    meta: &Ext2Meta,
    group: u32,
    field_offset: usize,
) -> Result<u32, i64> {
    ext2_gd_location_state(state, meta, group, field_offset)
}

fn ext2_group_block_bitmap_block_state(
    state: &mut VfsState,
    meta: &Ext2Meta,
    group: u32,
) -> Result<u32, i64> {
    ext2_group_bitmap_block_state(state, meta, group, 0)
}

fn ext2_group_inode_bitmap_block_state(
    state: &mut VfsState,
    meta: &Ext2Meta,
    group: u32,
) -> Result<u32, i64> {
    ext2_group_bitmap_block_state(state, meta, group, 4)
}

fn ext2_group_inode_table_block_state(
    state: &mut VfsState,
    meta: &Ext2Meta,
    group: u32,
) -> Result<u32, i64> {
    if group >= meta.group_count {
        return Err(EIO);
    }
    ext2_gd_location_state(state, meta, group, 8)
}

#[allow(dead_code)]
fn ext2_inode_offset_in_image(image: &[u8], meta: &Ext2Meta, ino: u32) -> Result<usize, i64> {
    if ino == 0 || ino > meta.total_inodes {
        return Err(ENOENT);
    }

    let idx = ino - 1;
    let group = idx / meta.inodes_per_group;
    let local = idx % meta.inodes_per_group;
    let inode_table_block = ext2_group_inode_table_block(image, meta, group)?;

    let inode_table_offset = (inode_table_block as usize)
        .checked_mul(meta.block_size)
        .ok_or(EIO)?;
    inode_table_offset
        .checked_add((local as usize) * meta.inode_size)
        .ok_or(EIO)
}

fn ext2_inode_offset_in_state(
    state: &mut VfsState,
    meta: &Ext2Meta,
    ino: u32,
) -> Result<usize, i64> {
    if ino == 0 || ino > meta.total_inodes {
        return Err(ENOENT);
    }

    let idx = ino - 1;
    let group = idx / meta.inodes_per_group;
    let local = idx % meta.inodes_per_group;
    let inode_table_block = ext2_group_inode_table_block_state(state, meta, group)?;

    let inode_table_offset = (inode_table_block as usize)
        .checked_mul(meta.block_size)
        .ok_or(EIO)?;
    inode_table_offset
        .checked_add((local as usize) * meta.inode_size)
        .ok_or(EIO)
}

#[allow(dead_code)]
fn ext2_write_inode(
    image: &mut [u8],
    meta: &Ext2Meta,
    ino: u32,
    inode: &Ext2Inode,
) -> Result<(), i64> {
    let inode_offset = ext2_inode_offset_in_image(image, meta, ino)?;
    if inode_offset
        .checked_add(meta.inode_size)
        .filter(|end| *end <= image.len())
        .is_none()
    {
        return Err(EIO);
    }

    write_u16_le(image, inode_offset, inode.mode)?;
    write_u16_le(image, inode_offset + 2, inode.uid)?;
    write_u32_le(image, inode_offset + 4, inode.size)?;
    write_u32_le(image, inode_offset + 8, inode.atime as u32)?;
    write_u32_le(image, inode_offset + 12, inode.ctime as u32)?;
    write_u32_le(image, inode_offset + 16, inode.mtime as u32)?;
    write_u16_le(image, inode_offset + 24, inode.gid)?;
    write_u16_le(image, inode_offset + 26, inode.links_count)?;
    write_u32_le(image, inode_offset + 28, inode.block_count_512)?;

    let mut i = 0usize;
    while i < 15 {
        write_u32_le(image, inode_offset + 40 + i * 4, inode.blocks[i])?;
        i += 1;
    }

    Ok(())
}

fn ext2_bitmap_get(bitmap: &[u8], bit_idx: u32) -> Result<bool, i64> {
    let byte_idx = (bit_idx / 8) as usize;
    if byte_idx >= bitmap.len() {
        return Err(EIO);
    }
    let bit = (bit_idx % 8) as u8;
    Ok((bitmap[byte_idx] & (1u8 << bit)) != 0)
}

fn ext2_bitmap_set(bitmap: &mut [u8], bit_idx: u32, value: bool) -> Result<(), i64> {
    let byte_idx = (bit_idx / 8) as usize;
    if byte_idx >= bitmap.len() {
        return Err(EIO);
    }
    let bit = (bit_idx % 8) as u8;
    let mask = 1u8 << bit;
    if value {
        bitmap[byte_idx] |= mask;
    } else {
        bitmap[byte_idx] &= !mask;
    }
    Ok(())
}

#[allow(dead_code)]
fn ext2_adjust_counter_u32(image: &mut [u8], offset: usize, delta: i32) -> Result<(), i64> {
    if delta == 0 {
        return Ok(());
    }
    let current = read_u32_le(image, offset)? as i64;
    let next = current + delta as i64;
    if next < 0 || next > u32::MAX as i64 {
        return Err(EIO);
    }
    write_u32_le(image, offset, next as u32)
}

#[allow(dead_code)]
fn ext2_adjust_counter_u16(image: &mut [u8], offset: usize, delta: i32) -> Result<(), i64> {
    if delta == 0 {
        return Ok(());
    }
    let current = read_u16_le(image, offset)? as i64;
    let next = current + delta as i64;
    if next < 0 || next > u16::MAX as i64 {
        return Err(EIO);
    }
    write_u16_le(image, offset, next as u16)
}

#[allow(dead_code)]
fn ext2_adjust_group_counters(
    image: &mut [u8],
    meta: &Ext2Meta,
    group: u32,
    block_delta: i32,
    inode_delta: i32,
    used_dir_delta: i32,
) -> Result<(), i64> {
    let desc_offset = ext2_group_desc_offset(meta, group)?;
    ext2_adjust_counter_u16(image, desc_offset + 12, block_delta)?;
    ext2_adjust_counter_u16(image, desc_offset + 14, inode_delta)?;
    ext2_adjust_counter_u16(image, desc_offset + 16, used_dir_delta)?;
    Ok(())
}

#[allow(dead_code)]
fn ext2_adjust_super_counters(
    image: &mut [u8],
    block_delta: i32,
    inode_delta: i32,
) -> Result<(), i64> {
    let sb = EXT2_SUPERBLOCK_OFFSET;
    ext2_adjust_counter_u32(image, sb + 12, block_delta)?;
    ext2_adjust_counter_u32(image, sb + 16, inode_delta)?;
    Ok(())
}

fn ext2_adjust_counter_u32_state(
    state: &mut VfsState,
    offset: usize,
    delta: i32,
) -> Result<(), i64> {
    if delta == 0 {
        return Ok(());
    }
    let current = ext2_read_u32_state(state, offset)? as i64;
    let next = current + delta as i64;
    if next < 0 || next > u32::MAX as i64 {
        return Err(EIO);
    }
    ext2_write_u32_state(state, offset, next as u32)
}

fn ext2_adjust_counter_u16_state(
    state: &mut VfsState,
    offset: usize,
    delta: i32,
) -> Result<(), i64> {
    if delta == 0 {
        return Ok(());
    }
    let current = ext2_read_u16_state(state, offset)? as i64;
    let next = current + delta as i64;
    if next < 0 || next > u16::MAX as i64 {
        return Err(EIO);
    }
    ext2_write_u16_state(state, offset, next as u16)
}

fn ext2_adjust_group_counters_state(
    state: &mut VfsState,
    meta: &Ext2Meta,
    group: u32,
    block_delta: i32,
    inode_delta: i32,
    used_dir_delta: i32,
) -> Result<(), i64> {
    ext2_gd_adjust_counter_state(state, meta, group, 12, block_delta)?;
    ext2_gd_adjust_counter_state(state, meta, group, 14, inode_delta)?;
    ext2_gd_adjust_counter_state(state, meta, group, 16, used_dir_delta)?;
    ext2_gd_refresh_csum(state, meta, group)
}

fn ext2_adjust_super_counters_state(
    state: &mut VfsState,
    block_delta: i32,
    inode_delta: i32,
) -> Result<(), i64> {
    let sb = EXT2_SUPERBLOCK_OFFSET;
    ext2_adjust_counter_u32_state(state, sb + 12, block_delta)?;
    ext2_adjust_counter_u32_state(state, sb + 16, inode_delta)?;
    Ok(())
}

fn ext2_block_group_and_bit(meta: &Ext2Meta, block_num: u32) -> Result<(u32, u32), i64> {
    if block_num < meta.first_data_block || block_num >= meta.total_blocks {
        return Err(EIO);
    }
    let rel = block_num - meta.first_data_block;
    let group = rel / meta.blocks_per_group;
    if group >= meta.group_count {
        return Err(EIO);
    }
    let bit = rel % meta.blocks_per_group;
    Ok((group, bit))
}

fn ext2_inode_group_and_bit(meta: &Ext2Meta, ino: u32) -> Result<(u32, u32), i64> {
    if ino == 0 || ino > meta.total_inodes {
        return Err(ENOENT);
    }
    let idx = ino - 1;
    Ok((idx / meta.inodes_per_group, idx % meta.inodes_per_group))
}

#[allow(dead_code)]
fn ext2_count_indirect_tree_blocks(
    image: &[u8],
    meta: &Ext2Meta,
    table_block: u32,
    depth: u8,
) -> Result<u32, i64> {
    if table_block == 0 {
        return Ok(0);
    }
    if depth == 0 {
        return Err(EIO);
    }

    let entries = ext2_indirect_entries_per_block(meta)?;
    let mut count = 1u32;
    let mut idx = 0usize;
    while idx < entries {
        let child = ext2_read_indirect_entry(image, meta, table_block, idx)?;
        if child != 0 {
            if depth == 1 {
                count = count.saturating_add(1);
            } else {
                count = count.saturating_add(ext2_count_indirect_tree_blocks(
                    image,
                    meta,
                    child,
                    depth - 1,
                )?);
            }
        }
        idx += 1;
    }

    Ok(count)
}

#[allow(dead_code)]
fn ext2_recalc_inode_block_count_512(
    image: &[u8],
    meta: &Ext2Meta,
    inode: &Ext2Inode,
) -> Result<u32, i64> {
    let sectors_per_block = (meta.block_size / 512) as u32;
    let mut used_blocks = 0u32;

    let mut i = 0usize;
    while i < EXT2_INODE_DIRECT_COUNT {
        if inode.blocks[i] != 0 {
            used_blocks = used_blocks.saturating_add(1);
        }
        i += 1;
    }

    used_blocks = used_blocks.saturating_add(ext2_count_indirect_tree_blocks(
        image,
        meta,
        inode.blocks[12],
        1,
    )?);
    used_blocks = used_blocks.saturating_add(ext2_count_indirect_tree_blocks(
        image,
        meta,
        inode.blocks[13],
        2,
    )?);
    used_blocks = used_blocks.saturating_add(ext2_count_indirect_tree_blocks(
        image,
        meta,
        inode.blocks[14],
        3,
    )?);

    Ok(used_blocks
        .saturating_mul(sectors_per_block)
        .saturating_add(ext2_inode_acl_sectors(meta, inode)))
}

fn ext2_count_indirect_tree_blocks_state(
    state: &mut VfsState,
    table_block: u32,
    depth: u8,
) -> Result<u32, i64> {
    if table_block == 0 {
        return Ok(0);
    }
    if depth == 0 {
        return Err(EIO);
    }

    let meta = state.ext2_meta;
    let entries = ext2_indirect_entries_per_block(&meta)?;
    let mut count = 1u32;
    let mut idx = 0usize;
    while idx < entries {
        let child = ext2_read_indirect_entry_from_state(state, table_block, idx)?;
        if child != 0 {
            if depth == 1 {
                count = count.saturating_add(1);
            } else {
                count = count.saturating_add(ext2_count_indirect_tree_blocks_state(
                    state,
                    child,
                    depth - 1,
                )?);
            }
        }
        idx += 1;
    }

    Ok(count)
}

fn indirect_recalc_block_count_512_state(
    state: &mut VfsState,
    inode: &Ext2Inode,
) -> Result<u64, i64> {
    let meta = state.ext2_meta;
    let sectors_per_block = (meta.block_size / 512) as u32;
    let mut used_blocks = 0u32;

    let mut i = 0usize;
    while i < EXT2_INODE_DIRECT_COUNT {
        if inode.blocks[i] != 0 {
            used_blocks = used_blocks.saturating_add(1);
        }
        i += 1;
    }

    used_blocks = used_blocks.saturating_add(ext2_count_indirect_tree_blocks_state(
        state,
        inode.blocks[12],
        1,
    )?);
    used_blocks = used_blocks.saturating_add(ext2_count_indirect_tree_blocks_state(
        state,
        inode.blocks[13],
        2,
    )?);
    used_blocks = used_blocks.saturating_add(ext2_count_indirect_tree_blocks_state(
        state,
        inode.blocks[14],
        3,
    )?);

    Ok((used_blocks as u64) * (sectors_per_block as u64)
        + ext2_inode_acl_sectors(&meta, inode) as u64)
}

/// Stores `sectors` (512-byte units) in `i_blocks`: 32 bits, or 48 with
/// `huge_file` (in filesystem blocks when the inode has
/// `EXT4_HUGE_FILE_FL`). `EFBIG` when the count does not fit.
fn ext2_set_block_count(meta: &Ext2Meta, inode: &mut Ext2Inode, sectors: u64) -> Result<(), i64> {
    if (meta.feature_ro_compat & EXT4_FEATURE_RO_COMPAT_HUGE_FILE) == 0 {
        inode.block_count_512 = u32::try_from(sectors).map_err(|_| EFBIG)?;
        return Ok(());
    }
    let raw = if (inode.flags & EXT4_HUGE_FILE_FL) != 0 {
        sectors / (meta.block_size as u64 / 512)
    } else {
        sectors
    };
    if raw >= (1u64 << 48) {
        return Err(EFBIG);
    }
    inode.block_count_512 = raw as u32;
    inode.blocks_hi = (raw >> 32) as u16;
    Ok(())
}

/// Recomputes `i_blocks` from the inode's block map.
fn ext2_update_block_count(state: &mut VfsState, inode: &mut Ext2Inode) -> Result<(), i64> {
    let sectors = ext2_recalc_inode_block_count_512_state(state, inode)?;
    let meta = state.ext2_meta;
    ext2_set_block_count(&meta, inode, sectors)
}

/// First mapped logical block at or after `from` in an indirect map,
/// skipping unallocated mapping subtrees.
fn indirect_next_mapped(
    state: &mut VfsState,
    inode: &Ext2Inode,
    from: usize,
) -> Result<Option<usize>, i64> {
    for i in from..EXT2_INODE_DIRECT_COUNT {
        if inode.blocks[i] != 0 {
            return Ok(Some(i));
        }
    }
    let meta = state.ext2_meta;
    let entries = ext2_indirect_entries_per_block(&meta)?;
    let mut base = EXT2_INODE_DIRECT_COUNT;
    let mut span = entries;
    for level in 1..=3u32 {
        if from < base + span {
            let start = from.saturating_sub(base);
            let root = inode.blocks[EXT2_INODE_DIRECT_COUNT + level as usize - 1];
            if let Some(off) = indirect_next_in_tree(state, root, level, start, entries)? {
                return Ok(Some(base + off));
            }
        }
        base += span;
        span = span.checked_mul(entries).ok_or(EIO)?;
    }
    Ok(None)
}

fn indirect_next_in_tree(
    state: &mut VfsState,
    table: u32,
    depth: u32,
    from: usize,
    entries: usize,
) -> Result<Option<usize>, i64> {
    if table == 0 {
        return Ok(None);
    }
    let child_span = entries.pow(depth - 1);
    let first = from / child_span;
    for i in first..entries {
        let child = ext2_read_indirect_entry_from_state(state, table, i)?;
        if child == 0 {
            continue;
        }
        if depth == 1 {
            return Ok(Some(i));
        }
        let child_from = if i == first { from % child_span } else { 0 };
        if let Some(off) = indirect_next_in_tree(state, child, depth - 1, child_from, entries)? {
            return Ok(Some(i * child_span + off));
        }
    }
    Ok(None)
}

/// `i_blocks` in 512-byte sectors. With ext4 `huge_file` the count gains 16
/// high bits, and `EXT4_HUGE_FILE_FL` means it is in filesystem blocks.
fn ext2_inode_blocks_512(meta: &Ext2Meta, inode: &Ext2Inode) -> u64 {
    if (meta.feature_ro_compat & EXT4_FEATURE_RO_COMPAT_HUGE_FILE) == 0 {
        return inode.block_count_512 as u64;
    }
    let raw = ((inode.blocks_hi as u64) << 32) | inode.block_count_512 as u64;
    if (inode.flags & EXT4_HUGE_FILE_FL) != 0 {
        raw * (meta.block_size as u64 / 512)
    } else {
        raw
    }
}

/// 512-byte sectors charged to `i_blocks` for the inode's external xattr block.
fn ext2_inode_acl_sectors(meta: &Ext2Meta, inode: &Ext2Inode) -> u32 {
    if inode.file_acl != 0 {
        (meta.block_size / 512) as u32
    } else {
        0
    }
}

/// Mirrors Linux `ext2_inode_is_fast_symlink`: a symlink whose target lives
/// inline in `i_block` owns no data blocks (an xattr block does not count).
fn ext2_inode_is_fast_symlink(meta: &Ext2Meta, inode: &Ext2Inode) -> bool {
    (inode.mode & EXT2_S_IFMT) == EXT2_S_IFLNK
        && inode.block_count_512 == ext2_inode_acl_sectors(meta, inode)
        && (inode.size as usize) < EXT2_FAST_SYMLINK_MAX
}

/// Sets the full size; the high half goes to `i_size_high` for regular
/// files only.
fn ext2_set_inode_size64(inode: &mut Ext2Inode, size: u64) {
    inode.size = size as u32;
    if (inode.mode & EXT2_S_IFMT) == EXT2_S_IFREG {
        inode.size_high = (size >> 32) as u32;
    }
}

/// Files over 2 GiB need the `large_file` ro-compat feature; like Linux,
/// the kernel turns it on when it first creates one.
fn ext2_note_large_file(state: &mut VfsState, size: u64) -> Result<(), i64> {
    if size <= 0x7FFF_FFFF
        || (state.ext2_meta.feature_ro_compat & EXT2_FEATURE_RO_COMPAT_LARGE_FILE) != 0
    {
        return Ok(());
    }
    state.ext2_meta.feature_ro_compat |= EXT2_FEATURE_RO_COMPAT_LARGE_FILE;
    let value = state.ext2_meta.feature_ro_compat;
    ext2_write_u32_state(state, EXT2_SUPERBLOCK_OFFSET + 100, value)
}

/// Full on-disk size: `i_size_high` only extends regular files (for
/// directories the same field is the legacy `i_dir_acl`).
fn ext2_inode_size64(inode: &Ext2Inode) -> u64 {
    if (inode.mode & EXT2_S_IFMT) == EXT2_S_IFREG {
        ((inode.size_high as u64) << 32) | inode.size as u64
    } else {
        inode.size as u64
    }
}

/// Stores a short symlink target inline in `i_block` of a freshly created
/// (block-less) symlink inode.
fn ext2_write_fast_symlink_target(state: &mut VfsState, ino: u32, target: &[u8]) -> Result<(), i64> {
    if target.is_empty() || target.len() >= EXT2_FAST_SYMLINK_MAX {
        return Err(EINVAL);
    }
    let mut inode = ext2_read_inode_from_state(state, ino)?;
    if (inode.mode & EXT2_S_IFMT) != EXT2_S_IFLNK || inode.block_count_512 != 0 {
        return Err(EINVAL);
    }
    // An inline (fast) symlink holds text, not an extent tree; e2fsck flags
    // fast symlinks that carry EXTENTS_FL.
    if (inode.flags & ext2_bmap::EXT4_EXTENTS_FL) != 0 {
        ext2_clear_inode_flags_state(state, ino, ext2_bmap::EXT4_EXTENTS_FL)?;
        inode.flags &= !ext2_bmap::EXT4_EXTENTS_FL;
    }
    let mut inline = [0u8; EXT2_FAST_SYMLINK_MAX];
    inline[..target.len()].copy_from_slice(target);
    let mut i = 0usize;
    while i < 15 {
        inode.blocks[i] = u32::from_le_bytes([
            inline[i * 4],
            inline[i * 4 + 1],
            inline[i * 4 + 2],
            inline[i * 4 + 3],
        ]);
        i += 1;
    }
    inode.size = target.len() as u32;
    let now = fs_time_tick(state);
    inode.mtime = now;
    inode.ctime = now;
    ext2_write_inode_to_state(state, ino, &inode)?;
    ext2_refresh_alias_nodes(state, ino, &inode);
    Ok(())
}

/// ext4 inode layout for a freshly allocated (zeroed) inode: extent-mapped
/// with an empty tree when the filesystem has `extent`, and `i_extra_isize`
/// set to `s_want_extra_isize` for large inodes (`extra_isize`).
fn ext2_init_new_inode_layout(state: &mut VfsState, inode: &mut Ext2Inode) -> Result<(), i64> {
    let meta = state.ext2_meta;
    let offset = ext2_inode_offset_in_state(state, &meta, inode.ino)?;
    if state.ext4_csum.map_or(false, |csum| csum.metadata_csum) {
        // A fresh i_generation per inode (Linux uses a random value); it
        // seeds this inode's checksums, so set it before any block is
        // written for the inode.
        let mut generation = [0u8; 4];
        fill_xorshift_bytes(state, &mut generation);
        inode.generation = u32::from_le_bytes(generation);
        ext2_write_u32_state(state, offset + 0x64, inode.generation)?;
    }
    if (meta.feature_incompat & EXT4_FEATURE_INCOMPAT_EXTENTS) != 0 {
        inode.flags |= ext2_bmap::EXT4_EXTENTS_FL;
        inode.blocks = ext2_bmap::empty_extent_root();
        ext2_write_u32_state(state, offset + 32, inode.flags)?;
    }
    if meta.inode_size > 128 {
        let want = ext2_read_u16_state(state, EXT2_SUPERBLOCK_OFFSET + 0x15E)?;
        let extra = if want >= 4 && (want as usize) <= meta.inode_size - 128 { want } else { 32 };
        if (extra as usize) <= meta.inode_size - 128 {
            ext2_write_u16_state(state, offset + 0x80, extra)?;
            if extra >= 24 {
                // i_crtime / i_crtime_extra: creation time.
                let (base, epoch) = ext4_encode_time(inode.ctime, true);
                ext2_write_u32_state(state, offset + 0x90, base)?;
                ext2_write_u32_state(state, offset + 0x94, epoch)?;
            }
        }
    }
    ext2_update_inode_csum(state, inode.ino)
}

/// `dir_nlink`: a directory whose link count would exceed EXT4_LINK_MAX
/// keeps 1 ("many"); otherwise links are counted normally.
const EXT4_LINK_MAX: u16 = 65000;

fn ext2_dir_links_inc(meta: &Ext2Meta, links: u16) -> u16 {
    if (meta.feature_ro_compat & EXT4_FEATURE_RO_COMPAT_DIR_NLINK) != 0
        && (links == 1 || links >= EXT4_LINK_MAX - 1)
    {
        return 1;
    }
    links.saturating_add(1)
}

fn ext2_dir_links_dec(meta: &Ext2Meta, links: u16) -> u16 {
    if (meta.feature_ro_compat & EXT4_FEATURE_RO_COMPAT_DIR_NLINK) != 0 && links == 1 {
        return 1;
    }
    links.saturating_sub(1)
}

/// Clears `mask` from the on-disk `i_flags` of `ino`. Kept separate from
/// `ext2_write_inode_to_state` so stale in-memory inode copies can never
/// resurrect a cleared flag.
fn ext2_set_inode_flags_state(state: &mut VfsState, ino: u32, mask: u32) -> Result<(), i64> {
    let meta = state.ext2_meta;
    let inode_offset = ext2_inode_offset_in_state(state, &meta, ino)?;
    ext2_ensure_range_resident(state, inode_offset, meta.inode_size)?;
    let flags = ext2_read_u32_state(state, inode_offset + 32)?;
    if (flags & mask) != mask {
        ext2_write_u32_state(state, inode_offset + 32, flags | mask)?;
        ext2_update_inode_csum(state, ino)?;
    }
    Ok(())
}

fn ext2_clear_inode_flags_state(state: &mut VfsState, ino: u32, mask: u32) -> Result<(), i64> {
    let meta = state.ext2_meta;
    let inode_offset = ext2_inode_offset_in_state(state, &meta, ino)?;
    ext2_ensure_range_resident(state, inode_offset, meta.inode_size)?;
    let flags = ext2_read_u32_state(state, inode_offset + 32)?;
    if (flags & mask) != 0 {
        ext2_write_u32_state(state, inode_offset + 32, flags & !mask)?;
        ext2_update_inode_csum(state, ino)?;
    }
    Ok(())
}

/// Drops this inode's reference on its external xattr block, freeing the
/// block when the last reference goes away. A block without the xattr magic
/// is left allocated rather than freeing something we do not understand.
fn ext2_release_xattr_block(
    state: &mut VfsState,
    journal_handle: Option<Ext2JournalHandle>,
    inode: &mut Ext2Inode,
) -> Result<(), i64> {
    let block = inode.file_acl;
    if block == 0 {
        return Ok(());
    }
    inode.file_acl = 0;
    let meta = state.ext2_meta;
    if block < meta.first_data_block || block >= meta.total_blocks {
        return Ok(());
    }

    let free_block = ext2_journal::with_handle_restartable_phase(
        state,
        journal_handle,
        Ext2JournalPhaseKind::XattrRelease,
        EXT2_JOURNAL_CREDITS_HELPER_XATTR_RELEASE,
        |state| {
            let (magic, refcount) = {
                let blk = ext2_block_slice_state(state, block)?;
                (read_u32_le(blk, 0)?, read_u32_le(blk, 4)?)
            };
            if magic != EXT2_XATTR_MAGIC {
                return Ok(false);
            }
            if refcount > 1 {
                let csum = state.ext4_csum.filter(|csum| csum.metadata_csum);
                {
                    let blk = ext2_block_slice_mut_state(state, block)?;
                    write_u32_le(blk, 4, refcount - 1)?;
                    if let Some(csum) = csum {
                        let value = csum.xattr_block_csum(block as u64, blk)?;
                        write_u32_le(blk, 0x10, value)?;
                    }
                }
                ext2_mark_dirty_block(state, block)?;
                return Ok(false);
            }
            Ok(true)
        },
    )?;
    if free_block {
        ext2_free_block(state, journal_handle, block)?;
    }
    Ok(())
}

fn ext2_read_inode_from_state(state: &mut VfsState, ino: u32) -> Result<Ext2Inode, i64> {
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

    let mode = ext2_read_u16_state(state, inode_offset)?;
    let uid = ext2_read_u16_state(state, inode_offset + 2)?;
    let size = ext2_read_u32_state(state, inode_offset + 4)?;
    let extra_isize = ext2_inode_extra_isize(state, inode_offset)?;
    let mut time_extra = [0u32; 3];
    let mut time_orig = [0u64; 3];
    for (i, base_off) in [8usize, 12, 16].into_iter().enumerate() {
        let base = ext2_read_u32_state(state, inode_offset + base_off)?;
        let (extra_off, needs) = EXT4_TIME_EXTRA[i];
        let extra = if extra_isize >= needs {
            let raw = ext2_read_u32_state(state, inode_offset + extra_off)?;
            time_extra[i] = raw;
            Some(raw)
        } else {
            None
        };
        time_orig[i] = ext4_decode_time(base, extra);
    }
    let [atime, ctime, mtime] = time_orig;
    let gid = ext2_read_u16_state(state, inode_offset + 24)?;
    let links_count = ext2_read_u16_state(state, inode_offset + 26)?;
    let block_count_512 = ext2_read_u32_state(state, inode_offset + 28)?;
    let flags = ext2_read_u32_state(state, inode_offset + 32)?;
    let file_acl = ext2_read_u32_state(state, inode_offset + 104)?;
    let size_high = ext2_read_u32_state(state, inode_offset + 108)?;
    let generation = ext2_read_u32_state(state, inode_offset + 0x64)?;
    let blocks_hi = ext2_read_u16_state(state, inode_offset + 0x74)?;
    ext2_verify_inode_csum(state, ino, inode_offset)?;

    let mut blocks = [0u32; 15];
    let mut i = 0usize;
    while i < 15 {
        blocks[i] = ext2_read_u32_state(state, inode_offset + 40 + i * 4)?;
        i += 1;
    }

    Ok(Ext2Inode {
        mode,
        uid,
        size,
        atime,
        ctime,
        mtime,
        links_count,
        gid,
        block_count_512,
        blocks,
        flags,
        file_acl,
        size_high,
        ino,
        generation,
        blocks_hi,
        time_extra,
        time_orig,
    })
}

fn ext2_refresh_alias_nodes(state: &mut VfsState, ext2_ino: u32, inode: &Ext2Inode) {
    let mut idx = 0usize;
    while idx < state.inodes.len() {
        let node = &mut state.inodes[idx];
        if node.used && node.backend == NodeBackend::Ext2 && node.ext2_ino == ext2_ino {
            node.size = ext2_inode_size64(inode);
            node.mode = inode.mode;
            node.uid = inode.uid;
            node.gid = inode.gid;
            node.links_count = inode.links_count;
            node.atime = inode.atime as u64;
            node.mtime = inode.mtime as u64;
            node.ctime = inode.ctime as u64;
        }
        idx += 1;
    }
}

fn ext2_indirect_entries_per_block(meta: &Ext2Meta) -> Result<usize, i64> {
    if (meta.block_size & 0x3) != 0 {
        return Err(EIO);
    }
    let entries = meta.block_size / core::mem::size_of::<u32>();
    if entries == 0 {
        return Err(EIO);
    }
    Ok(entries)
}

#[allow(dead_code)]
fn ext2_read_indirect_entry(
    image: &[u8],
    meta: &Ext2Meta,
    table_block: u32,
    index: usize,
) -> Result<u32, i64> {
    let table = ext2_block_slice(image, meta, table_block)?;
    let offset = index.checked_mul(core::mem::size_of::<u32>()).ok_or(EIO)?;
    read_u32_le(table, offset)
}

#[allow(dead_code)]
fn ext2_write_indirect_entry(
    image: &mut [u8],
    meta: &Ext2Meta,
    table_block: u32,
    index: usize,
    value: u32,
) -> Result<(), i64> {
    let table = ext2_block_slice_mut(image, meta, table_block)?;
    let offset = index.checked_mul(core::mem::size_of::<u32>()).ok_or(EIO)?;
    write_u32_le(table, offset, value)
}

fn ext2_read_indirect_entry_from_state(
    state: &mut VfsState,
    table_block: u32,
    index: usize,
) -> Result<u32, i64> {
    let meta = state.ext2_meta;
    let offset = index.checked_mul(core::mem::size_of::<u32>()).ok_or(EIO)?;
    if offset + core::mem::size_of::<u32>() > meta.block_size {
        return Err(EIO);
    }
    let table = ext2_block_slice_state(state, table_block)?;
    read_u32_le(table, offset)
}

fn ext2_write_indirect_entry_from_state(
    state: &mut VfsState,
    table_block: u32,
    index: usize,
    value: u32,
) -> Result<(), i64> {
    let meta = state.ext2_meta;
    let offset = index.checked_mul(core::mem::size_of::<u32>()).ok_or(EIO)?;
    if offset + core::mem::size_of::<u32>() > meta.block_size {
        return Err(EIO);
    }
    {
        let table = ext2_block_slice_mut_state(state, table_block)?;
        write_u32_le(table, offset, value)?;
    }
    ext2_mark_dirty_block(state, table_block)
}

fn ext2_max_addressable_file_size(meta: &Ext2Meta) -> Result<usize, i64> {
    let entries = ext2_indirect_entries_per_block(meta)?;
    let double = entries.checked_mul(entries).ok_or(EIO)?;
    let triple = double.checked_mul(entries).ok_or(EIO)?;
    let blocks = EXT2_INODE_DIRECT_COUNT
        .checked_add(entries)
        .and_then(|v| v.checked_add(double))
        .and_then(|v| v.checked_add(triple))
        .ok_or(EIO)?;
    blocks.checked_mul(meta.block_size).ok_or(EIO)
}

/// Largest size a regular file using `extents` (or an indirect map) may
/// reach: the map's reach (2^32 - 1 blocks for extents), and without
/// `huge_file` whatever keeps `i_blocks` within 32 bits of sectors (Linux
/// `ext4_max_size`).
fn ext2_max_file_size_for(meta: &Ext2Meta, extents: bool) -> Result<u64, i64> {
    let bs = meta.block_size as u64;
    let mut max_blocks = if extents {
        (1u64 << 32) - 1
    } else {
        (ext2_max_addressable_file_size(meta)? / meta.block_size) as u64
    };
    if (meta.feature_ro_compat & EXT4_FEATURE_RO_COMPAT_HUGE_FILE) == 0 {
        max_blocks = core::cmp::min(max_blocks, u32::MAX as u64 / (bs / 512));
    }
    Ok(max_blocks * bs)
}

fn ext2_max_file_size(meta: &Ext2Meta, inode: &Ext2Inode) -> Result<u64, i64> {
    ext2_max_file_size_for(meta, (inode.flags & ext2_bmap::EXT4_EXTENTS_FL) != 0)
}

/// Largest file size on this filesystem (new files use extents when the
/// `extent` feature is on).
fn ext2_fs_max_file_size(meta: &Ext2Meta) -> Result<u64, i64> {
    ext2_max_file_size_for(meta, (meta.feature_incompat & EXT4_FEATURE_INCOMPAT_EXTENTS) != 0)
}

fn ext2_logical_blocks_for_size(meta: &Ext2Meta, size: usize) -> Result<usize, i64> {
    if size == 0 {
        return Ok(0);
    }
    let rounded = size.checked_add(meta.block_size - 1).ok_or(EIO)?;
    Ok(rounded / meta.block_size)
}

#[cfg(test)]
fn ext2_data_block_for_logical_index(
    image: &[u8],
    meta: &Ext2Meta,
    inode: &Ext2Inode,
    block_idx: usize,
) -> Result<u32, i64> {
    if block_idx < EXT2_INODE_DIRECT_COUNT {
        return Ok(inode.blocks[block_idx]);
    }

    let entries = ext2_indirect_entries_per_block(meta)?;
    let mut remaining = block_idx - EXT2_INODE_DIRECT_COUNT;

    if remaining < entries {
        let table = inode.blocks[12];
        if table == 0 {
            return Ok(0);
        }
        return ext2_read_indirect_entry(image, meta, table, remaining);
    }
    remaining -= entries;

    let double_span = entries.checked_mul(entries).ok_or(EIO)?;
    if remaining < double_span {
        let table = inode.blocks[13];
        if table == 0 {
            return Ok(0);
        }
        let first = remaining / entries;
        let second = remaining % entries;
        let l1 = ext2_read_indirect_entry(image, meta, table, first)?;
        if l1 == 0 {
            return Ok(0);
        }
        return ext2_read_indirect_entry(image, meta, l1, second);
    }
    remaining -= double_span;

    let triple_span = double_span.checked_mul(entries).ok_or(EIO)?;
    if remaining < triple_span {
        let table = inode.blocks[14];
        if table == 0 {
            return Ok(0);
        }
        let first = remaining / double_span;
        let second = (remaining % double_span) / entries;
        let third = remaining % entries;
        let l1 = ext2_read_indirect_entry(image, meta, table, first)?;
        if l1 == 0 {
            return Ok(0);
        }
        let l2 = ext2_read_indirect_entry(image, meta, l1, second)?;
        if l2 == 0 {
            return Ok(0);
        }
        return ext2_read_indirect_entry(image, meta, l2, third);
    }

    Err(EIO)
}

fn indirect_lookup_state(
    state: &mut VfsState,
    inode: &Ext2Inode,
    block_idx: usize,
) -> Result<u32, i64> {
    if block_idx < EXT2_INODE_DIRECT_COUNT {
        return Ok(inode.blocks[block_idx]);
    }

    let meta = state.ext2_meta;
    let entries = ext2_indirect_entries_per_block(&meta)?;
    let mut remaining = block_idx - EXT2_INODE_DIRECT_COUNT;

    if remaining < entries {
        let table = inode.blocks[12];
        if table == 0 {
            return Ok(0);
        }
        return ext2_read_indirect_entry_from_state(state, table, remaining);
    }
    remaining -= entries;

    let double_span = entries.checked_mul(entries).ok_or(EIO)?;
    if remaining < double_span {
        let table = inode.blocks[13];
        if table == 0 {
            return Ok(0);
        }
        let first = remaining / entries;
        let second = remaining % entries;
        let l1 = ext2_read_indirect_entry_from_state(state, table, first)?;
        if l1 == 0 {
            return Ok(0);
        }
        return ext2_read_indirect_entry_from_state(state, l1, second);
    }
    remaining -= double_span;

    let triple_span = double_span.checked_mul(entries).ok_or(EIO)?;
    if remaining < triple_span {
        let table = inode.blocks[14];
        if table == 0 {
            return Ok(0);
        }
        let first = remaining / double_span;
        let second = (remaining % double_span) / entries;
        let third = remaining % entries;
        let l1 = ext2_read_indirect_entry_from_state(state, table, first)?;
        if l1 == 0 {
            return Ok(0);
        }
        let l2 = ext2_read_indirect_entry_from_state(state, l1, second)?;
        if l2 == 0 {
            return Ok(0);
        }
        return ext2_read_indirect_entry_from_state(state, l2, third);
    }

    Err(EIO)
}

fn ext2_table_has_nonzero_entries_state(
    state: &mut VfsState,
    table_block: u32,
) -> Result<bool, i64> {
    if table_block == 0 {
        return Ok(false);
    }
    let meta = state.ext2_meta;
    let entries = ext2_indirect_entries_per_block(&meta)?;
    let mut idx = 0usize;
    while idx < entries {
        if ext2_read_indirect_entry_from_state(state, table_block, idx)? != 0 {
            return Ok(true);
        }
        idx += 1;
    }
    Ok(false)
}

fn indirect_detach_block(
    state: &mut VfsState,
    journal_handle: Option<Ext2JournalHandle>,
    inode: &mut Ext2Inode,
    block_idx: usize,
) -> Result<u32, i64> {
    if block_idx < EXT2_INODE_DIRECT_COUNT {
        let block = inode.blocks[block_idx];
        inode.blocks[block_idx] = 0;
        return Ok(block);
    }

    let meta = state.ext2_meta;
    let entries = ext2_indirect_entries_per_block(&meta)?;
    let mut remaining = block_idx - EXT2_INODE_DIRECT_COUNT;

    if remaining < entries {
        let table = inode.blocks[12];
        if table == 0 {
            return Ok(0);
        }

        let current = ext2_read_indirect_entry_from_state(state, table, remaining)?;
        if current == 0 {
            return Ok(0);
        }

        ext2_journal::with_handle_restartable_phase(
            state,
            journal_handle,
            Ext2JournalPhaseKind::IndirectUnlink,
            EXT2_JOURNAL_CREDITS_HELPER_INDIRECT_UNLINK,
            |state| ext2_write_indirect_entry_from_state(state, table, remaining, 0),
        )?;
        let table_has_entries = ext2_table_has_nonzero_entries_state(state, table)?;
        if !table_has_entries {
            ext2_free_block(state, journal_handle, table)?;
            inode.blocks[12] = 0;
        }
        return Ok(current);
    }
    remaining -= entries;

    let double_span = entries.checked_mul(entries).ok_or(EIO)?;
    if remaining < double_span {
        let table = inode.blocks[13];
        if table == 0 {
            return Ok(0);
        }
        let first = remaining / entries;
        let second = remaining % entries;
        let l1 = ext2_read_indirect_entry_from_state(state, table, first)?;
        if l1 == 0 {
            return Ok(0);
        }

        let current = ext2_read_indirect_entry_from_state(state, l1, second)?;
        if current == 0 {
            return Ok(0);
        }

        ext2_journal::with_handle_restartable_phase(
            state,
            journal_handle,
            Ext2JournalPhaseKind::IndirectUnlink,
            EXT2_JOURNAL_CREDITS_HELPER_INDIRECT_UNLINK,
            |state| ext2_write_indirect_entry_from_state(state, l1, second, 0),
        )?;
        let l1_has_entries = ext2_table_has_nonzero_entries_state(state, l1)?;
        if !l1_has_entries {
            ext2_free_block(state, journal_handle, l1)?;
            ext2_journal::with_handle_restartable_phase(
                state,
                journal_handle,
                Ext2JournalPhaseKind::IndirectUnlink,
                EXT2_JOURNAL_CREDITS_HELPER_INDIRECT_UNLINK,
                |state| ext2_write_indirect_entry_from_state(state, table, first, 0),
            )?;
        }

        let table_has_entries = ext2_table_has_nonzero_entries_state(state, table)?;
        if !table_has_entries {
            ext2_free_block(state, journal_handle, table)?;
            inode.blocks[13] = 0;
        }
        return Ok(current);
    }
    remaining -= double_span;

    let triple_span = double_span.checked_mul(entries).ok_or(EIO)?;
    if remaining < triple_span {
        let table = inode.blocks[14];
        if table == 0 {
            return Ok(0);
        }
        let first = remaining / double_span;
        let second = (remaining % double_span) / entries;
        let third = remaining % entries;

        let l1 = ext2_read_indirect_entry_from_state(state, table, first)?;
        if l1 == 0 {
            return Ok(0);
        }
        let l2 = ext2_read_indirect_entry_from_state(state, l1, second)?;
        if l2 == 0 {
            return Ok(0);
        }

        let current = ext2_read_indirect_entry_from_state(state, l2, third)?;
        if current == 0 {
            return Ok(0);
        }

        ext2_journal::with_handle_restartable_phase(
            state,
            journal_handle,
            Ext2JournalPhaseKind::IndirectUnlink,
            EXT2_JOURNAL_CREDITS_HELPER_INDIRECT_UNLINK,
            |state| ext2_write_indirect_entry_from_state(state, l2, third, 0),
        )?;
        let l2_has_entries = ext2_table_has_nonzero_entries_state(state, l2)?;
        if !l2_has_entries {
            ext2_free_block(state, journal_handle, l2)?;
            ext2_journal::with_handle_restartable_phase(
                state,
                journal_handle,
                Ext2JournalPhaseKind::IndirectUnlink,
                EXT2_JOURNAL_CREDITS_HELPER_INDIRECT_UNLINK,
                |state| ext2_write_indirect_entry_from_state(state, l1, second, 0),
            )?;
        }

        let l1_has_entries = ext2_table_has_nonzero_entries_state(state, l1)?;
        if !l1_has_entries {
            ext2_free_block(state, journal_handle, l1)?;
            ext2_journal::with_handle_restartable_phase(
                state,
                journal_handle,
                Ext2JournalPhaseKind::IndirectUnlink,
                EXT2_JOURNAL_CREDITS_HELPER_INDIRECT_UNLINK,
                |state| ext2_write_indirect_entry_from_state(state, table, first, 0),
            )?;
        }

        let table_has_entries = ext2_table_has_nonzero_entries_state(state, table)?;
        if !table_has_entries {
            ext2_free_block(state, journal_handle, table)?;
            inode.blocks[14] = 0;
        }
        return Ok(current);
    }

    Err(EIO)
}

fn ext2_read_file(
    state: &mut VfsState,
    ino: u32,
    offset: u64,
    dst: &mut [u8],
) -> Result<usize, i64> {
    if dst.is_empty() {
        return Ok(0);
    }

    let meta = state.ext2_meta;
    let inode = ext2_read_inode_from_state(state, ino)?;
    let size = usize::try_from(ext2_inode_size64(&inode)).map_err(|_| EOVERFLOW)?;
    let offset = usize::try_from(offset).map_err(|_| EINVAL)?;
    if offset >= size {
        return Ok(0);
    }

    if ext2_inode_is_fast_symlink(&meta, &inode) {
        let mut inline = [0u8; EXT2_FAST_SYMLINK_MAX];
        let mut i = 0usize;
        while i < 15 {
            inline[i * 4..i * 4 + 4].copy_from_slice(&inode.blocks[i].to_le_bytes());
            i += 1;
        }
        let start = offset as usize;
        let count = core::cmp::min(dst.len(), size - start);
        dst[..count].copy_from_slice(&inline[start..start + count]);
        return Ok(count);
    }

    let mut copied = 0usize;
    let mut pos = offset;
    while copied < dst.len() && pos < size {
        let block_idx = pos / meta.block_size;
        let block_off = pos % meta.block_size;
        let remain_file = size - pos;
        let chunk = core::cmp::min(
            meta.block_size - block_off,
            core::cmp::min(remain_file, dst.len() - copied),
        );
        let block_no = ext2_data_block_for_logical_index_state(state, &inode, block_idx)?;

        if block_no == 0 {
            dst[copied..copied + chunk].fill(0);
        } else {
            let blk = ext2_block_slice_state(state, block_no)?;
            dst[copied..copied + chunk].copy_from_slice(&blk[block_off..block_off + chunk]);
        }

        copied += chunk;
        pos += chunk;
    }

    Ok(copied)
}

fn ext2_write_file(
    state: &mut VfsState,
    ino: u32,
    offset: u64,
    src: &[u8],
    append: bool,
) -> Result<WriteResult, i64> {
    let meta = state.ext2_meta;
    let journal_handle = ext2_journal::current_handle(state);

    let mut inode = ext2_read_inode_from_state(state, ino)?;
    let max_size = ext2_max_file_size(&meta, &inode)?;
    let old_size = ext2_inode_size64(&inode);
    let start = if append { old_size } else { offset };

    if src.is_empty() {
        return Ok(WriteResult {
            written: 0,
            new_offset: start,
        });
    }
    if start >= max_size {
        return Err(EFBIG);
    }

    if start > old_size {
        ext2_write_zero_range(state, journal_handle, &mut inode, old_size, start)?;
    }

    let count = core::cmp::min(src.len() as u64, max_size - start) as usize;
    let start = usize::try_from(start).map_err(|_| EFBIG)?;
    let mut copied = 0usize;
    let mut pos = start;
    while copied < count {
        let block_idx = pos / meta.block_size;
        let step_credits = ext2_estimate_data_write_step_credits(state, &inode, block_idx)?;
        let chunk = ext2_journal::with_handle_restartable_phase(
            state,
            journal_handle,
            Ext2JournalPhaseKind::DataWrite,
            step_credits,
            |state| {
                let block_off = pos % meta.block_size;
                let chunk = core::cmp::min(count - copied, meta.block_size - block_off);
                let block_no =
                    ext2_ensure_data_block(state, journal_handle, &mut inode, block_idx)?;

                {
                    let blk = ext2_block_slice_mut_state(state, block_no)?;
                    blk[block_off..block_off + chunk]
                        .copy_from_slice(&src[copied..copied + chunk]);
                }
                ext2_journal::note_data_block(state, block_no)?;
                ext2_mark_dirty_block(state, block_no)?;
                Ok(chunk)
            },
        )?;

        copied += chunk;
        pos += chunk;
    }

    let end = (start + count) as u64;
    if end > ext2_inode_size64(&inode) {
        ext2_set_inode_size64(&mut inode, end);
        ext2_note_large_file(state, end)?;
    }
    let now = fs_time_tick(state);
    inode.mtime = now;
    inode.ctime = now;
    inode.atime = now;
    ext2_update_block_count(state, &mut inode)?;
    ext2_journal::with_handle_restartable_phase(
        state,
        journal_handle,
        Ext2JournalPhaseKind::InodeCloseout,
        EXT2_JOURNAL_CREDITS_STEP_INODE_CLOSEOUT,
        |state| ext2_write_inode_to_state(state, ino, &inode),
    )?;
    ext2_refresh_alias_nodes(state, ino, &inode);

    Ok(WriteResult {
        written: count,
        new_offset: end as u64,
    })
}

fn ext2_free_indirect_tree_blocks(
    state: &mut VfsState,
    journal_handle: Option<Ext2JournalHandle>,
    table_block: u32,
    depth: u8,
) -> Result<(), i64> {
    if table_block == 0 {
        return Ok(());
    }
    if depth == 0 {
        return Err(EIO);
    }

    let meta = state.ext2_meta;
    let entries = ext2_indirect_entries_per_block(&meta)?;
    let mut idx = 0usize;
    while idx < entries {
        let child = ext2_read_indirect_entry_from_state(state, table_block, idx)?;
        if child != 0 {
            let step_credits = match depth {
                1 => EXT2_JOURNAL_CREDITS_STEP_FREE_DIRECT,
                2 => {
                    EXT2_JOURNAL_CREDITS_STEP_FREE_TABLE_ENTRY
                        + EXT2_JOURNAL_CREDITS_STEP_FREE_DIRECT
                }
                3 => {
                    EXT2_JOURNAL_CREDITS_STEP_FREE_TABLE_ENTRY * 2
                        + EXT2_JOURNAL_CREDITS_STEP_FREE_DIRECT
                        + EXT2_JOURNAL_CREDITS_STEP_FREE_TABLE_BLOCK
                }
                _ => EXT2_JOURNAL_CREDITS_STEP_FREE_DIRECT,
            };
            ext2_journal::with_handle_restartable_phase(
                state,
                journal_handle,
                Ext2JournalPhaseKind::FreeBlockBookkeeping,
                step_credits,
                |state| {
                    if depth == 1 {
                        ext2_free_block(state, journal_handle, child)
                    } else {
                        ext2_free_indirect_tree_blocks(state, journal_handle, child, depth - 1)
                    }
                },
            )?;
        }
        idx += 1;
    }

    ext2_free_block(state, journal_handle, table_block)
}

fn ext2_truncate_to_zero(state: &mut VfsState, ino: u32) -> Result<(), i64> {
    let mut inode = ext2_read_inode_from_state(state, ino)?;
    let journal_handle = ext2_journal::current_handle(state);
    bmap_free_all_blocks(state, journal_handle, &mut inode, true)?;

    ext2_set_inode_size64(&mut inode, 0);
    inode.block_count_512 = ext2_inode_acl_sectors(&state.ext2_meta, &inode);
    let now = fs_time_tick(state);
    inode.mtime = now;
    inode.ctime = now;
    inode.atime = now;
    ext2_journal::with_handle_restartable_phase(
        state,
        journal_handle,
        Ext2JournalPhaseKind::InodeCloseout,
        EXT2_JOURNAL_CREDITS_STEP_INODE_CLOSEOUT,
        |state| ext2_write_inode_to_state(state, ino, &inode),
    )?;
    ext2_refresh_alias_nodes(state, ino, &inode);
    Ok(())
}

fn ext2_truncate_to_length(state: &mut VfsState, ino: u32, length: u64) -> Result<(), i64> {
    let meta = state.ext2_meta;
    let mut inode = ext2_read_inode_from_state(state, ino)?;
    if length > ext2_max_file_size(&meta, &inode)? {
        return Err(EFBIG);
    }
    let journal_handle = ext2_journal::current_handle(state);
    let old_size = ext2_inode_size64(&inode);

    if length > old_size {
        ext2_write_zero_range(state, journal_handle, &mut inode, old_size, length)?;
    } else if length < old_size {
        let old_size = usize::try_from(old_size).map_err(|_| EFBIG)?;
        let length = length as usize;
        let old_blocks = ext2_logical_blocks_for_size(&meta, old_size)?;
        let new_blocks = ext2_logical_blocks_for_size(&meta, length)?;

        if length > 0 && (length % meta.block_size) != 0 {
            let block_idx = length / meta.block_size;
            let block_off = length % meta.block_size;
            let block_no = ext2_data_block_for_logical_index_state(state, &inode, block_idx)?;
            if block_no != 0 {
                ext2_journal::with_handle_restartable_phase(
                    state,
                    journal_handle,
                    Ext2JournalPhaseKind::DataZero,
                    EXT2_JOURNAL_CREDITS_STEP_EXISTING_DATA,
                    |state| {
                        {
                            let blk = ext2_block_slice_mut_state(state, block_no)?;
                            blk[block_off..].fill(0);
                        }
                        ext2_journal::note_data_block(state, block_no)?;
                        ext2_mark_dirty_block(state, block_no)
                    },
                )?;
            }
        }

        // Skip holes: a sparse multi-GiB file has few mapped blocks.
        let mut idx = new_blocks;
        while idx < old_blocks {
            idx = match ext2_bmap::ext2_next_mapped_block(state, &inode, idx)? {
                Some(next) if next < old_blocks => next,
                _ => break,
            };
            let step_credits = ext2_estimate_detach_step_credits(state, &inode, idx)?;
            ext2_journal::with_handle_restartable_phase(
                state,
                journal_handle,
                Ext2JournalPhaseKind::IndirectUnlink,
                step_credits,
                |state| {
                    let removed = ext2_detach_data_block(state, journal_handle, &mut inode, idx)?;
                    if removed != 0 {
                        ext2_free_block(state, journal_handle, removed)?;
                    }
                    Ok(())
                },
            )?;
            idx += 1;
        }
    }

    ext2_set_inode_size64(&mut inode, length);
    ext2_note_large_file(state, length)?;
    let now = fs_time_tick(state);
    inode.mtime = now;
    inode.ctime = now;
    inode.atime = now;
    ext2_update_block_count(state, &mut inode)?;
    ext2_journal::with_handle_restartable_phase(
        state,
        journal_handle,
        Ext2JournalPhaseKind::InodeCloseout,
        EXT2_JOURNAL_CREDITS_STEP_INODE_CLOSEOUT,
        |state| ext2_write_inode_to_state(state, ino, &inode),
    )?;
    ext2_refresh_alias_nodes(state, ino, &inode);
    Ok(())
}

fn ext2_for_each_dir_entry_state<F>(
    state: &mut VfsState,
    inode: &Ext2Inode,
    mut f: F,
) -> Result<(), i64>
where
    F: FnMut(u32, u8, &[u8]) -> Result<(), i64>,
{
    let meta = state.ext2_meta;
    let block_count = ext2_logical_blocks_for_size(&meta, inode.size as usize)?;
    let mut block_idx = 0usize;
    while block_idx < block_count {
        let block_num = ext2_data_block_for_logical_index_state(state, inode, block_idx)?;
        block_idx += 1;

        if block_num == 0 {
            continue;
        }

        let dir_seed = state
            .ext4_csum
            .filter(|csum| csum.metadata_csum)
            .map(|csum| csum.inode_seed_for(inode.ino, inode.generation));
        if let Some(seed) = dir_seed {
            let ok = {
                let block = ext2_block_slice_state(state, block_num)?;
                ext4_csum::dir_leaf_csum_ok(seed, block)?
            };
            if ok == Some(false) {
                ext2_report(state, "directory block checksum", block_num as u64);
                return Err(EIO);
            }
            // No leaf tail: an htree root/node block carries a dx_tail.
            if ok.is_none() && (inode.flags & EXT2_INDEX_FL) != 0 {
                let dx_ok = {
                    let block = ext2_block_slice_state(state, block_num)?;
                    ext2_htree::dx_csum_ok(seed, block)
                };
                if dx_ok == Some(false) {
                    ext2_report(state, "htree index checksum", block_num as u64);
                    return Err(EIO);
                }
            }
        }
        let block = ext2_block_slice_state(state, block_num)?;
        let mut pos = 0usize;

        while pos + 8 <= block.len() {
            let inode_no = read_u32_le(block, pos)?;
            let rec_len = read_u16_le(block, pos + 4)? as usize;
            let name_len = block[pos + 6] as usize;
            let file_type = block[pos + 7];

            if rec_len < 8 || (rec_len & 0x3) != 0 || pos + rec_len > block.len() {
                return Err(EIO);
            }

            if inode_no != 0 {
                if 8 + name_len > rec_len {
                    return Err(EIO);
                }
                let name = &block[pos + 8..pos + 8 + name_len];
                f(inode_no, file_type, name)?;
            }

            pos += rec_len;
        }
    }

    Ok(())
}

fn ext2_free_inode_blocks(state: &mut VfsState, inode: &mut Ext2Inode) -> Result<(), i64> {
    let journal_handle = ext2_journal::current_handle(state);
    let meta = state.ext2_meta;
    if ext2_inode_is_fast_symlink(&meta, inode) {
        // i_block holds the inline target text, not block pointers.
        inode.blocks = [0; 15];
        ext2_release_xattr_block(state, journal_handle, inode)?;
        ext2_set_inode_size64(inode, 0);
        inode.block_count_512 = 0;
        return Ok(());
    }
    ext2_release_xattr_block(state, journal_handle, inode)?;
    bmap_free_all_blocks(state, journal_handle, inode, false)?;
    ext2_set_inode_size64(inode, 0);
    inode.block_count_512 = 0;
    Ok(())
}

/// Indirect-map implementation behind `bmap_free_all_blocks`.
fn indirect_free_all_blocks(
    state: &mut VfsState,
    journal_handle: Option<Ext2JournalHandle>,
    inode: &mut Ext2Inode,
    phased: bool,
) -> Result<(), i64> {
    let mut idx = 0usize;
    while idx < EXT2_INODE_DIRECT_COUNT {
        let block = inode.blocks[idx];
        if block != 0 {
            if phased {
                ext2_journal::with_handle_restartable_phase(
                    state,
                    journal_handle,
                    Ext2JournalPhaseKind::FreeBlockBookkeeping,
                    EXT2_JOURNAL_CREDITS_STEP_FREE_DIRECT,
                    |state| ext2_free_block(state, journal_handle, block),
                )?;
            } else {
                ext2_free_block(state, journal_handle, block)?;
            }
            inode.blocks[idx] = 0;
        }
        idx += 1;
    }

    let mut level = 1u8;
    while level <= 3 {
        let slot = EXT2_INODE_DIRECT_COUNT + level as usize - 1;
        if inode.blocks[slot] != 0 {
            ext2_free_indirect_tree_blocks(state, journal_handle, inode.blocks[slot], level)?;
            inode.blocks[slot] = 0;
        }
        level += 1;
    }
    Ok(())
}

fn ext2_patch_dotdot(
    state: &mut VfsState,
    _journal_handle: Option<Ext2JournalHandle>,
    dir_ino: u32,
    parent_ino: u32,
) -> Result<(), i64> {
    let inode = ext2_read_inode_from_state(state, dir_ino)?;
    if ext2_mode_to_filetype(inode.mode) != Some(FileType::Directory) {
        return Err(ENOTDIR);
    }
    // Logical block 0 through the block map (extent-mapped directories keep
    // an extent header in i_block, not a block number).
    let block = ext2_data_block_for_logical_index_state(state, &inode, 0)?;
    if block == 0 {
        return Err(EIO);
    }
    let tail = ext2_dir_tail_len(state);

    let mut patched = false;
    {
        let blk = ext2_block_slice_mut_state(state, block)?;
        let usable = blk.len() - tail;
        let mut pos = 0usize;
        while pos + 8 <= usable {
            let rec_len = read_u16_le(blk, pos + 4)? as usize;
            let name_len = blk[pos + 6] as usize;
            if rec_len < 8 || (rec_len & 0x3) != 0 || pos + rec_len > usable {
                return Err(EIO);
            }
            if 8 + name_len <= rec_len {
                let name = &blk[pos + 8..pos + 8 + name_len];
                if name == b".." {
                    write_u32_le(blk, pos, parent_ino)?;
                    patched = true;
                    break;
                }
            }
            pos += rec_len;
        }
    }
    if patched {
        ext2_dir_block_finish(state, &inode, block)
    } else {
        Err(EIO)
    }
}

fn ext2_create_child_on_disk(
    state: &mut VfsState,
    journal_handle: Option<Ext2JournalHandle>,
    parent_ino: u32,
    name: &[u8],
    file_type: FileType,
) -> Result<(u32, u32), i64> {
    let is_dir = file_type == FileType::Directory;
    let ino = ext2_alloc_inode(state, journal_handle, is_dir)?;
    let meta = state.ext2_meta;
    let now = fs_time_tick(state);

    let mut inode = Ext2Inode {
        mode: match file_type {
            FileType::Regular => EXT2_DEF_FILE_MODE,
            FileType::Directory => EXT2_DEF_DIR_MODE,
            FileType::Symlink => EXT2_DEF_SYMLINK_MODE,
            FileType::None => return Err(EINVAL),
        },
        uid: 0,
        size: 0,
        atime: now,
        ctime: now,
        mtime: now,
        links_count: if is_dir { 2 } else { 1 },
        gid: 0,
        block_count_512: 0,
        blocks: [0; 15],
        flags: 0,
        file_acl: 0,
        size_high: 0,
        ino,
        generation: 0,
        blocks_hi: 0,
        time_extra: [0; 3],
        time_orig: [0; 3],
    };

    if let Err(err) = ext2_init_new_inode_layout(state, &mut inode) {
        let _ = ext2_free_inode(state, journal_handle, ino, is_dir);
        return Err(err);
    }

    if is_dir {
        // Through the block map so extent-mapped directories work too.
        let block = match ext2_ensure_data_block(state, journal_handle, &mut inode, 0) {
            Ok(v) => v,
            Err(err) => {
                let _ = ext2_free_inode(state, journal_handle, ino, true);
                return Err(err);
            }
        };
        inode.size = meta.block_size as u32;
        ext2_update_block_count(state, &mut inode)?;

        let tail = ext2_dir_tail_len(state);
        {
            let blk = ext2_block_slice_mut_state(state, block)?;
            blk.fill(0);
            ext2_write_dirent(blk, 0, ino, 12, FileType::Directory, b".")?;
            ext2_write_dirent(
                blk,
                12,
                parent_ino,
                meta.block_size - 12 - tail,
                FileType::Directory,
                b"..",
            )?;
        }
        ext2_dir_block_finish(state, &inode, block)?;
    }

    if let Err(err) = ext2_write_inode_to_state(state, ino, &inode) {
        let _ = bmap_free_all_blocks(state, journal_handle, &mut inode, false);
        let _ = ext2_free_inode(state, journal_handle, ino, is_dir);
        return Err(err);
    }

    if let Err(err) = ext2_add_dir_entry(state, journal_handle, parent_ino, ino, name, file_type) {
        let _ = bmap_free_all_blocks(state, journal_handle, &mut inode, false);
        let _ = ext2_free_inode(state, journal_handle, ino, is_dir);
        return Err(err);
    }

    if is_dir {
        let mut parent = ext2_read_inode_from_state(state, parent_ino)?;
        parent.links_count = ext2_dir_links_inc(&meta, parent.links_count);
        ext2_write_inode_to_state(state, parent_ino, &parent)?;
    }

    Ok((ino, inode.size))
}

fn ext2_mode_to_filetype(mode: u16) -> Option<FileType> {
    match mode & EXT2_S_IFMT {
        EXT2_S_IFDIR => Some(FileType::Directory),
        EXT2_S_IFREG => Some(FileType::Regular),
        EXT2_S_IFLNK => Some(FileType::Symlink),
        _ => None,
    }
}

fn filetype_to_dtype(file_type: FileType) -> u8 {
    match file_type {
        FileType::Directory => DT_DIR,
        FileType::Regular => DT_REG,
        FileType::Symlink => DT_LNK,
        FileType::None => DT_UNKNOWN,
    }
}

fn inode_mode_for_stat(node: &Inode) -> u32 {
    let mut mode = node.mode as u32;
    if (mode & STAT_S_IFMT) == 0 {
        mode |= match node.file_type {
            FileType::Directory => STAT_S_IFDIR,
            FileType::Regular => STAT_S_IFREG,
            FileType::Symlink => STAT_S_IFLNK,
            FileType::None => 0,
        };
    }
    if (mode & (STAT_S_IRUSR | STAT_S_IWUSR | STAT_S_IXUSR)) == 0 {
        mode |= match node.file_type {
            FileType::Directory => {
                STAT_S_IRUSR
                    | STAT_S_IWUSR
                    | STAT_S_IXUSR
                    | STAT_S_IRGRP
                    | STAT_S_IXGRP
                    | STAT_S_IROTH
                    | STAT_S_IXOTH
            }
            FileType::Regular => STAT_S_IRUSR | STAT_S_IWUSR | STAT_S_IRGRP | STAT_S_IROTH,
            FileType::Symlink => {
                STAT_S_IRUSR
                    | STAT_S_IWUSR
                    | STAT_S_IXUSR
                    | STAT_S_IRGRP
                    | STAT_S_IWGRP
                    | STAT_S_IXGRP
                    | STAT_S_IROTH
                    | STAT_S_IWOTH
                    | STAT_S_IXOTH
            }
            FileType::None => 0,
        };
    }
    mode
}

fn ensure_dir_path_tmpfs(state: &mut VfsState, path: &str) -> Result<u16, i64> {
    if path.is_empty() || !path.starts_with('/') {
        return Err(EINVAL);
    }
    if path == "/" {
        return Ok(ROOT_INODE);
    }

    let bytes = path.as_bytes();
    let mut cursor = 1usize;
    let mut current = ROOT_INODE;
    let mut name = [0u8; VFS_MAX_NAME];

    while let Some(len) = next_component(bytes, &mut cursor, &mut name)? {
        if state.inodes[current as usize].file_type != FileType::Directory {
            return Err(ENOTDIR);
        }

        if let Some(child) = find_child(state, current, &name[..len]) {
            if state.inodes[child as usize].file_type != FileType::Directory {
                return Err(ENOTDIR);
            }
            current = child;
            continue;
        }

        if state.inodes[current as usize].backend != NodeBackend::Tmpfs {
            return Err(EROFS);
        }

        let child = alloc_inode(
            state,
            FileType::Directory,
            NodeBackend::Tmpfs,
            current,
            &name[..len],
        )?;
        add_child(state, current, child)?;
        current = child;
    }

    Ok(current)
}

fn ext2_import_node(
    state: &mut VfsState,
    parent: u16,
    name: &[u8],
    inode_num: u32,
    check_dup: bool,
) -> Result<(), i64> {
    if name.is_empty() {
        return Ok(());
    }
    if name == b"." || name == b".." {
        return Ok(());
    }
    if name.len() >= VFS_MAX_NAME {
        return Err(ENAMETOOLONG);
    }
    if check_dup && find_child(state, parent, name).is_some() {
        return Ok(());
    }

    let ext2_inode = ext2_read_inode_from_state(state, inode_num)?;
    let file_type = match ext2_mode_to_filetype(ext2_inode.mode) {
        Some(ft) => ft,
        None => return Ok(()),
    };

    let node = alloc_inode(state, file_type, NodeBackend::Ext2, parent, name)?;
    add_child(state, parent, node)?;
    {
        let node_ref = &mut state.inodes[node as usize];
        node_ref.ext2_ino = inode_num;
        node_ref.size = ext2_inode_size64(&ext2_inode);
        node_ref.mode = ext2_inode.mode;
        node_ref.uid = ext2_inode.uid;
        node_ref.gid = ext2_inode.gid;
        node_ref.links_count = ext2_inode.links_count;
        node_ref.atime = ext2_inode.atime as u64;
        node_ref.mtime = ext2_inode.mtime as u64;
        node_ref.ctime = ext2_inode.ctime as u64;
        node_ref.ext2_children_loaded = file_type != FileType::Directory;
    }

    Ok(())
}

fn ext2_ensure_dir_children_loaded(state: &mut VfsState, dir: u16) -> Result<(), i64> {
    if !state.ext2_mounted || (dir as usize) >= state.inodes.len() {
        return Ok(());
    }

    let (node_used, node_file_type, node_backend, node_ext2_ino, children_loaded) = {
        let node = &state.inodes[dir as usize];
        (
            node.used,
            node.file_type,
            node.backend,
            node.ext2_ino,
            node.ext2_children_loaded,
        )
    };
    if !node_used || node_file_type != FileType::Directory || node_backend != NodeBackend::Ext2 {
        return Ok(());
    }
    if node_ext2_ino == 0 {
        return Err(EIO);
    }
    if children_loaded {
        return Ok(());
    }

    let ext2_inode = ext2_read_inode_from_state(state, node_ext2_ino)?;
    if ext2_mode_to_filetype(ext2_inode.mode) != Some(FileType::Directory) {
        return Err(EIO);
    }
    // (inode, name offset, name length) into one name buffer.
    let mut entries: Vec<(u32, usize, usize)> = Vec::new();
    let mut names: Vec<u8> = Vec::new();
    ext2_for_each_dir_entry_state(state, &ext2_inode, |child_ino, _ft, child_name| {
        if child_name == b"." || child_name == b".." {
            return Ok(());
        }
        if child_name.len() >= VFS_MAX_NAME {
            return Err(ENAMETOOLONG);
        }
        entries.try_reserve(1).map_err(|_| ENOSPC)?;
        names.try_reserve(child_name.len()).map_err(|_| ENOSPC)?;
        entries.push((child_ino, names.len(), child_name.len()));
        names.extend_from_slice(child_name);
        Ok(())
    })?;
    // Names imported earlier by single lookups (or tmpfs overlays at the
    // root) are already children; skip the duplicate check otherwise.
    let check_dup = !state.inodes[dir as usize].children.is_empty();
    if !check_dup {
        state.inodes[dir as usize]
            .children
            .try_reserve(entries.len())
            .map_err(|_| ENOSPC)?;
    }
    for &(child_ino, off, len) in entries.iter() {
        ext2_import_node(state, dir, &names[off..off + len], child_ino, check_dup)?;
    }

    let node_ref = &mut state.inodes[dir as usize];
    node_ref.size = ext2_inode_size64(&ext2_inode);
    node_ref.mode = ext2_inode.mode;
    node_ref.uid = ext2_inode.uid;
    node_ref.gid = ext2_inode.gid;
    node_ref.links_count = ext2_inode.links_count;
    node_ref.atime = ext2_inode.atime as u64;
    node_ref.mtime = ext2_inode.mtime as u64;
    node_ref.ctime = ext2_inode.ctime as u64;
    node_ref.ext2_children_loaded = true;
    Ok(())
}

fn clear_inode_subtree(state: &mut VfsState, inode: u16) {
    let children = core::mem::take(&mut state.inodes[inode as usize].children);
    for &child in children.iter() {
        if (child as usize) < state.inodes.len() && state.inodes[child as usize].used {
            clear_inode_subtree(state, child);
        }
    }

    clear_inode(&mut state.inodes[inode as usize]);
}

fn prune_root_overlays_after_ext2_mount(state: &mut VfsState) {
    let children = state.inodes[ROOT_INODE as usize].children.clone();
    let child_count = children.len();

    let mut idx = 0usize;
    while idx < child_count {
        let child = children[idx];
        if (child as usize) >= state.inodes.len() {
            idx += 1;
            continue;
        }
        let keep = {
            let node = &state.inodes[child as usize];
            if !node.used {
                false
            } else {
                let name_len = node.name_len as usize;
                node.file_type == FileType::Directory
                    && is_root_pseudo_mount_name(&node.name[..name_len])
            }
        };
        if !state.inodes[child as usize].used {
            idx += 1;
            continue;
        }
        if keep {
            idx += 1;
            continue;
        }
        let _ = remove_child(state, ROOT_INODE, child);
        mount_table_remove_by_inode(state, child);
        clear_inode_subtree(state, child);
        idx += 1;
    }
}

fn rollback_ext2_mount_import(state: &mut VfsState, mount_inode: u16) {
    let children = core::mem::take(&mut state.inodes[mount_inode as usize].children);
    for &child in children.iter() {
        if (child as usize) < state.inodes.len() && state.inodes[child as usize].used {
            clear_inode_subtree(state, child);
        }
    }

    let mount = &mut state.inodes[mount_inode as usize];
    mount.backend = NodeBackend::Tmpfs;
    mount.ext2_ino = 0;
    mount.ext2_children_loaded = true;
    mount.children = Vec::new();
    mount.size = 0;
    mount.links_count = 2;

    mount_table_remove_by_inode(state, mount_inode);
    state.ext2_mounted = false;
    state.ext2_mount_inode = ROOT_INODE;
    state.ext2_meta = Ext2Meta::empty();
    state.ext2_journal = Ext2JournalState::empty();
    state.ext2_journal_running_tx = Ext2JournalRunningTx::empty();
    state.ext2_journal_pending_txs.clear();
    state.ext2_image_len = 0;
    state.ext2_image.clear();
    state.ext2_backend = ext2_backend::Ext2Backend::none();
    ext2_clear_dirty_tracking(state);
}

fn inode_number(node: &Inode, inode_idx: u16) -> u64 {
    if node.backend == NodeBackend::Ext2 && node.ext2_ino != 0 {
        node.ext2_ino as u64
    } else {
        inode_idx as u64 + 1
    }
}

fn backend_mutable(state: &VfsState, backend: NodeBackend) -> bool {
    match backend {
        NodeBackend::Tmpfs => true,
        NodeBackend::Ext2 => !state.ext2_read_only,
        _ => false,
    }
}

/// Diagnostics callback: `what` names the failing structure, `id` is the
/// inode or block number.
pub type Ext2DiagFn = fn(what: &'static str, id: u64);

fn ext2_fs_name_for(state: &VfsState) -> &'static str {
    let meta = &state.ext2_meta;
    if (meta.feature_incompat & EXT4_FEATURE_INCOMPAT_ALL) != 0
        || (meta.feature_ro_compat & EXT4_FEATURE_RO_COMPAT_ALL) != 0
    {
        "ext4"
    } else if state.ext2_journal.present {
        "ext3"
    } else {
        "ext2"
    }
}

/// Mount-time ext4 checksum checks: superblock (`metadata_csum`) and every
/// group descriptor (`metadata_csum` crc32c or `gdt_csum` crc16). Enables
/// per-structure verification on the read path for `metadata_csum`.
fn ext2_verify_mount_checksums(state: &mut VfsState) -> Result<(), i64> {
    let meta = state.ext2_meta;
    let has_metadata_csum = (meta.feature_ro_compat & EXT4_FEATURE_RO_COMPAT_METADATA_CSUM) != 0;
    let has_gdt_csum = (meta.feature_ro_compat & EXT4_FEATURE_RO_COMPAT_GDT_CSUM) != 0;
    if !has_metadata_csum && !has_gdt_csum {
        return Ok(());
    }
    let mut sb = [0u8; 1024];
    ext2_read_range_from_state(state, EXT2_SUPERBLOCK_OFFSET, &mut sb)?;
    let csum = ext4_csum::Ext4Csum::from_superblock(&sb)?;
    if has_metadata_csum && !ext4_csum::superblock_csum_ok(&sb)? {
        ext2_report(state, "superblock checksum", 0);
        return Err(EIO);
    }
    let mut desc = [0u8; EXT4_MAX_DESC_SIZE];
    for group in 0..meta.group_count {
        let offset = ext2_group_desc_offset(&meta, group)?;
        ext2_read_range_from_state(state, offset, &mut desc[..meta.desc_size])?;
        if !csum.group_desc_ok(group, &desc[..meta.desc_size])? {
            ext2_report(state, "group descriptor checksum", group as u64);
            return Err(EIO);
        }
    }
    // Kept for both schemes: gdt_csum needs it to maintain descriptor
    // checksums on writes; per-structure crc32c checks key off
    // `metadata_csum` inside.
    state.ext4_csum = Some(csum);
    Ok(())
}

fn ext2_report(state: &VfsState, what: &'static str, id: u64) {
    if let Some(hook) = state.ext2_diag {
        hook(what, id);
    }
}

/// Verifies an ext4 inode checksum (`metadata_csum`); `EIO` on mismatch.
fn ext2_verify_inode_csum(state: &mut VfsState, ino: u32, inode_offset: usize) -> Result<(), i64> {
    let csum = match state.ext4_csum {
        Some(csum) if csum.inode_csums => csum,
        _ => return Ok(()),
    };
    let size = state.ext2_meta.inode_size;
    let mut raw = [0u8; EXT4_MAX_INODE_SIZE];
    if size > raw.len() {
        return Err(EIO);
    }
    ext2_read_range_from_state(state, inode_offset, &mut raw[..size])?;
    if csum.inode_ok(ino, &raw[..size])? {
        Ok(())
    } else {
        ext2_report(state, "inode checksum", ino as u64);
        Err(EIO)
    }
}

const EXT2_JOURNAL_CREDITS_CREATE: usize = 32;
const EXT2_JOURNAL_CREDITS_MKDIR: usize = 40;
const EXT2_JOURNAL_CREDITS_UNLINK: usize = 32;
const EXT2_JOURNAL_CREDITS_RENAME: usize = 48;
const EXT2_JOURNAL_CREDITS_LINK: usize = 24;
const EXT2_JOURNAL_CREDITS_SYMLINK: usize = 40;
const EXT2_JOURNAL_CREDITS_METADATA_UPDATE: usize = 8;
const EXT2_JOURNAL_CREDITS_VARIABLE_BASE: usize = 32;
const EXT2_JOURNAL_CREDITS_VARIABLE_SEGMENT_MAX: usize = 16;
const EXT2_JOURNAL_CREDITS_STEP_EXISTING_DATA: usize = 1;
const EXT2_JOURNAL_CREDITS_STEP_ALLOC_DIRECT: usize = 5;
const EXT2_JOURNAL_CREDITS_STEP_ALLOC_TABLE_ENTRY: usize = 6;
const EXT2_JOURNAL_CREDITS_STEP_ALLOC_TABLE_ROOT: usize = 4;
const EXT2_JOURNAL_CREDITS_STEP_FREE_DIRECT: usize = 4;
const EXT2_JOURNAL_CREDITS_STEP_FREE_TABLE_ENTRY: usize = 5;
const EXT2_JOURNAL_CREDITS_STEP_FREE_TABLE_BLOCK: usize = 4;
const EXT2_JOURNAL_CREDITS_STEP_INODE_CLOSEOUT: usize = 1;
const EXT2_JOURNAL_CREDITS_HELPER_ALLOC_BLOCK_BOOKKEEPING: usize = 3;
const EXT2_JOURNAL_CREDITS_HELPER_ALLOC_BLOCK_INIT: usize = 1;
const EXT2_JOURNAL_CREDITS_HELPER_FREE_BLOCK_BOOKKEEPING: usize = 3;
const EXT2_JOURNAL_CREDITS_HELPER_FREE_BLOCK_REVOKE: usize = 1;
const EXT2_JOURNAL_CREDITS_HELPER_ALLOC_INODE_BOOKKEEPING: usize = 3;
const EXT2_JOURNAL_CREDITS_HELPER_ALLOC_INODE_INIT: usize = 1;
const EXT2_JOURNAL_CREDITS_HELPER_FREE_INODE_BOOKKEEPING: usize = 3;
const EXT2_JOURNAL_CREDITS_HELPER_FREE_INODE_CLEAR: usize = 1;
const EXT2_JOURNAL_CREDITS_HELPER_INDIRECT_LINK: usize = 1;
const EXT2_JOURNAL_CREDITS_HELPER_INDIRECT_UNLINK: usize = 1;
const EXT2_JOURNAL_CREDITS_HELPER_XATTR_RELEASE: usize = 1;

struct Ext2JournalCreditPlan {
    initial_credits: usize,
    growth_chunk: usize,
}

fn ext2_plan_variable_journal_credits(total_estimate: usize) -> Ext2JournalCreditPlan {
    let segment = core::cmp::min(total_estimate, EXT2_JOURNAL_CREDITS_VARIABLE_SEGMENT_MAX);
    Ext2JournalCreditPlan {
        initial_credits: segment,
        growth_chunk: segment,
    }
}

fn ext2_logical_block_indirection_depth(meta: &Ext2Meta, block_idx: usize) -> Result<u8, i64> {
    if block_idx < EXT2_INODE_DIRECT_COUNT {
        return Ok(0);
    }
    let entries = ext2_indirect_entries_per_block(meta)?;
    let mut remaining = block_idx - EXT2_INODE_DIRECT_COUNT;
    if remaining < entries {
        return Ok(1);
    }
    remaining -= entries;
    let double_span = entries.checked_mul(entries).ok_or(EIO)?;
    if remaining < double_span {
        return Ok(2);
    }
    remaining -= double_span;
    let triple_span = double_span.checked_mul(entries).ok_or(EIO)?;
    if remaining < triple_span {
        return Ok(3);
    }
    Err(EIO)
}

fn ext2_estimate_data_write_step_credits(
    state: &mut VfsState,
    inode: &Ext2Inode,
    block_idx: usize,
) -> Result<usize, i64> {
    if ext2_data_block_for_logical_index_state(state, inode, block_idx)? != 0 {
        return Ok(EXT2_JOURNAL_CREDITS_STEP_EXISTING_DATA);
    }
    if ext2_bmap::block_map_kind(inode) == ext2_bmap::BlockMapKind::Extents {
        return ext2_bmap::extent_step_credits(state, inode);
    }
    let meta = state.ext2_meta;
    match ext2_logical_block_indirection_depth(&meta, block_idx)? {
        0 => Ok(EXT2_JOURNAL_CREDITS_STEP_ALLOC_DIRECT),
        1 => Ok(
            EXT2_JOURNAL_CREDITS_STEP_ALLOC_TABLE_ENTRY
                + if inode.blocks[12] == 0 {
                    EXT2_JOURNAL_CREDITS_STEP_ALLOC_TABLE_ROOT
                } else {
                    0
                },
        ),
        2 => {
            let entries = ext2_indirect_entries_per_block(&meta)?;
            let remaining = block_idx - EXT2_INODE_DIRECT_COUNT - entries;
            let l1_idx = remaining / entries;
            let mut credits = EXT2_JOURNAL_CREDITS_STEP_ALLOC_TABLE_ENTRY;
            if inode.blocks[13] == 0 {
                credits += EXT2_JOURNAL_CREDITS_STEP_ALLOC_TABLE_ROOT;
                credits += EXT2_JOURNAL_CREDITS_STEP_ALLOC_TABLE_ENTRY;
            } else if ext2_read_indirect_entry_from_state(state, inode.blocks[13], l1_idx)? == 0 {
                credits += EXT2_JOURNAL_CREDITS_STEP_ALLOC_TABLE_ENTRY;
            }
            Ok(credits)
        }
        3 => {
            let entries = ext2_indirect_entries_per_block(&meta)?;
            let double_span = entries.checked_mul(entries).ok_or(EIO)?;
            let remaining = block_idx - EXT2_INODE_DIRECT_COUNT - entries - double_span;
            let l1_idx = remaining / double_span;
            let l2_idx = (remaining % double_span) / entries;
            let mut credits = EXT2_JOURNAL_CREDITS_STEP_ALLOC_TABLE_ENTRY;
            if inode.blocks[14] == 0 {
                credits += EXT2_JOURNAL_CREDITS_STEP_ALLOC_TABLE_ROOT;
                credits += EXT2_JOURNAL_CREDITS_STEP_ALLOC_TABLE_ENTRY * 2;
            } else {
                let l1 = ext2_read_indirect_entry_from_state(state, inode.blocks[14], l1_idx)?;
                if l1 == 0 {
                    credits += EXT2_JOURNAL_CREDITS_STEP_ALLOC_TABLE_ENTRY * 2;
                } else if ext2_read_indirect_entry_from_state(state, l1, l2_idx)? == 0 {
                    credits += EXT2_JOURNAL_CREDITS_STEP_ALLOC_TABLE_ENTRY;
                }
            }
            Ok(credits)
        }
        _ => Err(EIO),
    }
}

fn ext2_estimate_detach_step_credits(
    state: &mut VfsState,
    inode: &Ext2Inode,
    block_idx: usize,
) -> Result<usize, i64> {
    if ext2_data_block_for_logical_index_state(state, inode, block_idx)? == 0 {
        return Ok(0);
    }
    if ext2_bmap::block_map_kind(inode) == ext2_bmap::BlockMapKind::Extents {
        return ext2_bmap::extent_step_credits(state, inode);
    }
    let meta = state.ext2_meta;
    match ext2_logical_block_indirection_depth(&meta, block_idx)? {
        0 => Ok(EXT2_JOURNAL_CREDITS_STEP_FREE_DIRECT),
        1 => Ok(
            EXT2_JOURNAL_CREDITS_STEP_FREE_TABLE_ENTRY + EXT2_JOURNAL_CREDITS_STEP_FREE_DIRECT,
        ),
        2 => Ok(
            EXT2_JOURNAL_CREDITS_STEP_FREE_TABLE_ENTRY * 2
                + EXT2_JOURNAL_CREDITS_STEP_FREE_DIRECT
                + EXT2_JOURNAL_CREDITS_STEP_FREE_TABLE_BLOCK,
        ),
        3 => Ok(
            EXT2_JOURNAL_CREDITS_STEP_FREE_TABLE_ENTRY * 3
                + EXT2_JOURNAL_CREDITS_STEP_FREE_DIRECT
                + EXT2_JOURNAL_CREDITS_STEP_FREE_TABLE_BLOCK * 2,
        ),
        _ => Err(EIO),
    }
}

fn ext2_count_blocks_in_byte_range(block_size: usize, start: usize, end: usize) -> Result<usize, i64> {
    if block_size == 0 {
        return Err(EIO);
    }
    if end <= start {
        return Ok(0);
    }
    let first = start / block_size;
    let last = (end - 1) / block_size;
    Ok(last.saturating_sub(first).saturating_add(1))
}

fn ext2_allocated_block_count_from_inode(meta: &Ext2Meta, inode: &Ext2Inode) -> Result<usize, i64> {
    let sectors_per_block = meta.block_size / 512;
    if sectors_per_block == 0 {
        return Err(EIO);
    }
    Ok((inode.block_count_512 as usize) / sectors_per_block)
}

fn ext2_estimate_write_credits(
    state: &mut VfsState,
    ino: u32,
    offset: u64,
    src_len: usize,
    append: bool,
) -> Result<usize, i64> {
    let meta = state.ext2_meta;
    let inode = ext2_read_inode_from_state(state, ino)?;
    let max_size = ext2_max_file_size(&meta, &inode)?;
    let old_size = ext2_inode_size64(&inode);
    let start = if append { old_size } else { offset };
    if start >= max_size {
        return Ok(EXT2_JOURNAL_CREDITS_VARIABLE_BASE);
    }
    let write_len = core::cmp::min(src_len as u64, max_size - start) as usize;
    let start = usize::try_from(start).map_err(|_| EFBIG)?;
    let end = start.checked_add(write_len).ok_or(EIO)?;
    // Growth is sparse: only the block holding the old end is touched.
    let gap_blocks: usize = if start as u64 > old_size { 1 } else { 0 };
    let write_blocks = ext2_count_blocks_in_byte_range(meta.block_size, start, end)?;
    gap_blocks
        .checked_add(write_blocks)
        .and_then(|blocks| blocks.checked_mul(4))
        .and_then(|credits| credits.checked_add(EXT2_JOURNAL_CREDITS_VARIABLE_BASE))
        .ok_or(ENOSPC)
}

fn ext2_estimate_truncate_to_zero_credits(state: &mut VfsState, ino: u32) -> Result<usize, i64> {
    let meta = state.ext2_meta;
    let inode = ext2_read_inode_from_state(state, ino)?;
    let allocated = ext2_allocated_block_count_from_inode(&meta, &inode)?;
    allocated
        .checked_mul(2)
        .and_then(|credits| credits.checked_add(EXT2_JOURNAL_CREDITS_VARIABLE_BASE))
        .ok_or(ENOSPC)
}

fn ext2_estimate_truncate_to_length_credits(
    state: &mut VfsState,
    ino: u32,
    length: u64,
) -> Result<usize, i64> {
    let meta = state.ext2_meta;
    let inode = ext2_read_inode_from_state(state, ino)?;
    let allocated = ext2_allocated_block_count_from_inode(&meta, &inode)?;
    // Only mapped blocks change (growth is sparse; shrinking frees what is
    // mapped), plus the partial block at either end.
    let changed_blocks = if length > ext2_inode_size64(&inode) { 1 } else { allocated + 1 };
    changed_blocks
        .checked_add(allocated)
        .and_then(|blocks| blocks.checked_mul(2))
        .and_then(|credits| credits.checked_add(EXT2_JOURNAL_CREDITS_VARIABLE_BASE))
        .ok_or(ENOSPC)
}

fn with_ext2_journal_scope_credits<R>(
    state: &mut VfsState,
    enabled: bool,
    credits: usize,
    f: impl FnOnce(&mut VfsState) -> Result<R, i64>,
) -> Result<R, i64> {
    with_ext2_journal_scope_credit_policy(state, enabled, credits, false, 0, f)
}

fn with_ext2_journal_scope_credit_policy<R>(
    state: &mut VfsState,
    enabled: bool,
    credits: usize,
    restartable: bool,
    credit_growth_chunk: usize,
    f: impl FnOnce(&mut VfsState) -> Result<R, i64>,
) -> Result<R, i64> {
    if !enabled {
        return f(state);
    }
    ext2_journal::begin_mutation_scope_with_credits_options(
        state,
        credits,
        restartable,
        credit_growth_chunk,
    )?;
    let result = f(state);
    let end_result = ext2_journal::end_mutation_scope(state);
    match (result, end_result) {
        (Err(err), _) => Err(err),
        (Ok(_), Err(err)) => Err(err),
        (Ok(value), Ok(())) => Ok(value),
    }
}

fn open_at_inner(
    state: &mut VfsState,
    start: u16,
    path: &str,
    flags: u32,
    _mode: u32,
) -> Result<OpenResult, i64> {
    let access = flags & O_ACCMODE;
    if access > O_RDWR {
        return Err(EINVAL);
    }

    let inode_idx = match lookup_path_from_inner(state, start, path) {
        Ok(inode) => {
            if (flags & O_CREAT) != 0 && (flags & O_EXCL) != 0 {
                return Err(EEXIST);
            }
            inode
        }
        Err(ENOENT) if (flags & O_CREAT) != 0 => create_regular_at_inner(state, start, path)?,
        Err(err) => return Err(err),
    };

    let inode_ref = &state.inodes[inode_idx as usize];
    let inode_file_type = inode_ref.file_type;
    let inode_backend = inode_ref.backend;
    let inode_ext2_ino = inode_ref.ext2_ino;
    if inode_file_type == FileType::Directory {
        if access != O_RDONLY {
            return Err(EISDIR);
        }
    } else if (flags & O_DIRECTORY) != 0 {
        return Err(ENOTDIR);
    }

    if (flags & O_TRUNC) != 0 && inode_file_type == FileType::Regular && access != O_RDONLY {
        if !backend_mutable(state, inode_backend) {
            match inode_backend {
                // Character-device style nodes tolerate O_TRUNC as a no-op.
                NodeBackend::DevNull
                | NodeBackend::DevZero
                | NodeBackend::DevConsole
                | NodeBackend::DevUrandom
                | NodeBackend::DevTty
                | NodeBackend::DevPtmx
                | NodeBackend::DevPtsSlave
                | NodeBackend::DevFramebuffer
                | NodeBackend::DevInputEvent => {}
                _ => return Err(EROFS),
            }
        }

        match inode_backend {
            NodeBackend::Tmpfs => {
                state.inodes[inode_idx as usize].size = 0;
                touch_inode_metadata(state, inode_idx, true, true, true);
            }
            NodeBackend::Ext2 => {
                let credits = if inode_ext2_ino == 0 {
                    return Err(EIO);
                } else {
                    ext2_estimate_truncate_to_zero_credits(state, inode_ext2_ino)?
                };
                let plan = ext2_plan_variable_journal_credits(credits);
                with_ext2_journal_scope_credit_policy(
                    state,
                    true,
                    plan.initial_credits,
                    true,
                    plan.growth_chunk,
                    |state| {
                    if inode_ext2_ino == 0 {
                        return Err(EIO);
                    }
                    ext2_truncate_to_zero(state, inode_ext2_ino)?;
                    state.inodes[inode_idx as usize].size = 0;
                    touch_inode_metadata(state, inode_idx, true, true, true);
                    Ok(())
                },
                )?;
            }
            _ => {}
        }
    }

    let pts_id = if inode_backend == NodeBackend::DevPtsSlave {
        state.inodes[inode_idx as usize].dev_id
    } else {
        0
    };

    Ok(OpenResult {
        inode: inode_idx,
        file_type: inode_file_type,
        is_console: inode_backend == NodeBackend::DevConsole,
        is_tty: inode_backend == NodeBackend::DevTty,
        is_ptmx: inode_backend == NodeBackend::DevPtmx,
        is_pts_slave: inode_backend == NodeBackend::DevPtsSlave,
        is_framebuffer: inode_backend == NodeBackend::DevFramebuffer,
        pts_id,
    })
}

fn dir_entry_at_offset(
    state: &mut VfsState,
    inode: u16,
    offset: u64,
) -> Result<Option<DirEntry>, i64> {
    if (inode as usize) >= state.inodes.len() {
        return Err(ENOENT);
    }
    let node = &state.inodes[inode as usize];
    if !node.used {
        return Err(ENOENT);
    }
    if node.file_type != FileType::Directory {
        return Err(ENOTDIR);
    }
    ext2_ensure_dir_children_loaded(state, inode)?;
    let node = &state.inodes[inode as usize];

    let mut entry = DirEntry::empty();
    match offset {
        0 => {
            entry.ino = inode_number(node, inode);
            entry.next_offset = 1;
            entry.d_type = DT_DIR;
            entry.name_len = 1;
            entry.name[0] = b'.';
            Ok(Some(entry))
        }
        1 => {
            let parent = node.parent;
            let parent_node = &state.inodes[parent as usize];
            entry.ino = inode_number(parent_node, parent);
            entry.next_offset = 2;
            entry.d_type = DT_DIR;
            entry.name_len = 2;
            entry.name[0] = b'.';
            entry.name[1] = b'.';
            Ok(Some(entry))
        }
        _ => {
            let idx = (offset - 2) as usize;
            if idx >= node.children.len() {
                return Ok(None);
            }
            let child = node.children[idx];
            let child_node = &state.inodes[child as usize];
            if !child_node.used {
                return Ok(None);
            }
            entry.ino = inode_number(child_node, child);
            entry.next_offset = offset + 1;
            entry.d_type = filetype_to_dtype(child_node.file_type);
            entry.name_len = child_node.name_len as usize;
            entry.name[..entry.name_len].copy_from_slice(&child_node.name[..entry.name_len]);
            Ok(Some(entry))
        }
    }
}

fn stat_inner(state: &mut VfsState, inode: u16) -> Result<FileStat, i64> {
    if (inode as usize) >= state.inodes.len() {
        return Err(ENOENT);
    }
    let node = &state.inodes[inode as usize];
    if !node.used {
        return Err(ENOENT);
    }
    let node_ino = inode_number(node, inode);
    let node_mode = inode_mode_for_stat(node);
    let node_uid = node.uid as u32;
    let node_gid = node.gid as u32;
    let node_nlink = core::cmp::max(1, node.links_count as u32);
    let node_size = node.size as u64;
    let node_atime = node.atime;
    let node_mtime = node.mtime;
    let node_ctime = node.ctime;
    let node_file_type = node.file_type;
    let node_backend = node.backend;
    let node_ext2_ino = node.ext2_ino;
    let node_unlinked = node.unlinked;

    let mut stat = FileStat {
        ino: node_ino,
        mode: node_mode,
        uid: node_uid,
        gid: node_gid,
        nlink: node_nlink,
        size: node_size,
        blksize: 4096,
        blocks: (node_size + 511) / 512,
        atime: node_atime,
        mtime: node_mtime,
        ctime: node_ctime,
        file_type: node_file_type,
    };

    if node_backend == NodeBackend::Ext2 {
        if node_ext2_ino == 0 {
            return Err(EIO);
        }
        let ext = ext2_read_inode_from_state(state, node_ext2_ino)?;
        stat.ino = node_ext2_ino as u64;
        stat.mode = ext.mode as u32;
        stat.uid = ext.uid as u32;
        stat.gid = ext.gid as u32;
        stat.nlink = core::cmp::max(1, ext.links_count as u32);
        stat.size = ext2_inode_size64(&ext);
        stat.blksize = state.ext2_meta.block_size as u32;
        stat.blocks = ext2_inode_blocks_512(&state.ext2_meta, &ext);
        stat.atime = ext.atime as u64;
        stat.mtime = ext.mtime as u64;
        stat.ctime = ext.ctime as u64;
    }
    if node_unlinked {
        // Removed but still open: no names left (Linux reports 0 links).
        stat.nlink = 0;
    }

    Ok(stat)
}

fn unlink_at_inner(
    state: &mut VfsState,
    start: u16,
    path: &str,
    remove_dir: bool,
) -> Result<(), i64> {
    let mut name = [0u8; VFS_MAX_NAME];
    let (parent, name_len) = resolve_parent_from_inner(state, start, path, &mut name)?;
    let child = find_child_loaded(state, parent, &name[..name_len])?.ok_or(ENOENT)?;
    if inode_is_mount_root(state, child) {
        return Err(EBUSY);
    }

    let parent_backend = state.inodes[parent as usize].backend;
    if !backend_mutable(state, parent_backend) {
        return Err(EROFS);
    }

    let child_ref = &state.inodes[child as usize];
    let child_file_type = child_ref.file_type;
    let child_child_count = child_ref.children.len();
    let child_ext2_ino = child_ref.ext2_ino;
    if remove_dir {
        if child_file_type != FileType::Directory {
            return Err(ENOTDIR);
        }
        if child_child_count != 0 {
            return Err(ENOTEMPTY);
        }
        // ext2 children are imported lazily: the disk is authoritative.
        if parent_backend == NodeBackend::Ext2
            && child_ext2_ino != 0
            && !ext2_directory_is_empty(state, child_ext2_ino)?
        {
            return Err(ENOTEMPTY);
        }
    } else if child_file_type == FileType::Directory {
        return Err(EISDIR);
    }

    if parent_backend == NodeBackend::Ext2 {
        if state.inodes[parent as usize].ext2_ino == 0 || child_ext2_ino == 0 {
            return Err(EIO);
        }
        let journal_handle = ext2_journal::current_handle(state);
        let parent_ino = state.inodes[parent as usize].ext2_ino;
        let removed = ext2_remove_dir_entry(state, journal_handle, parent_ino, &name[..name_len])?;
        if removed != child_ext2_ino {
            return Err(EIO);
        }

        let mut child_ext = ext2_read_inode_from_state(state, child_ext2_ino)?;
        // Still open (through this name or an earlier-removed one): keep the
        // inode and its blocks, parked on the orphan list until the last
        // release (or the next mount after a crash) frees them.
        let busy = ext2_inode_busy(state, child_ext2_ino);
        if remove_dir {
            if !ext2_directory_is_empty(state, child_ext2_ino)? {
                return Err(ENOTEMPTY);
            }
            if busy {
                child_ext.links_count = 0;
                child_ext.ctime = fs_time_tick(state);
                ext2_write_inode_to_state(state, child_ext2_ino, &child_ext)?;
                ext2_orphan_add(state, child_ext2_ino)?;
            } else {
                ext2_free_inode_blocks(state, &mut child_ext)?;
                ext2_free_inode(state, journal_handle, child_ext2_ino, true)?;
            }
            let mut parent_ext = ext2_read_inode_from_state(state, parent_ino)?;
            parent_ext.links_count = ext2_dir_links_dec(&state.ext2_meta, parent_ext.links_count);
            parent_ext.mtime = fs_time_tick(state);
            parent_ext.ctime = parent_ext.mtime;
            ext2_write_inode_to_state(state, parent_ino, &parent_ext)?;
        } else {
            if child_ext.links_count > 1 {
                child_ext.links_count -= 1;
                let now = fs_time_tick(state);
                child_ext.ctime = now;
                child_ext.mtime = now;
                ext2_write_inode_to_state(state, child_ext2_ino, &child_ext)?;
                ext2_refresh_alias_nodes(state, child_ext2_ino, &child_ext);
            } else if busy {
                child_ext.links_count = 0;
                child_ext.ctime = fs_time_tick(state);
                ext2_write_inode_to_state(state, child_ext2_ino, &child_ext)?;
                ext2_orphan_add(state, child_ext2_ino)?;
            } else {
                ext2_free_inode_blocks(state, &mut child_ext)?;
                ext2_free_inode(state, journal_handle, child_ext2_ino, false)?;
            }
        }
    }

    remove_child(state, parent, child)?;
    if state.inodes[child as usize].open_count > 0 {
        state.inodes[child as usize].unlinked = true;
    } else {
        clear_inode(&mut state.inodes[child as usize]);
    }
    touch_inode_metadata(state, parent, true, true, true);
    Ok(())
}

/// Some live VFS node for ext2 inode `ino` is still referenced.
fn ext2_inode_busy(state: &VfsState, ino: u32) -> bool {
    state.inodes.iter().any(|n| {
        n.used && n.backend == NodeBackend::Ext2 && n.ext2_ino == ino && n.open_count > 0
    })
}

const EXT2_SB_LAST_ORPHAN: usize = EXT2_SUPERBLOCK_OFFSET + 0xE8;
/// `i_dtime` links orphan inodes into a list headed by `s_last_orphan`.
const EXT2_INODE_DTIME: usize = 20;

fn ext2_inode_dtime(state: &mut VfsState, ino: u32) -> Result<u32, i64> {
    let meta = state.ext2_meta;
    let off = ext2_inode_offset_in_state(state, &meta, ino)?;
    ext2_ensure_range_resident(state, off, meta.inode_size)?;
    ext2_read_u32_state(state, off + EXT2_INODE_DTIME)
}

fn ext2_set_inode_dtime(state: &mut VfsState, ino: u32, value: u32) -> Result<(), i64> {
    let meta = state.ext2_meta;
    let off = ext2_inode_offset_in_state(state, &meta, ino)?;
    ext2_ensure_range_resident(state, off, meta.inode_size)?;
    ext2_write_u32_state(state, off + EXT2_INODE_DTIME, value)?;
    ext2_update_inode_csum(state, ino)
}

/// Pushes `ino` onto the on-disk orphan list (`ext4_orphan_add`, classic
/// `s_last_orphan` list), so e2fsck or the next mount finishes the deletion
/// after a crash.
fn ext2_orphan_add(state: &mut VfsState, ino: u32) -> Result<(), i64> {
    let head = ext2_read_u32_state(state, EXT2_SB_LAST_ORPHAN)?;
    ext2_set_inode_dtime(state, ino, head)?;
    ext2_write_u32_state(state, EXT2_SB_LAST_ORPHAN, ino)
}

/// Unlinks `ino` from the orphan list (`ext4_orphan_del`).
fn ext2_orphan_del(state: &mut VfsState, ino: u32) -> Result<(), i64> {
    let next = ext2_inode_dtime(state, ino)?;
    let head = ext2_read_u32_state(state, EXT2_SB_LAST_ORPHAN)?;
    if head == ino {
        ext2_write_u32_state(state, EXT2_SB_LAST_ORPHAN, next)?;
    } else {
        let mut cur = head;
        let mut guard = state.ext2_meta.total_inodes;
        while cur != 0 && guard > 0 {
            if cur > state.ext2_meta.total_inodes {
                return Err(EIO);
            }
            let after = ext2_inode_dtime(state, cur)?;
            if after == ino {
                ext2_set_inode_dtime(state, cur, next)?;
                break;
            }
            cur = after;
            guard -= 1;
        }
    }
    ext2_set_inode_dtime(state, ino, 0)
}

/// Frees an orphaned inode whose last name is gone (links 0) or trims the
/// blocks past `i_size` of one parked for a truncate (links > 0), as Linux's
/// `ext4_orphan_cleanup` does. Leaves it off the list.
fn ext2_orphan_finish(state: &mut VfsState, ino: u32) -> Result<(), i64> {
    let mut ext = ext2_read_inode_from_state(state, ino)?;
    let is_dir = ext2_mode_to_filetype(ext.mode) == Some(FileType::Directory);
    let credits = ext2_estimate_truncate_to_zero_credits(state, ino)?;
    let plan = ext2_plan_variable_journal_credits(credits);
    with_ext2_journal_scope_credit_policy(
        state,
        true,
        plan.initial_credits,
        true,
        plan.growth_chunk,
        |state| {
            ext2_orphan_del(state, ino)?;
            if ext.links_count == 0 {
                let handle = ext2_journal::current_handle(state);
                ext2_free_inode_blocks(state, &mut ext)?;
                // i_dtime stays 0 (ext2_orphan_del cleared it): without a
                // wall clock a tick count would look like an orphan-list
                // link to e2fsck.
                ext2_free_inode(state, handle, ino, is_dir)
            } else if (ext.mode & EXT2_S_IFMT) == EXT2_S_IFREG {
                ext2_trim_blocks_past_size(state, ino)
            } else {
                Ok(())
            }
        },
    )
}

/// Frees every mapped block past `i_size` (the end of an interrupted
/// truncate, whose size was already lowered).
fn ext2_trim_blocks_past_size(state: &mut VfsState, ino: u32) -> Result<(), i64> {
    let meta = state.ext2_meta;
    let mut inode = ext2_read_inode_from_state(state, ino)?;
    let size = usize::try_from(ext2_inode_size64(&inode)).map_err(|_| EFBIG)?;
    let keep = ext2_logical_blocks_for_size(&meta, size)?;
    let handle = ext2_journal::current_handle(state);
    let mut idx = keep;
    while let Some(next) = ext2_bmap::ext2_next_mapped_block(state, &inode, idx)? {
        let step_credits = ext2_estimate_detach_step_credits(state, &inode, next)?;
        ext2_journal::with_handle_restartable_phase(
            state,
            handle,
            Ext2JournalPhaseKind::IndirectUnlink,
            step_credits,
            |state| {
                let removed = ext2_detach_data_block(state, handle, &mut inode, next)?;
                if removed != 0 {
                    ext2_free_block(state, handle, removed)?;
                }
                Ok(())
            },
        )?;
        idx = next + 1;
    }
    ext2_update_block_count(state, &mut inode)?;
    ext2_write_inode_to_state(state, ino, &inode)?;
    ext2_refresh_alias_nodes(state, ino, &inode);
    Ok(())
}

const EXT4_SB_ORPHAN_FILE_INUM: usize = EXT2_SUPERBLOCK_OFFSET + 0x280;
const EXT4_ORPHAN_BLOCK_MAGIC: u32 = 0x0b10_ca04;

/// Checksum of one orphan file block (`ext4_orphan_file_block_csum`):
/// crc32c over the block's physical number (le64) and the entry array,
/// seeded by the orphan file inode.
fn ext4_orphan_block_csum(seed: u32, phys: u64, block: &[u8]) -> u32 {
    let csum = ext4_csum::crc32c(seed, &phys.to_le_bytes());
    ext4_csum::crc32c(csum, &block[..block.len() - 8])
}

/// `orphan_file`: checks every orphan block's magic and checksum (a bad
/// block fails the mount with `EIO`, as on Linux). With `process` (read-write
/// mounts) and `orphan_present` set, finishes each recorded orphan, clears
/// its entry, and clears `orphan_present`. Returns the orphans processed.
fn ext2_check_orphan_file(state: &mut VfsState, process: bool) -> Result<u32, i64> {
    if (state.ext2_meta.feature_compat & EXT4_FEATURE_COMPAT_ORPHAN_FILE) == 0 {
        return Ok(0);
    }
    let ino = ext2_read_u32_state(state, EXT4_SB_ORPHAN_FILE_INUM)?;
    if ino == 0 || ino > state.ext2_meta.total_inodes {
        ext2_report(state, "orphan file inode", ino as u64);
        return Err(EIO);
    }
    let file = ext2_read_inode_from_state(state, ino)?;
    let bs = state.ext2_meta.block_size;
    let blocks = (ext2_inode_size64(&file) / bs as u64) as usize;
    let seed = state
        .ext4_csum
        .filter(|c| c.metadata_csum)
        .map(|c| c.inode_seed_for(ino, file.generation));
    let present =
        (state.ext2_meta.feature_ro_compat & EXT4_FEATURE_RO_COMPAT_ORPHAN_PRESENT) != 0;
    let mut processed = 0u32;
    for lblk in 0..blocks {
        let phys = ext2_data_block_for_logical_index_state(state, &file, lblk)?;
        if phys == 0 {
            ext2_report(state, "orphan file block", lblk as u64);
            return Err(EIO);
        }
        let (magic_ok, csum_ok, entries) = {
            let blk = ext2_block_slice_state(state, phys)?;
            let magic = read_u32_le(blk, bs - 8)?;
            let stored = read_u32_le(blk, bs - 4)?;
            let csum_ok = seed.map_or(true, |s| stored == ext4_orphan_block_csum(s, phys as u64, blk));
            let mut entries = Vec::new();
            if present && process {
                for slot in 0..(bs - 8) / 4 {
                    let entry = read_u32_le(blk, slot * 4)?;
                    if entry != 0 {
                        entries.try_reserve(1).map_err(|_| ENOSPC)?;
                        entries.push((slot, entry));
                    }
                }
            }
            (magic == EXT4_ORPHAN_BLOCK_MAGIC, csum_ok, entries)
        };
        if !magic_ok || !csum_ok {
            ext2_report(state, "orphan file block checksum", phys as u64);
            return Err(EIO);
        }
        for (slot, orphan) in entries {
            if orphan > state.ext2_meta.total_inodes {
                ext2_report(state, "orphan file entry", orphan as u64);
                continue;
            }
            ext2_orphan_finish(state, orphan)?;
            with_ext2_journal_scope_credits(state, true, EXT2_JOURNAL_CREDITS_METADATA_UPDATE, |state| {
                {
                    let blk = ext2_block_slice_mut_state(state, phys)?;
                    write_u32_le(blk, slot * 4, 0)?;
                    if let Some(s) = seed {
                        let value = ext4_orphan_block_csum(s, phys as u64, blk);
                        write_u32_le(blk, bs - 4, value)?;
                    }
                }
                ext2_mark_dirty_block(state, phys)
            })?;
            processed += 1;
        }
    }
    if present && process {
        with_ext2_journal_scope_credits(state, true, EXT2_JOURNAL_CREDITS_METADATA_UPDATE, |state| {
            state.ext2_meta.feature_ro_compat &= !EXT4_FEATURE_RO_COMPAT_ORPHAN_PRESENT;
            let value = state.ext2_meta.feature_ro_compat;
            ext2_write_u32_state(state, EXT2_SUPERBLOCK_OFFSET + 100, value)
        })?;
    }
    Ok(processed)
}

/// Mount-time orphan processing (read-write mounts, after journal
/// recovery): every inode on the `s_last_orphan` list is finished. A
/// damaged list (an inode number out of range or a loop) stops processing
/// and is reported; e2fsck repairs it.
fn ext2_process_orphans(state: &mut VfsState) -> Result<u32, i64> {
    let mut processed = 0u32;
    let mut ino = ext2_read_u32_state(state, EXT2_SB_LAST_ORPHAN)?;
    let total = state.ext2_meta.total_inodes;
    while ino != 0 {
        if ino > total || processed > total {
            ext2_report(state, "orphan list", ino as u64);
            break;
        }
        ext2_orphan_finish(state, ino)?;
        processed += 1;
        ino = ext2_read_u32_state(state, EXT2_SB_LAST_ORPHAN)?;
    }
    Ok(processed)
}

/// Drops a reference; the last one on a removed node frees it and returns
/// true (any backend: the kernel drops the node's cached pages).
fn inode_release_inner(state: &mut VfsState, inode: u16) -> Result<bool, i64> {
    if (inode as usize) >= state.inodes.len() || !state.inodes[inode as usize].used {
        return Ok(false);
    }
    let node = &mut state.inodes[inode as usize];
    if node.open_count == 0 {
        return Ok(false);
    }
    node.open_count -= 1;
    if node.open_count > 0 || !node.unlinked {
        return Ok(false);
    }
    let (backend, ext2_ino) = (node.backend, node.ext2_ino);
    clear_inode(&mut state.inodes[inode as usize]);
    if backend != NodeBackend::Ext2 || ext2_ino == 0 || !state.ext2_mounted || state.ext2_read_only
    {
        return Ok(true);
    }
    if ext2_inode_busy(state, ext2_ino) {
        return Ok(true);
    }
    let ext = ext2_read_inode_from_state(state, ext2_ino)?;
    if ext.links_count == 0 {
        ext2_orphan_finish(state, ext2_ino)?;
    }
    Ok(true)
}

fn is_descendant_of(state: &VfsState, ancestor: u16, node: u16) -> bool {
    let mut cur = node;
    loop {
        if cur == ancestor {
            return true;
        }
        if cur == ROOT_INODE {
            return false;
        }
        let parent = state.inodes[cur as usize].parent;
        if parent == cur {
            return false;
        }
        cur = parent;
    }
}

fn rename_at_inner(
    state: &mut VfsState,
    old_start: u16,
    old_path: &str,
    new_start: u16,
    new_path: &str,
) -> Result<(), i64> {
    let mut old_name = [0u8; VFS_MAX_NAME];
    let mut new_name = [0u8; VFS_MAX_NAME];
    let (old_parent, old_len) =
        resolve_parent_from_inner(state, old_start, old_path, &mut old_name)?;
    let (new_parent, new_len) =
        resolve_parent_from_inner(state, new_start, new_path, &mut new_name)?;
    let child = find_child_loaded(state, old_parent, &old_name[..old_len])?.ok_or(ENOENT)?;

    if old_parent == new_parent && old_len == new_len && old_name[..old_len] == new_name[..new_len]
    {
        return Ok(());
    }

    if inode_is_mount_root(state, child) {
        return Err(EBUSY);
    }
    if find_child_loaded(state, new_parent, &new_name[..new_len])?.is_some() {
        return Err(EEXIST);
    }

    let old_backend = state.inodes[old_parent as usize].backend;
    let new_backend = state.inodes[new_parent as usize].backend;
    if old_backend != new_backend {
        return Err(EINVAL);
    }
    if !backend_mutable(state, old_backend) {
        return Err(EROFS);
    }

    let child_ref = &state.inodes[child as usize];
    let child_file_type = child_ref.file_type;
    let child_ext2_ino = child_ref.ext2_ino;
    if child_file_type == FileType::Directory && is_descendant_of(state, child, new_parent) {
        return Err(EINVAL);
    }

    if old_backend == NodeBackend::Ext2 {
        let old_parent_ino = state.inodes[old_parent as usize].ext2_ino;
        let new_parent_ino = state.inodes[new_parent as usize].ext2_ino;
        if old_parent_ino == 0 || new_parent_ino == 0 || child_ext2_ino == 0 {
            return Err(EIO);
        }
        let journal_handle = ext2_journal::current_handle(state);

        let removed =
            ext2_remove_dir_entry(state, journal_handle, old_parent_ino, &old_name[..old_len])?;
        if removed != child_ext2_ino {
            return Err(EIO);
        }
        if let Err(err) = ext2_add_dir_entry(
            state,
            journal_handle,
            new_parent_ino,
            child_ext2_ino,
            &new_name[..new_len],
            child_file_type,
        ) {
            let _ = ext2_add_dir_entry(
                state,
                journal_handle,
                old_parent_ino,
                child_ext2_ino,
                &old_name[..old_len],
                child_file_type,
            );
            return Err(err);
        }

        if child_file_type == FileType::Directory && old_parent_ino != new_parent_ino {
            ext2_patch_dotdot(state, journal_handle, child_ext2_ino, new_parent_ino)?;
            let mut old_parent_inode = ext2_read_inode_from_state(state, old_parent_ino)?;
            old_parent_inode.links_count = ext2_dir_links_dec(&state.ext2_meta, old_parent_inode.links_count);
            ext2_write_inode_to_state(state, old_parent_ino, &old_parent_inode)?;

            let mut new_parent_inode = ext2_read_inode_from_state(state, new_parent_ino)?;
            new_parent_inode.links_count = ext2_dir_links_inc(&state.ext2_meta, new_parent_inode.links_count);
            ext2_write_inode_to_state(state, new_parent_ino, &new_parent_inode)?;
        }

        let mut ext_child = ext2_read_inode_from_state(state, child_ext2_ino)?;
        let now = fs_time_tick(state);
        ext_child.ctime = now;
        ext_child.mtime = now;
        ext2_write_inode_to_state(state, child_ext2_ino, &ext_child)?;
    }

    remove_child(state, old_parent, child)?;
    add_child(state, new_parent, child)?;
    let node = &mut state.inodes[child as usize];
    node.parent = new_parent;
    node.name_len = new_len as u8;
    node.name.fill(0);
    node.name[..new_len].copy_from_slice(&new_name[..new_len]);
    touch_inode_metadata(state, old_parent, true, true, true);
    touch_inode_metadata(state, new_parent, true, true, true);
    touch_inode_metadata(state, child, false, false, true);
    Ok(())
}

fn link_at_inner(
    state: &mut VfsState,
    old_start: u16,
    old_path: &str,
    new_start: u16,
    new_path: &str,
) -> Result<(), i64> {
    let old_inode = lookup_path_from_inner(state, old_start, old_path)?;
    if inode_is_mount_root(state, old_inode) {
        return Err(EBUSY);
    }

    let mut new_name = [0u8; VFS_MAX_NAME];
    let (new_parent, new_len) =
        resolve_parent_from_inner(state, new_start, new_path, &mut new_name)?;
    if find_child_loaded(state, new_parent, &new_name[..new_len])?.is_some() {
        return Err(EEXIST);
    }

    let old_node = &state.inodes[old_inode as usize];
    if !old_node.used {
        return Err(ENOENT);
    }
    if old_node.file_type == FileType::Directory {
        return Err(EPERM);
    }

    let old_file_type = old_node.file_type;
    let old_backend = old_node.backend;
    let old_size = old_node.size as usize;
    let old_mode = old_node.mode;
    let old_uid = old_node.uid;
    let old_gid = old_node.gid;
    let old_atime = old_node.atime;
    let old_mtime = old_node.mtime;
    let old_ext2_ino = old_node.ext2_ino;

    let new_backend = state.inodes[new_parent as usize].backend;
    if old_backend != new_backend {
        return Err(EINVAL);
    }
    if !backend_mutable(state, old_backend) {
        return Err(EROFS);
    }
    if !has_free_vfs_inode(state) {
        return Err(ENOSPC);
    }

    match old_backend {
        NodeBackend::Tmpfs => {
            let new_inode =
                alloc_inode(state, old_file_type, NodeBackend::Tmpfs, new_parent, &new_name[..new_len])?;
            {
                let now = fs_time_tick(state);
                let old_links = state.inodes[old_inode as usize]
                    .links_count
                    .saturating_add(1);
                let (old, link) = two_inodes_mut(state, old_inode, new_inode).ok_or(EIO)?;
                old.links_count = old_links;
                old.ctime = now;

                link.size = old_size as u64;
                link.data = Vec::new();
                link.data.try_reserve_exact(old_size).map_err(|_| ENOSPC)?;
                link.data.extend_from_slice(&old.data[..old_size]);
                link.mode = old_mode;
                link.uid = old_uid;
                link.gid = old_gid;
                link.links_count = old_links;
                link.atime = old_atime;
                link.mtime = old_mtime;
                link.ctime = now;
            }
            add_child(state, new_parent, new_inode)?;
            touch_inode_metadata(state, new_parent, true, true, true);
            Ok(())
        }
        NodeBackend::Ext2 => {
            let new_parent_ext2_ino = state.inodes[new_parent as usize].ext2_ino;
            if old_ext2_ino == 0 || new_parent_ext2_ino == 0 {
                return Err(EIO);
            }
            let journal_handle = ext2_journal::current_handle(state);

            ext2_add_dir_entry(
                state,
                journal_handle,
                new_parent_ext2_ino,
                old_ext2_ino,
                &new_name[..new_len],
                old_file_type,
            )?;

            let mut ext_inode = ext2_read_inode_from_state(state, old_ext2_ino)?;
            ext_inode.links_count = ext_inode.links_count.saturating_add(1);
            ext_inode.ctime = fs_time_tick(state);
            ext2_write_inode_to_state(state, old_ext2_ino, &ext_inode)?;

            let new_inode = match alloc_inode(
                state,
                old_file_type,
                NodeBackend::Ext2,
                new_parent,
                &new_name[..new_len],
            ) {
                Ok(inode) => inode,
                Err(err) => {
                    let _ = ext2_remove_dir_entry(
                        state,
                        journal_handle,
                        new_parent_ext2_ino,
                        &new_name[..new_len],
                    );
                    if ext_inode.links_count > 0 {
                        ext_inode.links_count -= 1;
                    }
                    let _ = ext2_write_inode_to_state(state, old_ext2_ino, &ext_inode);
                    return Err(err);
                }
            };

            {
                let link = &mut state.inodes[new_inode as usize];
                link.ext2_ino = old_ext2_ino;
            }
            if let Err(err) = add_child(state, new_parent, new_inode) {
                clear_inode(&mut state.inodes[new_inode as usize]);
                let _ = ext2_remove_dir_entry(
                    state,
                    journal_handle,
                    new_parent_ext2_ino,
                    &new_name[..new_len],
                );
                if ext_inode.links_count > 0 {
                    ext_inode.links_count -= 1;
                }
                let _ = ext2_write_inode_to_state(state, old_ext2_ino, &ext_inode);
                return Err(err);
            }

            ext2_refresh_alias_nodes(state, old_ext2_ino, &ext_inode);
            touch_inode_metadata(state, new_parent, true, true, true);
            Ok(())
        }
        _ => Err(EROFS),
    }
}

fn symlink_at_inner(
    state: &mut VfsState,
    start: u16,
    target: &str,
    link_path: &str,
) -> Result<(), i64> {
    if link_path.is_empty() {
        return Err(ENOENT);
    }

    let target_bytes = target.as_bytes();
    let mut name = [0u8; VFS_MAX_NAME];
    let (parent, name_len) = resolve_parent_from_inner(state, start, link_path, &mut name)?;
    if find_child_loaded(state, parent, &name[..name_len])?.is_some() {
        return Err(EEXIST);
    }

    let parent_backend = state.inodes[parent as usize].backend;
    if !backend_mutable(state, parent_backend) {
        return Err(EROFS);
    }
    if !has_free_vfs_inode(state) {
        return Err(ENOSPC);
    }

    match parent_backend {
        NodeBackend::Tmpfs => {
            if target_bytes.len() > TMPFS_MAX_FILE_SIZE {
                return Err(ENOSPC);
            }
            let inode = alloc_inode(
                state,
                FileType::Symlink,
                NodeBackend::Tmpfs,
                parent,
                &name[..name_len],
            )?;
            {
                let node = &mut state.inodes[inode as usize];
                node.size = target_bytes.len() as u64;
                node.data.try_reserve_exact(target_bytes.len()).map_err(|_| ENOSPC)?;
                node.data.extend_from_slice(target_bytes);
            }
            add_child(state, parent, inode)?;
            touch_inode_metadata(state, parent, true, true, true);
            Ok(())
        }
        NodeBackend::Ext2 => {
            let parent_ino = state.inodes[parent as usize].ext2_ino;
            if parent_ino == 0 {
                return Err(EIO);
            }
            let journal_handle = ext2_journal::current_handle(state);

            let (ext2_ino, _) = ext2_create_child_on_disk(
                state,
                journal_handle,
                parent_ino,
                &name[..name_len],
                FileType::Symlink,
            )?;

            if !target_bytes.is_empty() && target_bytes.len() < EXT2_FAST_SYMLINK_MAX {
                // Like Linux ext2/ext3, short targets live inline in i_block
                // (a "fast" symlink); e2fsck rejects short slow symlinks.
                if let Err(err) = ext2_write_fast_symlink_target(state, ext2_ino, target_bytes) {
                    let _ =
                        ext2_remove_dir_entry(state, journal_handle, parent_ino, &name[..name_len]);
                    let _ = ext2_free_inode(state, journal_handle, ext2_ino, false);
                    return Err(err);
                }
            } else if !target_bytes.is_empty() {
                let wrote = match ext2_write_file(state, ext2_ino, 0, target_bytes, false) {
                    Ok(w) => w,
                    Err(err) => {
                        let _ =
                            ext2_remove_dir_entry(state, journal_handle, parent_ino, &name[..name_len]);
                        let mut child = ext2_read_inode_from_state(state, ext2_ino)?;
                        let _ = ext2_free_inode_blocks(state, &mut child);
                        let _ = ext2_free_inode(state, journal_handle, ext2_ino, false);
                        return Err(err);
                    }
                };
                if wrote.written != target_bytes.len() {
                    let _ =
                        ext2_remove_dir_entry(state, journal_handle, parent_ino, &name[..name_len]);
                    let mut child = ext2_read_inode_from_state(state, ext2_ino)?;
                    let _ = ext2_free_inode_blocks(state, &mut child);
                    let _ = ext2_free_inode(state, journal_handle, ext2_ino, false);
                    return Err(ENOSPC);
                }
            }

            let inode = match alloc_inode(
                state,
                FileType::Symlink,
                NodeBackend::Ext2,
                parent,
                &name[..name_len],
            ) {
                Ok(inode) => inode,
                Err(err) => {
                    let _ =
                        ext2_remove_dir_entry(state, journal_handle, parent_ino, &name[..name_len]);
                    if let Ok(mut child) = ext2_read_inode_from_state(state, ext2_ino) {
                        let _ = ext2_free_inode_blocks(state, &mut child);
                    }
                    let _ = ext2_free_inode(state, journal_handle, ext2_ino, false);
                    return Err(err);
                }
            };

            let child = ext2_read_inode_from_state(state, ext2_ino)?;
            {
                let node = &mut state.inodes[inode as usize];
                node.ext2_ino = ext2_ino;
                node.size = ext2_inode_size64(&child);
                node.mode = child.mode;
                node.uid = child.uid;
                node.gid = child.gid;
                node.links_count = child.links_count;
                node.atime = child.atime as u64;
                node.mtime = child.mtime as u64;
                node.ctime = child.ctime as u64;
            }
            if let Err(err) = add_child(state, parent, inode) {
                clear_inode(&mut state.inodes[inode as usize]);
                let _ =
                    ext2_remove_dir_entry(state, journal_handle, parent_ino, &name[..name_len]);
                if let Ok(mut child) = ext2_read_inode_from_state(state, ext2_ino) {
                    let _ = ext2_free_inode_blocks(state, &mut child);
                }
                let _ = ext2_free_inode(state, journal_handle, ext2_ino, false);
                return Err(err);
            }

            touch_inode_metadata(state, parent, true, true, true);
            Ok(())
        }
        _ => Err(EROFS),
    }
}

fn readlink_at_inner(
    state: &mut VfsState,
    start: u16,
    path: &str,
    dst: &mut [u8],
) -> Result<usize, i64> {
    if path.is_empty() {
        return Err(ENOENT);
    }
    let inode = lookup_path_from_inner(state, start, path)?;
    let node = &state.inodes[inode as usize];
    if !node.used {
        return Err(ENOENT);
    }
    if node.file_type != FileType::Symlink {
        return Err(EINVAL);
    }
    if dst.is_empty() {
        return Ok(0);
    }

    match node.backend {
        NodeBackend::Tmpfs => {
            let count = core::cmp::min(dst.len(), node.size as usize);
            dst[..count].copy_from_slice(&node.data[..count]);
            Ok(count)
        }
        NodeBackend::Ext2 => {
            if node.ext2_ino == 0 {
                return Err(EIO);
            }
            ext2_read_file(state, node.ext2_ino, 0, dst)
        }
        _ => Err(EINVAL),
    }
}

fn set_times_inner(state: &mut VfsState, inode: u16, atime: u64, mtime: u64) -> Result<(), i64> {
    if inode as usize >= state.inodes.len() {
        return Err(ENOENT);
    }
    if !state.inodes[inode as usize].used {
        return Err(ENOENT);
    }

    let backend = state.inodes[inode as usize].backend;
    if !backend_mutable(state, backend) {
        return Err(EROFS);
    }

    let ctime = fs_time_tick(state);
    match backend {
        NodeBackend::Tmpfs => {
            let node = &mut state.inodes[inode as usize];
            node.atime = atime;
            node.mtime = mtime;
            node.ctime = ctime;
            Ok(())
        }
        NodeBackend::Ext2 => {
            let ext2_ino = state.inodes[inode as usize].ext2_ino;
            if ext2_ino == 0 {
                return Err(EINVAL);
            }
            let mut ext_inode = ext2_read_inode_from_state(state, ext2_ino)?;
            ext_inode.atime = atime;
            ext_inode.mtime = mtime;
            ext_inode.ctime = ctime;
            ext2_write_inode_to_state(state, ext2_ino, &ext_inode)?;
            ext2_refresh_alias_nodes(state, ext2_ino, &ext_inode);
            Ok(())
        }
        _ => Err(EROFS),
    }
}

fn chmod_inner(state: &mut VfsState, inode: u16, mode: u16) -> Result<(), i64> {
    if inode as usize >= state.inodes.len() {
        return Err(ENOENT);
    }
    if !state.inodes[inode as usize].used {
        return Err(ENOENT);
    }

    let backend = state.inodes[inode as usize].backend;
    if !backend_mutable(state, backend) {
        return Err(EROFS);
    }

    let ctime = fs_time_tick(state);
    match backend {
        NodeBackend::Tmpfs => {
            let node = &mut state.inodes[inode as usize];
            let file_type_bits = node.mode & EXT2_S_IFMT;
            node.mode = file_type_bits | (mode & 0o7777);
            node.ctime = ctime;
            Ok(())
        }
        NodeBackend::Ext2 => {
            let ext2_ino = state.inodes[inode as usize].ext2_ino;
            if ext2_ino == 0 {
                return Err(EINVAL);
            }
            let mut ext_inode = ext2_read_inode_from_state(state, ext2_ino)?;
            let file_type_bits = ext_inode.mode & EXT2_S_IFMT;
            ext_inode.mode = file_type_bits | (mode & 0o7777);
            ext_inode.ctime = ctime;
            ext2_write_inode_to_state(state, ext2_ino, &ext_inode)?;
            ext2_refresh_alias_nodes(state, ext2_ino, &ext_inode);
            Ok(())
        }
        _ => Err(EROFS),
    }
}

fn chown_inner(state: &mut VfsState, inode: u16, uid: u32, gid: u32) -> Result<(), i64> {
    if inode as usize >= state.inodes.len() {
        return Err(ENOENT);
    }
    if !state.inodes[inode as usize].used {
        return Err(ENOENT);
    }

    let backend = state.inodes[inode as usize].backend;
    if !backend_mutable(state, backend) {
        return Err(EROFS);
    }

    let new_uid = if uid == 0xFFFF_FFFF {
        state.inodes[inode as usize].uid
    } else {
        uid as u16
    };
    let new_gid = if gid == 0xFFFF_FFFF {
        state.inodes[inode as usize].gid
    } else {
        gid as u16
    };

    let ctime = fs_time_tick(state);
    match backend {
        NodeBackend::Tmpfs => {
            let node = &mut state.inodes[inode as usize];
            node.uid = new_uid;
            node.gid = new_gid;
            node.ctime = ctime;
            Ok(())
        }
        NodeBackend::Ext2 => {
            let ext2_ino = state.inodes[inode as usize].ext2_ino;
            if ext2_ino == 0 {
                return Err(EINVAL);
            }
            let mut ext_inode = ext2_read_inode_from_state(state, ext2_ino)?;
            ext_inode.uid = new_uid;
            ext_inode.gid = new_gid;
            ext_inode.ctime = ctime;
            ext2_write_inode_to_state(state, ext2_ino, &ext_inode)?;
            ext2_refresh_alias_nodes(state, ext2_ino, &ext_inode);
            Ok(())
        }
        _ => Err(EROFS),
    }
}

pub fn root_inode() -> u16 {
    ROOT_INODE
}

pub fn init() {
    with_state(|state| {
        reset(state);
        ensure_init(state);
    });
}

pub fn set_proc_uptime_ticks(ticks: u64) {
    with_state(|state| {
        ensure_init(state);
        state.proc_uptime_ticks = ticks;
    });
}

pub fn set_proc_meminfo(total_bytes: u64, free_bytes: u64, used_bytes: u64) {
    with_state(|state| {
        ensure_init(state);
        state.proc_total_bytes = total_bytes;
        state.proc_free_bytes = free_bytes;
        state.proc_used_bytes = used_bytes;
    });
}

pub fn lookup(path: &str) -> Result<u16, i64> {
    with_state(|state| {
        ensure_init(state);
        lookup_path_inner(state, path)
    })
}

pub fn lookupat(start: u16, path: &str) -> Result<u16, i64> {
    with_state(|state| {
        ensure_init(state);
        lookup_path_from_inner(state, start, path)
    })
}

/// Absolute path of `inode`, built from the VFS parent links (no directory
/// scan, so it never imports large ext2 directories). `ENOENT` for a node
/// that no longer exists, `ENAMETOOLONG` when `out` is too small.
pub fn inode_path(inode: u16, out: &mut [u8]) -> Result<usize, i64> {
    with_state(|state| {
        ensure_init(state);
        if (inode as usize) >= state.inodes.len() || !state.inodes[inode as usize].used {
            return Err(ENOENT);
        }
        // Collect the chain leaf -> root, then emit it root -> leaf.
        let mut chain: Vec<u16> = Vec::new();
        let mut cur = inode;
        while cur != ROOT_INODE {
            if chain.len() > VFS_MAX_PATH {
                return Err(ENOENT);
            }
            chain.try_reserve(1).map_err(|_| ENOSPC)?;
            chain.push(cur);
            let node = &state.inodes[cur as usize];
            if !node.used || node.unlinked || (node.parent as usize) >= state.inodes.len() {
                return Err(ENOENT);
            }
            cur = node.parent;
        }
        if chain.is_empty() {
            if out.is_empty() {
                return Err(ENAMETOOLONG);
            }
            out[0] = b'/';
            return Ok(1);
        }
        let mut len = 0usize;
        for &node_idx in chain.iter().rev() {
            let node = &state.inodes[node_idx as usize];
            let name = &node.name[..node.name_len as usize];
            if len + 1 + name.len() > out.len() {
                return Err(ENAMETOOLONG);
            }
            out[len] = b'/';
            out[len + 1..len + 1 + name.len()].copy_from_slice(name);
            len += 1 + name.len();
        }
        Ok(len)
    })
}

/// Takes a reference on `inode`: an open file description (the kernel calls
/// this for every descriptor it creates or duplicates), a working directory,
/// or a file mapping. A referenced node survives unlink/rmdir until
/// `inode_open_release`.
pub fn inode_open_retain(inode: u16) {
    with_state(|state| {
        if let Some(node) = state.inodes.get_mut(inode as usize) {
            if node.used {
                node.open_count = node.open_count.saturating_add(1);
            }
        }
    })
}

/// Drops a reference taken by `inode_open_retain`. Releasing the last
/// reference to an unlinked node frees it (for ext2 its inode and blocks are
/// released and taken off the orphan list) and returns true: the caller then
/// drops cached pages for the inode and syncs ext2.
pub fn inode_open_release(inode: u16) -> bool {
    with_state(|state| inode_release_inner(state, inode).unwrap_or(false))
}

/// References recorded on `inode` (0 for unknown or free nodes).
pub fn inode_open_refs(inode: u16) -> u32 {
    with_state(|state| match state.inodes.get(inode as usize) {
        Some(node) if node.used => node.open_count,
        _ => 0,
    })
}

pub fn inode_is_dir(inode: u16) -> Result<bool, i64> {
    with_state(|state| {
        ensure_init(state);
        if (inode as usize) >= state.inodes.len() || !state.inodes[inode as usize].used {
            return Err(ENOENT);
        }
        Ok(state.inodes[inode as usize].file_type == FileType::Directory)
    })
}

pub fn mkdir(path: &str) -> Result<(), i64> {
    mkdirat(ROOT_INODE, path)
}

pub fn mkdirat(start: u16, path: &str) -> Result<(), i64> {
    with_state(|state| {
        ensure_init(state);
        mkdir_at_inner(state, start, path)
    })
}

pub fn open(path: &str, flags: u32, mode: u32) -> Result<OpenResult, i64> {
    openat(ROOT_INODE, path, flags, mode)
}

pub fn openat(start: u16, path: &str, flags: u32, mode: u32) -> Result<OpenResult, i64> {
    with_state(|state| {
        ensure_init(state);
        open_at_inner(state, start, path, flags, mode)
    })
}

pub fn stat(inode: u16) -> Result<FileStat, i64> {
    with_state(|state| {
        ensure_init(state);
        stat_inner(state, inode)
    })
}

pub fn readdir(inode: u16, offset: u64) -> Result<Option<DirEntry>, i64> {
    with_state(|state| {
        ensure_init(state);
        dir_entry_at_offset(state, inode, offset)
    })
}

pub fn unlink(path: &str) -> Result<(), i64> {
    unlinkat(ROOT_INODE, path)
}

pub fn unlinkat(start: u16, path: &str) -> Result<(), i64> {
    with_state(|state| {
        ensure_init(state);
        with_ext2_journal_scope_credits(state, true, EXT2_JOURNAL_CREDITS_UNLINK, |state| {
            unlink_at_inner(state, start, path, false)
        })
    })
}

pub fn rmdir(path: &str) -> Result<(), i64> {
    rmdirat(ROOT_INODE, path)
}

pub fn rmdirat(start: u16, path: &str) -> Result<(), i64> {
    with_state(|state| {
        ensure_init(state);
        with_ext2_journal_scope_credits(state, true, EXT2_JOURNAL_CREDITS_UNLINK, |state| {
            unlink_at_inner(state, start, path, true)
        })
    })
}

pub fn rename(old_path: &str, new_path: &str) -> Result<(), i64> {
    renameat(ROOT_INODE, old_path, ROOT_INODE, new_path)
}

pub fn renameat(old_start: u16, old_path: &str, new_start: u16, new_path: &str) -> Result<(), i64> {
    with_state(|state| {
        ensure_init(state);
        with_ext2_journal_scope_credits(state, true, EXT2_JOURNAL_CREDITS_RENAME, |state| {
            rename_at_inner(state, old_start, old_path, new_start, new_path)
        })
    })
}

pub fn linkat(old_start: u16, old_path: &str, new_start: u16, new_path: &str) -> Result<(), i64> {
    with_state(|state| {
        ensure_init(state);
        with_ext2_journal_scope_credits(state, true, EXT2_JOURNAL_CREDITS_LINK, |state| {
            link_at_inner(state, old_start, old_path, new_start, new_path)
        })
    })
}

pub fn symlinkat(start: u16, target: &str, link_path: &str) -> Result<(), i64> {
    with_state(|state| {
        ensure_init(state);
        with_ext2_journal_scope_credits(state, true, EXT2_JOURNAL_CREDITS_SYMLINK, |state| {
            symlink_at_inner(state, start, target, link_path)
        })
    })
}

pub fn readlinkat(start: u16, path: &str, dst: &mut [u8]) -> Result<usize, i64> {
    with_state(|state| {
        ensure_init(state);
        readlink_at_inner(state, start, path, dst)
    })
}

pub fn set_times(inode: u16, atime: u64, mtime: u64) -> Result<(), i64> {
    with_state(|state| {
        ensure_init(state);
        with_ext2_journal_scope_credits(
            state,
            true,
            EXT2_JOURNAL_CREDITS_METADATA_UPDATE,
            |state| set_times_inner(state, inode, atime, mtime),
        )
    })
}

pub fn chmod(inode: u16, mode: u16) -> Result<(), i64> {
    with_state(|state| {
        ensure_init(state);
        with_ext2_journal_scope_credits(
            state,
            true,
            EXT2_JOURNAL_CREDITS_METADATA_UPDATE,
            |state| chmod_inner(state, inode, mode),
        )
    })
}

pub fn chown(inode: u16, uid: u32, gid: u32) -> Result<(), i64> {
    with_state(|state| {
        ensure_init(state);
        with_ext2_journal_scope_credits(
            state,
            true,
            EXT2_JOURNAL_CREDITS_METADATA_UPDATE,
            |state| chown_inner(state, inode, uid, gid),
        )
    })
}

pub fn read(inode: u16, offset: u64, dst: &mut [u8]) -> Result<usize, i64> {
    with_state(|state| {
        ensure_init(state);

        if inode as usize >= state.inodes.len() {
            return Err(ENOENT);
        }

        let out = {
            let node = &state.inodes[inode as usize];
            if !node.used {
                return Err(ENOENT);
            }
            if node.file_type == FileType::Directory {
                return Err(EISDIR);
            }

            match node.backend {
                NodeBackend::Tmpfs => {
                    if dst.is_empty() {
                        return Ok(0);
                    }

                    let file_size = node.size as usize;
                    if offset as usize >= file_size {
                        return Ok(0);
                    }

                    let start = offset as usize;
                    let available = file_size - start;
                    let count = core::cmp::min(dst.len(), available);
                    dst[..count].copy_from_slice(&node.data[start..start + count]);
                    count
                }
                NodeBackend::Ext2 => {
                    if node.ext2_ino == 0 {
                        return Err(EIO);
                    }
                    ext2_read_file(state, node.ext2_ino, offset, dst)?
                }
                NodeBackend::DevNull => 0,
                NodeBackend::DevZero => {
                    dst.fill(0);
                    dst.len()
                }
                NodeBackend::DevConsole => 0,
                NodeBackend::DevUrandom => {
                    fill_xorshift_bytes(state, dst);
                    dst.len()
                }
                NodeBackend::ProcUptime
                | NodeBackend::ProcMeminfo
                | NodeBackend::ProcMounts
                | NodeBackend::ProcVersion => read_proc(node, state, offset, dst)?,
                NodeBackend::DevTty
                | NodeBackend::DevPtmx
                | NodeBackend::DevPtsSlave
                | NodeBackend::DevFramebuffer
                | NodeBackend::DevInputEvent => 0,
            }
        };
        touch_inode_metadata(state, inode, true, false, false);
        Ok(out)
    })
}

pub fn write(inode: u16, offset: u64, src: &[u8], append: bool) -> Result<WriteResult, i64> {
    with_state(|state| {
        ensure_init(state);

        if inode as usize >= state.inodes.len() {
            return Err(ENOENT);
        }

        let (used, file_type, backend, ext2_ino) = {
            let node = &state.inodes[inode as usize];
            (node.used, node.file_type, node.backend, node.ext2_ino)
        };
        if !used {
            return Err(ENOENT);
        }
        if file_type == FileType::Directory {
            return Err(EISDIR);
        }

        if backend == NodeBackend::Ext2 && state.ext2_read_only {
            return Err(EROFS);
        }
        let result = match backend {
            NodeBackend::Tmpfs => {
                let node_ref = &mut state.inodes[inode as usize];
                let mut start = if append {
                    node_ref.size as usize
                } else {
                    match usize::try_from(offset) {
                        Ok(v) => v,
                        Err(_) => return Err(EINVAL),
                    }
                };

                if start > TMPFS_MAX_FILE_SIZE {
                    return Err(ENOSPC);
                }

                if src.is_empty() {
                    return Ok(WriteResult {
                        written: 0,
                        new_offset: start as u64,
                    });
                }

                let remaining = TMPFS_MAX_FILE_SIZE - start;
                if remaining == 0 {
                    return Err(ENOSPC);
                }

                let count = core::cmp::min(src.len(), remaining);
                // `data` holds exactly `size` bytes; a gap reads as zeros.
                let end = start + count;
                if end > node_ref.data.len() {
                    let grow = end - node_ref.data.len();
                    node_ref.data.try_reserve(grow).map_err(|_| ENOSPC)?;
                    node_ref.data.resize(end, 0);
                }
                node_ref.data[start..end].copy_from_slice(&src[..count]);

                start += count;
                if start > node_ref.size as usize {
                    node_ref.size = start as u64;
                }

                WriteResult {
                    written: count,
                    new_offset: start as u64,
                }
            }
            NodeBackend::Ext2 => {
                if ext2_ino == 0 {
                    return Err(EIO);
                }
                let credits =
                    ext2_estimate_write_credits(state, ext2_ino, offset, src.len(), append)?;
                let plan = ext2_plan_variable_journal_credits(credits);
                let result = with_ext2_journal_scope_credit_policy(
                    state,
                    true,
                    plan.initial_credits,
                    true,
                    plan.growth_chunk,
                    |state| ext2_write_file(state, ext2_ino, offset, src, append),
                )?;
                state.inodes[inode as usize].size =
                    core::cmp::max(state.inodes[inode as usize].size, result.new_offset);
                result
            }
            NodeBackend::ProcUptime
            | NodeBackend::ProcMeminfo
            | NodeBackend::ProcMounts
            | NodeBackend::ProcVersion => return Err(EROFS),
            NodeBackend::DevNull
            | NodeBackend::DevZero
            | NodeBackend::DevConsole
            | NodeBackend::DevUrandom
            | NodeBackend::DevTty
            | NodeBackend::DevPtmx
            | NodeBackend::DevPtsSlave
            | NodeBackend::DevFramebuffer
            | NodeBackend::DevInputEvent => {
                let next = offset.saturating_add(src.len() as u64);
                WriteResult {
                    written: src.len(),
                    new_offset: next,
                }
            }
        };
        touch_inode_metadata(state, inode, false, true, true);
        Ok(result)
    })
}

pub fn seek(inode: u16, current: u64, offset: i64, whence: u32) -> Result<u64, i64> {
    with_state(|state| {
        ensure_init(state);

        if inode as usize >= state.inodes.len() {
            return Err(ENOENT);
        }

        let node = &state.inodes[inode as usize];
        if !node.used {
            return Err(ENOENT);
        }
        if node.file_type == FileType::Directory {
            return Err(EISDIR);
        }
        let node_backend = node.backend;
        let node_size = node.size as u64;
        let node_ext2_ino = node.ext2_ino;

        let end = match node_backend {
            NodeBackend::Tmpfs => node_size,
            NodeBackend::Ext2 => {
                if node_ext2_ino == 0 {
                    return Err(EIO);
                }
                ext2_inode_size64(&ext2_read_inode_from_state(state, node_ext2_ino)?)
            }
            NodeBackend::ProcUptime
            | NodeBackend::ProcMeminfo
            | NodeBackend::ProcMounts
            | NodeBackend::ProcVersion => {
                let mut buf = [0u8; PROCFS_BUFFER_MAX];
                format_proc_payload(state, node_backend, &mut buf) as u64
            }
            NodeBackend::DevNull
            | NodeBackend::DevZero
            | NodeBackend::DevConsole
            | NodeBackend::DevUrandom
            | NodeBackend::DevTty
            | NodeBackend::DevPtmx
            | NodeBackend::DevPtsSlave
            | NodeBackend::DevFramebuffer
            | NodeBackend::DevInputEvent => current,
        };

        let base = match whence {
            SEEK_SET => 0i128,
            SEEK_CUR => current as i128,
            SEEK_END => end as i128,
            _ => return Err(EINVAL),
        };

        let target = base + offset as i128;
        if target < 0 {
            return Err(EINVAL);
        }

        match node_backend {
            NodeBackend::Tmpfs => {
                if target > TMPFS_MAX_FILE_SIZE as i128 {
                    return Err(EINVAL);
                }
            }
            NodeBackend::Ext2 => {
                let max_size = ext2_fs_max_file_size(&state.ext2_meta)?;
                if target > max_size as i128 {
                    return Err(EINVAL);
                }
            }
            NodeBackend::ProcUptime
            | NodeBackend::ProcMeminfo
            | NodeBackend::ProcMounts
            | NodeBackend::ProcVersion => {
                if target > end as i128 {
                    return Err(EINVAL);
                }
            }
            NodeBackend::DevNull
            | NodeBackend::DevZero
            | NodeBackend::DevConsole
            | NodeBackend::DevUrandom
            | NodeBackend::DevTty
            | NodeBackend::DevPtmx
            | NodeBackend::DevPtsSlave
            | NodeBackend::DevFramebuffer
            | NodeBackend::DevInputEvent => {
                if target > u64::MAX as i128 {
                    return Err(EINVAL);
                }
            }
        }

        Ok(target as u64)
    })
}

pub fn file_size(inode: u16) -> Result<u64, i64> {
    with_state(|state| {
        ensure_init(state);

        if inode as usize >= state.inodes.len() {
            return Err(ENOENT);
        }

        let node = &state.inodes[inode as usize];
        if !node.used {
            return Err(ENOENT);
        }

        let size = match node.backend {
            NodeBackend::Tmpfs => node.size as u64,
            NodeBackend::Ext2 => {
                if node.ext2_ino == 0 {
                    return Err(EIO);
                }
                ext2_inode_size64(&ext2_read_inode_from_state(state, node.ext2_ino)?)
            }
            NodeBackend::ProcUptime
            | NodeBackend::ProcMeminfo
            | NodeBackend::ProcMounts
            | NodeBackend::ProcVersion => {
                let mut buf = [0u8; PROCFS_BUFFER_MAX];
                format_proc_payload(state, node.backend, &mut buf) as u64
            }
            NodeBackend::DevNull
            | NodeBackend::DevZero
            | NodeBackend::DevConsole
            | NodeBackend::DevUrandom
            | NodeBackend::DevTty
            | NodeBackend::DevPtmx
            | NodeBackend::DevPtsSlave
            | NodeBackend::DevFramebuffer
            | NodeBackend::DevInputEvent => 0,
        };

        Ok(size)
    })
}

pub fn truncate(inode: u16) -> Result<(), i64> {
    with_state(|state| {
        ensure_init(state);

        if inode as usize >= state.inodes.len() {
            return Err(ENOENT);
        }
        let (used, file_type, backend, ext2_ino) = {
            let node = &state.inodes[inode as usize];
            (node.used, node.file_type, node.backend, node.ext2_ino)
        };
        if !used {
            return Err(ENOENT);
        }
        if file_type != FileType::Regular {
            return Err(EINVAL);
        }
        if !backend_mutable(state, backend) {
            return Err(EROFS);
        }

        match backend {
            NodeBackend::Tmpfs => {
                state.inodes[inode as usize].size = 0;
                state.inodes[inode as usize].data = Vec::new();
                touch_inode_metadata(state, inode, false, true, true);
            }
            NodeBackend::Ext2 => {
                if ext2_ino == 0 {
                    return Err(EIO);
                }
                let credits = ext2_estimate_truncate_to_zero_credits(state, ext2_ino)?;
                let plan = ext2_plan_variable_journal_credits(credits);
                with_ext2_journal_scope_credit_policy(
                    state,
                    true,
                    plan.initial_credits,
                    true,
                    plan.growth_chunk,
                    |state| {
                    ext2_truncate_to_zero(state, ext2_ino)?;
                    state.inodes[inode as usize].size = 0;
                    touch_inode_metadata(state, inode, false, true, true);
                    Ok(())
                },
                )?;
            }
            _ => {}
        }
        Ok(())
    })
}

pub fn truncate_to(inode: u16, length: u64) -> Result<(), i64> {
    if length == 0 {
        return truncate(inode);
    }
    with_state(|state| {
        ensure_init(state);

        if inode as usize >= state.inodes.len() {
            return Err(ENOENT);
        }
        let (used, file_type, backend, ext2_ino) = {
            let node = &state.inodes[inode as usize];
            (node.used, node.file_type, node.backend, node.ext2_ino)
        };
        if !used {
            return Err(ENOENT);
        }
        if file_type != FileType::Regular {
            return Err(EINVAL);
        }
        if !backend_mutable(state, backend) {
            return Err(EROFS);
        }

        match backend {
            NodeBackend::Tmpfs => {
                let new_len = length as usize;
                if new_len > TMPFS_MAX_FILE_SIZE {
                    return Err(ENOSPC);
                }
                let node = &mut state.inodes[inode as usize];
                if new_len > node.data.len() {
                    let grow = new_len - node.data.len();
                    node.data.try_reserve(grow).map_err(|_| ENOSPC)?;
                }
                node.data.resize(new_len, 0);
                node.size = new_len as u64;
                touch_inode_metadata(state, inode, false, true, true);
            }
            NodeBackend::Ext2 => {
                if ext2_ino == 0 {
                    return Err(EIO);
                }
                let new_len = length;
                let credits = ext2_estimate_truncate_to_length_credits(state, ext2_ino, new_len)?;
                let plan = ext2_plan_variable_journal_credits(credits);
                with_ext2_journal_scope_credit_policy(
                    state,
                    true,
                    plan.initial_credits,
                    true,
                    plan.growth_chunk,
                    |state| {
                    ext2_truncate_to_length(state, ext2_ino, new_len)?;
                    state.inodes[inode as usize].size = new_len;
                    touch_inode_metadata(state, inode, false, true, true);
                    Ok(())
                },
                )?;
            }
            _ => {}
        }
        Ok(())
    })
}

pub fn inode_backend_is_ext2(inode: u16) -> bool {
    with_state(|state| {
        if inode as usize >= state.inodes.len() {
            return false;
        }
        state.inodes[inode as usize].backend == NodeBackend::Ext2
    })
}

pub fn inode_is_framebuffer(inode: u16) -> bool {
    with_state(|state| {
        ensure_init(state);
        if inode as usize >= state.inodes.len() {
            return false;
        }
        let node = &state.inodes[inode as usize];
        node.used && node.backend == NodeBackend::DevFramebuffer
    })
}

pub fn inode_input_device_id(inode: u16) -> Option<u16> {
    with_state(|state| {
        ensure_init(state);
        if inode as usize >= state.inodes.len() {
            return None;
        }
        let node = &state.inodes[inode as usize];
        if node.used && node.backend == NodeBackend::DevInputEvent {
            Some(node.dev_id)
        } else {
            None
        }
    })
}

pub fn ext2_block_size() -> u64 {
    with_state(|state| state.ext2_meta.block_size as u64)
}

pub fn ext2_total_blocks() -> u64 {
    with_state(|state| state.ext2_meta.total_blocks as u64)
}

pub fn ext2_total_inodes() -> u64 {
    with_state(|state| state.ext2_meta.total_inodes as u64)
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());
    static SAMPLE_EXT2: &[u8] = include_bytes!("../../../user/sample_ext2.img");

    fn dummy_ext2_backend_read(_ctx: u64, _block_index: u32, out: &mut [u8]) -> Result<(), i64> {
        out.fill(0);
        Ok(())
    }

    fn dummy_ext2_backend_write(_ctx: u64, _block_index: u32, _data: &[u8]) -> Result<(), i64> {
        Ok(())
    }

    struct TestBackendCtx {
        image: std::vec::Vec<u8>,
        reads: usize,
        writes: usize,
        flushes: usize,
        fail_write_after: Option<usize>,
    }

    fn test_ext2_backend_flush(ctx: u64) -> Result<(), i64> {
        let ctx = unsafe { &mut *(ctx as *mut TestBackendCtx) };
        ctx.flushes = ctx.flushes.saturating_add(1);
        Ok(())
    }

    fn test_ext2_backend_read(ctx: u64, block_index: u32, out: &mut [u8]) -> Result<(), i64> {
        if out.is_empty() {
            return Err(EIO);
        }
        let ctx = unsafe { &mut *(ctx as *mut TestBackendCtx) };
        let start = (block_index as usize).checked_mul(out.len()).ok_or(EIO)?;
        let end = start.checked_add(out.len()).ok_or(EIO)?;
        if end > ctx.image.len() {
            return Err(EIO);
        }
        out.copy_from_slice(&ctx.image[start..end]);
        ctx.reads = ctx.reads.saturating_add(1);
        Ok(())
    }

    fn test_ext2_backend_write(ctx: u64, block_index: u32, data: &[u8]) -> Result<(), i64> {
        if data.is_empty() {
            return Err(EIO);
        }
        let ctx = unsafe { &mut *(ctx as *mut TestBackendCtx) };
        if matches!(ctx.fail_write_after, Some(limit) if ctx.writes >= limit) {
            return Err(EIO);
        }
        let start = (block_index as usize).checked_mul(data.len()).ok_or(EIO)?;
        let end = start.checked_add(data.len()).ok_or(EIO)?;
        if end > ctx.image.len() {
            return Err(EIO);
        }
        ctx.image[start..end].copy_from_slice(data);
        ctx.writes = ctx.writes.saturating_add(1);
        Ok(())
    }

    fn mount_ext2_backend_image_at(
        mount_path: &str,
        image: &[u8],
        head_len: usize,
    ) -> std::boxed::Box<TestBackendCtx> {
        let mut ctx = std::boxed::Box::new(TestBackendCtx {
            image: std::vec::Vec::from(image),
            reads: 0,
            writes: 0,
            flushes: 0,
            fail_write_after: None,
        });
        let backend = Ext2BlockBackend {
            ctx: (&mut *ctx as *mut TestBackendCtx) as u64,
            read_block: test_ext2_backend_read,
            write_block: test_ext2_backend_write,
            flush: Some(test_ext2_backend_flush),
        };
        set_ext2_block_backend(backend).expect("set block backend");
        let head_len = core::cmp::min(head_len, image.len());
        mount_ext2_block_backend(mount_path, image.len(), &image[..head_len])
            .expect("mount ext2 backend");
        ctx
    }

    fn mount_ext2_backend_image_ro(image: &[u8]) -> std::boxed::Box<TestBackendCtx> {
        let mut ctx = std::boxed::Box::new(TestBackendCtx {
            image: std::vec::Vec::from(image),
            reads: 0,
            writes: 0,
            flushes: 0,
            fail_write_after: None,
        });
        let backend = Ext2BlockBackend {
            ctx: (&mut *ctx as *mut TestBackendCtx) as u64,
            read_block: test_ext2_backend_read,
            write_block: test_ext2_backend_write,
            flush: Some(test_ext2_backend_flush),
        };
        set_ext2_block_backend(backend).expect("set block backend");
        mount_ext2_block_backend_with_options("/mnt/ext2", image.len(), &image[..4096], true)
            .expect("mount ext2 read-only");
        ctx
    }

    fn mount_ext2_backend_image(image: &[u8], head_len: usize) -> std::boxed::Box<TestBackendCtx> {
        mount_ext2_backend_image_at("/mnt/ext2", image, head_len)
    }

    fn mount_ext2_backend_image_expect_err(image: &[u8], head_len: usize) -> i64 {
        let mut ctx = std::boxed::Box::new(TestBackendCtx {
            image: std::vec::Vec::from(image),
            reads: 0,
            writes: 0,
            flushes: 0,
            fail_write_after: None,
        });
        let backend = Ext2BlockBackend {
            ctx: (&mut *ctx as *mut TestBackendCtx) as u64,
            read_block: test_ext2_backend_read,
            write_block: test_ext2_backend_write,
            flush: Some(test_ext2_backend_flush),
        };
        set_ext2_block_backend(backend).expect("set block backend");
        let head_len = core::cmp::min(head_len, image.len());
        let err =
            mount_ext2_block_backend("/mnt/ext2", image.len(), &image[..head_len]).unwrap_err();
        clear_ext2_block_backend().expect("clear backend after failed mount");
        err
    }

    fn mount_sample_ext2() -> std::boxed::Box<TestBackendCtx> {
        mount_ext2_backend_image(SAMPLE_EXT2, 4096)
    }

    fn read_u32_be(buf: &[u8], offset: usize) -> u32 {
        u32::from_be_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ])
    }

    fn write_u32_be(buf: &mut [u8], offset: usize, value: u32) {
        let bytes = value.to_be_bytes();
        buf[offset] = bytes[0];
        buf[offset + 1] = bytes[1];
        buf[offset + 2] = bytes[2];
        buf[offset + 3] = bytes[3];
    }

    fn build_internal_journal_replay_image() -> (std::vec::Vec<u8>, u32, std::vec::Vec<u8>) {
        let mut image = std::vec::Vec::from(SAMPLE_EXT2);
        let meta = ext2_parse_meta_with_len(&image, image.len(), true).expect("parse sample ext2");
        let block_size = meta.block_size;

        let compat_off = EXT2_SUPERBLOCK_OFFSET + 92;
        let compat = read_u32_le(&image, compat_off).expect("read compat");
        write_u32_le(
            &mut image,
            compat_off,
            compat | EXT2_FEATURE_COMPAT_HAS_JOURNAL,
        )
        .expect("set has_journal");

        let incompat_off = EXT2_SUPERBLOCK_OFFSET + 96;
        let incompat = read_u32_le(&image, incompat_off).expect("read incompat");
        write_u32_le(
            &mut image,
            incompat_off,
            incompat | EXT2_FEATURE_INCOMPAT_RECOVER,
        )
        .expect("set recover");

        write_u32_le(&mut image, EXT2_SUPERBLOCK_OFFSET + 224, 8).expect("set journal inode");
        write_u32_le(&mut image, EXT2_SUPERBLOCK_OFFSET + 228, 0).expect("set journal dev");

        let journal_blocks = [40u32, 41, 42, 43, 44, 45, 46];
        let mut journal_inode = Ext2Inode {
            mode: EXT2_S_IFREG | 0o600,
            uid: 0,
            size: (journal_blocks.len() * block_size) as u32,
            atime: 0,
            ctime: 0,
            mtime: 0,
            links_count: 1,
            gid: 0,
            block_count_512: ((journal_blocks.len() * block_size) / 512) as u32,
            blocks: [0; 15],
            flags: 0,
            file_acl: 0,
            size_high: 0,
            ino: 0,
            generation: 0,
            blocks_hi: 0,
            time_extra: [0; 3],
            time_orig: [0; 3],
        };
        for (idx, block) in journal_blocks.iter().enumerate() {
            journal_inode.blocks[idx] = *block;
        }
        ext2_write_inode(&mut image, &meta, 8, &journal_inode).expect("write journal inode");

        let tx_sequence = 55u32;
        let target_block = 70u32;
        let payload = std::vec::Vec::from(&b"journal replay payload"[..]);

        let journal_super_off = (journal_blocks[0] as usize) * block_size;
        write_u32_be(&mut image, journal_super_off, 0xC03B_3998);
        write_u32_be(&mut image, journal_super_off + 4, 3);
        write_u32_be(&mut image, journal_super_off + 8, 0);
        write_u32_be(&mut image, journal_super_off + 12, block_size as u32);
        write_u32_be(
            &mut image,
            journal_super_off + 16,
            journal_blocks.len() as u32,
        );
        write_u32_be(&mut image, journal_super_off + 20, 1);
        write_u32_be(&mut image, journal_super_off + 24, tx_sequence);
        write_u32_be(&mut image, journal_super_off + 28, 1);

        let descriptor_off = (journal_blocks[1] as usize) * block_size;
        write_u32_be(&mut image, descriptor_off, 0xC03B_3998);
        write_u32_be(&mut image, descriptor_off + 4, 1);
        write_u32_be(&mut image, descriptor_off + 8, tx_sequence);
        write_u32_be(&mut image, descriptor_off + 12, target_block);
        write_u32_be(&mut image, descriptor_off + 16, 0x0000_0008);
        image[descriptor_off + 20..descriptor_off + 36].fill(0);

        let data_off = (journal_blocks[2] as usize) * block_size;
        image[data_off..data_off + block_size].fill(0);
        image[data_off..data_off + payload.len()].copy_from_slice(payload.as_slice());

        let commit_off = (journal_blocks[3] as usize) * block_size;
        write_u32_be(&mut image, commit_off, 0xC03B_3998);
        write_u32_be(&mut image, commit_off + 4, 2);
        write_u32_be(&mut image, commit_off + 8, tx_sequence);

        let target_off = (target_block as usize) * block_size;
        image[target_off..target_off + payload.len()].fill(b's');

        (image, target_block, payload)
    }

    fn build_internal_journal_wrapped_revoke_replay_image() -> (
        std::vec::Vec<u8>,
        u32,
        std::vec::Vec<u8>,
        u32,
        std::vec::Vec<u8>,
    ) {
        let mut image = std::vec::Vec::from(SAMPLE_EXT2);
        let meta = ext2_parse_meta_with_len(&image, image.len(), true).expect("parse sample ext2");
        let block_size = meta.block_size;

        let compat_off = EXT2_SUPERBLOCK_OFFSET + 92;
        let compat = read_u32_le(&image, compat_off).expect("read compat");
        write_u32_le(
            &mut image,
            compat_off,
            compat | EXT2_FEATURE_COMPAT_HAS_JOURNAL,
        )
        .expect("set has_journal");

        let incompat_off = EXT2_SUPERBLOCK_OFFSET + 96;
        let incompat = read_u32_le(&image, incompat_off).expect("read incompat");
        write_u32_le(
            &mut image,
            incompat_off,
            incompat | EXT2_FEATURE_INCOMPAT_RECOVER,
        )
        .expect("set recover");

        write_u32_le(&mut image, EXT2_SUPERBLOCK_OFFSET + 224, 8).expect("set journal inode");
        write_u32_le(&mut image, EXT2_SUPERBLOCK_OFFSET + 228, 0).expect("set journal dev");

        let journal_blocks = [40u32, 41, 42, 43, 44, 45, 46, 47, 48];
        let mut journal_inode = Ext2Inode {
            mode: EXT2_S_IFREG | 0o600,
            uid: 0,
            size: (journal_blocks.len() * block_size) as u32,
            atime: 0,
            ctime: 0,
            mtime: 0,
            links_count: 1,
            gid: 0,
            block_count_512: ((journal_blocks.len() * block_size) / 512) as u32,
            blocks: [0; 15],
            flags: 0,
            file_acl: 0,
            size_high: 0,
            ino: 0,
            generation: 0,
            blocks_hi: 0,
            time_extra: [0; 3],
            time_orig: [0; 3],
        };
        for (idx, block) in journal_blocks.iter().enumerate() {
            journal_inode.blocks[idx] = *block;
        }
        ext2_write_inode(&mut image, &meta, 8, &journal_inode).expect("write journal inode");

        let tx_sequence_first = u32::MAX;
        let tx_sequence_second = 0u32;
        let revoked_target_block = 70u32;
        let surviving_target_block = 71u32;
        let revoked_payload = std::vec::Vec::from(&b"revoked payload"[..]);
        let surviving_payload = std::vec::Vec::from(&b"surviving payload"[..]);

        let journal_super_off = (journal_blocks[0] as usize) * block_size;
        write_u32_be(&mut image, journal_super_off, 0xC03B_3998);
        write_u32_be(&mut image, journal_super_off + 4, 3);
        write_u32_be(&mut image, journal_super_off + 8, 0);
        write_u32_be(&mut image, journal_super_off + 12, block_size as u32);
        write_u32_be(
            &mut image,
            journal_super_off + 16,
            journal_blocks.len() as u32,
        );
        write_u32_be(&mut image, journal_super_off + 20, 1);
        write_u32_be(&mut image, journal_super_off + 24, tx_sequence_first);
        write_u32_be(&mut image, journal_super_off + 28, 1);

        let descriptor_first_off = (journal_blocks[1] as usize) * block_size;
        write_u32_be(&mut image, descriptor_first_off, 0xC03B_3998);
        write_u32_be(&mut image, descriptor_first_off + 4, 1);
        write_u32_be(&mut image, descriptor_first_off + 8, tx_sequence_first);
        write_u32_be(&mut image, descriptor_first_off + 12, revoked_target_block);
        write_u32_be(&mut image, descriptor_first_off + 16, 0x0000_0008);
        image[descriptor_first_off + 20..descriptor_first_off + 36].fill(0);

        let data_first_off = (journal_blocks[2] as usize) * block_size;
        image[data_first_off..data_first_off + block_size].fill(0);
        image[data_first_off..data_first_off + revoked_payload.len()]
            .copy_from_slice(revoked_payload.as_slice());

        let commit_first_off = (journal_blocks[3] as usize) * block_size;
        write_u32_be(&mut image, commit_first_off, 0xC03B_3998);
        write_u32_be(&mut image, commit_first_off + 4, 2);
        write_u32_be(&mut image, commit_first_off + 8, tx_sequence_first);

        let descriptor_second_off = (journal_blocks[4] as usize) * block_size;
        write_u32_be(&mut image, descriptor_second_off, 0xC03B_3998);
        write_u32_be(&mut image, descriptor_second_off + 4, 1);
        write_u32_be(&mut image, descriptor_second_off + 8, tx_sequence_second);
        write_u32_be(
            &mut image,
            descriptor_second_off + 12,
            surviving_target_block,
        );
        write_u32_be(&mut image, descriptor_second_off + 16, 0x0000_0008);
        image[descriptor_second_off + 20..descriptor_second_off + 36].fill(0);

        let data_second_off = (journal_blocks[5] as usize) * block_size;
        image[data_second_off..data_second_off + block_size].fill(0);
        image[data_second_off..data_second_off + surviving_payload.len()]
            .copy_from_slice(surviving_payload.as_slice());

        let revoke_off = (journal_blocks[6] as usize) * block_size;
        write_u32_be(&mut image, revoke_off, 0xC03B_3998);
        write_u32_be(&mut image, revoke_off + 4, 5);
        write_u32_be(&mut image, revoke_off + 8, tx_sequence_second);
        write_u32_be(&mut image, revoke_off + 12, 20);
        write_u32_be(&mut image, revoke_off + 16, revoked_target_block);

        let commit_second_off = (journal_blocks[7] as usize) * block_size;
        write_u32_be(&mut image, commit_second_off, 0xC03B_3998);
        write_u32_be(&mut image, commit_second_off + 4, 2);
        write_u32_be(&mut image, commit_second_off + 8, tx_sequence_second);

        let revoked_target_off = (revoked_target_block as usize) * block_size;
        image[revoked_target_off..revoked_target_off + revoked_payload.len()].fill(b's');
        let surviving_target_off = (surviving_target_block as usize) * block_size;
        image[surviving_target_off..surviving_target_off + surviving_payload.len()].fill(b'k');

        (
            image,
            revoked_target_block,
            revoked_payload,
            surviving_target_block,
            surviving_payload,
        )
    }

    #[test]
    fn open_create_write_read_roundtrip() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let opened = open("/tmp/notes.txt", O_CREAT | O_RDWR, 0).expect("open should work");
        assert_eq!(opened.file_type, FileType::Regular);

        let write = write(opened.inode, 0, b"hello", false).expect("write should work");
        assert_eq!(write.written, 5);

        let mut buf = [0u8; 8];
        let read_n = read(opened.inode, 0, &mut buf).expect("read should work");
        assert_eq!(read_n, 5);
        assert_eq!(&buf[..read_n], b"hello");
    }

    #[test]
    fn open_excl_fails_when_file_exists() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let _ = open("/tmp/excl.txt", O_CREAT | O_WRONLY, 0).expect("create should work");
        let err = open("/tmp/excl.txt", O_CREAT | O_EXCL | O_WRONLY, 0).unwrap_err();
        assert_eq!(err, EEXIST);
    }

    #[test]
    fn truncation_clears_contents() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let opened = open("/tmp/trunc.txt", O_CREAT | O_RDWR, 0).expect("open should work");
        let _ = write(opened.inode, 0, b"abcdef", false).expect("write should work");
        assert_eq!(file_size(opened.inode).unwrap(), 6);

        let _ = open("/tmp/trunc.txt", O_WRONLY | O_TRUNC, 0).expect("trunc open should work");
        assert_eq!(file_size(opened.inode).unwrap(), 0);
    }

    #[test]
    fn append_writes_at_end() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let opened = open("/tmp/append.txt", O_CREAT | O_RDWR, 0).expect("open should work");
        let _ = write(opened.inode, 0, b"foo", false).expect("initial write");
        let append = write(opened.inode, 0, b"bar", true).expect("append write");
        assert_eq!(append.new_offset, 6);

        let mut out = [0u8; 8];
        let n = read(opened.inode, 0, &mut out).expect("read should work");
        assert_eq!(&out[..n], b"foobar");
    }

    #[test]
    fn linkat_creates_second_visible_name() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let opened = open("/tmp/link-src.txt", O_CREAT | O_RDWR, 0).expect("create src");
        let _ = write(opened.inode, 0, b"linkdata", false).expect("write src");
        linkat(
            ROOT_INODE,
            "/tmp/link-src.txt",
            ROOT_INODE,
            "/tmp/link-dst.txt",
        )
        .expect("create link");

        let linked = open("/tmp/link-dst.txt", O_RDONLY, 0).expect("open linked name");
        let mut out = [0u8; 16];
        let n = read(linked.inode, 0, &mut out).expect("read linked");
        assert_eq!(&out[..n], b"linkdata");
    }

    #[test]
    fn set_times_updates_atime_and_mtime() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let opened = open("/tmp/times.txt", O_CREAT | O_RDWR, 0).expect("create");
        set_times(opened.inode, 123, 456).expect("set times");
        let stat = stat(opened.inode).expect("stat");
        assert_eq!(stat.atime, 123);
        assert_eq!(stat.mtime, 456);
    }

    #[test]
    fn symlinkat_and_readlinkat_roundtrip() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        symlinkat(ROOT_INODE, "/tmp/target.txt", "/tmp/link.txt").expect("create symlink");
        let mut out = [0u8; 64];
        let n = readlinkat(ROOT_INODE, "/tmp/link.txt", &mut out).expect("read symlink");
        assert_eq!(&out[..n], b"/tmp/target.txt");
    }

    #[test]
    fn mkdir_and_nested_create_work() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        mkdir("/tmp/work").expect("mkdir should work");
        let opened =
            open("/tmp/work/log.txt", O_CREAT | O_RDWR, 0).expect("nested create should work");
        let _ = write(opened.inode, 0, b"x", false).expect("write should work");

        let mut out = [0u8; 2];
        let n = read(opened.inode, 0, &mut out).expect("read should work");
        assert_eq!(&out[..n], b"x");
    }

    #[test]
    fn write_to_directory_is_rejected() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let dir = open("/tmp", O_RDONLY | O_DIRECTORY, 0).expect("directory open should work");
        let err = write(dir.inode, 0, b"bad", false).unwrap_err();
        assert_eq!(err, EISDIR);
    }

    #[test]
    fn unlinked_open_file_stays_usable_until_last_release() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let opened = open("/tmp/orphan.txt", O_CREAT | O_RDWR, 0).expect("create");
        inode_open_retain(opened.inode);
        inode_open_retain(opened.inode); // a dup
        write(opened.inode, 0, b"still here", false).expect("write");

        unlink("/tmp/orphan.txt").expect("unlink while open");
        assert_eq!(lookup("/tmp/orphan.txt"), Err(ENOENT));
        // The name can be reused immediately for a different file.
        let other = open("/tmp/orphan.txt", O_CREAT | O_RDWR, 0).expect("recreate");
        assert_ne!(other.inode, opened.inode);

        let st = stat(opened.inode).expect("fstat orphan");
        assert_eq!(st.nlink, 0);
        assert_eq!(st.size, 10);
        let mut buf = [0u8; 10];
        assert_eq!(read(opened.inode, 0, &mut buf).unwrap(), 10);
        assert_eq!(&buf, b"still here");
        write(opened.inode, 10, b"!", false).expect("write orphan");

        assert!(!inode_open_release(opened.inode));
        assert!(stat(opened.inode).is_ok());
        assert!(inode_open_release(opened.inode), "last release frees the orphan");
        assert_eq!(stat(opened.inode).unwrap_err(), ENOENT);
        assert_eq!(inode_open_refs(opened.inode), 0);
    }

    #[test]
    fn unlink_without_open_refs_frees_immediately() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let opened = open("/tmp/closed.txt", O_CREAT | O_RDWR, 0).expect("create");
        unlink("/tmp/closed.txt").expect("unlink");
        assert_eq!(stat(opened.inode).unwrap_err(), ENOENT);
    }

    #[test]
    fn tmpfs_directory_holds_more_than_64_entries() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        mkdir("/tmp/many").expect("mkdir");
        for i in 0..100 {
            let path = std::format!("/tmp/many/f{}", i);
            open(&path, O_CREAT | O_RDWR, 0).expect("create entry");
        }
        assert!(lookup("/tmp/many/f99").is_ok());
    }

    #[test]
    fn devfs_has_empty_fd_directory() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let dir = open("/dev/fd", O_RDONLY | O_DIRECTORY, 0).expect("open /dev/fd");
        assert_eq!(inode_is_dir(dir.inode), Ok(true));
        assert_eq!(lookup("/dev/fd"), Ok(dir.inode));
        // Only "." and "..": the kernel supplies the per-process entries.
        assert!(readdir(dir.inode, 0).unwrap().is_some());
        assert!(readdir(dir.inode, 1).unwrap().is_some());
        assert!(readdir(dir.inode, 2).unwrap().is_none());
    }

    #[test]
    fn devfs_null_and_zero_behave() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let null_fd = open("/dev/null", O_RDWR, 0).expect("open /dev/null");
        let mut null_read = [1u8; 4];
        assert_eq!(read(null_fd.inode, 0, &mut null_read).unwrap(), 0);
        assert_eq!(write(null_fd.inode, 0, b"drop", false).unwrap().written, 4);

        let zero_fd = open("/dev/zero", O_RDONLY, 0).expect("open /dev/zero");
        let mut zero_read = [1u8; 4];
        assert_eq!(read(zero_fd.inode, 0, &mut zero_read).unwrap(), 4);
        assert_eq!(zero_read, [0u8; 4]);
    }

    #[test]
    fn procfs_files_are_dynamic_and_read_only() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        set_proc_uptime_ticks(1234);
        set_proc_meminfo(128 * 1024 * 1024, 96 * 1024 * 1024, 32 * 1024 * 1024);

        let uptime = open("/proc/uptime", O_RDONLY, 0).expect("open /proc/uptime");
        let mut uptime_buf = [0u8; 64];
        let uptime_n = read(uptime.inode, 0, &mut uptime_buf).expect("read uptime");
        let uptime_text = core::str::from_utf8(&uptime_buf[..uptime_n]).expect("utf8");
        assert!(uptime_text.contains("12.34"));

        let meminfo = open("/proc/meminfo", O_RDONLY, 0).expect("open /proc/meminfo");
        let mut mem_buf = [0u8; 160];
        let mem_n = read(meminfo.inode, 0, &mut mem_buf).expect("read meminfo");
        let mem_text = core::str::from_utf8(&mem_buf[..mem_n]).expect("utf8");
        assert!(mem_text.contains("MemTotal:"));
        assert!(mem_text.contains("MemFree:"));

        let mounts = open("/proc/mounts", O_RDONLY, 0).expect("open /proc/mounts");
        let mut mounts_buf = [0u8; 192];
        let mounts_n = read(mounts.inode, 0, &mut mounts_buf).expect("read mounts");
        let mounts_text = core::str::from_utf8(&mounts_buf[..mounts_n]).expect("utf8");
        assert!(!mounts_text.contains("ext2 /mnt/ext2"));

        assert_eq!(write(meminfo.inode, 0, b"bad", false).unwrap_err(), EROFS);
    }

    #[test]
    fn dev_console_reports_console_open() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let console = open("/dev/console", O_RDWR, 0).expect("open /dev/console");
        assert!(console.is_console);
    }

    #[test]
    fn dev_fb0_reports_framebuffer_open_and_mode() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let fb = open("/dev/fb0", O_RDWR, 0).expect("open /dev/fb0");
        assert!(fb.is_framebuffer);
        assert!(!fb.is_console);
        assert!(inode_is_framebuffer(fb.inode));

        let st = stat(fb.inode).expect("stat /dev/fb0");
        assert_eq!(st.mode & STAT_S_IFMT, STAT_S_IFREG);
        assert_eq!(st.mode & 0o666, 0o666);
    }

    #[test]
    fn dev_input_events_are_present_with_stable_ids() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let kbd = open("/dev/input/event0", O_RDONLY, 0).expect("open event0");
        let ptr = open("/dev/input/event1", O_RDONLY, 0).expect("open event1");

        assert_eq!(inode_input_device_id(kbd.inode), Some(0));
        assert_eq!(inode_input_device_id(ptr.inode), Some(1));

        let st = stat(kbd.inode).expect("stat event0");
        assert_eq!(st.mode & STAT_S_IFMT, STAT_S_IFREG);
        assert_eq!(st.mode & 0o444, 0o444);
    }

    #[test]
    fn ext2_mount_and_read_sample_file() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let _ctx = mount_sample_ext2();
        let file = open("/mnt/ext2/hello.txt", O_RDONLY, 0).expect("open ext2 file");
        let motd = open("/mnt/ext2/etc/motd", O_RDONLY, 0).expect("open nested ext2 file");

        let mut buf = [0u8; 96];
        let n = read(file.inode, 0, &mut buf).expect("read ext2 file");
        let text = core::str::from_utf8(&buf[..n]).expect("utf8");
        assert!(text.contains("Hello from ext2 sample image"));

        let mut motd_buf = [0u8; 96];
        let motd_n = read(motd.inode, 0, &mut motd_buf).expect("read ext2 motd");
        let motd_text = core::str::from_utf8(&motd_buf[..motd_n]).expect("utf8");
        assert!(motd_text.contains("PeetOS ext2 sample filesystem"));

        let mounts = open("/proc/mounts", O_RDONLY, 0).expect("open /proc/mounts");
        let mut mounts_buf = [0u8; 256];
        let mounts_n = read(mounts.inode, 0, &mut mounts_buf).expect("read mounts");
        let mounts_text = core::str::from_utf8(&mounts_buf[..mounts_n]).expect("utf8");
        assert!(mounts_text.contains("ext2 /mnt/ext2 ext2 rw"));

        let write_res = write(file.inode, 0, b"Hey", false).expect("ext2 write works");
        assert_eq!(write_res.written, 3);

        let mut updated = [0u8; 8];
        let updated_n = read(file.inode, 0, &mut updated).expect("read updated");
        assert_eq!(&updated[..updated_n], b"Heylo fr");

        let created = open("/mnt/ext2/new.txt", O_CREAT | O_RDWR, 0).expect("create ext2 file");
        let wrote_new = write(created.inode, 0, b"fresh", false).expect("write new ext2");
        assert_eq!(wrote_new.written, 5);

        let mut new_buf = [0u8; 8];
        let new_n = read(created.inode, 0, &mut new_buf).expect("read new ext2");
        assert_eq!(&new_buf[..new_n], b"fresh");

        mkdir("/mnt/ext2/newdir").expect("mkdir ext2 dir");
        let nested = open("/mnt/ext2/newdir/note.txt", O_CREAT | O_RDWR, 0).expect("create nested");
        let nested_w = write(nested.inode, 0, b"vfs", false).expect("write nested");
        assert_eq!(nested_w.written, 3);

        linkat(
            ROOT_INODE,
            "/mnt/ext2/hello.txt",
            ROOT_INODE,
            "/mnt/ext2/hello-link.txt",
        )
        .expect("ext2 link");
        let linked = open("/mnt/ext2/hello-link.txt", O_RDONLY, 0).expect("open ext2 link");
        let mut linked_buf = [0u8; 32];
        let linked_n = read(linked.inode, 0, &mut linked_buf).expect("read ext2 link");
        assert!(linked_n > 0);

        let original = open("/mnt/ext2/hello.txt", O_RDONLY, 0).expect("open original ext2");
        let st = stat(original.inode).expect("stat original");
        assert!(st.nlink >= 2);

        unlink("/mnt/ext2/hello-link.txt").expect("unlink ext2 link");
        let st_after_unlink = stat(original.inode).expect("stat original after unlink");
        assert_eq!(st_after_unlink.nlink, 1);

        symlinkat(ROOT_INODE, "/mnt/ext2/hello.txt", "/mnt/ext2/hello-sym.txt")
            .expect("ext2 symlink");
        let mut link_target = [0u8; 128];
        let target_n =
            readlinkat(ROOT_INODE, "/mnt/ext2/hello-sym.txt", &mut link_target).expect("readlink");
        assert_eq!(&link_target[..target_n], b"/mnt/ext2/hello.txt");
    }

    #[test]
    fn ext2_mount_populates_directories_on_demand() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let _ctx = mount_sample_ext2();
        let ext2_root = lookup("/mnt/ext2").expect("lookup ext2 root");

        with_state(|state| {
            let root = &state.inodes[ext2_root as usize];
            assert_eq!(root.children.len(), 0);
            assert!(!root.ext2_children_loaded);
        });

        // A path lookup imports only the named entry...
        let _ = open("/mnt/ext2/hello.txt", O_RDONLY, 0).expect("open ext2 file");
        with_state(|state| {
            let root = &state.inodes[ext2_root as usize];
            assert!(!root.ext2_children_loaded);
            assert_eq!(root.children.len(), 1);
        });
        // ...readdir imports the rest, without duplicating it.
        let listed = count_dir_entries("/mnt/ext2");
        with_state(|state| {
            let root = &state.inodes[ext2_root as usize];
            assert!(root.ext2_children_loaded);
            assert_eq!(root.children.len(), listed);
        });
        let _ = open("/mnt/ext2/hello.txt", O_RDONLY, 0).expect("reopen ext2 file");
        assert_eq!(count_dir_entries("/mnt/ext2"), listed);
    }

    #[test]
    fn ext2_root_mount_supports_legacy_mnt_ext2_prefix() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let _ctx = mount_ext2_backend_image_at("/", SAMPLE_EXT2, 4096);

        let direct = open("/hello.txt", O_RDONLY, 0).expect("open root path");
        let compat = open("/mnt/ext2/hello.txt", O_RDONLY, 0).expect("open legacy path");

        let mut direct_buf = [0u8; 96];
        let direct_n = read(direct.inode, 0, &mut direct_buf).expect("read root file");
        let mut compat_buf = [0u8; 96];
        let compat_n = read(compat.inode, 0, &mut compat_buf).expect("read legacy file");

        assert_eq!(&direct_buf[..direct_n], &compat_buf[..compat_n]);

        let mounts = open("/proc/mounts", O_RDONLY, 0).expect("open /proc/mounts");
        let mut mounts_buf = [0u8; 256];
        let mounts_n = read(mounts.inode, 0, &mut mounts_buf).expect("read mounts");
        let mounts_text = core::str::from_utf8(&mounts_buf[..mounts_n]).expect("utf8");
        assert!(mounts_text.contains("ext2 / ext2 rw"));
    }

    #[test]
    fn ext2_root_mount_prunes_tmp_overlay_but_keeps_pseudo_mounts() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let shadow = open("/tmp/pre_mount.txt", O_CREAT | O_RDWR, 0).expect("create /tmp shadow");
        let _ = write(shadow.inode, 0, b"shadow", false).expect("write /tmp shadow");

        let temp = open("/temp/keep.txt", O_CREAT | O_RDWR, 0).expect("create /temp file");
        let _ = write(temp.inode, 0, b"temp", false).expect("write /temp file");

        let _ctx = mount_ext2_backend_image_at("/", SAMPLE_EXT2, 4096);

        assert_eq!(open("/tmp/pre_mount.txt", O_RDONLY, 0).unwrap_err(), ENOENT);

        let kept = open("/temp/keep.txt", O_RDONLY, 0).expect("open kept /temp file");
        let mut kept_buf = [0u8; 8];
        let kept_n = read(kept.inode, 0, &mut kept_buf).expect("read kept /temp file");
        assert_eq!(&kept_buf[..kept_n], b"temp");

        let _ = open("/dev/null", O_RDWR, 0).expect("open /dev/null after root mount");
        let _ = open("/proc/mounts", O_RDONLY, 0).expect("open /proc/mounts after root mount");
    }

    #[test]
    fn ext2_logical_block_lookup_handles_indirect_levels() {
        let _guard = TEST_LOCK.lock().expect("lock should work");

        let mut meta = Ext2Meta::empty();
        meta.block_size = 1024;
        let ptrs_per_block = meta.block_size / core::mem::size_of::<u32>();

        let mut image = std::vec::Vec::from([0u8; 1024 * 32]);
        let mut inode = Ext2Inode {
            mode: 0,
            uid: 0,
            size: 0,
            atime: 0,
            ctime: 0,
            mtime: 0,
            links_count: 0,
            gid: 0,
            block_count_512: 0,
            blocks: [0; 15],
            flags: 0,
            file_acl: 0,
            size_high: 0,
            ino: 0,
            generation: 0,
            blocks_hi: 0,
            time_extra: [0; 3],
            time_orig: [0; 3],
        };

        inode.blocks[0] = 2;
        inode.blocks[12] = 3;
        inode.blocks[13] = 4;
        inode.blocks[14] = 5;

        write_u32_le(&mut image, 3 * 1024, 6).expect("single indirect entry");
        write_u32_le(&mut image, 4 * 1024, 7).expect("double l1 entry");
        write_u32_le(&mut image, 7 * 1024 + 4, 8).expect("double l2 entry");
        write_u32_le(&mut image, 5 * 1024, 9).expect("triple l1 entry");
        write_u32_le(&mut image, 9 * 1024, 10).expect("triple l2 entry");
        write_u32_le(&mut image, 10 * 1024 + 8, 11).expect("triple l3 entry");

        assert_eq!(
            ext2_data_block_for_logical_index(&image, &meta, &inode, 0).unwrap(),
            2
        );
        assert_eq!(
            ext2_data_block_for_logical_index(&image, &meta, &inode, EXT2_INODE_DIRECT_COUNT)
                .unwrap(),
            6
        );
        assert_eq!(
            ext2_data_block_for_logical_index(&image, &meta, &inode, EXT2_INODE_DIRECT_COUNT + 1)
                .unwrap(),
            0
        );
        assert_eq!(
            ext2_data_block_for_logical_index(
                &image,
                &meta,
                &inode,
                EXT2_INODE_DIRECT_COUNT + ptrs_per_block + 1
            )
            .unwrap(),
            8
        );
        assert_eq!(
            ext2_data_block_for_logical_index(
                &image,
                &meta,
                &inode,
                EXT2_INODE_DIRECT_COUNT + ptrs_per_block + ptrs_per_block * ptrs_per_block + 2
            )
            .unwrap(),
            11
        );
    }

    #[test]
    fn ext2_mount_invalid_image_rolls_back_mount_state() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let mut bad = std::vec::Vec::from(SAMPLE_EXT2);
        bad[EXT2_SUPERBLOCK_OFFSET + 56] = 0;
        bad[EXT2_SUPERBLOCK_OFFSET + 57] = 0;

        let err = mount_ext2_backend_image_expect_err(&bad, 4096);
        assert_eq!(err, EINVAL);

        let created =
            open("/mnt/ext2/fallback.txt", O_CREAT | O_RDWR, 0).expect("tmpfs path still writable");
        let _ = write(created.inode, 0, b"ok", false).expect("write fallback");
    }

    #[test]
    fn ext2_parse_meta_tracks_journal_fields() {
        let _guard = TEST_LOCK.lock().expect("lock should work");

        let mut image = std::vec::Vec::from(SAMPLE_EXT2);
        let compat_off = EXT2_SUPERBLOCK_OFFSET + 92;
        let compat = read_u32_le(&image, compat_off).expect("read feature compat");
        write_u32_le(
            &mut image,
            compat_off,
            compat | EXT2_FEATURE_COMPAT_HAS_JOURNAL,
        )
        .expect("set has_journal");
        write_u32_le(&mut image, EXT2_SUPERBLOCK_OFFSET + 224, 8).expect("set journal inode");
        write_u32_le(&mut image, EXT2_SUPERBLOCK_OFFSET + 228, 0).expect("set journal dev");

        let meta = ext2_parse_meta_with_len(&image, image.len(), true).expect("parse journal sb");
        assert_ne!(meta.feature_compat & EXT2_FEATURE_COMPAT_HAS_JOURNAL, 0);
        assert_eq!(meta.journal_inode, 8);
        assert_eq!(meta.journal_dev, 0);
    }

    #[test]
    fn ext2_mount_rejects_journal_feature_bits() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let mut bad = std::vec::Vec::from(SAMPLE_EXT2);
        let compat_off = EXT2_SUPERBLOCK_OFFSET + 92;
        let compat = read_u32_le(&bad, compat_off).expect("read feature compat");
        write_u32_le(
            &mut bad,
            compat_off,
            compat | EXT2_FEATURE_COMPAT_HAS_JOURNAL,
        )
        .expect("set has_journal");

        let err = mount_ext2_backend_image_expect_err(&bad, 4096);
        assert_eq!(err, EINVAL);
    }

    #[test]
    fn ext2_mount_replays_internal_journal_and_clears_recover_state() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, target_block, payload) = build_internal_journal_replay_image();
        let ctx = mount_ext2_backend_image(&image, 4096);

        let block_size = 1024usize;
        let target_off = (target_block as usize) * block_size;
        assert_eq!(
            &ctx.image[target_off..target_off + payload.len()],
            payload.as_slice()
        );
        assert!(ctx.writes > 0);

        let incompat = read_u32_le(&ctx.image, EXT2_SUPERBLOCK_OFFSET + 96).expect("read incompat");
        assert_eq!(incompat & EXT2_FEATURE_INCOMPAT_RECOVER, 0);

        let journal_super_off = 40usize * block_size;
        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 24), 56);
        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 28), 0);

        let mounts = open("/proc/mounts", O_RDONLY, 0).expect("open /proc/mounts");
        let mut mounts_buf = [0u8; 256];
        let mounts_n = read(mounts.inode, 0, &mut mounts_buf).expect("read mounts");
        let mounts_text = core::str::from_utf8(&mounts_buf[..mounts_n]).expect("utf8");
        assert!(mounts_text.contains("ext3 /mnt/ext2 ext3 rw"));

        with_state(|state| {
            assert!(state.ext2_journal.present);
            assert_eq!(state.ext2_journal.mode, Ext2JournalMode::Ordered);
            assert_eq!(state.ext2_journal.inode, 8);
            assert_eq!(state.ext2_journal.start, 0);
            assert_eq!(state.ext2_journal.sequence, 56);
        });
    }

    #[test]
    fn ext2_journaled_mount_seals_ordered_transaction_state_after_write_scope() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);

        let file = open("/mnt/ext2/hello.txt", O_RDWR, 0).expect("open ext2 file");
        let _ = write(file.inode, 0, b"ordered", false).expect("write ext2 file");

        with_state(|state| {
            assert!(state.ext2_journal.present);
            assert_eq!(state.ext2_journal.mode, Ext2JournalMode::Ordered);
            assert!(!state.ext2_journal_running_tx.active);
            assert!(state.ext2_journal_running_tx.tracked_blocks.is_empty());
            assert!(state.ext2_journal_running_tx.data_blocks.is_empty());
            assert_eq!(state.ext2_journal_running_tx.scope_depth, 0);
            assert_eq!(state.ext2_journal_pending_txs.len(), 1);
            assert!(!state.ext2_journal_pending_txs[0].metadata_blocks.is_empty());
            assert!(!state.ext2_journal_pending_txs[0].data_blocks.is_empty());
            assert_eq!(state.ext2_journal_pending_txs[0].tx_id, 1);
            assert!(state.ext2_journal_pending_txs[0].used_credits > 0);
        });

        with_state(|state| {
            let flushed = ext2_sync::flush_all_dirty(state).expect("flush all dirty");
            assert!(flushed > 0);
            assert!(!state.ext2_journal_running_tx.active);
            assert!(state.ext2_journal_running_tx.tracked_blocks.is_empty());
            assert!(state.ext2_journal_running_tx.data_blocks.is_empty());
            assert!(state.ext2_journal_pending_txs.is_empty());
            assert!(!state.ext2_journal_committed_txs.is_empty());
        });
    }

    #[test]
    fn ext2_ordered_mode_writes_file_data_home_before_checkpoint() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let ctx = mount_ext2_backend_image(&image, 4096);

        let file = open("/mnt/ext2/hello.txt", O_RDWR, 0).expect("open ext2 file");
        let _ = write(file.inode, 0, b"ordered!", false).expect("write ext2 file");

        let file_ext2_ino = with_state(|state| state.inodes[file.inode as usize].ext2_ino);
        let file_home_block = with_state(|state| {
            let inode = ext2_read_inode_from_state(state, file_ext2_ino).expect("read ext2 inode");
            ext2_data_block_for_logical_index_state(state, &inode, 0).expect("resolve data block")
        });

        with_state(|state| {
            let flushed = ext2_sync::flush_all_dirty(state).expect("flush all dirty");
            assert!(flushed > 0);
            assert!(!state.ext2_journal_committed_txs.is_empty());
        });

        let block_size = 1024usize;
        let home_off = (file_home_block as usize) * block_size;
        assert_eq!(&ctx.image[home_off..home_off + 8], b"ordered!");

        with_state(|state| {
            let checkpointed =
                ext2_journal::checkpoint_all_committed(state).expect("checkpoint committed");
            assert!(checkpointed > 0);
        });
    }

    #[test]
    fn ext2_ordered_mode_nested_scopes_share_transaction_id_and_credits() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);

        with_state(|state| {
            let outer = ext2_journal::begin_mutation_scope_with_credits(state, 2)
                .expect("begin outer scope");
            let inner = ext2_journal::begin_mutation_scope_with_credits(state, 3)
                .expect("begin inner scope");
            assert_eq!(outer, inner);
            assert_eq!(state.ext2_journal_running_tx.tx_id, outer);
            assert_eq!(state.ext2_journal_running_tx.handle_count, 2);
            assert_eq!(state.ext2_journal_running_tx.reserved_credits, 5);

            ext2_ensure_block_resident(state, 1).expect("load block");
            {
                let block = ext2_block_slice_mut_state(state, 1).expect("mut block");
                block[0] ^= 0x11;
            }
            ext2_mark_dirty_block(state, 1).expect("mark dirty");

            ext2_journal::end_mutation_scope(state).expect("end inner scope");
            assert!(state.ext2_journal_pending_txs.is_empty());
            assert_eq!(state.ext2_journal_running_tx.tx_id, outer);
            assert_eq!(state.ext2_journal_running_tx.handle_count, 1);
            assert_eq!(state.ext2_journal_running_tx.scope_depth, 1);

            ext2_journal::end_mutation_scope(state).expect("end outer scope");
            assert_eq!(state.ext2_journal_pending_txs.len(), 1);
            let pending = &state.ext2_journal_pending_txs[0];
            assert_eq!(pending.tx_id, outer);
            assert_eq!(pending.reserved_credits, 5);
            assert_eq!(pending.used_credits, 1);
        });
    }

    #[test]
    fn ext2_ordered_mode_credit_budget_rejects_extra_unique_blocks() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);

        with_state(|state| {
            let tx_id = ext2_journal::begin_mutation_scope_with_credits(state, 1)
                .expect("begin credited scope");
            ext2_ensure_block_resident(state, 1).expect("load block 1");
            ext2_ensure_block_resident(state, 2).expect("load block 2");

            ext2_journal::note_metadata_block(state, 1).expect("track first block");
            assert_eq!(
                ext2_journal::note_metadata_block(state, 2).unwrap_err(),
                ENOSPC
            );

            ext2_journal::end_mutation_scope(state).expect("end credited scope");
            assert_eq!(state.ext2_journal_pending_txs.len(), 1);
            let pending = &state.ext2_journal_pending_txs[0];
            assert_eq!(pending.tx_id, tx_id);
            assert_eq!(pending.restart_count, 0);
            assert_eq!(pending.reserved_credits, 1);
            assert_eq!(pending.used_credits, 1);
            assert_eq!(pending.metadata_blocks, std::vec![1]);
        });
    }

    #[test]
    fn ext2_ordered_mode_restartable_scope_seals_multiple_segments() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);

        with_state(|state| {
            let tx_id = ext2_journal::begin_mutation_scope_with_credits_options(state, 1, true, 2)
                .expect("begin restartable scope");
            ext2_ensure_block_resident(state, 1).expect("load block 1");
            ext2_ensure_block_resident(state, 2).expect("load block 2");
            ext2_ensure_block_resident(state, 3).expect("load block 3");

            ext2_journal::note_metadata_block(state, 1).expect("track first block");
            ext2_journal::note_metadata_block(state, 2).expect("track second block");
            ext2_journal::note_metadata_block(state, 3).expect("track third block");

            assert_eq!(state.ext2_journal_running_tx.op_id, tx_id);
            assert_ne!(state.ext2_journal_running_tx.tx_id, tx_id);
            assert_eq!(state.ext2_journal_running_tx.segment_index, 1);
            assert_eq!(state.ext2_journal_running_tx.restart_count, 1);
            assert_eq!(state.ext2_journal_running_tx.reserved_credits, 2);
            assert_eq!(state.ext2_journal_running_tx.used_credits, 2);

            ext2_journal::end_mutation_scope(state).expect("end restartable scope");
            assert_eq!(state.ext2_journal_pending_txs.len(), 2);

            let first = &state.ext2_journal_pending_txs[0];
            assert_eq!(first.op_id, tx_id);
            assert_eq!(first.tx_id, tx_id);
            assert_eq!(first.segment_index, 0);
            assert_eq!(first.restart_count, 0);
            assert_eq!(first.reserved_credits, 1);
            assert_eq!(first.used_credits, 1);
            assert_eq!(first.metadata_blocks, std::vec![1]);

            let second = &state.ext2_journal_pending_txs[1];
            assert_eq!(second.op_id, tx_id);
            assert_ne!(second.tx_id, tx_id);
            assert_eq!(second.segment_index, 1);
            assert_eq!(second.restart_count, 1);
            assert_eq!(second.reserved_credits, 2);
            assert_eq!(second.used_credits, 2);
            assert_eq!(second.metadata_blocks, std::vec![2, 3]);
        });
    }

    #[test]
    fn ext2_ordered_mode_prepare_running_tx_credits_rotates_before_next_step() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);

        with_state(|state| {
            let op_id = ext2_journal::begin_mutation_scope_with_credits_options(state, 1, true, 1)
                .expect("begin restartable scope");
            ext2_ensure_block_resident(state, 1).expect("load block 1");
            ext2_ensure_block_resident(state, 2).expect("load block 2");

            ext2_journal::note_metadata_block(state, 1).expect("track first block");
            assert_eq!(state.ext2_journal_pending_txs.len(), 0);

            ext2_journal::begin_restartable_phase(
                state,
                Ext2JournalPhaseKind::DataWrite,
                1,
            )
            .expect("begin next phase");
            assert_eq!(state.ext2_journal_pending_txs.len(), 1);
            assert_eq!(state.ext2_journal_pending_txs[0].op_id, op_id);
            assert_eq!(state.ext2_journal_pending_txs[0].segment_index, 0);
            assert_eq!(state.ext2_journal_pending_txs[0].metadata_blocks, std::vec![1]);
            assert_eq!(state.ext2_journal_pending_txs[0].phase_entries, 0);
            assert_eq!(
                state.ext2_journal_running_tx.last_phase_kind,
                Ext2JournalPhaseKind::DataWrite
            );
            assert_eq!(state.ext2_journal_running_tx.phase_entries, 1);

            ext2_journal::note_metadata_block(state, 2).expect("track second block");
            assert_eq!(state.ext2_journal_running_tx.op_id, op_id);
            assert_eq!(state.ext2_journal_running_tx.segment_index, 1);
            assert_eq!(state.ext2_journal_running_tx.restart_count, 1);

            ext2_journal::end_mutation_scope(state).expect("end scope");
            assert_eq!(state.ext2_journal_pending_txs.len(), 2);
            assert_eq!(state.ext2_journal_pending_txs[1].metadata_blocks, std::vec![2]);
        });
    }

    #[test]
    fn ext2_ordered_mode_restartable_phase_helper_groups_work_into_named_subhandles() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);

        with_state(|state| {
            ext2_journal::begin_mutation_scope_with_credits_options(state, 4, true, 4)
                .expect("begin restartable scope");
            ext2_ensure_block_resident(state, 1).expect("load block 1");
            ext2_ensure_block_resident(state, 2).expect("load block 2");
            ext2_ensure_block_resident(state, 3).expect("load block 3");

            ext2_journal::with_restartable_phase(
                state,
                Ext2JournalPhaseKind::DataWrite,
                2,
                |state| {
                    ext2_journal::note_metadata_block(state, 1)?;
                    ext2_journal::note_metadata_block(state, 2)?;
                    Ok(())
                },
            )
            .expect("data-write phase");
            assert_eq!(state.ext2_journal_running_tx.phase_entries, 1);
            assert_eq!(
                state.ext2_journal_running_tx.last_phase_kind,
                Ext2JournalPhaseKind::DataWrite
            );

            ext2_journal::with_restartable_phase(
                state,
                Ext2JournalPhaseKind::InodeCloseout,
                1,
                |state| ext2_journal::note_metadata_block(state, 3),
            )
            .expect("inode-closeout phase");
            assert_eq!(state.ext2_journal_running_tx.phase_entries, 2);
            assert_eq!(
                state.ext2_journal_running_tx.last_phase_kind,
                Ext2JournalPhaseKind::InodeCloseout
            );

            ext2_journal::end_mutation_scope(state).expect("end scope");
            assert_eq!(state.ext2_journal_pending_txs.len(), 1);
            let pending = &state.ext2_journal_pending_txs[0];
            assert_eq!(pending.phase_entries, 2);
            assert_eq!(pending.last_phase_kind, Ext2JournalPhaseKind::InodeCloseout);
            assert_eq!(pending.metadata_blocks, std::vec![1, 2, 3]);
        });
    }

    #[test]
    fn ext2_ordered_mode_handle_phase_rejects_stale_handle() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);

        with_state(|state| {
            ext2_journal::begin_mutation_scope_with_credits_options(state, 2, true, 2)
                .expect("begin restartable scope");
            let handle = ext2_journal::current_handle(state).expect("current handle");
            ext2_journal::end_mutation_scope(state).expect("end scope");

            let err = ext2_journal::with_handle_restartable_phase(
                state,
                Some(handle),
                Ext2JournalPhaseKind::DataWrite,
                1,
                |_state| Ok(()),
            )
            .expect_err("stale handle should fail");
            assert_eq!(err, EINVAL);
        });
    }

    #[test]
    fn ext2_ordered_mode_step_credit_estimators_account_for_indirection() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);

        let file = open("/mnt/ext2/estimate.bin", O_CREAT | O_RDWR, 0).expect("create file");
        with_state(|state| {
            ext2_sync::flush_all_dirty(state).expect("flush create");
            ext2_journal::checkpoint_all_committed(state).expect("checkpoint create");
        });

        with_state(|state| {
            let ext2_ino = state.inodes[file.inode as usize].ext2_ino;
            let mut inode = ext2_read_inode_from_state(state, ext2_ino).expect("read inode");

            assert_eq!(
                ext2_estimate_data_write_step_credits(state, &inode, 0).expect("direct estimate"),
                EXT2_JOURNAL_CREDITS_STEP_ALLOC_DIRECT
            );
            assert_eq!(
                ext2_estimate_data_write_step_credits(state, &inode, EXT2_INODE_DIRECT_COUNT)
                    .expect("single estimate"),
                EXT2_JOURNAL_CREDITS_STEP_ALLOC_TABLE_ENTRY
                    + EXT2_JOURNAL_CREDITS_STEP_ALLOC_TABLE_ROOT
            );

            let single_root = ext2_alloc_block(state, None).expect("alloc single root");
            inode.blocks[12] = single_root;
            assert_eq!(
                ext2_estimate_data_write_step_credits(state, &inode, EXT2_INODE_DIRECT_COUNT)
                    .expect("single estimate with root"),
                EXT2_JOURNAL_CREDITS_STEP_ALLOC_TABLE_ENTRY
            );

            inode.blocks[0] = ext2_alloc_block(state, None).expect("alloc direct data");
            let single_leaf = ext2_alloc_block(state, None).expect("alloc single data");
            ext2_write_indirect_entry_from_state(state, single_root, 0, single_leaf)
                .expect("populate single leaf");
            assert_eq!(
                ext2_estimate_data_write_step_credits(state, &inode, 0)
                    .expect("existing direct estimate"),
                EXT2_JOURNAL_CREDITS_STEP_EXISTING_DATA
            );
            assert_eq!(
                ext2_estimate_detach_step_credits(state, &inode, 0).expect("detach direct estimate"),
                EXT2_JOURNAL_CREDITS_STEP_FREE_DIRECT
            );
            assert_eq!(
                ext2_estimate_detach_step_credits(state, &inode, EXT2_INODE_DIRECT_COUNT)
                    .expect("detach single estimate"),
                EXT2_JOURNAL_CREDITS_STEP_FREE_TABLE_ENTRY
                    + EXT2_JOURNAL_CREDITS_STEP_FREE_DIRECT
            );
        });
    }

    #[test]
    fn ext2_ordered_mode_indirect_step_estimate_rotates_before_allocation() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);

        let file = open("/mnt/ext2/indirect-rotate.bin", O_CREAT | O_RDWR, 0).expect("create file");
        with_state(|state| {
            ext2_sync::flush_all_dirty(state).expect("flush create");
            ext2_journal::checkpoint_all_committed(state).expect("checkpoint create");
        });

        with_state(|state| {
            let ext2_ino = state.inodes[file.inode as usize].ext2_ino;
            let inode = ext2_read_inode_from_state(state, ext2_ino).expect("read inode");
            let tx_id = ext2_journal::begin_mutation_scope_with_credits_options(state, 4, true, 4)
                .expect("begin restartable scope");
            ext2_ensure_block_resident(state, 1).expect("load block 1");
            ext2_journal::note_metadata_block(state, 1).expect("track initial metadata");

            let needed = ext2_estimate_data_write_step_credits(state, &inode, EXT2_INODE_DIRECT_COUNT)
                .expect("estimate indirect allocation");
            assert!(needed > 3);

            ext2_journal::begin_restartable_phase(
                state,
                Ext2JournalPhaseKind::IndirectLink,
                needed,
            )
            .expect("prepare indirect allocation");
            assert_eq!(state.ext2_journal_pending_txs.len(), 1);
            assert_eq!(state.ext2_journal_pending_txs[0].op_id, tx_id);
            assert_eq!(state.ext2_journal_pending_txs[0].segment_index, 0);
            assert_eq!(state.ext2_journal_running_tx.segment_index, 1);
            assert_eq!(
                state.ext2_journal_running_tx.last_phase_kind,
                Ext2JournalPhaseKind::IndirectLink
            );

            ext2_journal::end_mutation_scope(state).expect("end scope");
        });
    }

    #[test]
    fn ext2_ordered_mode_alloc_block_helper_rotates_before_bitmap_update() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);

        with_state(|state| {
            let tx_id = ext2_journal::begin_mutation_scope_with_credits_options(state, 1, true, 1)
                .expect("begin restartable alloc scope");
            ext2_ensure_block_resident(state, 1).expect("load block 1");
            ext2_journal::note_metadata_block(state, 1).expect("track initial metadata");

            let _ = ext2_alloc_block(state, ext2_journal::current_handle(state)).expect("alloc block");

            assert!(!state.ext2_journal_pending_txs.is_empty());
            assert!(state
                .ext2_journal_pending_txs
                .iter()
                .any(|tx| tx.op_id == tx_id));
            assert!(state.ext2_journal_running_tx.segment_index > 0);

            ext2_journal::end_mutation_scope(state).expect("end alloc scope");
            let segment_count = state
                .ext2_journal_pending_txs
                .iter()
                .filter(|tx| tx.op_id == tx_id)
                .count();
            assert!(segment_count >= 2);
        });
    }

    #[test]
    fn ext2_ordered_mode_alloc_inode_helper_rotates_before_inode_bitmap_update() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);

        with_state(|state| {
            let tx_id = ext2_journal::begin_mutation_scope_with_credits_options(state, 1, true, 1)
                .expect("begin restartable inode alloc scope");
            ext2_ensure_block_resident(state, 1).expect("load block 1");
            ext2_journal::note_metadata_block(state, 1).expect("track initial metadata");

            let _ = ext2_alloc_inode(state, ext2_journal::current_handle(state), false)
                .expect("alloc inode");

            assert!(!state.ext2_journal_pending_txs.is_empty());
            assert!(state
                .ext2_journal_pending_txs
                .iter()
                .any(|tx| tx.op_id == tx_id));
            assert!(state.ext2_journal_running_tx.segment_index > 0);

            ext2_journal::end_mutation_scope(state).expect("end inode alloc scope");
            let segment_count = state
                .ext2_journal_pending_txs
                .iter()
                .filter(|tx| tx.op_id == tx_id)
                .count();
            assert!(segment_count >= 2);
        });
    }

    #[test]
    fn ext2_ordered_mode_free_block_helper_can_split_revoke_phase() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);

        with_state(|state| {
            let block = ext2_alloc_block(state, None).expect("seed allocated block");
            ext2_sync::flush_all_dirty(state).expect("flush seed alloc");
            ext2_journal::checkpoint_all_committed(state).expect("checkpoint seed alloc");

            let tx_id = ext2_journal::begin_mutation_scope_with_credits_options(state, 1, true, 1)
                .expect("begin restartable free scope");
            ext2_ensure_block_resident(state, 1).expect("load block 1");
            ext2_journal::note_metadata_block(state, 1).expect("track initial metadata");

            ext2_free_block(state, ext2_journal::current_handle(state), block).expect("free block");
            ext2_journal::end_mutation_scope(state).expect("end free scope");

            let segment_count = state
                .ext2_journal_pending_txs
                .iter()
                .filter(|tx| tx.op_id == tx_id)
                .count();
            assert!(segment_count >= 2);
        });
    }

    #[test]
    fn ext2_ordered_mode_detach_helper_can_split_unlink_phase() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);

        let file = open("/mnt/ext2/detach.bin", O_CREAT | O_RDWR, 0).expect("create file");
        with_state(|state| {
            ext2_sync::flush_all_dirty(state).expect("flush create");
            ext2_journal::checkpoint_all_committed(state).expect("checkpoint create");
        });

        with_state(|state| {
            let ext2_ino = state.inodes[file.inode as usize].ext2_ino;
            let mut inode = ext2_read_inode_from_state(state, ext2_ino).expect("read inode");
            let single_root = ext2_alloc_block(state, None).expect("alloc single root");
            let single_leaf = ext2_alloc_block(state, None).expect("alloc single leaf");
            ext2_write_indirect_entry_from_state(state, single_root, 0, single_leaf)
                .expect("link single leaf");
            inode.blocks[12] = single_root;
            ext2_write_inode_to_state(state, ext2_ino, &inode).expect("persist inode");
            ext2_sync::flush_all_dirty(state).expect("flush seeded inode");
            ext2_journal::checkpoint_all_committed(state).expect("checkpoint seeded inode");

            let tx_id = ext2_journal::begin_mutation_scope_with_credits_options(state, 1, true, 1)
                .expect("begin restartable detach scope");
            ext2_ensure_block_resident(state, 1).expect("load block 1");
            ext2_journal::note_metadata_block(state, 1).expect("track initial metadata");

            let removed = ext2_detach_data_block(
                state,
                ext2_journal::current_handle(state),
                &mut inode,
                EXT2_INODE_DIRECT_COUNT,
            )
                .expect("detach single indirect block");
            assert_eq!(removed, single_leaf);
            ext2_free_block(state, ext2_journal::current_handle(state), removed)
                .expect("free detached block");
            ext2_journal::end_mutation_scope(state).expect("end detach scope");

            let segment_count = state
                .ext2_journal_pending_txs
                .iter()
                .filter(|tx| tx.op_id == tx_id)
                .count();
            assert!(segment_count >= 2);
        });
    }

    #[test]
    fn ext2_ordered_mode_public_metadata_mutator_reserves_bounded_credits() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);

        let file = open("/mnt/ext2/hello.txt", O_RDWR, 0).expect("open ext2 file");
        chmod(file.inode, 0o600).expect("chmod file");

        with_state(|state| {
            assert_eq!(state.ext2_journal_pending_txs.len(), 1);
            let pending = &state.ext2_journal_pending_txs[0];
            assert_eq!(
                pending.reserved_credits,
                EXT2_JOURNAL_CREDITS_METADATA_UPDATE
            );
            assert!(pending.used_credits > 0);
        });
    }

    #[test]
    fn ext2_ordered_mode_write_reserves_estimated_bounded_credits() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);

        let file = open("/mnt/ext2/hello.txt", O_RDWR, 0).expect("open ext2 file");
        let ext2_ino = with_state(|state| state.inodes[file.inode as usize].ext2_ino);
        let expected = with_state(|state| {
            ext2_estimate_write_credits(state, ext2_ino, 0, b"credit write".len(), false)
                .expect("estimate write credits")
        });
        let plan = ext2_plan_variable_journal_credits(expected);

        let _ = write(file.inode, 0, b"credit write", false).expect("write file");

        with_state(|state| {
            assert_eq!(state.ext2_journal_pending_txs.len(), 1);
            let pending = &state.ext2_journal_pending_txs[0];
            assert_eq!(pending.reserved_credits, plan.initial_credits);
            assert!(pending.used_credits > 0);
        });
    }

    #[test]
    fn ext2_ordered_mode_truncate_reserves_estimated_bounded_credits() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);

        let file = open("/mnt/ext2/trunc-credit.bin", O_CREAT | O_RDWR, 0).expect("create file");
        let payload = [0x44u8; 2048];
        let _ = write(file.inode, 0, &payload, false).expect("seed file");
        with_state(|state| {
            ext2_sync::flush_all_dirty(state).expect("flush seed write");
            ext2_journal::checkpoint_all_committed(state).expect("checkpoint seed write");
        });

        let ext2_ino = with_state(|state| state.inodes[file.inode as usize].ext2_ino);
        let expected = with_state(|state| {
            ext2_estimate_truncate_to_length_credits(state, ext2_ino, 512)
                .expect("estimate truncate credits")
        });
        let plan = ext2_plan_variable_journal_credits(expected);

        truncate_to(file.inode, 512).expect("truncate file");

        with_state(|state| {
            assert_eq!(state.ext2_journal_pending_txs.len(), 1);
            let pending = &state.ext2_journal_pending_txs[0];
            assert_eq!(pending.reserved_credits, plan.initial_credits);
            assert!(pending.used_credits > 0);
        });
    }

    #[test]
    fn ext2_ordered_mode_large_public_truncate_splits_into_planned_segments() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);

        let file = open("/mnt/ext2/segmented.bin", O_CREAT | O_RDWR, 0).expect("create file");
        let payload = [0x5Au8; 32 * 1024];
        let _ = write(file.inode, 0, &payload, false).expect("seed file");
        with_state(|state| {
            ext2_sync::flush_all_dirty(state).expect("flush seed write");
            ext2_journal::checkpoint_all_committed(state).expect("checkpoint seed write");
        });

        let expected = with_state(|state| {
            ext2_estimate_truncate_to_zero_credits(state, state.inodes[file.inode as usize].ext2_ino)
                .expect("estimate large truncate credits")
        });
        let plan = ext2_plan_variable_journal_credits(expected);
        assert!(expected > plan.initial_credits);

        truncate(file.inode).expect("truncate large file");

        with_state(|state| {
            assert!(state.ext2_journal_pending_txs.len() >= 2);
            let op_id = state.ext2_journal_pending_txs[0].op_id;
            let mut tx_idx = 0usize;
            let mut saw_later_segment = false;
            while tx_idx < state.ext2_journal_pending_txs.len() {
                let tx = &state.ext2_journal_pending_txs[tx_idx];
                assert_eq!(tx.op_id, op_id);
                assert_eq!(tx.reserved_credits, plan.initial_credits);
                if tx.segment_index > 0 {
                    saw_later_segment = true;
                    assert_ne!(tx.tx_id, op_id);
                    assert!(tx.restart_count > 0);
                }
                tx_idx += 1;
            }
            assert!(saw_later_segment);
        });
    }

    #[test]
    fn ext2_ordered_mode_large_write_ends_with_metadata_closeout_segment() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);

        let file = open("/mnt/ext2/closeout.bin", O_CREAT | O_RDWR, 0).expect("create file");
        with_state(|state| {
            ext2_sync::flush_all_dirty(state).expect("flush create");
            ext2_journal::checkpoint_all_committed(state).expect("checkpoint create");
        });
        let payload = [0xA5u8; 32 * 1024];

        with_state(|state| {
            let ext2_ino = state.inodes[file.inode as usize].ext2_ino;
            let tx_id = ext2_journal::begin_mutation_scope_with_credits_options(state, 1, true, 1)
                .expect("begin restartable write scope");
            let wrote = ext2_write_file(state, ext2_ino, 0, &payload, false).expect("large write");
            assert_eq!(wrote.written, payload.len());
            ext2_journal::end_mutation_scope(state).expect("end restartable write scope");

            // Earlier segments may already have been committed by dirty-pressure
            // writeback during the write (they used to pile up behind an empty
            // sealed segment that wedged the queue); either way the operation
            // must have been split, and its tail must be the closeout segment.
            assert!(!state.ext2_journal_pending_txs.is_empty());
            assert!(
                state.ext2_journal_pending_txs.len() >= 2
                    || !state.ext2_journal_committed_txs.is_empty()
            );
            let first_op = state.ext2_journal_pending_txs[0].op_id;
            assert_eq!(first_op, tx_id);
            let last = state.ext2_journal_pending_txs.last().expect("last pending tx");
            assert_eq!(last.op_id, first_op);
            assert!(last.segment_index > 0);
            assert!(last.data_blocks.is_empty());
            assert!(!last.metadata_blocks.is_empty());
            assert_eq!(last.last_phase_kind, Ext2JournalPhaseKind::InodeCloseout);
            assert!(last.phase_entries >= 1);
        });
    }

    #[test]
    fn ext2_ordered_mode_journals_metadata_without_file_data_tag() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let ctx = mount_ext2_backend_image(&image, 4096);

        let file = open("/mnt/ext2/hello.txt", O_RDWR, 0).expect("open ext2 file");
        let _ = write(file.inode, 0, b"ordered!", false).expect("write ext2 file");

        let file_ext2_ino = with_state(|state| state.inodes[file.inode as usize].ext2_ino);
        let file_home_block = with_state(|state| {
            let inode = ext2_read_inode_from_state(state, file_ext2_ino).expect("read ext2 inode");
            ext2_data_block_for_logical_index_state(state, &inode, 0).expect("resolve data block")
        });

        with_state(|state| {
            let flushed = ext2_sync::flush_all_dirty(state).expect("flush all dirty");
            assert!(flushed > 0);
        });

        let descriptor_off = 41usize * 1024usize;
        assert_eq!(read_u32_be(&ctx.image, descriptor_off), 0xC03B_3998);
        assert_eq!(read_u32_be(&ctx.image, descriptor_off + 4), 1);

        let mut off = descriptor_off + 12;
        loop {
            let home_block = read_u32_be(&ctx.image, off);
            let flags = read_u32_be(&ctx.image, off + 4);
            assert_ne!(home_block, file_home_block);
            off += 8;
            if (flags & 0x0000_0008) != 0 {
                break;
            }
        }
    }

    #[test]
    fn ext2_ordered_mode_prefers_running_transaction_metadata_set() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);

        let file = open("/mnt/ext2/hello.txt", O_RDWR, 0).expect("open ext2 file");
        let _ = write(file.inode, 0, b"ordered!", false).expect("write ext2 file");

        with_state(|state| {
            let tracked_metadata = state.ext2_journal_pending_txs[0].metadata_blocks.clone();
            assert!(!tracked_metadata.is_empty());

            let unrelated_block = 63u32;
            assert!(!tracked_metadata.contains(&unrelated_block));
            assert!(!state.ext2_journal_pending_txs[0].data_blocks.contains(&unrelated_block));

            {
                let blk = ext2_block_slice_mut_state(state, unrelated_block).expect("mut block");
                blk[0] ^= 0x5A;
            }
            ext2_mark_dirty_block(state, unrelated_block).expect("mark unrelated dirty");
            assert!(
                state
                    .ext2_journal_running_tx
                    .tracked_blocks
                    .binary_search(&unrelated_block)
                    .is_err()
            );

            let flushed = ext2_journal::flush_dirty_batch(state, 64).expect("flush tx batch");
            assert!(flushed > 0);
            assert!(!state.ext2_journal_committed_txs.is_empty());

            let updates = &state.ext2_journal_committed_txs[0].updates;
            assert!(!updates.is_empty());
            for update in updates.iter() {
                assert!(tracked_metadata.contains(&update.home_block));
                assert_ne!(update.home_block, unrelated_block);
            }
        });
    }

    #[test]
    fn ext2_ordered_mode_queues_separate_pending_transactions_per_scope() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);

        let file = open("/mnt/ext2/hello.txt", O_RDWR, 0).expect("open ext2 file");
        let _ = write(file.inode, 0, b"scope1", false).expect("first write");
        let _ = truncate_to(file.inode, 3).expect("truncate second scope");

        with_state(|state| {
            assert!(!state.ext2_journal_running_tx.active);
            assert_eq!(state.ext2_journal_running_tx.scope_depth, 0);
            assert_eq!(state.ext2_journal_pending_txs.len(), 2);
        });
    }

    #[test]
    fn ext2_ordered_mode_seals_revoke_blocks_with_truncate_transaction() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);
        // Keep dirty-pressure writeback from committing earlier transactions
        // so the create/write/truncate transactions are all still pending.
        set_ext2_cache_policy(EXT2_CACHE_MAX_BLOCKS_DEFAULT, 100).expect("cache policy");

        let file = open("/mnt/ext2/revoke.bin", O_CREAT | O_RDWR, 0).expect("create file");
        let payload = [0xABu8; 2048];
        let _ = write(file.inode, 0, &payload, false).expect("write payload");
        truncate(file.inode).expect("truncate file");

        with_state(|state| {
            assert!(state.ext2_journal_pending_txs.len() >= 2);
            let revoke_tx = state.ext2_journal_pending_txs.last().expect("pending tx");
            assert!(!revoke_tx.revoke_blocks.is_empty());
        });
    }

    #[test]
    fn ext2_ordered_mode_commits_sealed_metadata_snapshot_before_newer_cache_state() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let ctx = mount_ext2_backend_image(&image, 4096);
        // Keep dirty-pressure writeback from committing the create before the
        // chmod seals, so both sealed transactions are still pending together.
        set_ext2_cache_policy(EXT2_CACHE_MAX_BLOCKS_DEFAULT, 100).expect("cache policy");

        let file = open("/mnt/ext2/snapshot.txt", O_CREAT | O_RDWR, 0).expect("create file");
        chmod(file.inode, 0o600).expect("chmod file");

        let (shared_block, first_snapshot, second_snapshot) = with_state(|state| {
            assert!(state.ext2_journal_pending_txs.len() >= 2);
            let first = &state.ext2_journal_pending_txs[0];
            let second = &state.ext2_journal_pending_txs[1];

            let mut shared = None;
            let mut idx = 0usize;
            while idx < first.metadata_blocks.len() {
                let block = first.metadata_blocks[idx];
                if second.metadata_blocks.binary_search(&block).is_ok() {
                    shared = Some(block);
                    break;
                }
                idx += 1;
            }

            let shared_block = shared.expect("shared metadata block");
            let first_snapshot = first
                .metadata_snapshots
                .iter()
                .find(|entry| entry.home_block == shared_block)
                .expect("first snapshot")
                .data
                .clone();
            let second_snapshot = second
                .metadata_snapshots
                .iter()
                .find(|entry| entry.home_block == shared_block)
                .expect("second snapshot")
                .data
                .clone();
            assert_ne!(first_snapshot, second_snapshot);

            let flushed = ext2_journal::flush_dirty_batch(state, 64).expect("flush first tx");
            assert!(flushed > 0);
            ext2_journal::checkpoint_all_committed(state).expect("checkpoint first tx");

            let cache_idx = ext2_cache_find(state, shared_block).expect("shared block cached");
            assert!(state.ext2_cached_blocks[cache_idx].dirty);
            assert_eq!(
                state.ext2_cached_blocks[cache_idx].data.as_slice(),
                second_snapshot.as_slice()
            );

            (shared_block, first_snapshot, second_snapshot)
        });

        let block_off = (shared_block as usize) * 1024usize;
        assert_eq!(
            &ctx.image[block_off..block_off + first_snapshot.len()],
            first_snapshot.as_slice()
        );
        assert_ne!(
            &ctx.image[block_off..block_off + second_snapshot.len()],
            second_snapshot.as_slice()
        );
    }

    #[test]
    fn ext2_ordered_mode_flush_dirty_block_preserves_pending_transaction_order() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);
        set_ext2_cache_policy(16, 90).expect("set relaxed cache policy");

        let file1 = open("/mnt/ext2/hello.txt", O_RDWR, 0).expect("open first file");
        let _ = write(file1.inode, 0, b"first tx", false).expect("write first file");

        let file2 = open("/mnt/ext2/later.bin", O_CREAT | O_RDWR, 0).expect("create second file");
        let payload = [0xCDu8; 1024];
        let _ = write(file2.inode, 0, &payload, false).expect("write second file");

        with_state(|state| {
            assert!(state.ext2_journal_pending_txs.len() >= 2);
            let target_block = *state.ext2_journal_pending_txs.last().expect("last pending tx")
                .data_blocks
                .first()
                .expect("target data block");

            let mut target_owner_idx = None;
            let mut tx_idx = 0usize;
            while tx_idx < state.ext2_journal_pending_txs.len() {
                let tx = &state.ext2_journal_pending_txs[tx_idx];
                if tx.metadata_blocks.binary_search(&target_block).is_ok()
                    || tx.data_blocks.binary_search(&target_block).is_ok()
                {
                    target_owner_idx = Some(tx_idx);
                    break;
                }
                tx_idx += 1;
            }
            let target_owner_idx = target_owner_idx.expect("target owner tx");
            assert!(target_owner_idx > 0);

            let mut retired_prefix_blocks = std::vec::Vec::new();
            tx_idx = 0;
            while tx_idx <= target_owner_idx {
                let tx = &state.ext2_journal_pending_txs[tx_idx];
                retired_prefix_blocks.extend_from_slice(tx.metadata_blocks.as_slice());
                retired_prefix_blocks.extend_from_slice(tx.data_blocks.as_slice());
                tx_idx += 1;
            }

            ext2_journal::flush_dirty_block(state, target_block).expect("flush later dirty block");

            tx_idx = 0;
            while tx_idx < state.ext2_journal_pending_txs.len() {
                let tx = &state.ext2_journal_pending_txs[tx_idx];
                let mut block_idx = 0usize;
                while block_idx < retired_prefix_blocks.len() {
                    let block = retired_prefix_blocks[block_idx];
                    assert!(tx.metadata_blocks.binary_search(&block).is_err());
                    assert!(tx.data_blocks.binary_search(&block).is_err());
                    block_idx += 1;
                }
                tx_idx += 1;
            }
        });
    }

    #[test]
    fn ext2_ordered_mode_dirty_pressure_flushes_oldest_pending_transaction() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);
        set_ext2_cache_policy(16, 90).expect("set relaxed cache policy");

        let file1 = open("/mnt/ext2/hello.txt", O_RDWR, 0).expect("open first file");
        let _ = write(file1.inode, 0, b"first pending tx", false).expect("write first file");

        let file2 = open("/mnt/ext2/pressure.bin", O_CREAT | O_RDWR, 0).expect("create second file");
        let payload = [0xEFu8; 1024];
        let _ = write(file2.inode, 0, &payload, false).expect("write second file");
        set_ext2_cache_policy(16, 10).expect("set pressure cache policy");

        with_state(|state| {
            assert!(state.ext2_journal_pending_txs.len() >= 2);
            let first_metadata = state.ext2_journal_pending_txs[0].metadata_blocks.clone();
            assert!(!first_metadata.is_empty());

            ext2_cache_maybe_flush_dirty_pressure(state).expect("pressure flush");

            assert!(!state.ext2_journal_committed_txs.is_empty());
            let first_committed = &state.ext2_journal_committed_txs[0].updates;
            assert!(!first_committed.is_empty());
            for update in first_committed.iter() {
                assert!(first_metadata.contains(&update.home_block));
            }
        });
    }

    #[test]
    fn ext2_mount_replay_revoke_handles_wrapped_transaction_sequences() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, revoked_block, revoked_payload, surviving_block, surviving_payload) =
            build_internal_journal_wrapped_revoke_replay_image();
        let ctx = mount_ext2_backend_image(&image, 4096);

        let block_size = 1024usize;
        let revoked_off = (revoked_block as usize) * block_size;
        let surviving_off = (surviving_block as usize) * block_size;

        assert!(ctx.image[revoked_off..revoked_off + revoked_payload.len()]
            .iter()
            .all(|byte| *byte == b's'));
        assert_eq!(
            &ctx.image[surviving_off..surviving_off + surviving_payload.len()],
            surviving_payload.as_slice()
        );

        let incompat = read_u32_le(&ctx.image, EXT2_SUPERBLOCK_OFFSET + 96).expect("read incompat");
        assert_eq!(incompat & EXT2_FEATURE_INCOMPAT_RECOVER, 0);

        let journal_super_off = 40usize * block_size;
        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 24), 1);
        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 28), 0);

        with_state(|state| {
            assert!(state.ext2_journal.present);
            assert_eq!(state.ext2_journal.start, 0);
            assert_eq!(state.ext2_journal.sequence, 1);
        });
    }

    #[test]
    fn ext2_journaled_flush_emits_runtime_transaction() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, target_block, _payload) = build_internal_journal_replay_image();
        let ctx = mount_ext2_backend_image(&image, 4096);

        with_state(|state| {
            {
                let block =
                    ext2_block_slice_mut_state(state, target_block).expect("target block mut");
                block[0..4].copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
            }
            ext2_mark_dirty_block(state, target_block).expect("mark dirty");
            assert!(state.ext2_dirty_blocks.binary_search(&target_block).is_ok());
            ext2_sync::flush_all_dirty(state).expect("flush all dirty");
            assert!(state.ext2_dirty_blocks.is_empty());
        });

        let block_size = 1024usize;
        let journal_super_off = 40usize * block_size;
        let descriptor_off = 41usize * block_size;
        let data_off = 42usize * block_size;
        let commit_off = 43usize * block_size;
        let target_off = (target_block as usize) * block_size;

        // s_sequence names the oldest live transaction (the one at s_start).
        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 24), 56);
        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 28), 1);

        assert_eq!(read_u32_be(&ctx.image, descriptor_off), 0xC03B_3998);
        assert_eq!(read_u32_be(&ctx.image, descriptor_off + 4), 1);
        assert_eq!(read_u32_be(&ctx.image, descriptor_off + 8), 56);
        assert_eq!(read_u32_be(&ctx.image, descriptor_off + 12), target_block);
        assert_eq!(read_u32_be(&ctx.image, descriptor_off + 16), 0x0000_000A);

        assert_eq!(
            &ctx.image[data_off..data_off + 4],
            &[0x11, 0x22, 0x33, 0x44]
        );

        assert_eq!(read_u32_be(&ctx.image, commit_off), 0xC03B_3998);
        assert_eq!(read_u32_be(&ctx.image, commit_off + 4), 2);
        assert_eq!(read_u32_be(&ctx.image, commit_off + 8), 56);

        assert_eq!(&ctx.image[target_off..target_off + 4], b"jour");

        with_state(|state| {
            let checkpointed =
                ext2_journal::checkpoint_all_committed(state).expect("checkpoint committed");
            assert_eq!(checkpointed, 1);
            assert!(state.ext2_journal_committed_txs.is_empty());
            assert_eq!(state.ext2_journal.start, 0);
        });

        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 24), 57);
        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 28), 0);
        assert_eq!(
            &ctx.image[target_off..target_off + 4],
            &[0x11, 0x22, 0x33, 0x44]
        );
        assert!(ctx.writes > 0);
    }

    #[test]
    fn ext2_journaled_flush_batches_multiple_blocks_in_one_transaction() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, target_block, _payload) = build_internal_journal_replay_image();
        let second_block = target_block + 1;
        let ctx = mount_ext2_backend_image(&image, 4096);

        with_state(|state| {
            {
                let first = ext2_block_slice_mut_state(state, target_block)
                    .expect("first target block mut");
                first[0..4].copy_from_slice(&[0xA1, 0xB2, 0xC3, 0xD4]);
            }
            ext2_mark_dirty_block(state, target_block).expect("mark first dirty");

            {
                let second = ext2_block_slice_mut_state(state, second_block)
                    .expect("second target block mut");
                second[0..4].copy_from_slice(&[0x01, 0x23, 0x45, 0x67]);
            }
            ext2_mark_dirty_block(state, second_block).expect("mark second dirty");

            ext2_sync::flush_all_dirty(state).expect("flush all dirty");
            assert!(state.ext2_dirty_blocks.is_empty());
        });

        let block_size = 1024usize;
        let journal_super_off = 40usize * block_size;
        let descriptor_off = 41usize * block_size;
        let first_data_off = 42usize * block_size;
        let second_data_off = 43usize * block_size;
        let commit_off = 44usize * block_size;
        let first_target_off = (target_block as usize) * block_size;
        let second_target_off = (second_block as usize) * block_size;

        // s_sequence names the oldest live transaction (the one at s_start).
        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 24), 56);
        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 28), 1);

        assert_eq!(read_u32_be(&ctx.image, descriptor_off), 0xC03B_3998);
        assert_eq!(read_u32_be(&ctx.image, descriptor_off + 4), 1);
        assert_eq!(read_u32_be(&ctx.image, descriptor_off + 8), 56);
        assert_eq!(read_u32_be(&ctx.image, descriptor_off + 12), target_block);
        assert_eq!(read_u32_be(&ctx.image, descriptor_off + 16), 0x0000_0002);
        assert_eq!(read_u32_be(&ctx.image, descriptor_off + 20), second_block);
        assert_eq!(read_u32_be(&ctx.image, descriptor_off + 24), 0x0000_000A);

        assert_eq!(
            &ctx.image[first_data_off..first_data_off + 4],
            &[0xA1, 0xB2, 0xC3, 0xD4]
        );
        assert_eq!(
            &ctx.image[second_data_off..second_data_off + 4],
            &[0x01, 0x23, 0x45, 0x67]
        );

        assert_eq!(read_u32_be(&ctx.image, commit_off), 0xC03B_3998);
        assert_eq!(read_u32_be(&ctx.image, commit_off + 4), 2);
        assert_eq!(read_u32_be(&ctx.image, commit_off + 8), 56);

        with_state(|state| {
            let checkpointed =
                ext2_journal::checkpoint_all_committed(state).expect("checkpoint committed");
            assert_eq!(checkpointed, 2);
            assert!(state.ext2_journal_committed_txs.is_empty());
        });

        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 24), 57);
        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 28), 0);
        assert_eq!(
            &ctx.image[first_target_off..first_target_off + 4],
            &[0xA1, 0xB2, 0xC3, 0xD4]
        );
        assert_eq!(
            &ctx.image[second_target_off..second_target_off + 4],
            &[0x01, 0x23, 0x45, 0x67]
        );
        assert!(ctx.writes > 0);
    }

    #[test]
    fn ext2_journaled_flush_rotates_runtime_head_between_transactions() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, target_block, _payload) = build_internal_journal_replay_image();
        let ctx = mount_ext2_backend_image(&image, 4096);

        with_state(|state| {
            {
                let block =
                    ext2_block_slice_mut_state(state, target_block).expect("target block mut #1");
                block[0..4].copy_from_slice(&[0x10, 0x20, 0x30, 0x40]);
            }
            ext2_mark_dirty_block(state, target_block).expect("mark dirty #1");
            ext2_sync::flush_all_dirty(state).expect("flush #1");

            {
                let block =
                    ext2_block_slice_mut_state(state, target_block).expect("target block mut #2");
                block[0..4].copy_from_slice(&[0x50, 0x60, 0x70, 0x80]);
            }
            ext2_mark_dirty_block(state, target_block).expect("mark dirty #2");
            ext2_sync::flush_all_dirty(state).expect("flush #2");
            assert!(state.ext2_dirty_blocks.is_empty());
            assert_eq!(state.ext2_journal.sequence, 58);
            assert_eq!(state.ext2_journal.runtime_head_jblock, 1);
            assert_eq!(state.ext2_journal_committed_txs.len(), 2);
        });

        let block_size = 1024usize;
        let journal_super_off = 40usize * block_size;
        let first_descriptor_off = 41usize * block_size;
        let second_descriptor_off = 44usize * block_size;
        let second_data_off = 45usize * block_size;
        let second_commit_off = 46usize * block_size;
        let target_off = (target_block as usize) * block_size;

        // s_sequence names the oldest live transaction (the one at s_start).
        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 24), 56);
        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 28), 1);

        assert_eq!(read_u32_be(&ctx.image, first_descriptor_off), 0xC03B_3998);
        assert_eq!(read_u32_be(&ctx.image, first_descriptor_off + 4), 1);
        assert_eq!(read_u32_be(&ctx.image, first_descriptor_off + 8), 56);

        assert_eq!(read_u32_be(&ctx.image, second_descriptor_off), 0xC03B_3998);
        assert_eq!(read_u32_be(&ctx.image, second_descriptor_off + 4), 1);
        assert_eq!(read_u32_be(&ctx.image, second_descriptor_off + 8), 57);
        assert_eq!(
            read_u32_be(&ctx.image, second_descriptor_off + 12),
            target_block
        );
        assert_eq!(
            read_u32_be(&ctx.image, second_descriptor_off + 16),
            0x0000_000A
        );

        assert_eq!(
            &ctx.image[second_data_off..second_data_off + 4],
            &[0x50, 0x60, 0x70, 0x80]
        );

        assert_eq!(read_u32_be(&ctx.image, second_commit_off), 0xC03B_3998);
        assert_eq!(read_u32_be(&ctx.image, second_commit_off + 4), 2);
        assert_eq!(read_u32_be(&ctx.image, second_commit_off + 8), 57);

        assert_eq!(&ctx.image[target_off..target_off + 4], b"jour");

        with_state(|state| {
            let checkpointed =
                ext2_journal::checkpoint_all_committed(state).expect("checkpoint committed");
            assert_eq!(checkpointed, 2);
            assert_eq!(state.ext2_journal.start, 0);
            assert!(state.ext2_journal_committed_txs.is_empty());
        });

        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 28), 0);
        assert_eq!(
            &ctx.image[target_off..target_off + 4],
            &[0x50, 0x60, 0x70, 0x80]
        );
    }

    #[test]
    fn ext2_journaled_flush_wraps_runtime_transaction_layout() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, target_block, _payload) = build_internal_journal_replay_image();
        let second_block = target_block + 1;
        let ctx = mount_ext2_backend_image(&image, 4096);

        with_state(|state| {
            {
                let first =
                    ext2_block_slice_mut_state(state, target_block).expect("first block mut #1");
                first[0..4].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
            }
            ext2_mark_dirty_block(state, target_block).expect("mark dirty #1");
            ext2_sync::flush_all_dirty(state).expect("flush #1");

            {
                let first =
                    ext2_block_slice_mut_state(state, target_block).expect("first block mut #2");
                first[0..4].copy_from_slice(&[0x01, 0x11, 0x21, 0x31]);
            }
            ext2_mark_dirty_block(state, target_block).expect("mark first dirty #2");

            {
                let second =
                    ext2_block_slice_mut_state(state, second_block).expect("second block mut #2");
                second[0..4].copy_from_slice(&[0x41, 0x51, 0x61, 0x71]);
            }
            ext2_mark_dirty_block(state, second_block).expect("mark second dirty #2");

            ext2_sync::flush_all_dirty(state).expect("flush #2");
            assert!(state.ext2_dirty_blocks.is_empty());
            assert_eq!(state.ext2_journal.sequence, 58);
            assert_eq!(state.ext2_journal.runtime_head_jblock, 2);
            assert_eq!(state.ext2_journal_committed_txs.len(), 1);
        });

        let block_size = 1024usize;
        let journal_super_off = 40usize * block_size;
        let wrapped_commit_off = 41usize * block_size;
        let descriptor_off = 44usize * block_size;
        let first_data_off = 45usize * block_size;
        let second_data_off = 46usize * block_size;
        let first_target_off = (target_block as usize) * block_size;
        let second_target_off = (second_block as usize) * block_size;

        // s_sequence names the oldest live transaction (the one at s_start).
        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 24), 57);
        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 28), 4);

        assert_eq!(read_u32_be(&ctx.image, descriptor_off), 0xC03B_3998);
        assert_eq!(read_u32_be(&ctx.image, descriptor_off + 4), 1);
        assert_eq!(read_u32_be(&ctx.image, descriptor_off + 8), 57);
        assert_eq!(read_u32_be(&ctx.image, descriptor_off + 12), target_block);
        assert_eq!(read_u32_be(&ctx.image, descriptor_off + 16), 0x0000_0002);
        assert_eq!(read_u32_be(&ctx.image, descriptor_off + 20), second_block);
        assert_eq!(read_u32_be(&ctx.image, descriptor_off + 24), 0x0000_000A);

        assert_eq!(
            &ctx.image[first_data_off..first_data_off + 4],
            &[0x01, 0x11, 0x21, 0x31]
        );
        assert_eq!(
            &ctx.image[second_data_off..second_data_off + 4],
            &[0x41, 0x51, 0x61, 0x71]
        );

        assert_eq!(read_u32_be(&ctx.image, wrapped_commit_off), 0xC03B_3998);
        assert_eq!(read_u32_be(&ctx.image, wrapped_commit_off + 4), 2);
        assert_eq!(read_u32_be(&ctx.image, wrapped_commit_off + 8), 57);

        assert_eq!(
            &ctx.image[first_target_off..first_target_off + 4],
            &[0xAA, 0xBB, 0xCC, 0xDD]
        );

        with_state(|state| {
            let checkpointed =
                ext2_journal::checkpoint_all_committed(state).expect("checkpoint committed");
            assert_eq!(checkpointed, 2);
            assert_eq!(state.ext2_journal.start, 0);
            assert!(state.ext2_journal_committed_txs.is_empty());
        });

        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 28), 0);
        assert_eq!(
            &ctx.image[first_target_off..first_target_off + 4],
            &[0x01, 0x11, 0x21, 0x31]
        );
        assert_eq!(
            &ctx.image[second_target_off..second_target_off + 4],
            &[0x41, 0x51, 0x61, 0x71]
        );
    }

    #[test]
    fn ext2_journaled_flush_emits_revoke_block_from_pending_revoke_set() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, target_block, _payload) = build_internal_journal_replay_image();
        let ctx = mount_ext2_backend_image(&image, 4096);

        with_state(|state| {
            state
                .ext2_journal_pending_revokes
                .push(target_block.saturating_add(9));
            {
                let block =
                    ext2_block_slice_mut_state(state, target_block).expect("target block mut");
                block[0..4].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
            }
            ext2_mark_dirty_block(state, target_block).expect("mark dirty");
            ext2_sync::flush_all_dirty(state).expect("flush all dirty");
            assert!(state.ext2_dirty_blocks.is_empty());
            assert!(state.ext2_journal_pending_revokes.is_empty());
        });

        let block_size = 1024usize;
        let revoke_off = 43usize * block_size;
        assert_eq!(read_u32_be(&ctx.image, revoke_off), 0xC03B_3998);
        assert_eq!(read_u32_be(&ctx.image, revoke_off + 4), 5);
        assert_eq!(read_u32_be(&ctx.image, revoke_off + 8), 56);
        assert_eq!(read_u32_be(&ctx.image, revoke_off + 12), 20);
        assert_eq!(read_u32_be(&ctx.image, revoke_off + 16), target_block + 9);
    }

    #[test]
    fn ext2_journal_runtime_io_error_aborts_and_blocks_future_flushes() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, target_block, _payload) = build_internal_journal_replay_image();
        let mut ctx = mount_ext2_backend_image(&image, 4096);
        ctx.fail_write_after = Some(ctx.writes.saturating_add(2));

        with_state(|state| {
            {
                let block =
                    ext2_block_slice_mut_state(state, target_block).expect("target block mut");
                block[0..4].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
            }
            ext2_mark_dirty_block(state, target_block).expect("mark dirty");

            assert_eq!(ext2_sync::flush_all_dirty(state).unwrap_err(), EIO);
            assert!(state.ext2_journal.aborted);
            assert_eq!(state.ext2_journal.abort_errno, EIO);
            assert!(state.ext2_dirty_blocks.contains(&target_block));
        });

        let writes_after_abort = ctx.writes;
        ctx.fail_write_after = None;

        with_state(|state| {
            assert_eq!(ext2_sync::flush_all_dirty(state).unwrap_err(), EIO);
            assert!(state.ext2_journal.aborted);
            assert!(state.ext2_dirty_blocks.contains(&target_block));
        });

        assert_eq!(ctx.writes, writes_after_abort);
    }

    #[test]
    fn ext2_journal_checkpoint_io_error_keeps_committed_tx_and_aborts() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, target_block, _payload) = build_internal_journal_replay_image();
        let mut ctx = mount_ext2_backend_image(&image, 4096);

        with_state(|state| {
            {
                let block =
                    ext2_block_slice_mut_state(state, target_block).expect("target block mut");
                block[0..4].copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
            }
            ext2_mark_dirty_block(state, target_block).expect("mark dirty");
            ext2_sync::flush_all_dirty(state).expect("flush all dirty");
            assert_eq!(state.ext2_journal_committed_txs.len(), 1);
        });

        ctx.fail_write_after = Some(ctx.writes);

        with_state(|state| {
            assert_eq!(
                ext2_journal::checkpoint_all_committed(state).unwrap_err(),
                EIO
            );
            assert!(state.ext2_journal.aborted);
            assert_eq!(state.ext2_journal.abort_errno, EIO);
            assert_eq!(state.ext2_journal_committed_txs.len(), 1);
        });

        let writes_after_abort = ctx.writes;
        ctx.fail_write_after = None;

        with_state(|state| {
            assert_eq!(
                ext2_journal::checkpoint_all_committed(state).unwrap_err(),
                EIO
            );
            assert!(state.ext2_journal.aborted);
            assert_eq!(state.ext2_journal_committed_txs.len(), 1);
        });

        assert_eq!(ctx.writes, writes_after_abort);
    }

    #[test]
    fn ext2_mount_rejects_unknown_incompat_features() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let mut bad = std::vec::Vec::from(SAMPLE_EXT2);
        let incompat_off = EXT2_SUPERBLOCK_OFFSET + 96;
        let incompat = read_u32_le(&bad, incompat_off).expect("read feature incompat");
        // 0x8000 = inline_data: still unsupported even read-only.
        write_u32_le(&mut bad, incompat_off, incompat | 0x8000).expect("set unsupported incompat");

        let err = mount_ext2_backend_image_expect_err(&bad, 4096);
        assert_eq!(err, EINVAL);
    }

    #[test]
    fn ext2_mount_rejects_unimplemented_compat_features() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let unsupported_compat = [
            EXT2_FEATURE_COMPAT_DIR_PREALLOC,
            EXT2_FEATURE_COMPAT_IMAGIC_INODES,
        ];

        for feature in unsupported_compat {
            let mut bad = std::vec::Vec::from(SAMPLE_EXT2);
            let compat_off = EXT2_SUPERBLOCK_OFFSET + 92;
            let compat = read_u32_le(&bad, compat_off).expect("read feature compat");
            write_u32_le(&mut bad, compat_off, compat | feature).expect("set unsupported compat");

            let err = mount_ext2_backend_image_expect_err(&bad, 4096);
            assert_eq!(err, EINVAL);
        }
    }

    #[test]
    fn ext2_mount_rejects_unimplemented_ro_compat_features() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let mut bad = std::vec::Vec::from(SAMPLE_EXT2);
        let ro_compat_off = EXT2_SUPERBLOCK_OFFSET + 100;
        let ro_compat = read_u32_le(&bad, ro_compat_off).expect("read feature ro compat");
        write_u32_le(
            &mut bad,
            ro_compat_off,
            ro_compat | EXT2_FEATURE_RO_COMPAT_BTREE_DIR,
        )
        .expect("set unsupported ro compat");

        let err = mount_ext2_backend_image_expect_err(&bad, 4096);
        assert_eq!(err, EINVAL);
    }

    #[test]
    fn ext2_mutations_persist_in_exported_image() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let _ctx = mount_sample_ext2();

        let created = open("/mnt/ext2/persist.txt", O_CREAT | O_RDWR, 0).expect("create persist");
        let _ = write(created.inode, 0, b"persist", false).expect("write persist");
        mkdir("/mnt/ext2/persist_dir").expect("mkdir persist dir");
        let hello = open("/mnt/ext2/hello.txt", O_WRONLY | O_TRUNC, 0).expect("truncate hello");
        let _ = write(hello.inode, 0, b"rewritten", false).expect("rewrite hello");

        let mut exported = std::vec![0u8; SAMPLE_EXT2.len()];
        let image_len = export_ext2_image(&mut exported).expect("export image");

        let remount_image = std::vec::Vec::from(&exported[..image_len]);
        init();
        let _ctx2 = mount_ext2_backend_image(&remount_image, 4096);

        let persist = open("/mnt/ext2/persist.txt", O_RDONLY, 0).expect("open persisted file");
        let mut persist_buf = [0u8; 16];
        let persist_n = read(persist.inode, 0, &mut persist_buf).expect("read persisted file");
        assert_eq!(&persist_buf[..persist_n], b"persist");

        let persisted_hello =
            open("/mnt/ext2/hello.txt", O_RDONLY, 0).expect("open rewritten hello");
        let mut hello_buf = [0u8; 16];
        let hello_n = read(persisted_hello.inode, 0, &mut hello_buf).expect("read rewritten hello");
        assert_eq!(&hello_buf[..hello_n], b"rewritten");

        let nested = open("/mnt/ext2/persist_dir/new.txt", O_CREAT | O_RDWR, 0)
            .expect("create in persisted dir");
        let _ = write(nested.inode, 0, b"x", false).expect("write nested");
    }

    #[test]
    fn ext2_write_past_direct_blocks_uses_indirect_mapping() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let _ctx = mount_sample_ext2();
        let block_size = ext2_block_size() as usize;
        let direct_limit = EXT2_INODE_DIRECT_COUNT * block_size;
        assert!(
            direct_limit < TMPFS_MAX_FILE_SIZE,
            "fixture must have direct limit below write cap"
        );

        let file = open("/mnt/ext2/indirect.bin", O_CREAT | O_RDWR, 0).expect("create large file");
        let payload_len = direct_limit + 257;
        let mut payload = std::vec![0u8; payload_len];
        let mut i = 0usize;
        while i < payload_len {
            payload[i] = (i % 251) as u8;
            i += 1;
        }

        let wrote = write(file.inode, 0, &payload, false).expect("write payload");
        assert_eq!(wrote.written, payload_len);
        assert_eq!(wrote.new_offset, payload_len as u64);

        let mut read_back = std::vec![0u8; payload_len];
        let n = read(file.inode, 0, &mut read_back).expect("read payload");
        assert_eq!(n, payload_len);
        assert_eq!(read_back, payload);

        let st = stat(file.inode).expect("stat large file");
        assert_eq!(st.size, payload_len as u64);
        assert!(st.blocks > (EXT2_INODE_DIRECT_COUNT as u64 * (block_size / 512) as u64));

        let mut exported = std::vec![0u8; SAMPLE_EXT2.len()];
        let image_len = export_ext2_image(&mut exported).expect("export image");

        init();
        let _ctx2 = mount_ext2_backend_image(&exported[..image_len], 4096);
        let reopened = open("/mnt/ext2/indirect.bin", O_RDONLY, 0).expect("open persisted file");
        let mut persisted = std::vec![0u8; payload_len];
        let persisted_n = read(reopened.inode, 0, &mut persisted).expect("read persisted payload");
        assert_eq!(persisted_n, payload_len);
        assert_eq!(persisted, payload);
    }

    #[test]
    fn ext2_truncate_reclaims_indirect_blocks() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let _ctx = mount_sample_ext2();
        let block_size = ext2_block_size() as usize;
        let direct_limit = EXT2_INODE_DIRECT_COUNT * block_size;
        assert!(
            direct_limit < TMPFS_MAX_FILE_SIZE,
            "fixture must have direct limit below write cap"
        );

        let file =
            open("/mnt/ext2/reclaim.bin", O_CREAT | O_RDWR, 0).expect("create reclaim target");
        let free_before = with_state(|state| {
            ext2_read_u32_state(state, EXT2_SUPERBLOCK_OFFSET + 12)
                .expect("read free blocks before")
        });

        let payload_len = direct_limit + 193;
        let payload = std::vec![0x5Au8; payload_len];
        let _ = write(file.inode, 0, &payload, false).expect("write reclaim payload");

        let free_after_write = with_state(|state| {
            ext2_read_u32_state(state, EXT2_SUPERBLOCK_OFFSET + 12)
                .expect("read free blocks after write")
        });
        assert!(free_after_write < free_before);

        let ext2_ino = with_state(|state| state.inodes[file.inode as usize].ext2_ino);
        let inode_after_write = with_state(|state| {
            ext2_read_inode_from_state(state, ext2_ino).expect("read inode after write")
        });
        assert_ne!(inode_after_write.blocks[12], 0);

        truncate(file.inode).expect("truncate large file");

        let inode_after_trunc = with_state(|state| {
            ext2_read_inode_from_state(state, ext2_ino).expect("read inode after truncate")
        });
        assert_eq!(inode_after_trunc.size, 0);
        assert_eq!(inode_after_trunc.blocks[12], 0);
        assert_eq!(inode_after_trunc.blocks[13], 0);
        assert_eq!(inode_after_trunc.blocks[14], 0);

        let free_after_trunc = with_state(|state| {
            ext2_read_u32_state(state, EXT2_SUPERBLOCK_OFFSET + 12)
                .expect("read free blocks after truncate")
        });
        assert_eq!(free_after_trunc, free_before);
    }

    #[test]
    fn ext2_truncate_to_shrink_and_extend_roundtrip() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let _ctx = mount_sample_ext2();

        let block_size = ext2_block_size() as usize;
        let direct_limit = EXT2_INODE_DIRECT_COUNT * block_size;
        let payload_len = core::cmp::min(TMPFS_MAX_FILE_SIZE, direct_limit.saturating_add(97));

        let file = open("/mnt/ext2/truncate_to.bin", O_CREAT | O_RDWR, 0).expect("create file");
        let mut payload = std::vec![0u8; payload_len];
        let mut i = 0usize;
        while i < payload_len {
            payload[i] = (i % 239) as u8;
            i += 1;
        }
        let wrote = write(file.inode, 0, &payload, false).expect("write payload");
        assert_eq!(wrote.written, payload_len);

        let shrink_len = core::cmp::max(13usize, block_size + 13);
        truncate_to(file.inode, shrink_len as u64).expect("truncate shrink");
        assert_eq!(
            file_size(file.inode).expect("size after shrink"),
            shrink_len as u64
        );

        let mut shrunk = std::vec![0u8; shrink_len];
        let shrunk_n = read(file.inode, 0, &mut shrunk).expect("read shrunk");
        assert_eq!(shrunk_n, shrink_len);
        assert_eq!(shrunk, payload[..shrink_len]);

        let extend_len = shrink_len + 211;
        truncate_to(file.inode, extend_len as u64).expect("truncate extend");
        assert_eq!(
            file_size(file.inode).expect("size after extend"),
            extend_len as u64
        );

        let mut extended = std::vec![0u8; extend_len];
        let extended_n = read(file.inode, 0, &mut extended).expect("read extended");
        assert_eq!(extended_n, extend_len);
        assert_eq!(&extended[..shrink_len], &payload[..shrink_len]);
        assert!(extended[shrink_len..].iter().all(|b| *b == 0));
    }

    #[test]
    fn ext2_directory_entry_growth_reaches_indirect_blocks() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let _ctx = mount_sample_ext2();

        with_state(|state| {
            let mut root = ext2_read_inode_from_state(state, EXT2_ROOT_INODE).expect("read root");
            let mut name = [b'n'; 240];
            let hex = *b"0123456789abcdef";

            let mut inserted = 0usize;
            while root.blocks[12] == 0 && inserted < 256 {
                let mut digit = 0usize;
                while digit < 8 {
                    let shift = 4 * (7 - digit);
                    name[digit] = hex[(inserted >> shift) & 0xF];
                    digit += 1;
                }

                ext2_add_dir_entry(
                    state,
                    None,
                    EXT2_ROOT_INODE,
                    EXT2_ROOT_INODE,
                    &name,
                    FileType::Regular,
                )
                .expect("append root dir entry");

                root = ext2_read_inode_from_state(state, EXT2_ROOT_INODE).expect("reload root");
                inserted += 1;
            }

            assert_ne!(
                root.blocks[12], 0,
                "expected single-indirect directory growth after {} inserts",
                inserted
            );

            let mut seen = 0usize;
            ext2_for_each_dir_entry_state(state, &root, |_ino, _ft, _name| {
                seen = seen.saturating_add(1);
                Ok(())
            })
            .expect("scan grown directory");
            assert!(seen >= inserted);
        });
    }

    #[test]
    fn ext2_dirty_run_tracking_and_sync_checkpoint() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let _ctx = mount_sample_ext2();
        assert_eq!(ext2_next_dirty_run(0).expect("scan clean image"), None);

        let file = open("/mnt/ext2/dirty-tracker.txt", O_CREAT | O_RDWR, 0).expect("create file");
        let _ = write(file.inode, 0, b"abc", false).expect("write file");

        let run = ext2_next_dirty_run(0)
            .expect("scan dirty image")
            .expect("dirty run exists");
        assert!(run.block_count > 0);

        let block_size = ext2_block_size() as usize;
        let mut one_block = std::vec![0u8; block_size];
        let copied = copy_ext2_blocks(run.start_block, &mut one_block).expect("copy dirty block");
        assert_eq!(copied, block_size);

        mark_ext2_blocks_synced().expect("mark synced");
        assert_eq!(ext2_next_dirty_run(0).expect("scan after checkpoint"), None);
    }

    #[test]
    fn ext2_backend_config_and_mount_guards() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        assert_eq!(set_ext2_cache_policy(0, 30).unwrap_err(), EINVAL);
        assert_eq!(set_ext2_cache_policy(8, 0).unwrap_err(), EINVAL);
        assert_eq!(set_ext2_cache_policy(8, 101).unwrap_err(), EINVAL);
        set_ext2_cache_policy(EXT2_CACHE_MAX_BLOCKS_DEFAULT, 30).expect("set valid cache policy");

        assert_eq!(ext2_backend_kind(), Ext2BackendKind::None);
        let backend = Ext2BlockBackend {
            ctx: 0xA5A5,
            read_block: dummy_ext2_backend_read,
            write_block: dummy_ext2_backend_write,
            flush: None,
        };

        set_ext2_block_backend(backend).expect("set block backend");
        assert_eq!(ext2_backend_kind(), Ext2BackendKind::BlockOps);

        clear_ext2_block_backend().expect("clear block backend");
        assert_eq!(ext2_backend_kind(), Ext2BackendKind::None);

        let _ctx = mount_sample_ext2();
        assert_eq!(ext2_backend_kind(), Ext2BackendKind::BlockOps);
        assert_eq!(set_ext2_block_backend(backend).unwrap_err(), EBUSY);
        assert_eq!(clear_ext2_block_backend().unwrap_err(), EBUSY);

        init();
        assert_eq!(ext2_backend_kind(), Ext2BackendKind::None);
    }

    #[test]
    fn ext2_block_backend_mount_uses_lazy_residency() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let ctx = mount_sample_ext2();
        assert_eq!(ext2_backend_kind(), Ext2BackendKind::BlockOps);
        assert!(ctx.reads > 0);
        assert!(ctx.reads < (SAMPLE_EXT2.len() / 1024));
        let reads_after_mount = ctx.reads;

        let ext2_root = lookup("/mnt/ext2").expect("lookup ext2 root");
        with_state(|state| {
            let root = &state.inodes[ext2_root as usize];
            assert_eq!(root.children.len(), 0);
            assert!(!root.ext2_children_loaded);
        });

        let opened = open("/mnt/ext2/hello.txt", O_RDONLY, 0).expect("open sample file");
        let mut buf = [0u8; 64];
        let n = read(opened.inode, 0, &mut buf).expect("read sample file");
        assert!(n > 0);
        assert!(ctx.reads > reads_after_mount);
        with_state(|state| {
            let root = &state.inodes[ext2_root as usize];
            assert!(!root.ext2_children_loaded, "lookups import single entries");
            assert_eq!(root.children.len(), 1);
        });
    }

    #[test]
    fn ext2_block_backend_export_includes_unresident_blocks() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let ctx = mount_sample_ext2();
        let reads_before_export = ctx.reads;

        let mut exported = std::vec![0u8; SAMPLE_EXT2.len()];
        let image_len = export_ext2_image(&mut exported).expect("export image");
        assert_eq!(image_len, SAMPLE_EXT2.len());
        assert_eq!(&exported[..image_len], SAMPLE_EXT2);
        assert!(ctx.reads > reads_before_export);
    }

    #[test]
    fn ext2_block_backend_head_marks_full_blocks_only() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let meta = ext2_parse_meta_with_len(SAMPLE_EXT2, SAMPLE_EXT2.len(), true)
            .expect("parse sample ext2");
        assert_eq!(meta.block_size, 1024);
        let head_len = meta.block_size + (meta.block_size / 2);

        let _ctx = mount_ext2_backend_image(SAMPLE_EXT2, head_len);

        // Block 1 is only half covered by the head: if mount reads it (the
        // orphan check reads the superblock), it must come from the backend.
        with_state(|state| {
            assert!(ext2_block_cached(state, 0));
            if ext2_block_cached(state, 1) {
                let idx = ext2_cache_find(state, 1).unwrap();
                assert_eq!(&state.ext2_cached_blocks[idx].data[..], &SAMPLE_EXT2[1024..2048]);
            }
        });
    }

    #[test]
    fn ext2_cache_clock_enforces_block_cap() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let ctx = mount_sample_ext2();
        set_ext2_cache_policy(8, 100).expect("set cache policy");
        with_state(|state| {
            let max_probe =
                core::cmp::min(48usize, state.ext2_tracked_block_count.saturating_sub(1));
            let mut block = 1usize;
            while block <= max_probe {
                ext2_ensure_block_resident(state, block as u32).expect("load block");
                block += 1;
            }
            assert!(state.ext2_cached_blocks.len() <= 8);
        });
        assert_eq!(ctx.writes, 0);
    }

    #[test]
    fn ext2_cache_clock_flushes_dirty_victim_on_eviction() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let ctx = mount_sample_ext2();
        set_ext2_cache_policy(1, 100).expect("set cache policy");
        with_state(|state| {
            let block_size = state.ext2_meta.block_size;
            ext2_write_range_to_state(state, block_size, b"z").expect("dirty block 1");
            assert!(state.ext2_dirty_blocks.binary_search(&1).is_ok());
            ext2_ensure_block_resident(state, 2).expect("load block 2");
            assert!(state.ext2_cached_blocks.len() <= 1);
            assert!(!ext2_cache_has_block(state, 1));
            assert!(state.ext2_dirty_blocks.binary_search(&1).is_err());
        });
        assert!(ctx.writes > 0);
    }

    #[test]
    fn ext2_ordered_mode_eviction_flushes_oldest_pending_transaction() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);
        set_ext2_cache_policy(16, 100).expect("set relaxed cache policy");

        let file = open("/mnt/ext2/hello.txt", O_RDWR, 0).expect("open file");
        let _ = write(file.inode, 0, b"evict tx", false).expect("write file");

        let mut retired_prefix_blocks = std::vec::Vec::new();
        with_state(|state| {
            assert_eq!(state.ext2_journal_pending_txs.len(), 1);
            let first_tx = &state.ext2_journal_pending_txs[0];
            retired_prefix_blocks.extend_from_slice(first_tx.metadata_blocks.as_slice());
            retired_prefix_blocks.extend_from_slice(first_tx.data_blocks.as_slice());
            assert!(!retired_prefix_blocks.is_empty());
        });

        set_ext2_cache_policy(1, 100).expect("set eviction cache policy");

        with_state(|state| {
            ext2_ensure_block_resident(state, 2).expect("force eviction");
            assert!(state.ext2_journal_pending_txs.is_empty());
            let mut tx_idx = 0usize;
            while tx_idx < state.ext2_journal_pending_txs.len() {
                let tx = &state.ext2_journal_pending_txs[tx_idx];
                let mut block_idx = 0usize;
                while block_idx < retired_prefix_blocks.len() {
                    let block = retired_prefix_blocks[block_idx];
                    assert!(tx.metadata_blocks.binary_search(&block).is_err());
                    assert!(tx.data_blocks.binary_search(&block).is_err());
                    block_idx += 1;
                }
                tx_idx += 1;
            }
        });
    }

    #[test]
    fn ext2_ordered_mode_dirty_pressure_does_not_flush_running_scope() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let ctx = mount_ext2_backend_image(&image, 4096);
        set_ext2_cache_policy(4, 25).expect("set cache policy");
        let writes_before = ctx.writes;
        with_state(|state| {
            ext2_journal::begin_mutation_scope_with_credits(state, 0).expect("begin scope");

            let mut block = 1u32;
            while block <= 4 {
                ext2_ensure_block_resident(state, block).expect("load cache set");
                block += 1;
            }

            {
                let first = ext2_block_slice_mut_state(state, 1).expect("first block");
                first[0] = b'a';
            }
            ext2_mark_dirty_block(state, 1).expect("dirty first block");
            {
                let second = ext2_block_slice_mut_state(state, 2).expect("second block");
                second[0] = b'b';
            }
            ext2_mark_dirty_block(state, 2).expect("dirty second block");

            assert!(state.ext2_journal_running_tx.active);
            assert!(state.ext2_journal_pending_txs.is_empty());
            assert!(state.ext2_journal_committed_txs.is_empty());
            assert_eq!(ctx.writes, writes_before);

            ext2_journal::end_mutation_scope(state).expect("end scope");
            assert_eq!(state.ext2_journal_pending_txs.len(), 1);
        });
    }

    #[test]
    fn ext2_ordered_mode_eviction_rejects_dirty_running_scope_victim() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let (image, _target_block, _payload) = build_internal_journal_replay_image();
        let _ctx = mount_ext2_backend_image(&image, 4096);
        with_state(|state| {
            ext2_journal::begin_mutation_scope_with_credits(state, 0).expect("begin scope");
            let block_size = state.ext2_meta.block_size;
            ext2_write_range_to_state(state, block_size, b"z").expect("dirty block 1");

            ext2_set_cache_policy(state, 1, 100).expect("set cache policy");
            assert_eq!(ext2_ensure_block_resident(state, 2).unwrap_err(), ENOSPC);

            ext2_journal::end_mutation_scope(state).expect("end scope");
        });
    }

    #[test]
    fn ext2_cache_dirty_pressure_flushes_in_background_path() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let ctx = mount_sample_ext2();
        set_ext2_cache_policy(4, 25).expect("set cache policy");
        with_state(|state| {
            let mut block = 1u32;
            while block <= 4 {
                ext2_ensure_block_resident(state, block).expect("load cache set");
                block += 1;
            }

            let block_size = state.ext2_meta.block_size;
            ext2_write_range_to_state(state, block_size, b"a").expect("dirty first block");
            assert_eq!(state.ext2_dirty_blocks.len(), 1);
            ext2_write_range_to_state(state, block_size * 2, b"b").expect("dirty second block");
            assert!(state.ext2_dirty_blocks.len() <= 1);
        });
        assert!(ctx.writes > 0);
    }

    #[test]
    fn openat_readdir_and_stat_work() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        mkdir("/tmp/atdir").expect("mkdir /tmp/atdir");
        let dir = lookup("/tmp/atdir").expect("lookup /tmp/atdir");
        let file = openat(dir, "child.txt", O_CREAT | O_RDWR, 0).expect("openat create child");
        let _ = write(file.inode, 0, b"abc", false).expect("write child");

        let mut off = 0u64;
        let first = readdir(dir, off)
            .expect("readdir first")
            .expect("dot entry");
        assert_eq!(&first.name[..first.name_len], b".");
        off = first.next_offset;

        let second = readdir(dir, off)
            .expect("readdir second")
            .expect("dotdot entry");
        assert_eq!(&second.name[..second.name_len], b"..");
        off = second.next_offset;

        let third = readdir(dir, off)
            .expect("readdir child")
            .expect("child entry");
        assert_eq!(&third.name[..third.name_len], b"child.txt");
        assert_eq!(third.d_type, DT_REG);

        let st = stat(file.inode).expect("stat child");
        assert_eq!(st.size, 3);
        assert_eq!(st.file_type, FileType::Regular);
        assert_ne!(st.mode & STAT_S_IFMT, 0);
    }

    #[test]
    fn tmpfs_unlink_rmdir_and_rename_work() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let file = open("/tmp/old.txt", O_CREAT | O_RDWR, 0).expect("create /tmp/old.txt");
        let _ = write(file.inode, 0, b"x", false).expect("write old");
        rename("/tmp/old.txt", "/tmp/new.txt").expect("rename tmpfs file");
        assert_eq!(open("/tmp/old.txt", O_RDONLY, 0).unwrap_err(), ENOENT);
        let newf = open("/tmp/new.txt", O_RDONLY, 0).expect("open renamed file");
        let mut buf = [0u8; 2];
        let n = read(newf.inode, 0, &mut buf).expect("read renamed file");
        assert_eq!(&buf[..n], b"x");

        unlink("/tmp/new.txt").expect("unlink renamed file");
        assert_eq!(open("/tmp/new.txt", O_RDONLY, 0).unwrap_err(), ENOENT);

        mkdir("/tmp/dir").expect("mkdir dir");
        let _ = open("/tmp/dir/inner.txt", O_CREAT | O_RDWR, 0).expect("create nested");
        assert_eq!(rmdir("/tmp/dir").unwrap_err(), ENOTEMPTY);
        unlink("/tmp/dir/inner.txt").expect("unlink nested");
        rmdir("/tmp/dir").expect("rmdir empty");
    }

    #[test]
    fn ext2_unlink_rmdir_and_rename_persist() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let _ctx = mount_sample_ext2();

        rename("/mnt/ext2/hello.txt", "/mnt/ext2/hello2.txt").expect("rename ext2 file");
        assert_eq!(
            open("/mnt/ext2/hello.txt", O_RDONLY, 0).unwrap_err(),
            ENOENT
        );
        let renamed = open("/mnt/ext2/hello2.txt", O_RDONLY, 0).expect("open renamed ext2");
        assert!(stat(renamed.inode).unwrap().size > 0);

        let todel = open("/mnt/ext2/todel.txt", O_CREAT | O_RDWR, 0).expect("create todel");
        let _ = write(todel.inode, 0, b"remove", false).expect("write todel");
        unlink("/mnt/ext2/todel.txt").expect("unlink ext2 file");
        assert_eq!(
            open("/mnt/ext2/todel.txt", O_RDONLY, 0).unwrap_err(),
            ENOENT
        );

        mkdir("/mnt/ext2/todir").expect("mkdir ext2 dir");
        rmdir("/mnt/ext2/todir").expect("rmdir ext2 dir");
        assert_eq!(
            open("/mnt/ext2/todir", O_RDONLY | O_DIRECTORY, 0).unwrap_err(),
            ENOENT
        );

        let mut exported = std::vec![0u8; SAMPLE_EXT2.len()];
        let image_len = export_ext2_image(&mut exported).expect("export");

        init();
        let _ctx2 = mount_ext2_backend_image(&exported[..image_len], 4096);
        let _ = open("/mnt/ext2/hello2.txt", O_RDONLY, 0).expect("renamed survives remount");
        assert_eq!(
            open("/mnt/ext2/hello.txt", O_RDONLY, 0).unwrap_err(),
            ENOENT
        );
        assert_eq!(
            open("/mnt/ext2/todel.txt", O_RDONLY, 0).unwrap_err(),
            ENOENT
        );
        assert_eq!(
            open("/mnt/ext2/todir", O_RDONLY | O_DIRECTORY, 0).unwrap_err(),
            ENOENT
        );
    }

    // ---- Stock `mke2fs -t ext3` image (user/sample_ext3.img) ----
    //
    // Built by scripts/build_ext3_sample.sh with the default ext3 feature set:
    // has_journal ext_attr resize_inode dir_index filetype sparse_super
    // large_file. Contents: hello.txt, sub/nested.txt, bigdir/ (50 entries,
    // htree-indexed), xattr_block.txt (external xattr block),
    // xattr_inline.txt (in-inode xattr), large.bin (sparse 5 GiB), and
    // fastlink -> hello.txt (inline fast symlink).

    static SAMPLE_EXT3: &[u8] = include_bytes!("../../../user/sample_ext3.img");
    const EXT3_BIGDIR_ENTRIES: usize = 50;
    /// 70 bytes: too long for i_block, so it must become a slow symlink.
    const EXT3_LONG_SYMLINK_TARGET: &str =
        "../bigdir/entry_with_a_fairly_long_name_001/../../sub/nested.txt/x/y/z";

    fn mount_sample_ext3() -> std::boxed::Box<TestBackendCtx> {
        mount_ext2_backend_image(SAMPLE_EXT3, 4096)
    }

    fn ext2_raw_inode_for_path(path: &str) -> (u32, Ext2Inode) {
        let node = lookup(path).expect("lookup ext2 path");
        let ino = stat(node).expect("stat ext2 path").ino as u32;
        let inode = with_state(|state| ext2_read_inode_from_state(state, ino)).expect("read inode");
        (ino, inode)
    }

    fn ext2_free_counts() -> (u32, u32) {
        with_state(|state| {
            let blocks = ext2_read_u32_state(state, EXT2_SUPERBLOCK_OFFSET + 12)?;
            let inodes = ext2_read_u32_state(state, EXT2_SUPERBLOCK_OFFSET + 16)?;
            Ok::<_, i64>((blocks, inodes))
        })
        .expect("read superblock free counts")
    }

    fn ext2_block_in_use(block: u32) -> bool {
        with_state(|state| {
            let meta = state.ext2_meta;
            let (group, bit) = ext2_block_group_and_bit(&meta, block)?;
            let bitmap_block = ext2_group_block_bitmap_block_state(state, &meta, group)?;
            let bitmap = ext2_block_slice_state(state, bitmap_block)?;
            ext2_bitmap_get(bitmap, bit)
        })
        .expect("read block bitmap")
    }

    fn count_dir_entries(path: &str) -> usize {
        let dir = open(path, O_RDONLY | O_DIRECTORY, 0).expect("open dir");
        let mut offset = 0u64;
        let mut count = 0usize;
        while let Some(entry) = readdir(dir.inode, offset).expect("readdir") {
            let name = &entry.name[..entry.name_len];
            if name != b"." && name != b".." {
                count += 1;
            }
            offset = entry.next_offset;
        }
        count
    }

    fn read_whole(path: &str) -> std::vec::Vec<u8> {
        let opened = open(path, O_RDONLY, 0).expect("open for read");
        let mut buf = std::vec![0u8; 256];
        let n = read(opened.inode, 0, &mut buf).expect("read file");
        buf.truncate(n);
        buf
    }

    #[test]
    fn ext3_stock_image_mounts_with_default_feature_set() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let compat = read_u32_le(SAMPLE_EXT3, EXT2_SUPERBLOCK_OFFSET + 92).unwrap();
        let ro_compat = read_u32_le(SAMPLE_EXT3, EXT2_SUPERBLOCK_OFFSET + 100).unwrap();
        for bit in [
            EXT2_FEATURE_COMPAT_HAS_JOURNAL,
            EXT2_FEATURE_COMPAT_EXT_ATTR,
            EXT2_FEATURE_COMPAT_RESIZE_INO,
            EXT2_FEATURE_COMPAT_DIR_INDEX,
        ] {
            assert_ne!(compat & bit, 0, "fixture missing compat bit {bit:#x}");
        }
        assert_ne!(ro_compat & EXT2_FEATURE_RO_COMPAT_LARGE_FILE, 0);

        let _ctx = mount_sample_ext3();
        assert!(ext2_has_journal());
        assert_eq!(read_whole("/mnt/ext2/hello.txt"), b"hello ext3\n");
        assert_eq!(read_whole("/mnt/ext2/sub/nested.txt"), b"nested\n");
        assert_eq!(read_whole("/mnt/ext2/xattr_block.txt"), b"xattr block file\n");
        assert_eq!(read_whole("/mnt/ext2/xattr_inline.txt"), b"xattr inline file\n");
    }

    #[test]
    fn ext3_htree_directory_reads_linearly() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let _ctx = mount_sample_ext3();

        let (_, bigdir) = ext2_raw_inode_for_path("/mnt/ext2/bigdir");
        assert_ne!(bigdir.flags & EXT2_INDEX_FL, 0, "fixture dir should be htree-indexed");
        assert!(bigdir.size as usize > 1024, "fixture dir should span several blocks");

        assert_eq!(count_dir_entries("/mnt/ext2/bigdir"), EXT3_BIGDIR_ENTRIES);
        assert_eq!(
            read_whole("/mnt/ext2/bigdir/entry_with_a_fairly_long_name_037"),
            b"37\n"
        );
    }

    #[test]
    fn ext3_htree_insert_keeps_index_and_persists() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let _ctx = mount_sample_ext3();

        let created = open("/mnt/ext2/bigdir/added_after_mount", O_CREAT | O_RDWR, 0)
            .expect("create in htree dir");
        let _ = write(created.inode, 0, b"new", false).expect("write new entry");

        let (_, bigdir) = ext2_raw_inode_for_path("/mnt/ext2/bigdir");
        assert_ne!(bigdir.flags & EXT2_INDEX_FL, 0, "insert must keep the htree index");

        // Export is not a crash snapshot: apply the journal first so replay
        // at the next mount cannot bring back older committed copies.
        flush_ext2_dirty_blocks().expect("commit");
        checkpoint_ext2_journal().expect("checkpoint");
        let mut exported = std::vec![0u8; SAMPLE_EXT3.len()];
        let image_len = export_ext2_image(&mut exported).expect("export");
        init();
        let _ctx2 = mount_ext2_backend_image(&exported[..image_len], 4096);
        assert_eq!(count_dir_entries("/mnt/ext2/bigdir"), EXT3_BIGDIR_ENTRIES + 1);
        assert_eq!(read_whole("/mnt/ext2/bigdir/added_after_mount"), b"new");
        assert_eq!(
            read_whole("/mnt/ext2/bigdir/entry_with_a_fairly_long_name_001"),
            b"1\n"
        );
    }

    #[test]
    fn ext3_xattr_block_is_counted_and_released_on_unlink() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let _ctx = mount_sample_ext3();

        let (_, inode) = ext2_raw_inode_for_path("/mnt/ext2/xattr_block.txt");
        let xattr_block = inode.file_acl;
        assert_ne!(xattr_block, 0, "fixture file should own an xattr block");
        assert!(ext2_block_in_use(xattr_block));

        // O_TRUNC + rewrite must keep the xattr block charged in i_blocks.
        let opened = open("/mnt/ext2/xattr_block.txt", O_WRONLY | O_TRUNC, 0).expect("trunc");
        let after_trunc = stat(opened.inode).expect("stat after trunc");
        assert_eq!(after_trunc.blocks, 2, "1 KiB xattr block = 2 sectors");
        let _ = write(opened.inode, 0, b"rewritten", false).expect("rewrite");
        assert_eq!(stat(opened.inode).unwrap().blocks, 4);
        let (_, inode) = ext2_raw_inode_for_path("/mnt/ext2/xattr_block.txt");
        assert_eq!(inode.file_acl, xattr_block, "rewrite must keep i_file_acl");

        let (free_blocks_before, _) = ext2_free_counts();
        unlink("/mnt/ext2/xattr_block.txt").expect("unlink xattr file");
        assert!(!ext2_block_in_use(xattr_block), "xattr block should be freed");
        let (free_blocks_after, _) = ext2_free_counts();
        assert_eq!(free_blocks_after, free_blocks_before + 2, "data block + xattr block");
    }

    #[test]
    fn ext3_shared_xattr_block_only_drops_refcount() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        // Find the xattr block in the raw fixture and mark it shared (refcount 2).
        let _ctx = mount_sample_ext3();
        let (_, inode) = ext2_raw_inode_for_path("/mnt/ext2/xattr_block.txt");
        let xattr_block = inode.file_acl;
        init();
        let mut image = std::vec::Vec::from(SAMPLE_EXT3);
        let off = xattr_block as usize * 1024;
        assert_eq!(read_u32_le(&image, off).unwrap(), EXT2_XATTR_MAGIC);
        assert_eq!(read_u32_le(&image, off + 4).unwrap(), 1);
        write_u32_le(&mut image, off + 4, 2).unwrap();

        let _ctx = mount_ext2_backend_image(&image, 4096);
        unlink("/mnt/ext2/xattr_block.txt").expect("unlink");
        assert!(ext2_block_in_use(xattr_block), "shared xattr block must stay allocated");
        let refcount = with_state(|state| {
            let blk = ext2_block_slice_state(state, xattr_block)?;
            read_u32_le(blk, 4)
        })
        .unwrap();
        assert_eq!(refcount, 1);
    }

    #[test]
    fn ext3_fast_symlink_reads_inline_target_and_unlinks_without_freeing_blocks() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let _ctx = mount_sample_ext3();

        let mut buf = [0u8; 64];
        let n = readlinkat(root_inode(), "/mnt/ext2/fastlink", &mut buf).expect("readlink");
        assert_eq!(&buf[..n], b"hello.txt");

        let (free_blocks_before, free_inodes_before) = ext2_free_counts();
        unlink("/mnt/ext2/fastlink").expect("unlink fast symlink");
        let (free_blocks_after, free_inodes_after) = ext2_free_counts();
        assert_eq!(free_blocks_after, free_blocks_before, "fast symlink owns no blocks");
        assert_eq!(free_inodes_after, free_inodes_before + 1);
        assert_eq!(read_whole("/mnt/ext2/hello.txt"), b"hello ext3\n");

        // Short symlinks the kernel creates must be fast too (no data block).
        let (free_blocks_before, _) = ext2_free_counts();
        symlinkat(root_inode(), "sub/nested.txt", "/mnt/ext2/newfast").expect("symlink");
        assert_eq!(ext2_free_counts().0, free_blocks_before, "fast symlink allocates no block");
        let n = readlinkat(root_inode(), "/mnt/ext2/newfast", &mut buf).expect("readlink new");
        assert_eq!(&buf[..n], b"sub/nested.txt");
        unlink("/mnt/ext2/newfast").expect("unlink new fast symlink");
        assert_eq!(ext2_free_counts().0, free_blocks_before);
    }

    const GIB: u64 = 1024 * 1024 * 1024;

    /// Files past 4 GiB on 1 KiB-block ext3 (indirect maps): reads, writes
    /// and truncation use the full 64-bit size, growth stays sparse, and the
    /// result is exported for e2fsck (PEETOS_EXT3_LARGE_FSCK_IMAGE).
    #[test]
    fn ext3_large_files_use_64bit_sizes() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let ctx = mount_sample_ext3();
        set_ext2_cache_policy(EXT2_CACHE_MAX_BLOCKS_DEFAULT, 100).expect("cache policy");

        let opened = open("/mnt/ext2/large.bin", O_RDWR, 0).expect("open large file");
        assert_eq!(stat(opened.inode).unwrap().size, 5 * GIB);
        assert_eq!(seek(opened.inode, 0, 0, SEEK_END).unwrap(), 5 * GIB);
        let mut buf = [0xAAu8; 16];
        let n = read(opened.inode, 4 * GIB, &mut buf).expect("read past 4 GiB");
        assert_eq!((n, buf), (16, [0u8; 16]), "hole reads as zeros");
        let (free_before, _) = ext2_free_counts();
        let wrote = write(opened.inode, 5 * GIB + 100, b"beyond five", false).unwrap();
        assert_eq!(wrote.new_offset, 5 * GIB + 111);
        let (free_after, _) = ext2_free_counts();
        assert!(free_before - free_after <= 4, "sparse growth allocates the block and its maps");
        let n = read(opened.inode, 5 * GIB + 100, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"beyond five");
        // Past the double-indirect reach (~16 MiB... 1 KiB blocks reach
        // 16 GiB through triple-indirect tables).
        let _ = write(opened.inode, 15 * GIB, b"triple", false).unwrap();
        assert_eq!(stat(opened.inode).unwrap().size, 15 * GIB + 6);
        let max = ext2_max_file_size_for(&with_state(|s| s.ext2_meta), false).unwrap();
        assert_eq!(write(opened.inode, max, b"x", false).unwrap_err(), EFBIG);
        truncate_to(opened.inode, 5 * GIB + 105).unwrap();
        assert_eq!(stat(opened.inode).unwrap().size, 5 * GIB + 105);
        let n = read(opened.inode, 5 * GIB + 100, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"beyon");
        assert_eq!(read(opened.inode, 15 * GIB, &mut buf).unwrap(), 0);

        // A new file grows past 4 GiB too.
        let big = open("/mnt/ext2/big_new.bin", O_CREAT | O_RDWR, 0).unwrap();
        let _ = write(big.inode, 6 * GIB, b"six", false).unwrap();
        truncate_to(big.inode, 7 * GIB).unwrap();
        assert_eq!(stat(big.inode).unwrap().size, 7 * GIB);
        flush_ext2_dirty_blocks().unwrap();
        checkpoint_ext2_journal().unwrap();
        let synced = ctx.image.clone();
        if let Ok(path) = std::env::var("PEETOS_EXT3_LARGE_FSCK_IMAGE") {
            std::fs::write(&path, &synced).expect("write fsck image");
        }

        init();
        let _ctx2 = mount_ext2_backend_image(&synced, 4096);
        let big = open("/mnt/ext2/big_new.bin", O_RDONLY, 0).unwrap();
        assert_eq!(stat(big.inode).unwrap().size, 7 * GIB);
        let n = read(big.inode, 6 * GIB, &mut buf).unwrap();
        assert_eq!(&buf[..3], b"six");
        assert_eq!(n, 16);
        // O_TRUNC and unlink release everything.
        let (free_before, _) = ext2_free_counts();
        let _ = open("/mnt/ext2/big_new.bin", O_WRONLY | O_TRUNC, 0).unwrap();
        unlink("/mnt/ext2/large.bin").unwrap();
        assert!(ext2_free_counts().0 > free_before);
    }

    /// Extent files: the 32 KiB write cap is gone (dense megabytes), files
    /// grow sparsely to 100 GiB, and shrinking a sparse file skips its holes.
    /// Exported as PEETOS_EXT4_LARGE_FSCK_IMAGE.
    #[test]
    fn ext4_large_sparse_and_dense_files() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let ctx = mount_sample_ext4();
        set_ext2_cache_policy(EXT2_CACHE_MAX_BLOCKS_DEFAULT, 100).expect("cache policy");

        let dense = open("/mnt/ext2/dense.bin", O_CREAT | O_RDWR, 0).unwrap();
        let mut payload = std::vec![0u8; 1024 * 1024];
        for (i, b) in payload.iter_mut().enumerate() {
            *b = (i * 7 % 251) as u8;
        }
        let wrote = write(dense.inode, 0, &payload, false).unwrap();
        assert_eq!(wrote.written, payload.len());

        let sparse = open("/mnt/ext2/sparse.bin", O_CREAT | O_RDWR, 0).unwrap();
        let (free_before, _) = ext2_free_counts();
        let _ = write(sparse.inode, 0, b"head", false).unwrap();
        let _ = write(sparse.inode, 100 * GIB, b"tail at 100 GiB", false).unwrap();
        let _ = write(sparse.inode, 50 * GIB, b"middle", false).unwrap();
        assert!(free_before - ext2_free_counts().0 <= 4);
        assert_eq!(stat(sparse.inode).unwrap().size, 100 * GIB + 15);
        assert_eq!(stat(sparse.inode).unwrap().blocks, 3 * 8);
        truncate_to(sparse.inode, 60 * GIB).unwrap();
        assert_eq!(stat(sparse.inode).unwrap().blocks, 2 * 8);
        let mut buf = [0u8; 8];
        let n = read(sparse.inode, 50 * GIB, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"middle\0\0");
        flush_ext2_dirty_blocks().unwrap();
        checkpoint_ext2_journal().unwrap();
        let synced = ctx.image.clone();
        ext4_verify_all_checksums(&synced);
        if let Ok(path) = std::env::var("PEETOS_EXT4_LARGE_FSCK_IMAGE") {
            std::fs::write(&path, &synced).expect("write fsck image");
        }

        init();
        let _ctx2 = mount_ext2_backend_image(&synced, 4096);
        let dense = open("/mnt/ext2/dense.bin", O_RDONLY, 0).unwrap();
        let mut back = std::vec![0u8; payload.len()];
        assert_eq!(read(dense.inode, 0, &mut back).unwrap(), payload.len());
        assert!(back == payload, "dense megabyte reads back");
        let sparse = open("/mnt/ext2/sparse.bin", O_RDONLY, 0).unwrap();
        assert_eq!(stat(sparse.inode).unwrap().size, 60 * GIB);
    }

    #[test]
    fn ext4_time_encoding_matches_linux() {
        // Base is signed 32-bit seconds, the extra epoch bits add 2^32 each.
        assert_eq!(ext4_encode_time(0x9000_0000, true), (0x9000_0000, 1));
        assert_eq!(ext4_decode_time(0x9000_0000, Some(1)), 0x9000_0000);
        assert_eq!(ext4_encode_time(0x1_2345_6789, true), (0x2345_6789, 1));
        assert_eq!(ext4_decode_time(0x2345_6789, Some(1)), 0x1_2345_6789);
        assert_eq!(ext4_encode_time(1_700_000_000, true), (1_700_000_000, 0));
        // Without extra fields the time clamps to 2038; with them to 2446.
        assert_eq!(ext4_encode_time(0x9000_0000, false), (0x7FFF_FFFF, 0));
        assert_eq!(ext4_encode_time(u64::MAX >> 1, true), (0x7FFF_FFFF, 3));
        // Nanoseconds in the upper 30 bits do not change the seconds.
        assert_eq!(ext4_decode_time(5, Some((999_999_999 << 2) | 1)), 5 + (1 << 32));
    }

    /// Post-2038 times round-trip through utimensat and the extra epoch
    /// bits; inode writes keep Linux-written nanoseconds, `i_version` and the
    /// project id; new inodes get `i_crtime`. Exported as
    /// PEETOS_EXT4_TIME_FSCK_IMAGE.
    #[test]
    fn ext4_timestamps_past_2038_and_preserved_inode_fields() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        let mut image = std::vec::Vec::from(SAMPLE_EXT4_NOCSUM);
        let meta = ext4_layout(&image);
        // Inode of hello.txt: find it through a throwaway mount.
        init();
        let probe = mount_ext2_backend_image(&image, 4096);
        let hello_ino = stat(lookup("/mnt/ext2/hello.txt").unwrap()).unwrap().ino as u32;
        drop(probe);
        let off = ext2_inode_offset_in_image(&image, &meta, hello_ino).unwrap();
        // Linux-written fields the kernel must not clobber.
        image[off + 0x88..off + 0x8C].copy_from_slice(&((123_456_789u32 << 2) | 0).to_le_bytes());
        image[off + 0x24..off + 0x28].copy_from_slice(&0x1111_2222u32.to_le_bytes()); // l_i_version
        image[off + 0x98..off + 0x9C].copy_from_slice(&0x3333_4444u32.to_le_bytes()); // i_version_hi
        image[off + 0x9C..off + 0xA0].copy_from_slice(&77u32.to_le_bytes()); // i_projid
        let mtime_before = read_u32_le(&image, off + 16).unwrap();

        init();
        let ctx = mount_ext2_backend_image(&image, 4096);
        let hello = lookup("/mnt/ext2/hello.txt").unwrap();
        chmod(hello, 0o600).unwrap();
        flush_ext2_dirty_blocks().unwrap();
        checkpoint_ext2_journal().unwrap();
        let raw = ext4_raw_inode(&ctx.image, &meta, hello_ino);
        assert_eq!(read_u32_le(raw, 16).unwrap(), mtime_before, "mtime untouched by chmod");
        assert_eq!(read_u32_le(raw, 0x88).unwrap(), 123_456_789 << 2, "nanoseconds kept");
        assert_eq!(read_u32_le(raw, 0x24).unwrap(), 0x1111_2222);
        assert_eq!(read_u32_le(raw, 0x98).unwrap(), 0x3333_4444);
        assert_eq!(read_u32_le(raw, 0x9C).unwrap(), 77);

        // 2124 and 2046.
        let atime = 0x1_2345_6789u64;
        let mtime = 0x9000_0000u64;
        set_times(hello, atime, mtime).unwrap();
        let st = stat(hello).unwrap();
        assert_eq!((st.atime, st.mtime), (atime, mtime));
        let created = open("/mnt/ext2/born.txt", O_CREAT | O_RDWR, 0).unwrap();
        let born_ino = stat(created.inode).unwrap().ino as u32;
        flush_ext2_dirty_blocks().unwrap();
        checkpoint_ext2_journal().unwrap();
        let raw = ext4_raw_inode(&ctx.image, &meta, hello_ino);
        assert_eq!(read_u32_le(raw, 8).unwrap(), 0x2345_6789);
        assert_eq!(read_u32_le(raw, 0x8C).unwrap() & 3, 1, "atime epoch bits");
        assert_eq!(read_u32_le(raw, 16).unwrap(), 0x9000_0000);
        assert_eq!(read_u32_le(raw, 0x88).unwrap(), 1, "new mtime: epoch 1, no stale nanoseconds");
        assert_eq!(read_u32_le(raw, 0x9C).unwrap(), 77, "project id survives");
        let born = ext4_raw_inode(&ctx.image, &meta, born_ino);
        assert_eq!(read_u32_le(born, 0x90).unwrap(), read_u32_le(born, 12).unwrap(), "crtime = ctime");
        let synced = ctx.image.clone();
        if let Ok(path) = std::env::var("PEETOS_EXT4_TIME_FSCK_IMAGE") {
            std::fs::write(&path, &synced).expect("write fsck image");
        }

        init();
        let _ctx2 = mount_ext2_backend_image(&synced, 4096);
        let st = stat(lookup("/mnt/ext2/hello.txt").unwrap()).unwrap();
        assert_eq!((st.atime, st.mtime), (atime, mtime), "decoded after remount");
    }

    fn last_orphan(image: &[u8]) -> u32 {
        read_u32_le(image, EXT2_SUPERBLOCK_OFFSET + 0xE8).unwrap()
    }

    /// Unlink while open on the stock ext4 image: the data stays readable
    /// through the open node, the inode sits on the orphan list, the last
    /// release frees it. A crash in between leaves an orphan that the next
    /// mount (and e2fsck) finish. Images: PEETOS_EXT4_ORPHAN_CRASH_IMAGE
    /// (unrecovered), PEETOS_EXT4_ORPHAN_FSCK_IMAGE (after release).
    #[test]
    fn ext4_unlink_while_open_uses_the_orphan_list() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let ctx = mount_sample_ext4();
        set_ext2_cache_policy(EXT2_CACHE_MAX_BLOCKS_DEFAULT, 100).expect("cache policy");
        let (free_blocks0, free_inodes0) = ext2_free_counts();
        let f = open("/mnt/ext2/sub/doomed", O_CREAT | O_RDWR, 0).unwrap();
        let data = std::vec![0x5Au8; 3 * 4096];
        let _ = write(f.inode, 0, &data, false).unwrap();
        let ino = stat(f.inode).unwrap().ino as u32;
        inode_open_retain(f.inode);
        unlink("/mnt/ext2/sub/doomed").unwrap();
        assert_eq!(lookup("/mnt/ext2/sub/doomed"), Err(ENOENT));
        let mut buf = std::vec![0u8; data.len()];
        assert_eq!(read(f.inode, 0, &mut buf).unwrap(), data.len(), "still readable");
        assert!(buf == data);
        let _ = write(f.inode, 0, b"still writable", false).unwrap();
        assert_eq!(stat(f.inode).unwrap().nlink, 0);
        assert!(ext2_inode_in_use(ino), "inode kept while open");
        assert!(ext2_free_counts().0 < free_blocks0);
        flush_ext2_dirty_blocks().expect("commit without checkpoint");
        assert_eq!(last_orphan(&ctx.image), 0, "home superblock still old (journaled)");
        let crashed = ctx.image.clone();
        if let Ok(path) = std::env::var("PEETOS_EXT4_ORPHAN_CRASH_IMAGE") {
            std::fs::write(&path, &crashed).expect("write crash image");
        }

        // Normal path: the last release frees everything.
        assert!(inode_open_release(f.inode));
        assert!(!ext2_inode_in_use(ino));
        assert_eq!(ext2_free_counts(), (free_blocks0, free_inodes0));
        flush_ext2_dirty_blocks().unwrap();
        checkpoint_ext2_journal().unwrap();
        assert_eq!(last_orphan(&ctx.image), 0);
        ext4_verify_all_checksums(&ctx.image);
        if let Ok(path) = std::env::var("PEETOS_EXT4_ORPHAN_FSCK_IMAGE") {
            std::fs::write(&path, &ctx.image).expect("write fsck image");
        }

        // Crash path: replay brings back the orphan, mount finishes it.
        init();
        let ctx2 = mount_ext2_backend_image(&crashed, 4096);
        assert!(!ext2_inode_in_use(ino), "mount freed the orphan");
        assert_eq!(ext2_free_counts(), (free_blocks0, free_inodes0));
        flush_ext2_dirty_blocks().unwrap();
        checkpoint_ext2_journal().unwrap();
        assert_eq!(last_orphan(&ctx2.image), 0);
        ext4_verify_all_checksums(&ctx2.image);
    }

    #[test]
    fn ext3_open_hard_links_and_directories_outlive_their_names() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let _ctx = mount_sample_ext3();
        let (free_blocks0, free_inodes0) = ext2_free_counts();

        // Two names; the open one is removed first, so the second unlink
        // drops the last link while the inode is still in use.
        let a = open("/mnt/ext2/sub/a", O_CREAT | O_RDWR, 0).unwrap();
        let _ = write(a.inode, 0, b"shared", false).unwrap();
        linkat(root_inode(), "/mnt/ext2/sub/a", root_inode(), "/mnt/ext2/sub/b").unwrap();
        inode_open_retain(a.inode);
        unlink("/mnt/ext2/sub/a").unwrap();
        let ino = stat(a.inode).unwrap().ino as u32;
        unlink("/mnt/ext2/sub/b").unwrap();
        assert!(ext2_inode_in_use(ino));
        assert_eq!(read_whole_inode(a.inode), b"shared");
        assert_eq!(with_state(|s| last_orphan_state(s)), ino);
        assert!(inode_open_release(a.inode));
        assert!(!ext2_inode_in_use(ino));
        assert_eq!(with_state(|s| last_orphan_state(s)), 0);

        // rmdir of a directory that is still referenced (an open fd or a
        // working directory): it disappears, takes no new entries, and is
        // freed on release.
        mkdir("/mnt/ext2/sub/gone").unwrap();
        let dir = lookup("/mnt/ext2/sub/gone").unwrap();
        inode_open_retain(dir);
        rmdir("/mnt/ext2/sub/gone").unwrap();
        assert_eq!(lookup("/mnt/ext2/sub/gone"), Err(ENOENT));
        assert_eq!(openat(dir, "new", O_CREAT | O_RDWR, 0).unwrap_err(), ENOENT);
        assert_eq!(mkdirat(dir, "child"), Err(ENOENT));
        let mut buf = [0u8; VFS_MAX_PATH];
        assert_eq!(inode_path(dir, &mut buf), Err(ENOENT));
        assert!(inode_open_release(dir));
        assert_eq!(ext2_free_counts(), (free_blocks0, free_inodes0));
        assert_eq!(with_state(|s| last_orphan_state(s)), 0);
        // Releasing an unreferenced or stale handle is harmless.
        assert!(!inode_open_release(dir));
        assert!(!inode_open_release(60000));
    }

    fn last_orphan_state(state: &mut VfsState) -> u32 {
        ext2_read_u32_state(state, EXT2_SB_LAST_ORPHAN).unwrap()
    }

    fn read_whole_inode(inode: u16) -> std::vec::Vec<u8> {
        let mut buf = std::vec![0u8; 256];
        let n = read(inode, 0, &mut buf).unwrap();
        buf.truncate(n);
        buf
    }

    #[test]
    fn tmpfs_unlink_while_open_keeps_data_until_release() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let f = open("/tmp/open_unlinked", O_CREAT | O_RDWR, 0).unwrap();
        let _ = write(f.inode, 0, b"tmp data", false).unwrap();
        inode_open_retain(f.inode);
        unlink("/tmp/open_unlinked").unwrap();
        assert_eq!(lookup("/tmp/open_unlinked"), Err(ENOENT));
        assert_eq!(read_whole_inode(f.inode), b"tmp data");
        // The slot is not reused while referenced.
        let other = open("/tmp/other", O_CREAT | O_RDWR, 0).unwrap();
        assert_ne!(other.inode, f.inode);
        assert!(inode_open_release(f.inode), "last release frees the tmpfs node");
        assert_eq!(stat(f.inode), Err(ENOENT));
    }

    /// An orphan with links left is a truncate that did not finish: the next
    /// mount trims the blocks past i_size. Exported as
    /// PEETOS_EXT4_TRUNC_ORPHAN_FSCK_IMAGE.
    #[test]
    fn mount_finishes_truncate_orphans() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        let mut image = std::vec::Vec::from(SAMPLE_EXT4_NOCSUM);
        init();
        let probe = mount_ext2_backend_image(&image, 4096);
        let frag = lookup("/mnt/ext2/frag.bin").unwrap();
        let frag_ino = stat(frag).unwrap().ino as u32;
        let blocks_before = stat(frag).unwrap().blocks;
        drop(probe);
        let meta = ext4_layout(&image);
        let off = ext2_inode_offset_in_image(&image, &meta, frag_ino).unwrap();
        image[off + 4..off + 8].copy_from_slice(&(2 * 4096u32).to_le_bytes());
        image[off + 20..off + 24].copy_from_slice(&0u32.to_le_bytes());
        let sb = EXT2_SUPERBLOCK_OFFSET + 0xE8;
        image[sb..sb + 4].copy_from_slice(&frag_ino.to_le_bytes());

        init();
        let ctx = mount_ext2_backend_image(&image, 4096);
        let frag = lookup("/mnt/ext2/frag.bin").unwrap();
        let st = stat(frag).unwrap();
        assert_eq!(st.size, 2 * 4096);
        assert!(st.blocks < blocks_before, "blocks past i_size were freed");
        assert!(st.blocks <= 3 * 8, "two data blocks (+ at most an extent leaf)");
        flush_ext2_dirty_blocks().unwrap();
        checkpoint_ext2_journal().unwrap();
        assert_eq!(last_orphan(&ctx.image), 0);
        if let Ok(path) = std::env::var("PEETOS_EXT4_TRUNC_ORPHAN_FSCK_IMAGE") {
            std::fs::write(&path, &ctx.image).expect("write fsck image");
        }
    }

    static SAMPLE_EXT4_ORPHAN: &[u8] = include_bytes!("../../../user/sample_ext4_orphan.img");

    /// `orphan_file` (newer mkfs.ext4 default): the image mounts read-write,
    /// mkfs-written orphan blocks pass the magic/checksum check, recorded
    /// orphans are finished at mount and `orphan_present` cleared, and a
    /// damaged orphan block fails the mount. Exported as
    /// PEETOS_EXT4_ORPHAN_FILE_FSCK_IMAGE.
    #[test]
    fn ext4_orphan_file_entries_are_processed_at_mount() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let _clean = mount_ext2_backend_image(SAMPLE_EXT4_ORPHAN, 4096);
        assert!(!ext2_mount_read_only());
        let doomed = open("/mnt/ext2/sub/orphaned", O_CREAT | O_RDWR, 0).unwrap();
        let _ = write(doomed.inode, 0, &[7u8; 8192], false).unwrap();
        let doomed_ino = stat(doomed.inode).unwrap().ino as u32;
        let (free_blocks0, free_inodes0) = (ext2_free_counts().0, ext2_free_counts().1);
        // Drop the name but keep the inode, as a crash would leave it.
        inode_open_retain(doomed.inode);
        unlink("/mnt/ext2/sub/orphaned").unwrap();
        flush_ext2_dirty_blocks().unwrap();
        checkpoint_ext2_journal().unwrap();
        let mut image = _clean.image.clone();
        // Move the orphan from the s_last_orphan list into the orphan file
        // (where Linux records it with orphan_file).
        let sb = EXT2_SUPERBLOCK_OFFSET;
        assert_eq!(last_orphan(&image), doomed_ino);
        image[sb + 0xE8..sb + 0xEC].fill(0);
        let meta = ext4_layout(&image);
        let csum = ext4_csum_params(&image);
        let off = ext2_inode_offset_in_image(&image, &meta, doomed_ino).unwrap();
        image[off + 20..off + 24].fill(0);
        let fixed = csum.inode(doomed_ino, &image[off..off + meta.inode_size]).unwrap();
        let (lo, hi) = (fixed as u16, (fixed >> 16) as u16);
        image[off + 0x7C..off + 0x7E].copy_from_slice(&lo.to_le_bytes());
        image[off + 0x82..off + 0x84].copy_from_slice(&hi.to_le_bytes());
        let orphan_ino = read_u32_le(&image, sb + 0x280).unwrap();
        let oraw = ext4_raw_inode(&image, &meta, orphan_ino).to_vec();
        let ogen = read_u32_le(&oraw, 0x64).unwrap();
        let seed = csum.inode_seed_for(orphan_ino, ogen);
        let first = read_u32_le(&oraw, 0x28 + 12 + 8).unwrap() as usize * 4096;
        assert!(
            read_u32_le(&image, first + 4088).unwrap() == EXT4_ORPHAN_BLOCK_MAGIC
                && read_u32_le(&image, first + 4092).unwrap()
                    == ext4_orphan_block_csum(seed, (first / 4096) as u64, &image[first..first + 4096]),
            "mkfs-written orphan block verifies"
        );
        image[first + 40..first + 44].copy_from_slice(&doomed_ino.to_le_bytes());
        let value = ext4_orphan_block_csum(seed, (first / 4096) as u64, &image[first..first + 4096]);
        image[first + 4092..first + 4096].copy_from_slice(&value.to_le_bytes());
        let ro = read_u32_le(&image, sb + 100).unwrap() | EXT4_FEATURE_RO_COMPAT_ORPHAN_PRESENT;
        image[sb + 100..sb + 104].copy_from_slice(&ro.to_le_bytes());
        let sbsum = ext4_csum::superblock_csum(&image[sb..sb + 1024]).unwrap();
        image[sb + 0x3FC..sb + 0x400].copy_from_slice(&sbsum.to_le_bytes());

        // A damaged orphan block fails the mount.
        let mut bad = image.clone();
        bad[first + 100] ^= 1;
        init();
        assert_eq!(mount_ext2_backend_image_expect_err(&bad, 4096), EIO);

        init();
        let ctx = mount_ext2_backend_image(&image, 4096);
        assert!(!ext2_inode_in_use(doomed_ino), "orphan finished at mount");
        let _ = (free_blocks0, free_inodes0);
        flush_ext2_dirty_blocks().unwrap();
        checkpoint_ext2_journal().unwrap();
        assert_eq!(read_u32_le(&ctx.image, sb + 100).unwrap() & EXT4_FEATURE_RO_COMPAT_ORPHAN_PRESENT, 0);
        assert_eq!(read_u32_le(&ctx.image, first + 40).unwrap(), 0, "entry cleared");
        assert_eq!(
            read_u32_le(&ctx.image, first + 4092).unwrap(),
            ext4_orphan_block_csum(seed, (first / 4096) as u64, &ctx.image[first..first + 4096])
        );
        ext4_verify_all_checksums(&ctx.image);
        if let Ok(path) = std::env::var("PEETOS_EXT4_ORPHAN_FILE_FSCK_IMAGE") {
            std::fs::write(&path, &ctx.image).expect("write fsck image");
        }
    }

    /// The first file over 2 GiB turns on `large_file`, as Linux does.
    #[test]
    fn large_file_feature_is_enabled_on_demand() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        let mut image = std::vec::Vec::from(SAMPLE_EXT4_NOCSUM);
        let sb = EXT2_SUPERBLOCK_OFFSET;
        let ro = read_u32_le(&image, sb + 100).unwrap() & !EXT2_FEATURE_RO_COMPAT_LARGE_FILE;
        image[sb + 100..sb + 104].copy_from_slice(&ro.to_le_bytes());
        init();
        let ctx = mount_ext2_backend_image(&image, 4096);
        let f = open("/mnt/ext2/small_then_big", O_CREAT | O_RDWR, 0).unwrap();
        let _ = write(f.inode, GIB, b"1 GiB", false).unwrap();
        flush_ext2_dirty_blocks().unwrap();
        checkpoint_ext2_journal().unwrap();
        assert_eq!(read_u32_le(&ctx.image, sb + 100).unwrap() & EXT2_FEATURE_RO_COMPAT_LARGE_FILE, 0);
        let _ = write(f.inode, 3 * GIB, b"3 GiB", false).unwrap();
        flush_ext2_dirty_blocks().unwrap();
        checkpoint_ext2_journal().unwrap();
        assert_ne!(read_u32_le(&ctx.image, sb + 100).unwrap() & EXT2_FEATURE_RO_COMPAT_LARGE_FILE, 0);
    }

    /// Regression: freeing blocks that were dirtied in the same ordered
    /// transaction (large.bin's indirect tables) used to leave a revoke-only
    /// remainder in the pending queue, so a later batched flush failed with
    /// EIO. Freed blocks are now forgotten (revoke only) and the queue drains.
    #[test]
    fn ext3_freed_dirty_blocks_do_not_wedge_pending_transactions() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let _ctx = mount_sample_ext3();

        unlink("/mnt/ext2/large.bin").expect("unlink large file");
        let (freed_revokes, freed_tracked) = with_state(|state| {
            let tx = state.ext2_journal_pending_txs.last().expect("sealed unlink tx");
            (tx.revoke_blocks.len(), tx.metadata_blocks.clone())
        });
        assert_eq!(freed_revokes, 4, "1 data block + 3 indirect tables revoked");
        let revoked =
            with_state(|state| state.ext2_journal_pending_txs.last().unwrap().revoke_blocks.clone());
        for block in &revoked {
            assert!(
                !freed_tracked.contains(block),
                "freed block {block} must not also be journaled"
            );
        }

        mkdir("/mnt/ext2/sub/after_free").expect("mkdir reusing freed blocks");
        flush_ext2_dirty_blocks().expect("batched flush must not wedge");
        assert!(with_state(|state| state.ext2_journal_pending_txs.is_empty()));
        checkpoint_ext2_journal().expect("checkpoint");
    }

    fn image_incompat(image: &[u8]) -> u32 {
        read_u32_le(image, EXT2_SUPERBLOCK_OFFSET + 96).unwrap()
    }

    /// Journal superblock `s_start` straight from a raw image (the journal
    /// superblock is the first block of the journal inode, inode 8).
    fn image_journal_start(image: &[u8]) -> u32 {
        let meta = ext2_parse_meta_with_len(image, image.len(), true).unwrap();
        let ino_off = ext2_inode_offset_in_image(image, &meta, meta.journal_inode).unwrap();
        let first_block = read_u32_le(image, ino_off + 40).unwrap() as usize;
        read_u32_be(image, first_block * meta.block_size + 28)
    }

    #[test]
    fn ext3_recover_flag_tracks_live_journal_transactions() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let ctx = mount_sample_ext3();
        assert_eq!(image_incompat(&ctx.image) & EXT2_FEATURE_INCOMPAT_RECOVER, 0);

        let created = open("/mnt/ext2/recover.txt", O_CREAT | O_RDWR, 0).expect("create");
        let _ = write(created.inode, 0, b"journaled", false).expect("write");
        flush_ext2_dirty_blocks().expect("flush");

        // Committed but not checkpointed: disk must say "needs recovery".
        assert_ne!(image_journal_start(&ctx.image), 0);
        assert_ne!(image_incompat(&ctx.image) & EXT2_FEATURE_INCOMPAT_RECOVER, 0);
        assert!(with_state(|state| ext2_journal::recover_marked(state)));

        checkpoint_ext2_journal().expect("checkpoint");
        assert_eq!(image_journal_start(&ctx.image), 0);
        assert_eq!(image_incompat(&ctx.image) & EXT2_FEATURE_INCOMPAT_RECOVER, 0);
        assert!(!with_state(|state| ext2_journal::recover_marked(state)));

        // A later commit marks it again.
        let _ = write(created.inode, 0, b"journaled again", false).expect("rewrite");
        flush_ext2_dirty_blocks().expect("flush again");
        assert_ne!(image_incompat(&ctx.image) & EXT2_FEATURE_INCOMPAT_RECOVER, 0);
    }

    /// Simulated crash: commit transactions to the journal but never
    /// checkpoint, then treat the backend image as the disk after power loss.
    /// The kernel's own mount must replay it; when `PEETOS_EXT3_CRASH_IMAGE`
    /// is set the image is written there so tests/ext3_fsck.sh can have
    /// e2fsck (Linux's jbd2 recovery code) replay it too.
    #[test]
    fn ext3_crash_image_needs_recovery_and_replays() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let ctx = mount_sample_ext3();

        let created = open("/mnt/ext2/bigdir/crash_new", O_CREAT | O_RDWR, 0).expect("create");
        let _ = write(created.inode, 0, b"survives crash", false).expect("write");
        unlink("/mnt/ext2/xattr_block.txt").expect("unlink xattr file");
        unlink("/mnt/ext2/large.bin").expect("unlink large file");
        mkdir("/mnt/ext2/sub/crash_dir").expect("mkdir");
        symlinkat(root_inode(), "../hello.txt", "/mnt/ext2/sub/crash_link").expect("symlink");
        flush_ext2_dirty_blocks().expect("flush without checkpoint");

        let crashed = ctx.image.clone();
        assert_ne!(image_incompat(&crashed) & EXT2_FEATURE_INCOMPAT_RECOVER, 0);
        assert_ne!(image_journal_start(&crashed), 0);
        if let Ok(path) = std::env::var("PEETOS_EXT3_CRASH_IMAGE") {
            std::fs::write(&path, &crashed).expect("write crash image");
        }

        init();
        let ctx2 = mount_ext2_backend_image(&crashed, 4096);
        assert_eq!(image_incompat(&ctx2.image) & EXT2_FEATURE_INCOMPAT_RECOVER, 0);
        assert_eq!(image_journal_start(&ctx2.image), 0);
        assert_eq!(read_whole("/mnt/ext2/bigdir/crash_new"), b"survives crash");
        assert_eq!(open("/mnt/ext2/xattr_block.txt", O_RDONLY, 0).unwrap_err(), ENOENT);
        assert_eq!(open("/mnt/ext2/large.bin", O_RDONLY, 0).unwrap_err(), ENOENT);
        let _ = open("/mnt/ext2/sub/crash_dir", O_RDONLY | O_DIRECTORY, 0).expect("dir replayed");
        let mut buf = [0u8; 32];
        let n = readlinkat(root_inode(), "/mnt/ext2/sub/crash_link", &mut buf).unwrap();
        assert_eq!(&buf[..n], b"../hello.txt");
    }

    fn ext2_inode_in_use(ino: u32) -> bool {
        with_state(|state| {
            let meta = state.ext2_meta;
            let (group, bit) = ext2_inode_group_and_bit(&meta, ino)?;
            let bitmap_block = ext2_group_inode_bitmap_block_state(state, &meta, group)?;
            let bitmap = ext2_block_slice_state(state, bitmap_block)?;
            ext2_bitmap_get(bitmap, bit)
        })
        .expect("read inode bitmap")
    }

    fn flush_pending_txs(count: usize) {
        with_state(|state| {
            let target = state.ext2_journal_pending_txs.len().saturating_sub(count);
            while state.ext2_journal_pending_txs.len() > target {
                let flushed = ext2_journal::flush_oldest_ordered_pending_tx(state, 64)
                    .expect("flush oldest pending tx");
                assert!(flushed > 0, "pending queue must make progress");
            }
        });
    }

    /// Risk: committing an older transaction used to strip shared metadata
    /// blocks (inode bitmap, group descriptors, superblock, parent directory)
    /// from every *newer* pending transaction, so the newer transaction
    /// committed without its own changes to them. A crash right after that
    /// commit replayed a file whose inode was not marked in-use.
    #[test]
    fn ext3_newer_transaction_keeps_its_snapshot_of_shared_blocks() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let ctx = mount_sample_ext3();
        // Hold both sealed transactions in the pending queue until the test
        // flushes them explicitly.
        set_ext2_cache_policy(EXT2_CACHE_MAX_BLOCKS_DEFAULT, 100).expect("cache policy");

        let a = open("/mnt/ext2/sub/shared_a", O_CREAT | O_RDWR, 0).expect("create a");
        let b = open("/mnt/ext2/sub/shared_b", O_CREAT | O_RDWR, 0).expect("create b");
        let b_ino = stat(b.inode).unwrap().ino as u32;
        let _ = a;

        let inode_bitmap = with_state(|state| {
            let meta = state.ext2_meta;
            let (group, _) = ext2_inode_group_and_bit(&meta, b_ino).unwrap();
            ext2_group_inode_bitmap_block_state(state, &meta, group).unwrap()
        });
        let owners = with_state(|state| {
            state
                .ext2_journal_pending_txs
                .iter()
                .filter(|tx| tx.metadata_blocks.binary_search(&inode_bitmap).is_ok())
                .count()
        });
        assert_eq!(owners, 2, "both creates must journal the shared inode bitmap");

        flush_pending_txs(1);
        let newer_still_owns = with_state(|state| {
            state.ext2_journal_pending_txs.iter().any(|tx| {
                tx.metadata_blocks.binary_search(&inode_bitmap).is_ok()
                    && tx.metadata_snapshots.iter().any(|s| s.home_block == inode_bitmap)
            })
        });
        assert!(newer_still_owns, "newer transaction lost its inode bitmap snapshot");

        // Commit the newer transaction too, then "crash" before any further
        // writeback or checkpoint.
        flush_pending_txs(1);
        let crashed = ctx.image.clone();
        init();
        let _ctx2 = mount_ext2_backend_image(&crashed, 4096);
        let _ = open("/mnt/ext2/sub/shared_b", O_RDONLY, 0).expect("b replayed");
        assert!(ext2_inode_in_use(b_ino), "replayed inode must be marked in the bitmap");
    }

    /// Risk: a block journaled by an older committed transaction, then freed
    /// (revoked) and reused as ordered file data by a newer one, used to be
    /// overwritten with the stale journaled copy when the older transaction
    /// was checkpointed.
    #[test]
    fn ext3_checkpoint_does_not_overwrite_reused_freed_block() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let ctx = mount_sample_ext3();

        // f1 spills into an indirect table (journaled metadata).
        let f1 = open("/mnt/ext2/sub/reuse_f1", O_CREAT | O_RDWR, 0).expect("create f1");
        let _ = write(f1.inode, 0, &[0x11u8; 13 * 1024], false).expect("write f1");
        flush_ext2_dirty_blocks().expect("commit f1");
        let (_, f1_inode) = ext2_raw_inode_for_path("/mnt/ext2/sub/reuse_f1");
        let table_block = f1_inode.blocks[12];
        assert_ne!(table_block, 0);
        let journaled = with_state(|state| {
            state
                .ext2_journal_committed_txs
                .iter()
                .any(|tx| tx.updates.iter().any(|u| u.home_block == table_block))
        });
        assert!(journaled, "indirect table should sit in a committed, uncheckpointed tx");

        // Free everything (revokes the table), then reuse the freed range as
        // file data. A 1-block file first shifts allocation by one so the old
        // table block becomes a *data* block of f2.
        unlink("/mnt/ext2/sub/reuse_f1").expect("unlink f1");
        let f0 = open("/mnt/ext2/sub/reuse_f0", O_CREAT | O_RDWR, 0).expect("create f0");
        let _ = write(f0.inode, 0, b"x", false).expect("write f0");
        let f2 = open("/mnt/ext2/sub/reuse_f2", O_CREAT | O_RDWR, 0).expect("create f2");
        let _ = write(f2.inode, 0, &[0x22u8; 13 * 1024], false).expect("write f2");
        flush_ext2_dirty_blocks().expect("commit reuse");

        let (_, f2_inode) = ext2_raw_inode_for_path("/mnt/ext2/sub/reuse_f2");
        assert!(
            f2_inode.blocks[..12].contains(&table_block),
            "test setup: old table block {table_block} should be reused as f2 data"
        );

        checkpoint_ext2_journal().expect("checkpoint");
        let mut buf = std::vec![0u8; 13 * 1024];
        let reopened = open("/mnt/ext2/sub/reuse_f2", O_RDONLY, 0).unwrap();
        let n = read(reopened.inode, 0, &mut buf).unwrap();
        assert_eq!(n, buf.len());
        assert!(buf.iter().all(|&b| b == 0x22), "live view corrupted by checkpoint");

        let synced = ctx.image.clone();
        init();
        let _ctx2 = mount_ext2_backend_image(&synced, 4096);
        let reopened = open("/mnt/ext2/sub/reuse_f2", O_RDONLY, 0).unwrap();
        let n = read(reopened.inode, 0, &mut buf).unwrap();
        assert_eq!(n, buf.len());
        assert!(buf.iter().all(|&b| b == 0x22), "on-disk f2 data overwritten by checkpoint");
    }

    /// One transaction larger than a descriptor block's 126 tags must commit
    /// atomically as a single journal transaction with several descriptor
    /// blocks (it used to be torn into independent commits), and replay.
    #[test]
    fn ext3_large_transaction_spans_descriptor_blocks_and_replays() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let ctx = mount_sample_ext3();
        set_ext2_cache_policy(EXT2_CACHE_MAX_BLOCKS_DEFAULT, 100).expect("cache policy");

        const COUNT: usize = 200;
        let mut targets = std::vec::Vec::new();
        let mut block = 2047u32;
        while targets.len() < COUNT {
            if !ext2_block_in_use(block) {
                targets.push(block);
            }
            block -= 1;
        }
        targets.sort_unstable();
        let pattern = |b: u32| (b % 251) as u8 ^ 0x5A;

        with_state(|state| {
            for &b in &targets {
                ext2_block_slice_mut_state(state, b).unwrap().fill(pattern(b));
                ext2_mark_dirty_block(state, b).unwrap();
            }
            let committed = ext2_journal::flush_dirty_batch(state, 1000).expect("commit");
            assert_eq!(committed, COUNT);
            assert_eq!(state.ext2_journal_committed_txs.len(), 1, "one atomic transaction");
            // 200 data blocks + 2 descriptors (126 tags each) + 1 commit.
            assert_eq!(state.ext2_journal_committed_txs[0].log_block_count, 203);
        });

        let crashed = ctx.image.clone();
        if let Ok(path) = std::env::var("PEETOS_EXT3_MULTIDESC_IMAGE") {
            std::fs::write(&path, &crashed).expect("write multi-descriptor image");
            let list: std::string::String = targets
                .iter()
                .map(|b| std::format!("{} {}\n", b, pattern(*b)))
                .collect();
            std::fs::write(std::format!("{path}.blocks"), list).expect("write block list");
        }

        init();
        let ctx2 = mount_ext2_backend_image(&crashed, 4096);
        for &b in &targets {
            let off = b as usize * 1024;
            assert!(
                ctx2.image[off..off + 1024].iter().all(|&x| x == pattern(b)),
                "block {b} not replayed"
            );
        }
    }

    // ---- Journal checksums, features, and barrier cost ----

    fn image_journal_super_off(image: &[u8]) -> usize {
        let meta = ext2_parse_meta_with_len(image, image.len(), true).unwrap();
        let ino_off = ext2_inode_offset_in_image(image, &meta, meta.journal_inode).unwrap();
        let flags = read_u32_le(image, ino_off + 32).unwrap();
        let first = if (flags & ext2_bmap::EXT4_EXTENTS_FL) != 0 {
            // Depth-0 extent root: first extent's ee_start (lo at +8, hi at +6).
            assert_eq!(read_u16_le(image, ino_off + 40 + 6).unwrap(), 0);
            let lo = read_u32_le(image, ino_off + 40 + 12 + 8).unwrap() as u64;
            let hi = read_u16_le(image, ino_off + 40 + 12 + 6).unwrap() as u64;
            (hi << 32) | lo
        } else {
            read_u32_le(image, ino_off + 40).unwrap() as u64
        };
        first as usize * meta.block_size
    }

    #[test]
    fn ext3_crc32_be_matches_jbd2_definition() {
        // CRC-32/MPEG-2 check value: Linux crc32_be(~0, "123456789").
        assert_eq!(ext2_journal::crc32_be_for_test(!0, b"123456789"), 0x0376_E6E7);
    }

    #[test]
    fn ext3_mount_enables_journal_checksums_and_commits_carry_crc32() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let jsb = image_journal_super_off(SAMPLE_EXT3);
        assert_eq!(read_u32_be(SAMPLE_EXT3, jsb + 36) & 1, 0, "stock journal has no checksum");

        let ctx = mount_sample_ext3();
        assert!(with_state(|state| ext2_journal::checksum_enabled_for_test(state)));
        assert_eq!(read_u32_be(&ctx.image, jsb + 36) & 1, 1, "COMPAT_CHECKSUM persisted");

        let created = open("/mnt/ext2/csum.txt", O_CREAT | O_RDWR, 0).expect("create");
        let _ = write(created.inode, 0, b"checksummed", false).expect("write");
        flush_ext2_dirty_blocks().expect("flush");

        // Recompute each committed transaction's CRC from the raw log and
        // compare with its commit block.
        let txs: std::vec::Vec<(u32, u32)> = with_state(|state| {
            state
                .ext2_journal_committed_txs
                .iter()
                .map(|tx| (tx.descriptor_jblock, tx.log_block_count))
                .collect()
        });
        assert!(!txs.is_empty());
        for (descriptor_jblock, log_blocks) in txs {
            let mut crc = !0u32;
            let mut commit_fs = 0u32;
            for i in 0..log_blocks {
                let fs_block = with_state(|state| {
                    ext2_journal::journal_fs_block_for_test(state, descriptor_jblock + i)
                })
                .unwrap() as usize;
                let blk = &ctx.image[fs_block * 1024..fs_block * 1024 + 1024];
                let block_type = read_u32_be(blk, 4);
                if i + 1 == log_blocks {
                    assert_eq!(block_type, 2, "last log block is the commit");
                    commit_fs = fs_block as u32;
                } else if block_type != 5 {
                    // Descriptors and journaled data count; revoke blocks don't.
                    crc = ext2_journal::crc32_be_for_test(crc, blk);
                }
            }
            let commit = &ctx.image[commit_fs as usize * 1024..commit_fs as usize * 1024 + 1024];
            assert_eq!(commit[12], 1, "h_chksum_type = JBD2_CRC32_CHKSUM");
            assert_eq!(commit[13], 4, "h_chksum_size");
            assert_eq!(read_u32_be(commit, 16), crc, "commit CRC32");
        }
    }

    /// A journaled block damaged after its commit (a torn or misdirected
    /// write the barrier could not prevent) must stop replay at that
    /// transaction instead of writing garbage home.
    #[test]
    fn ext3_corrupted_journal_block_stops_replay_at_that_transaction() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let ctx = mount_sample_ext3();
        set_ext2_cache_policy(EXT2_CACHE_MAX_BLOCKS_DEFAULT, 100).expect("cache policy");

        let a = open("/mnt/ext2/sub/csum_a", O_CREAT | O_RDWR, 0).expect("create a");
        let _ = write(a.inode, 0, b"intact", false).expect("write a");
        flush_ext2_dirty_blocks().expect("commit a");
        let before_b = with_state(|state| state.ext2_journal_committed_txs.len());

        let _b = open("/mnt/ext2/sub/csum_b", O_CREAT | O_RDWR, 0).expect("create b");
        flush_ext2_dirty_blocks().expect("commit b");
        let victim_fs = with_state(|state| {
            let tx = &state.ext2_journal_committed_txs[before_b];
            let jblock = tx.updates[0].journal_block;
            ext2_journal::journal_fs_block_for_test(state, jblock)
        })
        .unwrap() as usize;

        let mut crashed = ctx.image.clone();
        crashed[victim_fs * 1024 + 100] ^= 0xFF;
        if let Ok(path) = std::env::var("PEETOS_EXT3_CORRUPT_IMAGE") {
            std::fs::write(&path, &crashed).expect("write corrupt image");
        }

        init();
        let _ctx2 = mount_ext2_backend_image(&crashed, 4096);
        assert_eq!(read_whole("/mnt/ext2/sub/csum_a"), b"intact", "earlier tx replays");
        assert_eq!(
            open("/mnt/ext2/sub/csum_b", O_RDONLY, 0).unwrap_err(),
            ENOENT,
            "transaction with a bad checksum must not replay"
        );
    }

    #[test]
    fn ext3_mount_rejects_unknown_journal_incompat_features() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        // ASYNC_COMMIT and FAST_COMMIT stay unsupported.
        for bit in [0x4u32, 0x20] {
            init();
            let mut image = std::vec::Vec::from(SAMPLE_EXT3);
            let jsb = image_journal_super_off(&image);
            let incompat = read_u32_be(&image, jsb + 40);
            image[jsb + 40..jsb + 44].copy_from_slice(&(incompat | bit).to_be_bytes());
            assert_eq!(mount_ext2_backend_image_expect_err(&image, 4096), EINVAL, "bit {bit:#x}");
        }
    }

    /// The post-commit barrier is deferred to the end of the flush: N commits
    /// cost N + 1 device flushes instead of 2N.
    #[test]
    fn ext3_flush_issues_one_barrier_per_commit_plus_one() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let ctx = mount_sample_ext3();
        set_ext2_cache_policy(EXT2_CACHE_MAX_BLOCKS_DEFAULT, 100).expect("cache policy");
        for name in ["/mnt/ext2/sub/bar_1", "/mnt/ext2/sub/bar_2", "/mnt/ext2/sub/bar_3"] {
            let _ = open(name, O_CREAT | O_RDWR, 0).expect("create");
        }
        let commits_before = with_state(|state| state.ext2_journal.sequence);
        let flushes_before = ctx.flushes;
        flush_ext2_dirty_blocks().expect("flush");
        let commits = with_state(|state| state.ext2_journal.sequence) - commits_before;
        assert!(commits >= 3);
        assert_eq!(ctx.flushes - flushes_before, commits as usize + 1);
        assert!(!with_state(|state| state.ext2_journal.commit_barrier_pending));
    }

    fn ext2_data_blocks_of(path: &str) -> std::vec::Vec<u32> {
        let (_, inode) = ext2_raw_inode_for_path(path);
        inode.blocks[..12].iter().copied().filter(|&b| b != 0).collect()
    }

    /// Found by the QEMU journal smoke: with commits running ahead of
    /// checkpoints, checkpointing only the oldest transaction copied its
    /// older bitmap snapshot back into the (clean, newer) cache, and the next
    /// allocation reused inodes/blocks that a later transaction owned.
    #[test]
    fn ext3_partial_checkpoint_does_not_roll_back_cached_bitmaps() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let _ctx = mount_sample_ext3();
        set_ext2_cache_policy(EXT2_CACHE_MAX_BLOCKS_DEFAULT, 100).expect("cache policy");

        let f1 = open("/mnt/ext2/sub/roll_1", O_CREAT | O_RDWR, 0).expect("create 1");
        let _ = write(f1.inode, 0, &[1u8; 2048], false).expect("write 1");
        flush_ext2_dirty_blocks().expect("commit 1");
        let committed_after_first = with_state(|state| state.ext2_journal_committed_txs.len());
        let f2 = open("/mnt/ext2/sub/roll_2", O_CREAT | O_RDWR, 0).expect("create 2");
        let _ = write(f2.inode, 0, &[2u8; 2048], false).expect("write 2");
        flush_ext2_dirty_blocks().expect("commit 2");

        // Journal-full style partial checkpoint: only the first file's txs.
        with_state(|state| ext2_journal::checkpoint_oldest_for_test(state, committed_after_first))
            .expect("partial checkpoint");

        let f3 = open("/mnt/ext2/sub/roll_3", O_CREAT | O_RDWR, 0).expect("create 3");
        let _ = write(f3.inode, 0, &[3u8; 2048], false).expect("write 3");
        let inos = [f1, f2, f3].map(|f| stat(f.inode).unwrap().ino);
        assert!(inos[0] != inos[1] && inos[1] != inos[2] && inos[0] != inos[2], "{inos:?}");
        let b2 = ext2_data_blocks_of("/mnt/ext2/sub/roll_2");
        let b3 = ext2_data_blocks_of("/mnt/ext2/sub/roll_3");
        assert!(b3.iter().all(|b| !b2.contains(b)), "roll_3 reused roll_2 blocks: {b2:?} {b3:?}");
    }

    /// Same corruption through cache eviction: a clean block whose newest
    /// version is only in the journal was dropped and re-read stale from its
    /// home location.
    #[test]
    fn ext3_evicting_uncheckpointed_block_does_not_resurrect_stale_bitmap() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let _ctx = mount_sample_ext3();
        set_ext2_cache_policy(EXT2_CACHE_MAX_BLOCKS_DEFAULT, 100).expect("cache policy");

        let f1 = open("/mnt/ext2/sub/evict_1", O_CREAT | O_RDWR, 0).expect("create 1");
        let _ = write(f1.inode, 0, &[1u8; 2048], false).expect("write 1");
        flush_ext2_dirty_blocks().expect("commit 1");
        assert!(with_state(|state| !state.ext2_journal_committed_txs.is_empty()));

        // Squeeze the cache so the committed (clean) bitmaps get evicted.
        set_ext2_cache_policy(4, 100).expect("tiny cache");
        set_ext2_cache_policy(EXT2_CACHE_MAX_BLOCKS_DEFAULT, 100).expect("restore cache");

        let f2 = open("/mnt/ext2/sub/evict_2", O_CREAT | O_RDWR, 0).expect("create 2");
        let _ = write(f2.inode, 0, &[2u8; 2048], false).expect("write 2");
        assert_ne!(stat(f1.inode).unwrap().ino, stat(f2.inode).unwrap().ino);
        let b1 = ext2_data_blocks_of("/mnt/ext2/sub/evict_1");
        let b2 = ext2_data_blocks_of("/mnt/ext2/sub/evict_2");
        assert!(b2.iter().all(|b| !b1.contains(b)), "evict_2 reused evict_1 blocks");
        let mut buf = std::vec![0u8; 4096];
        let reopened = open("/mnt/ext2/sub/evict_1", O_RDONLY, 0).unwrap();
        let n = read(reopened.inode, 0, &mut buf).unwrap();
        assert_eq!(&buf[..n], &[1u8; 2048][..]);
    }

    // ---- Volatile write-cache crash model ----

    enum VolatileEvent {
        Write(u32, std::vec::Vec<u8>),
        Barrier,
    }

    /// Backend whose writes sit in a volatile cache until a barrier drains
    /// them. `current` serves reads; `events` lets a test rebuild the durable
    /// state and the undrained writes at any point in time.
    struct VolatileCtx {
        current: std::vec::Vec<u8>,
        events: std::vec::Vec<VolatileEvent>,
    }

    fn volatile_read(ctx: u64, block: u32, out: &mut [u8]) -> Result<(), i64> {
        let ctx = unsafe { &mut *(ctx as *mut VolatileCtx) };
        let start = block as usize * out.len();
        out.copy_from_slice(ctx.current.get(start..start + out.len()).ok_or(EIO)?);
        Ok(())
    }

    fn volatile_write(ctx: u64, block: u32, data: &[u8]) -> Result<(), i64> {
        let ctx = unsafe { &mut *(ctx as *mut VolatileCtx) };
        let start = block as usize * data.len();
        ctx.current
            .get_mut(start..start + data.len())
            .ok_or(EIO)?
            .copy_from_slice(data);
        ctx.events.push(VolatileEvent::Write(block, std::vec::Vec::from(data)));
        Ok(())
    }

    fn volatile_flush(ctx: u64) -> Result<(), i64> {
        let ctx = unsafe { &mut *(ctx as *mut VolatileCtx) };
        ctx.events.push(VolatileEvent::Barrier);
        Ok(())
    }

    /// Disk contents after a crash at event `t`, where the cache managed to
    /// write back only the newest `keep_newest` undrained writes.
    fn volatile_crash_image(
        base: &[u8],
        events: &[VolatileEvent],
        t: usize,
        keep_newest: usize,
    ) -> std::vec::Vec<u8> {
        let mut durable = std::vec::Vec::from(base);
        let mut pending: std::vec::Vec<(u32, &[u8])> = std::vec::Vec::new();
        for event in &events[..t] {
            match event {
                VolatileEvent::Write(block, data) => pending.push((*block, data.as_slice())),
                VolatileEvent::Barrier => {
                    for (block, data) in pending.drain(..) {
                        let off = block as usize * data.len();
                        durable[off..off + data.len()].copy_from_slice(data);
                    }
                }
            }
        }
        let skip = pending.len().saturating_sub(keep_newest);
        for (block, data) in &pending[skip..] {
            let off = *block as usize * data.len();
            durable[off..off + data.len()].copy_from_slice(data);
        }
        durable
    }

    fn undrained_at(events: &[VolatileEvent], t: usize) -> usize {
        let mut pending = 0usize;
        for event in &events[..t] {
            match event {
                VolatileEvent::Write(..) => pending += 1,
                VolatileEvent::Barrier => pending = 0,
            }
        }
        pending
    }

    /// Crash at every backend write of a create/write/unlink/mkdir + sync
    /// sequence, with a reordering cache that may persist only the newest
    /// undrained writes. Every crash image must mount, recover, and show each
    /// operation either fully applied or not at all. With the journal's write
    /// barriers this holds; without them a commit record can reach the disk
    /// ahead of the journaled blocks it seals.
    #[test]
    fn ext3_crash_at_every_write_with_reordering_cache_stays_consistent() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        crash_at_every_write(SAMPLE_EXT3, "PEETOS_EXT3_CRASH_DIR");
    }

    /// The same crash model on a writable ext4 image (extents, flex_bg,
    /// uninit_bg/gdt_csum): extent tree rewrites, lazy group initialization
    /// and descriptor checksums must all be crash-consistent.
    #[test]
    fn ext4_crash_at_every_write_with_reordering_cache_stays_consistent() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        crash_at_every_write(SAMPLE_EXT4_NOCSUM, "PEETOS_EXT4_CRASH_DIR");
    }

    fn crash_at_every_write(base: &[u8], dump_env: &str) {
        fn workload() {
            let created = open("/mnt/ext2/sub/crash_v", O_CREAT | O_RDWR, 0).expect("create");
            let _ = write(created.inode, 0, CRASH_PAYLOAD, false).expect("write");
            unlink("/mnt/ext2/xattr_block.txt").expect("unlink");
            mkdir("/mnt/ext2/sub/crash_vdir").expect("mkdir");
        }
        fn check(t: usize, keep: usize) {
            match open("/mnt/ext2/sub/crash_v", O_RDONLY, 0) {
                Ok(opened) => {
                    let mut buf = [0u8; 64];
                    let n = read(opened.inode, 0, &mut buf).expect("read crash_v");
                    assert!(
                        n == 0 || &buf[..n] == CRASH_PAYLOAD,
                        "t={t} keep={keep}: torn file contents {:?}",
                        &buf[..n]
                    );
                    let ino = stat(opened.inode).unwrap().ino as u32;
                    assert!(ext2_inode_in_use(ino), "t={t} keep={keep}: inode not in bitmap");
                }
                Err(err) => assert_eq!(err, ENOENT, "t={t} keep={keep}"),
            }
        }
        crash_at_every_write_with(base, dump_env, workload, check);
    }

    const CRASH_PAYLOAD: &[u8] = b"crash-consistent payload";

    /// Runs `workload` on a reordering write cache, then for every crash
    /// point recovers the image and runs `check(t, keep)` on it.
    fn crash_at_every_write_with(
        base: &[u8],
        dump_env: &str,
        workload: fn(),
        check: fn(usize, usize),
    ) {
        init();
        let mut vctx = std::boxed::Box::new(VolatileCtx {
            current: std::vec::Vec::from(base),
            events: std::vec::Vec::new(),
        });
        set_ext2_block_backend(Ext2BlockBackend {
            ctx: (&mut *vctx as *mut VolatileCtx) as u64,
            read_block: volatile_read,
            write_block: volatile_write,
            flush: Some(volatile_flush),
        })
        .expect("volatile backend");
        mount_ext2_block_backend("/mnt/ext2", base.len(), &base[..4096]).expect("mount volatile");
        workload();
        flush_ext2_dirty_blocks().expect("flush");
        checkpoint_ext2_journal().expect("checkpoint");
        let events = std::mem::take(&mut vctx.events);
        assert!(events.iter().any(|e| matches!(e, VolatileEvent::Barrier)));

        let dump_dir = std::env::var(dump_env).ok();
        let mut images = 0usize;
        for t in 0..=events.len() {
            if t > 0 && !matches!(events[t - 1], VolatileEvent::Write(..)) {
                continue;
            }
            let undrained = undrained_at(&events, t);
            let mut choices = std::vec![0, undrained, undrained / 2, 1];
            choices.retain(|&k| k <= undrained);
            choices.sort_unstable();
            choices.dedup();
            for keep in choices {
                let crashed = volatile_crash_image(base, &events, t, keep);
                init();
                let ctx = mount_ext2_backend_image(&crashed, 4096);
                check(t, keep);
                if let Some(dir) = &dump_dir {
                    std::fs::write(std::format!("{dir}/crash_{t:04}_{keep:03}.img"), &ctx.image)
                        .expect("dump recovered image");
                }
                images += 1;
            }
        }
        assert!(images > 20, "expected many crash points, got {images}");
    }

    /// Applies a representative mutation mix to the stock ext3 image, syncs
    /// it, and checks it remounts. When `PEETOS_EXT3_FSCK_IMAGE` is set the
    /// synced image is written there so tests/ext3_fsck.sh can run
    /// `e2fsck -fn` on it.
    #[test]
    fn ext3_stock_image_mutations_sync_and_remount() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let ctx = mount_sample_ext3();

        let created = open("/mnt/ext2/bigdir/fsck_new", O_CREAT | O_RDWR, 0).expect("create");
        let _ = write(created.inode, 0, b"fsck", false).expect("write");
        unlink("/mnt/ext2/bigdir/entry_with_a_fairly_long_name_010").expect("unlink in htree");
        unlink("/mnt/ext2/xattr_block.txt").expect("unlink xattr block file");
        let inline = open("/mnt/ext2/xattr_inline.txt", O_WRONLY | O_TRUNC, 0).expect("trunc");
        let _ = write(inline.inode, 0, b"inline rewritten", false).expect("rewrite inline");
        unlink("/mnt/ext2/fastlink").expect("unlink fast symlink");
        unlink("/mnt/ext2/large.bin").expect("unlink large file");
        mkdir("/mnt/ext2/sub/newdir").expect("mkdir");
        symlinkat(root_inode(), "../hello.txt", "/mnt/ext2/sub/newlink").expect("symlink");
        symlinkat(root_inode(), EXT3_LONG_SYMLINK_TARGET, "/mnt/ext2/sub/longlink")
            .expect("slow symlink");
        renameat(
            root_inode(),
            "/mnt/ext2/sub/nested.txt",
            root_inode(),
            "/mnt/ext2/bigdir/nested_moved.txt",
        )
        .expect("rename into htree dir");

        flush_ext2_dirty_blocks().expect("flush");
        checkpoint_ext2_journal().expect("checkpoint");
        let synced = ctx.image.clone();

        if let Ok(path) = std::env::var("PEETOS_EXT3_FSCK_IMAGE") {
            std::fs::write(&path, &synced).expect("write fsck image");
        }

        init();
        let _ctx2 = mount_ext2_backend_image(&synced, 4096);
        assert_eq!(read_whole("/mnt/ext2/bigdir/fsck_new"), b"fsck");
        assert_eq!(read_whole("/mnt/ext2/bigdir/nested_moved.txt"), b"nested\n");
        assert_eq!(read_whole("/mnt/ext2/xattr_inline.txt"), b"inline rewritten");
        let mut link_buf = [0u8; 64];
        let n = readlinkat(root_inode(), "/mnt/ext2/sub/newlink", &mut link_buf).unwrap();
        assert_eq!(&link_buf[..n], b"../hello.txt");
        let mut long_buf = [0u8; 128];
        let n = readlinkat(root_inode(), "/mnt/ext2/sub/longlink", &mut long_buf).unwrap();
        assert_eq!(&long_buf[..n], EXT3_LONG_SYMLINK_TARGET.as_bytes());
        assert_eq!(count_dir_entries("/mnt/ext2/bigdir"), EXT3_BIGDIR_ENTRIES + 1);
        assert_eq!(open("/mnt/ext2/large.bin", O_RDONLY, 0).unwrap_err(), ENOENT);
    }

    // ---- ext4 M0: fixtures, descriptor layer, checksums, block-map seam ----
    //
    // user/sample_ext4.img (stock mkfs.ext4) and user/sample_ext4_nocsum.img
    // (^metadata_csum,uninit_bg) come from scripts/build_ext4_sample.sh: four
    // 1024-block groups, flex_bg (all bitmaps/inode tables in group 0), groups
    // 1-3 INODE_UNINIT (group 1 also BLOCK_UNINIT), 4 KiB blocks, 64-byte
    // descriptors, 256-byte inodes; frag.bin (inode 76) has a depth-1 extent
    // tree. Nothing ext4-specific mounts yet; these tests read raw images.

    static SAMPLE_EXT4: &[u8] = include_bytes!("../../../user/sample_ext4.img");
    static SAMPLE_EXT4_NOCSUM: &[u8] = include_bytes!("../../../user/sample_ext4_nocsum.img");
    const EXT4_FRAG_INO: u32 = 76;

    fn ext4_layout(image: &[u8]) -> Ext2Meta {
        ext2_parse_layout_with_len(image, image.len(), true).expect("parse ext4 layout")
    }

    fn ext4_csum_params(image: &[u8]) -> ext4_csum::Ext4Csum {
        ext4_csum::Ext4Csum::from_superblock(
            &image[EXT2_SUPERBLOCK_OFFSET..EXT2_SUPERBLOCK_OFFSET + 1024],
        )
        .expect("checksum params")
    }

    fn ext4_desc<'a>(image: &'a [u8], meta: &Ext2Meta, group: u32) -> &'a [u8] {
        let off = ext2_group_desc_offset(meta, group).unwrap();
        &image[off..off + meta.desc_size]
    }

    fn ext4_raw_inode<'a>(image: &'a [u8], meta: &Ext2Meta, ino: u32) -> &'a [u8] {
        let off = ext2_inode_offset_in_image(image, meta, ino).unwrap();
        &image[off..off + meta.inode_size]
    }

    fn ext4_block<'a>(image: &'a [u8], meta: &Ext2Meta, block: u32) -> &'a [u8] {
        let off = block as usize * meta.block_size;
        &image[off..off + meta.block_size]
    }

    /// Allocated inodes: initialized groups, below the unused tail, bitmap set.
    fn ext4_allocated_inodes(image: &[u8], meta: &Ext2Meta) -> std::vec::Vec<u32> {
        let mut out = std::vec::Vec::new();
        for group in 0..meta.group_count {
            let desc = ext4_desc(image, meta, group);
            let flags = read_u16_le(desc, 0x12).unwrap();
            if (flags & ext4_csum::EXT4_BG_INODE_UNINIT) != 0 {
                continue;
            }
            let unused = read_u16_le(desc, 0x1C).unwrap() as u32
                | ((read_u16_le(desc, 0x32).unwrap() as u32) << 16);
            let bitmap_block = ext2_gd_location(image, meta, group, 4).unwrap();
            let bitmap = ext4_block(image, meta, bitmap_block);
            for idx in 0..meta.inodes_per_group.saturating_sub(unused) {
                if ext2_bitmap_get(bitmap, idx).unwrap() {
                    out.push(group * meta.inodes_per_group + idx + 1);
                }
            }
        }
        out
    }

    #[test]
    fn ext4_crc_known_answers() {
        // CRC-32C check value: standard init/final inversion around crc32c_le.
        assert_eq!(!ext4_csum::crc32c(!0, b"123456789"), 0xE306_9283);
        // CRC-16/ARC check value (poly 0xA001 reflected, init 0).
        assert_eq!(ext4_csum::crc16(0, b"123456789"), 0xBB3D);
    }

    // ---- ext4 M1: read-only mount ----

    static EXT4_DIAG_REPORTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn ext4_count_diag(_what: &'static str, _id: u64) {
        EXT4_DIAG_REPORTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    fn bigdir_name(i: usize) -> std::string::String {
        let mut name = std::format!("entry_{i:03}_");
        while name.len() < 60 {
            name.push('x');
        }
        name
    }

    fn read_all(path: &str, max: usize) -> std::vec::Vec<u8> {
        let opened = open(path, O_RDONLY, 0).expect("open for read");
        let mut buf = std::vec![0u8; max];
        let mut total = 0usize;
        loop {
            let n = read(opened.inode, total as u64, &mut buf[total..]).expect("read");
            if n == 0 || total + n == max {
                total += n;
                break;
            }
            total += n;
        }
        buf.truncate(total);
        buf
    }

    #[test]
    fn ext4_images_mount_read_only_and_read_every_file() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        // Read every file through a read-only mount (the stock image) and a
        // writable one (the uninit_bg image); both are writable ext4 since M3.
        for (image, read_only) in [(SAMPLE_EXT4, true), (SAMPLE_EXT4_NOCSUM, false)] {
            init();
            let ctx = if read_only {
                mount_ext2_backend_image_ro(image)
            } else {
                mount_ext2_backend_image(image, 4096)
            };
            assert_eq!(ext2_fs_name(), "ext4");
            assert_eq!(ext2_mount_read_only(), read_only);

            assert_eq!(read_whole("/mnt/ext2/hello.txt"), b"hello ext4\n");
            assert_eq!(read_whole("/mnt/ext2/sub/nested.txt"), b"nested\n");
            assert_eq!(read_whole("/mnt/ext2/xattr_block.txt"), b"xattr block file\n");
            let mut buf = [0u8; 32];
            let n = readlinkat(root_inode(), "/mnt/ext2/fastlink", &mut buf).unwrap();
            assert_eq!(&buf[..n], b"hello.txt");

            // htree directory, read linearly.
            assert_eq!(count_dir_entries("/mnt/ext2/bigdir"), 62);
            for i in [1usize, 37, 62] {
                let path = std::format!("/mnt/ext2/bigdir/{}", bigdir_name(i));
                assert_eq!(read_whole(&path), std::format!("{i}\n").as_bytes());
            }

            // frag.bin: depth-1 extent tree; data every other 4 KiB block.
            let frag = read_all("/mnt/ext2/frag.bin", 64 * 4096);
            assert_eq!(frag.len(), 38 * 4096 + 12);
            for blk in 0..39usize {
                let chunk = &frag[blk * 4096..core::cmp::min(frag.len(), (blk + 1) * 4096)];
                if blk % 2 == 0 {
                    let expected = std::format!("fragment {:02}\n", blk / 2);
                    assert_eq!(&chunk[..12], expected.as_bytes(), "block {blk}");
                    assert!(chunk[12..].iter().all(|&b| b == 0));
                } else {
                    assert!(chunk.iter().all(|&b| b == 0), "hole {blk}");
                }
            }
            let frag_stat = stat(open("/mnt/ext2/frag.bin", O_RDONLY, 0).unwrap().inode).unwrap();
            assert_eq!(frag_stat.blocks, 21 * 8, "20 data blocks + 1 extent leaf");

            let large = open("/mnt/ext2/large.bin", O_RDONLY, 0).unwrap();
            assert_eq!(stat(large.inode).unwrap().size, 5 * 1024 * 1024 * 1024);
            // 64-bit sizes (M5): the sparse 5 GiB file reads its holes.
            let n = read(large.inode, 4 * 1024 * 1024 * 1024 + 7, &mut buf).unwrap();
            assert!(n > 0 && buf[..n].iter().all(|&b| b == 0));

            if read_only {
                assert_eq!(ctx.writes, 0, "a read-only mount must not write");
            }
        }
    }

    #[test]
    fn ext4_read_only_mount_refuses_every_mutation() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let ctx = mount_ext2_backend_image_ro(SAMPLE_EXT4);
        assert_eq!(open("/mnt/ext2/new.txt", O_CREAT | O_RDWR, 0).unwrap_err(), EROFS);
        assert_eq!(mkdir("/mnt/ext2/newdir").unwrap_err(), EROFS);
        let hello = open("/mnt/ext2/hello.txt", O_RDONLY, 0).unwrap();
        assert_eq!(write(hello.inode, 0, b"x", false).unwrap_err(), EROFS);
        assert_eq!(truncate(hello.inode).unwrap_err(), EROFS);
        assert_eq!(truncate_to(hello.inode, 1).unwrap_err(), EROFS);
        assert_eq!(chmod(hello.inode, 0o600).unwrap_err(), EROFS);
        assert_eq!(chown(hello.inode, 1, 1).unwrap_err(), EROFS);
        assert_eq!(set_times(hello.inode, 1, 1).unwrap_err(), EROFS);
        assert_eq!(unlink("/mnt/ext2/hello.txt").unwrap_err(), EROFS);
        assert_eq!(rmdir("/mnt/ext2/sub").unwrap_err(), EROFS);
        assert_eq!(
            renameat(root_inode(), "/mnt/ext2/hello.txt", root_inode(), "/mnt/ext2/h2").unwrap_err(),
            EROFS
        );
        assert_eq!(
            linkat(root_inode(), "/mnt/ext2/hello.txt", root_inode(), "/mnt/ext2/h3").unwrap_err(),
            EROFS
        );
        assert_eq!(
            symlinkat(root_inode(), "hello.txt", "/mnt/ext2/l2").unwrap_err(),
            EROFS
        );
        let mut mounts = [0u8; 512];
        let proc_mounts = open("/proc/mounts", O_RDONLY, 0).unwrap();
        let n = read(proc_mounts.inode, 0, &mut mounts).unwrap();
        let text = core::str::from_utf8(&mounts[..n]).unwrap();
        assert!(text.contains("ext4 /mnt/ext2 ext4 ro"), "{text}");
        assert_eq!(ctx.writes, 0);
    }

    fn mount_sample_ext4() -> std::boxed::Box<TestBackendCtx> {
        mount_ext2_backend_image(SAMPLE_EXT4, 4096)
    }

    fn ext4_ino_of(path: &str) -> u32 {
        init();
        let _ctx = mount_sample_ext4();
        stat(lookup(path).unwrap()).unwrap().ino as u32
    }

    /// Each metadata_csum structure, corrupted by one byte, makes the access
    /// that reads it fail with EIO and reports it through the diag hook.
    #[test]
    fn ext4_checksum_mismatches_return_eio() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        let meta = ext4_layout(SAMPLE_EXT4);
        let hello_ino = ext4_ino_of("/mnt/ext2/hello.txt");
        let sub_ino = ext4_ino_of("/mnt/ext2/sub");

        let expect_diag = |before: u64| {
            assert!(EXT4_DIAG_REPORTS.load(std::sync::atomic::Ordering::SeqCst) > before);
        };
        let diag_now = || EXT4_DIAG_REPORTS.load(std::sync::atomic::Ordering::SeqCst);

        // Superblock.
        let mut image = std::vec::Vec::from(SAMPLE_EXT4);
        image[EXT2_SUPERBLOCK_OFFSET + 0x78] ^= 1;
        init();
        set_ext2_diag_hook(Some(ext4_count_diag));
        let before = diag_now();
        assert_eq!(mount_ext2_backend_image_expect_err(&image, 4096), EIO);
        expect_diag(before);

        // Group descriptor (crc32c) and the crc16 variant.
        for source in [SAMPLE_EXT4, SAMPLE_EXT4_NOCSUM] {
            let mut image = std::vec::Vec::from(source);
            let off = ext2_group_desc_offset(&meta, 2).unwrap();
            image[off + 0x0E] ^= 1; // free inodes count
            init();
            set_ext2_diag_hook(Some(ext4_count_diag));
            assert_eq!(mount_ext2_backend_image_expect_err(&image, 4096), EIO);
        }

        // Inode: lookup imports the child and verifies it.
        let mut image = std::vec::Vec::from(SAMPLE_EXT4);
        let off = ext2_inode_offset_in_image(&image, &meta, hello_ino).unwrap();
        image[off + 0x08] ^= 1; // atime
        init();
        set_ext2_diag_hook(Some(ext4_count_diag));
        let before = diag_now();
        let _ctx = mount_ext2_backend_image(&image, 4096);
        assert_eq!(open("/mnt/ext2/hello.txt", O_RDONLY, 0).unwrap_err(), EIO);
        expect_diag(before);

        // Extent leaf block of frag.bin.
        let mut image = std::vec::Vec::from(SAMPLE_EXT4);
        let raw = ext4_raw_inode(SAMPLE_EXT4, &meta, EXT4_FRAG_INO);
        let leaf = read_u32_le(raw, 0x28 + 12 + 4).unwrap() as usize;
        image[leaf * 4096 + 12] ^= 1;
        init();
        set_ext2_diag_hook(Some(ext4_count_diag));
        let before = diag_now();
        let _ctx = mount_ext2_backend_image(&image, 4096);
        let frag = open("/mnt/ext2/frag.bin", O_RDONLY, 0).unwrap();
        let mut buf = [0u8; 16];
        assert_eq!(read(frag.inode, 0, &mut buf).unwrap_err(), EIO);
        expect_diag(before);

        // Directory leaf block of /sub.
        let mut image = std::vec::Vec::from(SAMPLE_EXT4);
        let raw = ext4_raw_inode(SAMPLE_EXT4, &meta, sub_ino);
        let dir_block = read_u32_le(raw, 0x28 + 12 + 8).unwrap() as usize;
        image[dir_block * 4096 + 20] ^= 1;
        init();
        set_ext2_diag_hook(Some(ext4_count_diag));
        let before = diag_now();
        let _ctx = mount_ext2_backend_image(&image, 4096);
        assert_eq!(open("/mnt/ext2/sub/nested.txt", O_RDONLY, 0).unwrap_err(), EIO);
        expect_diag(before);
        set_ext2_diag_hook(None);
    }

    // ---- ext4 M1: recovery of Linux-format (64bit + csum v3) journals ----
    //
    // user/sample_ext4_journal.img: the stock fixture plus three unrecovered
    // transactions written by debugfs (journal_64bit, journal_checksum_v3):
    // tx1 3500=J 3501=K, tx2 revokes 3501, tx3 3502=L. The journal inode maps
    // journal blocks 0..1023 to filesystem blocks 2048..3071, so tx1 is
    // jblocks 1 (descriptor) 2-3 (data) 4 (commit), tx2 5-6, tx3 7-9.

    static SAMPLE_EXT4_JOURNAL: &[u8] = include_bytes!("../../../user/sample_ext4_journal.img");
    const EXT4_JOURNAL_FS_BASE: usize = 2048;

    fn ext4_jblock_off(jblock: usize) -> usize {
        (EXT4_JOURNAL_FS_BASE + jblock) * 4096
    }

    fn ext4_block_is(image: &[u8], block: usize, byte: u8) -> bool {
        image[block * 4096..(block + 1) * 4096].iter().all(|&b| b == byte)
    }

    #[test]
    fn ext4_linux_format_journal_replays_on_read_only_mount() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        assert!(ext4_block_is(SAMPLE_EXT4_JOURNAL, 3500, 0), "fixture: not yet replayed");
        let incompat = read_u32_le(SAMPLE_EXT4_JOURNAL, EXT2_SUPERBLOCK_OFFSET + 96).unwrap();
        assert_ne!(incompat & EXT2_FEATURE_INCOMPAT_RECOVER, 0);

        init();
        let ctx = mount_ext2_backend_image_ro(SAMPLE_EXT4_JOURNAL);
        assert!(ext2_mount_read_only());
        assert!(ext4_block_is(&ctx.image, 3500, b'J'), "tx1 replayed");
        assert!(ext4_block_is(&ctx.image, 3501, 0), "tx2 revoked tx1's copy of 3501");
        assert!(ext4_block_is(&ctx.image, 3502, b'L'), "revoke-only tx2 did not end the log");

        // Recovery finished cleanly: no needs_recovery, empty log, and both
        // superblock checksums still valid after the kernel's writes.
        let sb = &ctx.image[EXT2_SUPERBLOCK_OFFSET..EXT2_SUPERBLOCK_OFFSET + 1024];
        assert_eq!(read_u32_le(sb, 96).unwrap() & EXT2_FEATURE_INCOMPAT_RECOVER, 0);
        assert!(ext4_csum::superblock_csum_ok(sb).unwrap(), "fs superblock checksum");
        let jsb = &ctx.image[ext4_jblock_off(0)..ext4_jblock_off(0) + 4096];
        assert_eq!(read_u32_be(jsb, 28), 0, "journal s_start");
        assert_eq!(
            ext2_journal::journal_super_csum_for_test(jsb).unwrap(),
            read_u32_be(jsb, 0xFC),
            "journal superblock checksum"
        );
        assert_eq!(read_whole("/mnt/ext2/hello.txt"), b"hello ext4\n");
        if let Ok(path) = std::env::var("PEETOS_EXT4_REPLAYED_IMAGE") {
            std::fs::write(&path, &ctx.image).expect("write replayed image");
        }

        // A second mount of the recovered image has nothing to replay.
        let recovered = ctx.image.clone();
        init();
        let ctx2 = mount_ext2_backend_image(&recovered, 4096);
        assert_eq!(ctx2.writes, 0);
    }

    #[test]
    fn ext4_journal_checksum_failures_follow_jbd2_rules() {
        let _guard = TEST_LOCK.lock().expect("lock should work");

        // Data block (tx1, jblock 2) or descriptor (jblock 1): recovery fails
        // and nothing is written; the image still needs recovery.
        for jblock in [2usize, 1] {
            let mut image = std::vec::Vec::from(SAMPLE_EXT4_JOURNAL);
            image[ext4_jblock_off(jblock) + 100] ^= 1;
            init();
            assert_eq!(mount_ext2_backend_image_expect_err(&image, 4096), EIO, "jblock {jblock}");
        }

        // Commit block of tx3 (jblock 9): the log ends after tx2.
        let mut image = std::vec::Vec::from(SAMPLE_EXT4_JOURNAL);
        image[ext4_jblock_off(9) + 100] ^= 1;
        init();
        let ctx = mount_ext2_backend_image(&image, 4096);
        assert!(ext4_block_is(&ctx.image, 3500, b'J'));
        assert!(ext4_block_is(&ctx.image, 3501, 0));
        assert!(ext4_block_is(&ctx.image, 3502, 0), "tx3 had a bad commit checksum");

        // Journal superblock.
        let mut image = std::vec::Vec::from(SAMPLE_EXT4_JOURNAL);
        image[ext4_jblock_off(0) + 0x40] ^= 1; // s_nr_users
        init();
        assert_eq!(mount_ext2_backend_image_expect_err(&image, 4096), EIO);
    }

    /// A read-write mount keeps writing the journal's own format: here an ext3
    /// journal switched to 64bit-less CSUM_V3 gets v3 transactions from the
    /// kernel, which the kernel's recovery (and e2fsck) can replay.
    #[test]
    fn ext3_read_write_mount_writes_csum_v3_transactions() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        let mut image = std::vec::Vec::from(SAMPLE_EXT3);
        let jsb = image_journal_super_off(&image);
        let incompat = read_u32_be(&image, jsb + 0x28);
        image[jsb + 0x28..jsb + 0x2C].copy_from_slice(&(incompat | 0x10).to_be_bytes());
        image[jsb + 0x50] = 4; // crc32c
        let csum = ext2_journal::journal_super_csum_for_test(&image[jsb..jsb + 1024]).unwrap();
        image[jsb + 0xFC..jsb + 0x100].copy_from_slice(&csum.to_be_bytes());
        init();
        let ctx = mount_ext2_backend_image(&image, 4096);
        assert!(!ext2_mount_read_only());
        let created = open("/mnt/ext2/v3.txt", O_CREAT | O_RDWR, 0).expect("create");
        let _ = write(created.inode, 0, b"v3 transaction", false).expect("write");
        flush_ext2_dirty_blocks().expect("commit");
        let crashed = ctx.image.clone();
        init();
        let _ctx2 = mount_ext2_backend_image(&crashed, 4096);
        assert_eq!(read_whole("/mnt/ext2/v3.txt"), b"v3 transaction");
    }

    // ---- ext4 M2: read-write on ext4 without metadata_csum ----

    fn mount_ext4_nocsum() -> std::boxed::Box<TestBackendCtx> {
        let ctx = mount_ext2_backend_image(SAMPLE_EXT4_NOCSUM, 4096);
        set_ext2_cache_policy(EXT2_CACHE_MAX_BLOCKS_DEFAULT, 100).expect("cache policy");
        ctx
    }

    fn raw_extents(path: &str) -> (u32, u16, u16) {
        // (i_flags, root depth, root entries) of the live inode.
        let (_, inode) = ext2_raw_inode_for_path(path);
        let root = {
            let mut r = [0u8; 60];
            for (i, w) in inode.blocks.iter().enumerate() {
                r[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
            }
            r
        };
        (
            inode.flags,
            u16::from_le_bytes([root[6], root[7]]),
            u16::from_le_bytes([root[2], root[3]]),
        )
    }

    fn write_blocks(path: &str, pattern: impl Fn(usize) -> u8, blocks: core::ops::Range<usize>) {
        let opened = open(path, O_CREAT | O_RDWR, 0).expect("open for write");
        for b in blocks {
            let data = [pattern(b); 4096];
            let _ = write(opened.inode, (b * 4096) as u64, &data, false).expect("write block");
        }
    }

    fn assert_blocks(path: &str, pattern: impl Fn(usize) -> Option<u8>, blocks: usize) {
        let data = read_all(path, blocks * 4096 + 1);
        assert_eq!(data.len(), blocks * 4096, "{path} size");
        for b in 0..blocks {
            let expected = pattern(b).unwrap_or(0);
            assert!(
                data[b * 4096..(b + 1) * 4096].iter().all(|&x| x == expected),
                "{path} block {b}"
            );
        }
    }

    #[test]
    fn ext4_nocsum_image_mounts_read_write() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let _ctx = mount_ext4_nocsum();
        assert_eq!(ext2_fs_name(), "ext4");
        assert!(!ext2_mount_read_only());
        let mut mounts = [0u8; 512];
        let proc_mounts = open("/proc/mounts", O_RDONLY, 0).unwrap();
        let n = read(proc_mounts.inode, 0, &mut mounts).unwrap();
        assert!(core::str::from_utf8(&mounts[..n]).unwrap().contains("ext4 /mnt/ext2 ext4 rw"));
    }

    #[test]
    fn ext4_extent_files_write_truncate_and_unlink() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let _ctx = mount_ext4_nocsum();
        let (free_before, inodes_before) = ext2_free_counts();

        // Sequential writes extend a single extent (goal allocation).
        write_blocks("/mnt/ext2/seq.bin", |b| 0x40 + b as u8, 0..3);
        let (flags, depth, entries) = raw_extents("/mnt/ext2/seq.bin");
        assert_ne!(flags & ext2_bmap::EXT4_EXTENTS_FL, 0, "new files are extent-mapped");
        assert_eq!((depth, entries), (0, 1), "contiguous blocks form one extent");
        assert_blocks("/mnt/ext2/seq.bin", |b| Some(0x40 + b as u8), 3);
        let seq = open("/mnt/ext2/seq.bin", O_RDONLY, 0).unwrap();
        assert_eq!(stat(seq.inode).unwrap().blocks, 3 * 8);

        // Fill a hole inside frag.bin's depth-1 tree.
        write_blocks("/mnt/ext2/frag.bin", |_| 0xEE, 1..2);
        let frag = read_all("/mnt/ext2/frag.bin", 64 * 4096);
        assert!(frag[4096..8192].iter().all(|&b| b == 0xEE));
        assert_eq!(&frag[..12], b"fragment 00\n");
        assert_eq!(&frag[2 * 4096..2 * 4096 + 12], b"fragment 01\n");

        // Shrink frag.bin: the tree collapses back into the inode.
        let frag_node = open("/mnt/ext2/frag.bin", O_RDWR, 0).unwrap();
        truncate_to(frag_node.inode, 3 * 4096).expect("truncate frag");
        let (_, depth, _) = raw_extents("/mnt/ext2/frag.bin");
        assert_eq!(depth, 0, "few extents fit in the inode again");
        assert_eq!(stat(frag_node.inode).unwrap().blocks, 3 * 8, "blocks 0,1,2 remain");
        truncate(frag_node.inode).expect("truncate to zero");
        let (_, depth, entries) = raw_extents("/mnt/ext2/frag.bin");
        assert_eq!((depth, entries), (0, 0));

        unlink("/mnt/ext2/seq.bin").expect("unlink seq");
        unlink("/mnt/ext2/frag.bin").expect("unlink frag");
        let (free_after, inodes_after) = ext2_free_counts();
        // frag.bin held 20 data blocks + 1 leaf; seq.bin was created and removed.
        assert_eq!(free_after, free_before + 21);
        assert_eq!(inodes_after, inodes_before + 1);
    }

    #[test]
    fn ext4_interleaved_writes_grow_and_shrink_extent_depth() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let _ctx = mount_ext4_nocsum();
        // Alternate allocations between two files: no two neighbouring
        // logical blocks of a file are physically adjacent -> 8 extents each.
        for b in 0..8usize {
            write_blocks("/mnt/ext2/a.bin", |b| 0xA0 + b as u8, b..b + 1);
            write_blocks("/mnt/ext2/b.bin", |b| 0xB0 + b as u8, b..b + 1);
        }
        for path in ["/mnt/ext2/a.bin", "/mnt/ext2/b.bin"] {
            let (_, depth, entries) = raw_extents(path);
            assert_eq!((depth, entries), (1, 1), "{path}: one leaf block under the root");
        }
        assert_blocks("/mnt/ext2/a.bin", |b| Some(0xA0 + b as u8), 8);
        assert_blocks("/mnt/ext2/b.bin", |b| Some(0xB0 + b as u8), 8);
        let a = open("/mnt/ext2/a.bin", O_RDWR, 0).unwrap();
        assert_eq!(stat(a.inode).unwrap().blocks, 9 * 8, "8 data blocks + 1 leaf");

        truncate_to(a.inode, 2 * 4096).expect("shrink a");
        let (_, depth, entries) = raw_extents("/mnt/ext2/a.bin");
        assert_eq!((depth, entries), (0, 2));
        assert_eq!(stat(a.inode).unwrap().blocks, 2 * 8, "leaf block freed");
        assert_blocks("/mnt/ext2/a.bin", |b| Some(0xA0 + b as u8), 2);
    }

    #[test]
    fn ext4_symlinks_and_directories_use_ext4_layout() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let _ctx = mount_ext4_nocsum();
        mkdir("/mnt/ext2/newdir").expect("mkdir");
        let (flags, depth, entries) = raw_extents("/mnt/ext2/newdir");
        assert_ne!(flags & ext2_bmap::EXT4_EXTENTS_FL, 0);
        assert_eq!((depth, entries), (0, 1));
        let opened = open("/mnt/ext2/newdir/inner.txt", O_CREAT | O_RDWR, 0).expect("create");
        let _ = write(opened.inode, 0, b"inner", false).unwrap();
        assert_eq!(read_whole("/mnt/ext2/newdir/inner.txt"), b"inner");

        symlinkat(root_inode(), "newdir/inner.txt", "/mnt/ext2/fast2").expect("fast symlink");
        let (flags, _, _) = raw_extents("/mnt/ext2/fast2");
        assert_eq!(flags & ext2_bmap::EXT4_EXTENTS_FL, 0, "fast symlinks carry no extent flag");
        let long = "newdir/../newdir/../newdir/../newdir/../newdir/../newdir/inner.txt";
        symlinkat(root_inode(), long, "/mnt/ext2/slow2").expect("slow symlink");
        let (flags, _, _) = raw_extents("/mnt/ext2/slow2");
        assert_ne!(flags & ext2_bmap::EXT4_EXTENTS_FL, 0, "slow symlinks are extent-mapped");
        let mut buf = [0u8; 128];
        let n = readlinkat(root_inode(), "/mnt/ext2/slow2", &mut buf).unwrap();
        assert_eq!(&buf[..n], long.as_bytes());

        // i_extra_isize is initialized for new 256-byte inodes.
        let meta = ext4_layout(SAMPLE_EXT4_NOCSUM);
        let ino = stat(opened.inode).unwrap().ino as u32;
        let extra = with_state(|state| {
            let off = ext2_inode_offset_in_state(state, &meta, ino)?;
            ext2_read_u16_state(state, off + 0x80)
        })
        .unwrap();
        assert_eq!(extra, 32);
    }

    /// Lazy-init groups: exhausting group 0's free inodes initializes an
    /// INODE_UNINIT group's bitmap; a goal allocation in a BLOCK_UNINIT group
    /// builds its block bitmap. Descriptor checksums (crc16) stay valid.
    #[test]
    fn ext4_uninit_groups_are_initialized_on_first_use() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let ctx = mount_ext4_nocsum();
        let meta = ext4_layout(SAMPLE_EXT4_NOCSUM);
        let flags_of = |group: u32| {
            with_state(|state| ext2_gd_flags_state(state, &meta, group)).unwrap()
        };
        assert_ne!(flags_of(1) & EXT4_BG_INODE_UNINIT, 0);
        assert_ne!(flags_of(1) & EXT4_BG_BLOCK_UNINIT, 0);

        // Group 0 has 47 free inodes; 60 creates spill into group 1.
        let mut last_ino = 0;
        for i in 0..60usize {
            let dir = if i < 40 { "/mnt/ext2" } else { "/mnt/ext2/sub" };
            let opened = open(&std::format!("{dir}/many_{i:02}"), O_CREAT | O_RDWR, 0)
                .expect("create many");
            last_ino = stat(opened.inode).unwrap().ino as u32;
        }
        assert!(last_ino > meta.inodes_per_group, "allocated from group 1: {last_ino}");
        assert_eq!(flags_of(1) & EXT4_BG_INODE_UNINIT, 0, "inode bitmap initialized");

        // Block allocation steered into group 1 (BLOCK_UNINIT).
        let block = with_state(|state| ext2_alloc_block_goal(state, None, 1024 + 100)).unwrap();
        assert!((1024..2048).contains(&block), "block {block} from group 1");
        assert_eq!(flags_of(1) & EXT4_BG_BLOCK_UNINIT, 0, "block bitmap initialized");

        flush_ext2_dirty_blocks().expect("flush");
        checkpoint_ext2_journal().expect("checkpoint");
        let csum = ext4_csum_params(SAMPLE_EXT4_NOCSUM);
        for group in 0..meta.group_count {
            let desc = ext4_desc(&ctx.image, &meta, group);
            assert!(csum.group_desc_ok(group, desc).unwrap(), "descriptor {group} checksum");
        }
    }

    /// Mutation mix on the writable ext4 image, synced; exported for
    /// tests/ext3_fsck.sh (PEETOS_EXT4_FSCK_IMAGE) and remounted.
    #[test]
    fn ext4_mutations_sync_and_remount() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let ctx = mount_ext4_nocsum();
        write_blocks("/mnt/ext2/seq.bin", |b| 0x51 + b as u8, 0..5);
        for b in 0..6usize {
            write_blocks("/mnt/ext2/i1.bin", |b| 0x61 + b as u8, b..b + 1);
            write_blocks("/mnt/ext2/i2.bin", |b| 0x71 + b as u8, b..b + 1);
        }
        write_blocks("/mnt/ext2/frag.bin", |_| 0xEE, 3..4);
        mkdir("/mnt/ext2/newdir").expect("mkdir");
        let inner = open("/mnt/ext2/newdir/inner.txt", O_CREAT | O_RDWR, 0).unwrap();
        let _ = write(inner.inode, 0, b"inner", false).unwrap();
        symlinkat(root_inode(), "newdir/inner.txt", "/mnt/ext2/fast2").unwrap();
        symlinkat(
            root_inode(),
            "newdir/../newdir/../newdir/../newdir/../newdir/../newdir/inner.txt",
            "/mnt/ext2/slow2",
        )
        .unwrap();
        // Stays inside the VFS node table (VFS_MAX_INODES) on remount; group
        // spill-over is covered by ext4_uninit_groups_are_initialized_on_first_use.
        for i in 0..20usize {
            let dir = if i < 12 { "/mnt/ext2" } else { "/mnt/ext2/newdir" };
            let opened = open(&std::format!("{dir}/f{i:02}"), O_CREAT | O_RDWR, 0).unwrap();
            let _ = write(opened.inode, 0, &[i as u8; 100], false).unwrap();
        }
        // Force BLOCK_UNINIT group 1 to be initialized, then give the block
        // back (a leaked block would be a real bitmap error for e2fsck).
        with_state(|state| {
            let block = ext2_alloc_block_goal(state, None, 1024 + 100)?;
            ext2_free_block(state, None, block)
        })
        .unwrap();
        unlink("/mnt/ext2/xattr_block.txt").unwrap();
        unlink("/mnt/ext2/large.bin").unwrap();
        let i2 = open("/mnt/ext2/i2.bin", O_RDWR, 0).unwrap();
        truncate_to(i2.inode, 4096 + 10).unwrap();
        renameat(root_inode(), "/mnt/ext2/sub/nested.txt", root_inode(), "/mnt/ext2/newdir/moved.txt")
            .unwrap();
        // bigdir sits at the VFS child cap (62 entries + "." + ".." = 64,
        // VFS_MAX_CHILDREN; lifted in M4), so make room before inserting
        // into the htree directory.
        unlink(&std::format!("/mnt/ext2/bigdir/{}", bigdir_name(1))).unwrap();
        let created = open("/mnt/ext2/bigdir/added", O_CREAT | O_RDWR, 0).unwrap();
        let _ = write(created.inode, 0, b"htree insert", false).unwrap();

        flush_ext2_dirty_blocks().expect("flush");
        checkpoint_ext2_journal().expect("checkpoint");
        let synced = ctx.image.clone();
        if let Ok(path) = std::env::var("PEETOS_EXT4_FSCK_IMAGE") {
            std::fs::write(&path, &synced).expect("write fsck image");
        }

        init();
        let _ctx2 = mount_ext2_backend_image(&synced, 4096);
        assert_blocks("/mnt/ext2/seq.bin", |b| Some(0x51 + b as u8), 5);
        assert_blocks("/mnt/ext2/i1.bin", |b| Some(0x61 + b as u8), 6);
        assert_eq!(read_whole("/mnt/ext2/newdir/inner.txt"), b"inner");
        assert_eq!(read_whole("/mnt/ext2/newdir/moved.txt"), b"nested\n");
        assert_eq!(read_whole("/mnt/ext2/bigdir/added"), b"htree insert");
        assert_eq!(count_dir_entries("/mnt/ext2/bigdir"), 62);
        assert_eq!(open("/mnt/ext2/large.bin", O_RDONLY, 0).unwrap_err(), ENOENT);
    }


    // ---- M4: htree maintenance and large directories ----

    /// Root block of an indexed directory: (hash version, indirect levels,
    /// root entry count).
    fn htree_root_info(path: &str) -> (u8, u8, u16) {
        let (_, dir) = ext2_raw_inode_for_path(path);
        assert_ne!(dir.flags & EXT2_INDEX_FL, 0, "{path} should be indexed");
        with_state(|state| {
            let phys = ext2_data_block_for_logical_index_state(state, &dir, 0).unwrap();
            let blk = ext2_block_slice_state(state, phys).unwrap();
            (blk[0x1C], blk[0x1E], read_u16_le(blk, 0x22).unwrap())
        })
    }

    fn padded_name(prefix: &str, i: usize, len: usize) -> std::string::String {
        let mut name = std::format!("{prefix}{i:05}_");
        while name.len() < len {
            name.push((b'a' + (name.len() % 26) as u8) as char);
        }
        name
    }

    /// Fills `dir` with `count` hard links to one file (directory entries
    /// without using up inodes), names `padded_name(prefix, i, len)`.
    fn fill_with_links(dir: &str, prefix: &str, count: usize, len: usize) {
        let hub = std::format!("{dir}/hub");
        let opened = open(&hub, O_CREAT | O_RDWR, 0).expect("create hub");
        let _ = write(opened.inode, 0, b"hub", false).expect("write hub");
        for i in 0..count {
            let name = std::format!("{dir}/{}", padded_name(prefix, i, len));
            linkat(root_inode(), &hub, root_inode(), &name)
                .unwrap_or_else(|e| panic!("link {i} ({name}): {e}"));
        }
    }

    /// Thousands of entries in a new directory on the stock ext4 image: the
    /// one-block directory becomes an htree, leaves split, every checksum
    /// (including dx_tail) stays valid, lookups after remount read one leaf
    /// each, and e2fsck can check the result (PEETOS_EXT4_HTREE_FSCK_IMAGE).
    #[test]
    fn ext4_large_directory_keeps_a_valid_htree() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let ctx = mount_sample_ext4();
        set_ext2_cache_policy(EXT2_CACHE_MAX_BLOCKS_DEFAULT, 100).expect("cache policy");
        mkdir("/mnt/ext2/huge").unwrap();
        const N: usize = 3000;
        fill_with_links("/mnt/ext2/huge", "link_", N, 40);
        let (version, levels, leaves) = htree_root_info("/mnt/ext2/huge");
        assert_eq!(version, ext2_htree::DX_HASH_HALF_MD4, "s_def_hash_version");
        assert_eq!(levels, 0);
        assert!(leaves > 10, "{leaves} leaves");
        for i in (0..N).step_by(3) {
            unlink(&std::format!("/mnt/ext2/huge/{}", padded_name("link_", i, 40))).unwrap();
        }
        // A 255-byte name (VFS names are no longer capped at 63 bytes); the
        // full path would exceed VFS_MAX_PATH, so go relative to the dir.
        let long = padded_name("long_", 0, 255);
        let huge_dir = lookup("/mnt/ext2/huge").unwrap();
        let opened = openat(huge_dir, &long, O_CREAT | O_RDWR, 0).unwrap();
        let _ = write(opened.inode, 0, b"long name", false).unwrap();
        flush_ext2_dirty_blocks().unwrap();
        checkpoint_ext2_journal().unwrap();
        let synced = ctx.image.clone();
        ext4_verify_all_checksums(&synced);
        if let Ok(path) = std::env::var("PEETOS_EXT4_HTREE_FSCK_IMAGE") {
            std::fs::write(&path, &synced).expect("write fsck image");
        }

        init();
        let _ctx2 = mount_ext2_backend_image(&synced, 4096);
        ext2_htree::DX_LEAF_READS.with(|c| c.set(0));
        for i in (0..N).step_by(7) {
            let name = std::format!("/mnt/ext2/huge/{}", padded_name("link_", i, 40));
            match lookup(&name) {
                Ok(_) => assert_ne!(i % 3, 0, "{name} was unlinked"),
                Err(err) => {
                    assert_eq!(err, ENOENT);
                    assert_eq!(i % 3, 0, "{name} lost");
                }
            }
        }
        let lookups = (0..N).step_by(7).count();
        let reads = ext2_htree::DX_LEAF_READS.with(|c| c.get());
        assert!(reads >= lookups && reads <= lookups + 8, "{reads} leaf reads for {lookups} lookups");
        with_state(|state| {
            let huge = lookup_path_inner(state, "/mnt/ext2/huge").unwrap();
            assert!(!state.inodes[huge as usize].ext2_children_loaded, "lookups stayed lazy");
        });
        let reopened = openat(lookup("/mnt/ext2/huge").unwrap(), &long, O_RDONLY, 0).unwrap();
        let mut buf = [0u8; 16];
        let n = read(reopened.inode, 0, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"long name");
        // hub + the 2/3 of the links that remain + the long name.
        assert_eq!(count_dir_entries("/mnt/ext2/huge"), 1 + (N - (N + 2) / 3) + 1);
        let hub = stat(lookup("/mnt/ext2/huge/hub").unwrap()).unwrap();
        assert_eq!(hub.nlink as usize, 1 + N - (N + 2) / 3);
    }

    /// 1 KiB blocks and 200-byte names fill the 124-entry dx root: the index
    /// grows a second level (dx nodes), and nodes split as it keeps growing.
    /// Exported for e2fsck as PEETOS_EXT3_HTREE_FSCK_IMAGE.
    #[test]
    fn ext3_htree_grows_a_second_index_level() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let ctx = mount_sample_ext3();
        set_ext2_cache_policy(EXT2_CACHE_MAX_BLOCKS_DEFAULT, 100).expect("cache policy");
        mkdir("/mnt/ext2/deep").unwrap();
        const N: usize = 700;
        fill_with_links("/mnt/ext2/deep", "deep_", N, 200);
        let (_, levels, root_count) = htree_root_info("/mnt/ext2/deep");
        assert_eq!(levels, 1, "index should have grown a level");
        assert!(root_count >= 2, "dx nodes should have split ({root_count} root entries)");
        for i in (0..N).step_by(5) {
            unlink(&std::format!("/mnt/ext2/deep/{}", padded_name("deep_", i, 200))).unwrap();
        }
        flush_ext2_dirty_blocks().unwrap();
        checkpoint_ext2_journal().unwrap();
        let synced = ctx.image.clone();
        if let Ok(path) = std::env::var("PEETOS_EXT3_HTREE_FSCK_IMAGE") {
            std::fs::write(&path, &synced).expect("write fsck image");
        }
        init();
        let _ctx2 = mount_ext2_backend_image(&synced, 4096);
        for i in 0..N {
            let name = std::format!("/mnt/ext2/deep/{}", padded_name("deep_", i, 200));
            assert_eq!(lookup(&name).is_ok(), i % 5 != 0, "{name}");
        }
        assert_eq!(count_dir_entries("/mnt/ext2/deep"), 1 + N - N / 5);
    }

    /// Every hash flavour (legacy/half_md4/tea, signed and unsigned name
    /// bytes) for new indexes, with names using bytes >= 0x80. Images go to
    /// PEETOS_HTREE_HASH_FSCK_DIR for e2fsck, which recomputes the hashes.
    #[test]
    fn htree_hash_variants_index_high_byte_names() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        let dump_dir = std::env::var("PEETOS_HTREE_HASH_FSCK_DIR").ok();
        for version in [ext2_htree::DX_HASH_LEGACY, ext2_htree::DX_HASH_HALF_MD4, ext2_htree::DX_HASH_TEA] {
            for unsigned in [false, true] {
                let mut image = std::vec::Vec::from(SAMPLE_EXT3);
                let sb = EXT2_SUPERBLOCK_OFFSET;
                image[sb + 0xFC] = version;
                let mut flags = read_u32_le(&image, sb + 0x160).unwrap();
                flags &= !(ext2_htree::EXT2_FLAGS_SIGNED_HASH | ext2_htree::EXT2_FLAGS_UNSIGNED_HASH);
                flags |= if unsigned {
                    ext2_htree::EXT2_FLAGS_UNSIGNED_HASH
                } else {
                    ext2_htree::EXT2_FLAGS_SIGNED_HASH
                };
                image[sb + 0x160..sb + 0x164].copy_from_slice(&flags.to_le_bytes());
                init();
                let ctx = mount_ext2_backend_image(&image, 4096);
                mkdir("/mnt/ext2/hashed").unwrap();
                let hub = open("/mnt/ext2/hashed/hub", O_CREAT | O_RDWR, 0).unwrap();
                let _ = write(hub.inode, 0, b"h", false).unwrap();
                let mut names = std::vec::Vec::new();
                for i in 0..120usize {
                    // UTF-8 names whose bytes include 0xC3/0xA9 etc.
                    let name = std::format!("h\u{e9}ll\u{f6}_{i:03}_\u{263a}_padding_padding");
                    linkat(root_inode(), "/mnt/ext2/hashed/hub", root_inode(), &std::format!("/mnt/ext2/hashed/{name}")).unwrap();
                    names.push(name);
                }
                let (root_version, _, _) = htree_root_info("/mnt/ext2/hashed");
                assert_eq!(root_version, version);
                flush_ext2_dirty_blocks().unwrap();
                checkpoint_ext2_journal().unwrap();
                let synced = ctx.image.clone();
                if let Some(dir) = &dump_dir {
                    std::fs::write(std::format!("{dir}/hash_{version}_{}.img", unsigned as u8), &synced)
                        .expect("write hash image");
                }
                init();
                let _ctx2 = mount_ext2_backend_image(&synced, 4096);
                ext2_htree::DX_LEAF_READS.with(|c| c.set(0));
                for name in &names {
                    lookup(&std::format!("/mnt/ext2/hashed/{name}")).unwrap();
                }
                assert!(ext2_htree::DX_LEAF_READS.with(|c| c.get()) >= names.len());
            }
        }
    }

    #[test]
    fn inode_path_follows_parent_links_without_loading_directories() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let _ctx = mount_sample_ext4();
        let mut buf = [0u8; VFS_MAX_PATH];
        let nested = lookup("/mnt/ext2/sub/nested.txt").unwrap();
        let n = inode_path(nested, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"/mnt/ext2/sub/nested.txt");
        let n = inode_path(root_inode(), &mut buf).unwrap();
        assert_eq!(&buf[..n], b"/");
        assert_eq!(inode_path(nested, &mut buf[..8]), Err(ENAMETOOLONG));
        with_state(|state| {
            let sub = lookup_path_inner(state, "/mnt/ext2/sub").unwrap();
            assert!(!state.inodes[sub as usize].ext2_children_loaded);
        });
        unlink("/mnt/ext2/sub/nested.txt").unwrap();
        assert_eq!(inode_path(nested, &mut buf), Err(ENOENT));
    }

    #[test]
    fn htree_rmdir_of_lazily_loaded_directory_checks_the_disk() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let ctx = mount_sample_ext4();
        // "sub" has entries on disk but none imported yet.
        assert_eq!(rmdir("/mnt/ext2/sub"), Err(ENOTEMPTY));
        flush_ext2_dirty_blocks().unwrap();
        checkpoint_ext2_journal().unwrap();
        let image = ctx.image.clone();
        init();
        let _ctx2 = mount_ext2_backend_image(&image, 4096);
        assert!(count_dir_entries("/mnt/ext2/sub") > 0, "rmdir must not remove entries");
    }

    /// Crash points while inserts split htree leaves on the stock image.
    #[test]
    fn ext4_htree_split_crash_points_stay_consistent() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        fn workload() {
            fill_with_links("/mnt/ext2/bigdir", "crash_", 70, 60);
        }
        fn check(t: usize, keep: usize) {
            for i in 0..62usize {
                if i == 0 {
                    continue;
                }
                lookup(&std::format!("/mnt/ext2/bigdir/{}", bigdir_name(i)))
                    .unwrap_or_else(|e| panic!("t={t} keep={keep}: entry {i} lost ({e})"));
            }
            let mut present = 0usize;
            for i in 0..70usize {
                if lookup(&std::format!("/mnt/ext2/bigdir/{}", padded_name("crash_", i, 60))).is_ok() {
                    present += 1;
                }
            }
            if let Ok(hub) = lookup("/mnt/ext2/bigdir/hub") {
                assert_eq!(stat(hub).unwrap().nlink as usize, 1 + present, "t={t} keep={keep}");
            } else {
                assert_eq!(present, 0, "t={t} keep={keep}");
            }
        }
        crash_at_every_write_with(SAMPLE_EXT4, "PEETOS_EXT4_HTREE_CRASH_DIR", workload, check);
        let (_, levels, leaves) = htree_root_info("/mnt/ext2/bigdir");
        assert_eq!(levels, 0);
        assert!(leaves >= 3, "the workload should split a leaf ({leaves} leaves)");
    }

    // ---- ext4 M3: metadata_csum read-write (stock ext4) ----

    /// Leaf blocks of an extent-mapped inode (depth 0 or 1), from raw bytes.
    fn raw_extent_blocks(image: &[u8], meta: &Ext2Meta, raw: &[u8]) -> (std::vec::Vec<u32>, std::vec::Vec<u32>) {
        // (data blocks, extent tree blocks)
        let mut data = std::vec::Vec::new();
        let mut tree = std::vec::Vec::new();
        let root = 0x28;
        let depth = read_u16_le(raw, root + 6).unwrap();
        let entries = read_u16_le(raw, root + 2).unwrap() as usize;
        let mut leaves: std::vec::Vec<std::vec::Vec<u8>> = std::vec::Vec::new();
        if depth == 0 {
            leaves.push(raw[root..root + 60].to_vec());
        } else {
            assert_eq!(depth, 1, "tests only build depth <= 1");
            for e in 0..entries {
                let leaf = read_u32_le(raw, root + 12 + e * 12 + 4).unwrap();
                tree.push(leaf);
                leaves.push(ext4_block(image, meta, leaf).to_vec());
            }
        }
        for node in &leaves {
            let n = read_u16_le(node, 2).unwrap() as usize;
            for e in 0..n {
                let at = 12 + e * 12;
                let mut len = read_u16_le(node, at + 4).unwrap() as u32;
                if len > 32768 {
                    len -= 32768;
                }
                let start = read_u32_le(node, at + 8).unwrap();
                for b in 0..len {
                    data.push(start + b);
                }
            }
        }
        (data, tree)
    }

    /// Every metadata_csum checksum in `image` must verify: superblock,
    /// descriptors, initialized bitmaps, allocated inodes, extent tree blocks
    /// and directory blocks (each with a valid leaf tail: no tail-less former
    /// htree index blocks may remain).
    fn ext4_verify_all_checksums(image: &[u8]) {
        let meta = ext4_layout(image);
        let csum = ext4_csum_params(image);
        assert!(csum.metadata_csum);
        let sb = &image[EXT2_SUPERBLOCK_OFFSET..EXT2_SUPERBLOCK_OFFSET + 1024];
        assert!(ext4_csum::superblock_csum_ok(sb).unwrap(), "superblock checksum");
        for group in 0..meta.group_count {
            let desc = ext4_desc(image, &meta, group);
            assert!(csum.group_desc_ok(group, desc).unwrap(), "descriptor {group}");
            let flags = read_u16_le(desc, 0x12).unwrap();
            if (flags & EXT4_BG_BLOCK_UNINIT) == 0 {
                let bitmap = ext4_block(image, &meta, ext2_gd_location(image, &meta, group, 0).unwrap());
                assert!(csum.block_bitmap_ok(desc, bitmap).unwrap(), "block bitmap {group}");
            }
            if (flags & EXT4_BG_INODE_UNINIT) == 0 {
                let bitmap = ext4_block(image, &meta, ext2_gd_location(image, &meta, group, 4).unwrap());
                assert!(csum.inode_bitmap_ok(desc, bitmap).unwrap(), "inode bitmap {group}");
            }
        }
        for ino in ext4_allocated_inodes(image, &meta) {
            let raw = ext4_raw_inode(image, &meta, ino);
            assert!(csum.inode_ok(ino, raw).unwrap(), "inode {ino} checksum");
            let flags = read_u32_le(raw, 32).unwrap();
            if (flags & ext2_bmap::EXT4_EXTENTS_FL) == 0 || ino == 8 {
                continue; // no extent tree, or the journal (checked by replay)
            }
            let seed = csum.inode_seed(ino, raw).unwrap();
            let (data, tree) = raw_extent_blocks(image, &meta, raw);
            for leaf in tree {
                assert!(
                    ext4_csum::extent_block_csum_ok(seed, ext4_block(image, &meta, leaf)).unwrap(),
                    "inode {ino} extent block {leaf}"
                );
            }
            if (read_u16_le(raw, 0).unwrap() & EXT2_S_IFMT) == EXT2_S_IFDIR {
                for block in data {
                    let ok = ext4_csum::dir_leaf_csum_ok(seed, ext4_block(image, &meta, block)).unwrap();
                    let indexed = (flags & EXT2_INDEX_FL) != 0;
                    match ok {
                        Some(valid) => assert!(valid, "dir inode {ino} block {block} checksum"),
                        None => {
                            assert!(indexed, "dir inode {ino} block {block} lacks a tail");
                            assert_eq!(
                                ext2_htree::dx_csum_ok(seed, ext4_block(image, &meta, block)),
                                Some(true),
                                "dir inode {ino} index block {block} dx_tail"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn ext4_stock_image_mounts_read_write_and_switches_journal_to_csum_v3() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let ctx = mount_sample_ext4();
        assert_eq!(ext2_fs_name(), "ext4");
        assert!(!ext2_mount_read_only());
        let jsb = image_journal_super_off(&ctx.image);
        let compat = read_u32_be(&ctx.image, jsb + 0x24);
        let incompat = read_u32_be(&ctx.image, jsb + 0x28);
        assert_eq!(compat & 1, 0, "no v1 commit checksums alongside v3");
        assert_ne!(incompat & 0x10, 0, "JBD2 CSUM_V3");
        assert_ne!(incompat & 0x02, 0, "JBD2 64BIT (the filesystem is 64bit)");
        assert_eq!(
            ext2_journal::journal_super_csum_for_test(&ctx.image[jsb..jsb + 1024]).unwrap(),
            read_u32_be(&ctx.image, jsb + 0xFC)
        );
    }

    fn ext4_csum_mutation_mix() {
        write_blocks("/mnt/ext2/seq.bin", |b| 0x51 + b as u8, 0..5);
        for b in 0..6usize {
            write_blocks("/mnt/ext2/i1.bin", |b| 0x61 + b as u8, b..b + 1);
            write_blocks("/mnt/ext2/i2.bin", |b| 0x71 + b as u8, b..b + 1);
        }
        write_blocks("/mnt/ext2/frag.bin", |_| 0xEE, 3..4);
        mkdir("/mnt/ext2/d1").unwrap();
        mkdir("/mnt/ext2/d2").unwrap();
        mkdir("/mnt/ext2/d1/moving").unwrap();
        let inner = open("/mnt/ext2/d1/moving/inner.txt", O_CREAT | O_RDWR, 0).unwrap();
        let _ = write(inner.inode, 0, b"inner", false).unwrap();
        // A directory moved across parents: its ".." entry is rewritten.
        renameat(root_inode(), "/mnt/ext2/d1/moving", root_inode(), "/mnt/ext2/d2/moving")
            .unwrap();
        symlinkat(root_inode(), "d2/moving/inner.txt", "/mnt/ext2/fast2").unwrap();
        symlinkat(
            root_inode(),
            "d2/../d2/../d2/../d2/../d2/../d2/moving/../moving/inner.txt",
            "/mnt/ext2/slow2",
        )
        .unwrap();
        for i in 0..20usize {
            let dir = if i < 12 { "/mnt/ext2" } else { "/mnt/ext2/d1" };
            let opened = open(&std::format!("{dir}/f{i:02}"), O_CREAT | O_RDWR, 0).unwrap();
            let _ = write(opened.inode, 0, &[i as u8; 100], false).unwrap();
        }
        unlink("/mnt/ext2/xattr_block.txt").unwrap();
        unlink("/mnt/ext2/large.bin").unwrap();
        let i2 = open("/mnt/ext2/i2.bin", O_RDWR, 0).unwrap();
        truncate_to(i2.inode, 4096 + 10).unwrap();
        // Insert into the htree directory (stays within the VFS child cap).
        unlink(&std::format!("/mnt/ext2/bigdir/{}", bigdir_name(1))).unwrap();
        let created = open("/mnt/ext2/bigdir/added", O_CREAT | O_RDWR, 0).unwrap();
        let _ = write(created.inode, 0, b"htree insert", false).unwrap();
    }

    /// Stock mkfs.ext4 image written by the kernel: every checksum stays
    /// valid, the result remounts (verifying checksums on read), and it is
    /// exported for e2fsck (PEETOS_EXT4_CSUM_FSCK_IMAGE).
    #[test]
    fn ext4_csum_mutations_keep_every_checksum_valid() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let ctx = mount_sample_ext4();
        set_ext2_cache_policy(EXT2_CACHE_MAX_BLOCKS_DEFAULT, 100).expect("cache policy");
        ext4_csum_mutation_mix();
        // Force group 1's BLOCK_UNINIT bitmap into use (and give it back).
        with_state(|state| {
            let block = ext2_alloc_block_goal(state, None, 1024 + 100)?;
            ext2_free_block(state, None, block)
        })
        .unwrap();
        flush_ext2_dirty_blocks().expect("flush");
        checkpoint_ext2_journal().expect("checkpoint");
        let synced = ctx.image.clone();
        ext4_verify_all_checksums(&synced);
        if let Ok(path) = std::env::var("PEETOS_EXT4_CSUM_FSCK_IMAGE") {
            std::fs::write(&path, &synced).expect("write fsck image");
        }

        init();
        let _ctx2 = mount_ext2_backend_image(&synced, 4096);
        assert_blocks("/mnt/ext2/seq.bin", |b| Some(0x51 + b as u8), 5);
        assert_blocks("/mnt/ext2/i1.bin", |b| Some(0x61 + b as u8), 6);
        assert_eq!(read_whole("/mnt/ext2/d2/moving/inner.txt"), b"inner");
        assert_eq!(read_whole("/mnt/ext2/bigdir/added"), b"htree insert");
        assert_eq!(count_dir_entries("/mnt/ext2/bigdir"), 62);
        let mut buf = [0u8; 128];
        let n = readlinkat(root_inode(), "/mnt/ext2/fast2", &mut buf).unwrap();
        assert_eq!(&buf[..n], b"d2/moving/inner.txt");
        // ".." of the moved directory names its new parent.
        let d2 = stat(lookup("/mnt/ext2/d2").unwrap()).unwrap().ino as u32;
        let mut parent_seen = 0u32;
        with_state(|state| {
            let n = lookup_path_inner(state, "/mnt/ext2/d2/moving")?;
            let ino = state.inodes[n as usize].ext2_ino;
            let moving = ext2_read_inode_from_state(state, ino)?;
            ext2_for_each_dir_entry_state(state, &moving, |child, _ft, name| {
                if name == b".." {
                    parent_seen = child;
                }
                Ok(())
            })
        })
        .unwrap();
        assert_eq!(parent_seen, d2);
    }

    /// Writing into the debugfs-written csum-v3 journal fixture: recovery
    /// first, then normal read-write use.
    #[test]
    fn ext4_linux_format_journal_fixture_mounts_read_write() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let ctx = mount_ext2_backend_image(SAMPLE_EXT4_JOURNAL, 4096);
        assert!(!ext2_mount_read_only());
        assert!(ext4_block_is(&ctx.image, 3500, b'J'));
        let created = open("/mnt/ext2/after_replay.txt", O_CREAT | O_RDWR, 0).unwrap();
        let _ = write(created.inode, 0, b"after replay", false).unwrap();
        flush_ext2_dirty_blocks().unwrap();
        checkpoint_ext2_journal().unwrap();
        ext4_verify_all_checksums(&ctx.image);
    }

    #[test]
    fn ext4_csum_crash_at_every_write_with_reordering_cache_stays_consistent() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        crash_at_every_write(SAMPLE_EXT4, "PEETOS_EXT4_CSUM_CRASH_DIR");
    }

    /// Committed but not checkpointed on the stock image: the journal holds
    /// kernel-written csum-v3 transactions. The kernel recovers them; with
    /// PEETOS_EXT4_CSUM_CRASH_IMAGE the unrecovered image is exported so
    /// tests/ext3_fsck.sh can have e2fsck (Linux jbd2 recovery) replay it.
    #[test]
    fn ext4_csum_crash_image_replays_kernel_written_v3_journal() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let ctx = mount_sample_ext4();
        set_ext2_cache_policy(EXT2_CACHE_MAX_BLOCKS_DEFAULT, 100).expect("cache policy");
        let created = open("/mnt/ext2/sub/crash_csum", O_CREAT | O_RDWR, 0).unwrap();
        let _ = write(created.inode, 0, b"survives crash", false).unwrap();
        write_blocks("/mnt/ext2/sub/crash_blocks", |b| 0x30 + b as u8, 0..3);
        unlink("/mnt/ext2/xattr_block.txt").unwrap();
        mkdir("/mnt/ext2/sub/crash_dir").unwrap();
        flush_ext2_dirty_blocks().expect("commit without checkpoint");
        let crashed = ctx.image.clone();
        assert_ne!(read_u32_le(&crashed, EXT2_SUPERBLOCK_OFFSET + 96).unwrap() & EXT2_FEATURE_INCOMPAT_RECOVER, 0);
        if let Ok(path) = std::env::var("PEETOS_EXT4_CSUM_CRASH_IMAGE") {
            std::fs::write(&path, &crashed).expect("write crash image");
        }
        init();
        let ctx2 = mount_ext2_backend_image(&crashed, 4096);
        assert_eq!(read_whole("/mnt/ext2/sub/crash_csum"), b"survives crash");
        assert_blocks("/mnt/ext2/sub/crash_blocks", |b| Some(0x30 + b as u8), 3);
        flush_ext2_dirty_blocks().unwrap();
        checkpoint_ext2_journal().unwrap();
        ext4_verify_all_checksums(&ctx2.image);
    }

    /// 4 KiB-block ext2-family images keep the superblock in block 0
    /// (s_first_data_block = 0); block 0 must be readable there.
    #[test]
    fn ext4_block_zero_is_metadata_with_4k_blocks() {
        let meta = ext4_layout(SAMPLE_EXT4);
        assert_eq!(meta.first_data_block, 0);
        assert!(!ext2_block_is_null(&meta, 0));
        let ext3 = ext2_parse_meta_with_len(SAMPLE_EXT3, SAMPLE_EXT3.len(), true).unwrap();
        assert_eq!(ext3.first_data_block, 1);
        assert!(ext2_block_is_null(&ext3, 0));
    }

    #[test]
    fn ext4_layout_uses_64_byte_descriptors_and_flex_bg_locations() {
        for image in [SAMPLE_EXT4, SAMPLE_EXT4_NOCSUM] {
            let meta = ext4_layout(image);
            assert_eq!(meta.block_size, 4096);
            assert_eq!(meta.desc_size, 64);
            assert_eq!(meta.inode_size, 256);
            assert_eq!(meta.group_count, 4);
            // flex_bg packs every group's metadata into group 0 (dumpe2fs).
            for group in 0..4u32 {
                assert_eq!(ext2_gd_location(image, &meta, group, 0).unwrap(), 65 + group);
                assert_eq!(ext2_gd_location(image, &meta, group, 4).unwrap(), 69 + group);
                assert_eq!(
                    ext2_group_inode_table_block(image, &meta, group).unwrap(),
                    73 + 8 * group
                );
            }
        }
        // A non-zero high half (> 2^32 blocks) is refused, not truncated.
        let mut image = std::vec::Vec::from(SAMPLE_EXT4);
        let meta = ext4_layout(&image);
        let off = ext2_group_desc_offset(&meta, 1).unwrap();
        write_u32_le(&mut image, off + 0x20, 1).unwrap();
        assert_eq!(ext2_gd_location(&image, &meta, 1, 0).unwrap_err(), EIO);
        // 64bit with an out-of-range s_desc_size is invalid.
        let mut bad = std::vec::Vec::from(SAMPLE_EXT4);
        bad[EXT2_SUPERBLOCK_OFFSET + 0xFE..EXT2_SUPERBLOCK_OFFSET + 0x100]
            .copy_from_slice(&48u16.to_le_bytes());
        assert_eq!(
            ext2_parse_layout_with_len(&bad, bad.len(), true).err(),
            Some(EINVAL)
        );
    }

    #[test]
    fn ext4_superblock_and_group_descriptor_checksums_verify() {
        let sb = &SAMPLE_EXT4[EXT2_SUPERBLOCK_OFFSET..EXT2_SUPERBLOCK_OFFSET + 1024];
        assert!(ext4_csum::superblock_csum_ok(sb).unwrap());
        let mut bad_sb = std::vec::Vec::from(sb);
        bad_sb[0x78] ^= 1; // volume label byte
        assert!(!ext4_csum::superblock_csum_ok(&bad_sb).unwrap());

        // metadata_csum: crc32c low 16 bits; uninit_bg: crc16.
        for (image, metadata_csum) in [(SAMPLE_EXT4, true), (SAMPLE_EXT4_NOCSUM, false)] {
            let meta = ext4_layout(image);
            let csum = ext4_csum_params(image);
            assert_eq!(csum.metadata_csum, metadata_csum);
            assert_eq!(csum.gdt_csum, !metadata_csum);
            for group in 0..meta.group_count {
                let desc = ext4_desc(image, &meta, group);
                assert!(csum.group_desc(group, desc).unwrap().is_some());
                assert!(csum.group_desc_ok(group, desc).unwrap(), "group {group}");
                let mut bad = std::vec::Vec::from(desc);
                bad[0x0C] ^= 1; // free blocks count
                assert!(!csum.group_desc_ok(group, &bad).unwrap(), "group {group} corrupt");
            }
        }
    }

    #[test]
    fn ext4_bitmap_and_inode_checksums_verify() {
        let image = SAMPLE_EXT4;
        let meta = ext4_layout(image);
        let csum = ext4_csum_params(image);
        let mut bitmaps_checked = 0;
        for group in 0..meta.group_count {
            let desc = ext4_desc(image, &meta, group);
            let flags = read_u16_le(desc, 0x12).unwrap();
            if (flags & ext4_csum::EXT4_BG_BLOCK_UNINIT) == 0 {
                let bitmap = ext4_block(image, &meta, ext2_gd_location(image, &meta, group, 0).unwrap());
                assert!(csum.block_bitmap_ok(desc, bitmap).unwrap(), "block bitmap {group}");
                bitmaps_checked += 1;
            }
            if (flags & ext4_csum::EXT4_BG_INODE_UNINIT) == 0 {
                let bitmap = ext4_block(image, &meta, ext2_gd_location(image, &meta, group, 4).unwrap());
                assert!(csum.inode_bitmap_ok(desc, bitmap).unwrap(), "inode bitmap {group}");
                bitmaps_checked += 1;
            }
        }
        assert_eq!(bitmaps_checked, 4, "3 initialized block bitmaps + 1 inode bitmap");

        let inodes = ext4_allocated_inodes(image, &meta);
        assert_eq!(inodes.len(), 512 - 431, "matches dumpe2fs free inode count");
        for &ino in &inodes {
            assert!(csum.inode_ok(ino, ext4_raw_inode(image, &meta, ino)).unwrap(), "inode {ino}");
        }
        let mut bad = std::vec::Vec::from(ext4_raw_inode(image, &meta, EXT4_FRAG_INO));
        bad[0x04] ^= 1; // i_size
        assert!(!csum.inode_ok(EXT4_FRAG_INO, &bad).unwrap());
    }

    #[test]
    fn ext4_extent_and_directory_block_checksums_verify() {
        let image = SAMPLE_EXT4;
        let meta = ext4_layout(image);
        let csum = ext4_csum_params(image);

        // frag.bin: depth-1 tree; the in-inode root holds one index entry.
        let raw = ext4_raw_inode(image, &meta, EXT4_FRAG_INO);
        let root = 0x28;
        assert_eq!(read_u16_le(raw, root).unwrap(), ext4_csum::EXT4_EXTENT_MAGIC);
        assert_eq!(read_u16_le(raw, root + 6).unwrap(), 1, "depth");
        let leaf_lo = read_u32_le(raw, root + 12 + 4).unwrap();
        let leaf_hi = read_u16_le(raw, root + 12 + 8).unwrap();
        assert_eq!(leaf_hi, 0);
        let seed = csum.inode_seed(EXT4_FRAG_INO, raw).unwrap();
        let leaf = ext4_block(image, &meta, leaf_lo);
        assert_eq!(read_u16_le(leaf, 2).unwrap(), 20, "20 extents in the leaf");
        assert!(ext4_csum::extent_block_csum_ok(seed, leaf).unwrap());
        let mut bad = std::vec::Vec::from(leaf);
        bad[12] ^= 1;
        assert!(!ext4_csum::extent_block_csum_ok(seed, &bad).unwrap());

        // Every directory data block reachable through a depth-0 root.
        let mut leaves = 0;
        let mut dx_blocks = 0;
        for ino in ext4_allocated_inodes(image, &meta) {
            let raw = ext4_raw_inode(image, &meta, ino);
            if (read_u16_le(raw, 0).unwrap() & EXT2_S_IFMT) != EXT2_S_IFDIR {
                continue;
            }
            assert_eq!(read_u16_le(raw, root + 6).unwrap(), 0, "small dirs have depth 0");
            let seed = csum.inode_seed(ino, raw).unwrap();
            let entries = read_u16_le(raw, root + 2).unwrap() as usize;
            for e in 0..entries {
                let ext = root + 12 + e * 12;
                let len = read_u16_le(raw, ext + 4).unwrap() as u32;
                let start = read_u32_le(raw, ext + 8).unwrap();
                for b in 0..len {
                    match ext4_csum::dir_leaf_csum_ok(seed, ext4_block(image, &meta, start + b))
                        .unwrap()
                    {
                        Some(ok) => {
                            assert!(ok, "dir inode {ino} block {}", start + b);
                            leaves += 1;
                        }
                        None => dx_blocks += 1,
                    }
                }
            }
        }
        assert!(leaves >= 4, "root, sub, lost+found, bigdir leaves: {leaves}");
        assert!(dx_blocks >= 1, "bigdir has an htree root block");
    }

    /// The block-map seam refuses extent-mapped inodes until M1 implements
    /// the extent backend: on an ext2/ext3 filesystem such an inode is corrupt.
    #[test]
    fn ext4_extent_flagged_inode_on_ext2_returns_eio() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();
        let mut image = std::vec::Vec::from(SAMPLE_EXT3);
        let meta = ext2_parse_meta_with_len(&image, image.len(), true).unwrap();
        // Root directory entry for hello.txt: find its inode via a mount.
        let ino = {
            let _ctx = mount_ext2_backend_image(&image, 4096);
            let node = lookup("/mnt/ext2/hello.txt").unwrap();
            stat(node).unwrap().ino as u32
        };
        let off = ext2_inode_offset_in_image(&image, &meta, ino).unwrap();
        let flags = read_u32_le(&image, off + 32).unwrap();
        write_u32_le(&mut image, off + 32, flags | ext2_bmap::EXT4_EXTENTS_FL).unwrap();

        init();
        let _ctx = mount_ext2_backend_image(&image, 4096);
        let opened = open("/mnt/ext2/hello.txt", O_RDONLY, 0).expect("open");
        let mut buf = [0u8; 16];
        assert_eq!(read(opened.inode, 0, &mut buf).unwrap_err(), EIO);
    }
}
