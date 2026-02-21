#![no_std]

extern crate alloc;

mod ext2_alloc;
mod ext2_api;
mod ext2_backend;
mod ext2_dir;
mod ext2_journal;
mod ext2_sync;

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::hint::spin_loop;
use core::sync::atomic::{AtomicBool, Ordering};
use ext2_alloc::{
    ext2_alloc_block, ext2_alloc_inode, ext2_ensure_data_block, ext2_free_block, ext2_free_inode,
    ext2_write_inode_to_state, ext2_write_zero_range,
};
use ext2_dir::{
    ext2_add_dir_entry, ext2_directory_is_empty, ext2_remove_dir_entry, ext2_write_dirent,
};

pub use ext2_api::{
    checkpoint_ext2_journal, clear_ext2_block_backend, copy_ext2_blocks, export_ext2_image,
    ext2_backend_kind, ext2_has_journal, ext2_next_dirty_run, flush_ext2_dirty_blocks,
    mark_ext2_blocks_synced, mount_ext2_block_backend, set_ext2_block_backend,
    set_ext2_cache_policy,
};
pub use ext2_backend::{Ext2BackendKind, Ext2BlockBackend, Ext2BlockReadFn, Ext2BlockWriteFn};

pub const VFS_MAX_PATH: usize = 256;
pub const VFS_MAX_NAME: usize = 64;
pub const VFS_MAX_INODES: usize = 256;
pub const VFS_MAX_CHILDREN: usize = 64;
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

const ROOT_INODE: u16 = 0;
const PROCFS_BUFFER_MAX: usize = 512;
const PROC_TICK_HZ: u64 = 100;
const MOUNT_MAX: usize = 8;

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
const EXT2_GROUP_DESC_SIZE: usize = 32;
const EXT2_INODE_DIRECT_COUNT: usize = 12;
const EXT2_CACHE_MAX_BLOCKS_DEFAULT: usize = 2048;
const EXT2_CACHE_DIRTY_HIGH_WATERMARK_PERCENT_DEFAULT: u8 = 30;

const EXT2_FEATURE_COMPAT_DIR_PREALLOC: u32 = 0x0001;
const EXT2_FEATURE_COMPAT_IMAGIC_INODES: u32 = 0x0002;
const EXT2_FEATURE_COMPAT_HAS_JOURNAL: u32 = 0x0004;
const EXT2_FEATURE_COMPAT_EXT_ATTR: u32 = 0x0008;
const EXT2_FEATURE_COMPAT_RESIZE_INO: u32 = 0x0010;
const EXT2_FEATURE_COMPAT_DIR_INDEX: u32 = 0x0020;
const EXT2_FEATURE_COMPAT_SUPPORTED: u32 = EXT2_FEATURE_COMPAT_DIR_PREALLOC
    | EXT2_FEATURE_COMPAT_IMAGIC_INODES
    | EXT2_FEATURE_COMPAT_HAS_JOURNAL
    | EXT2_FEATURE_COMPAT_EXT_ATTR
    | EXT2_FEATURE_COMPAT_RESIZE_INO
    | EXT2_FEATURE_COMPAT_DIR_INDEX;

const EXT2_FEATURE_INCOMPAT_FILETYPE: u32 = 0x0002;
const EXT2_FEATURE_INCOMPAT_RECOVER: u32 = 0x0004;
const EXT2_FEATURE_INCOMPAT_JOURNAL_DEV: u32 = 0x0008;
const EXT2_FEATURE_INCOMPAT_SUPPORTED: u32 = EXT2_FEATURE_INCOMPAT_FILETYPE
    | EXT2_FEATURE_INCOMPAT_RECOVER
    | EXT2_FEATURE_INCOMPAT_JOURNAL_DEV;

const EXT2_FEATURE_RO_COMPAT_SPARSE_SUPER: u32 = 0x0001;
const EXT2_FEATURE_RO_COMPAT_LARGE_FILE: u32 = 0x0002;
const EXT2_FEATURE_RO_COMPAT_SUPPORTED: u32 =
    EXT2_FEATURE_RO_COMPAT_SPARSE_SUPER | EXT2_FEATURE_RO_COMPAT_LARGE_FILE;

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenResult {
    pub inode: u16,
    pub file_type: FileType,
    pub is_console: bool,
    pub is_tty: bool,
    pub is_ptmx: bool,
    pub is_pts_slave: bool,
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

#[derive(Clone, Copy)]
struct Inode {
    used: bool,
    file_type: FileType,
    backend: NodeBackend,
    parent: u16,
    name: [u8; VFS_MAX_NAME],
    name_len: u8,
    size: u32,
    child_count: u8,
    children: [u16; VFS_MAX_CHILDREN],
    data: [u8; TMPFS_MAX_FILE_SIZE],
    ext2_ino: u32,
    ext2_children_loaded: bool,
    mode: u16,
    uid: u16,
    gid: u16,
    links_count: u16,
    atime: u64,
    mtime: u64,
    ctime: u64,
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
            child_count: 0,
            children: [0; VFS_MAX_CHILDREN],
            data: [0; TMPFS_MAX_FILE_SIZE],
            ext2_ino: 0,
            ext2_children_loaded: false,
            mode: 0,
            uid: 0,
            gid: 0,
            links_count: 0,
            atime: 0,
            mtime: 0,
            ctime: 0,
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
    inode: u32,
    inode_blocks: [u32; 15],
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
}

impl Ext2JournalState {
    const fn empty() -> Self {
        Self {
            present: false,
            replaying: false,
            inode: 0,
            inode_blocks: [0; 15],
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
        }
    }
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
    inodes: [Inode; VFS_MAX_INODES],
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
    ext2_journal_committed_txs: Vec<Ext2JournalCommittedTx>,
    ext2_journal_pending_revokes: Vec<u32>,
    prng_state: u64,
}

impl VfsState {
    const fn new() -> Self {
        Self {
            initialized: false,
            inodes: [Inode::empty(); VFS_MAX_INODES],
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
            ext2_journal_committed_txs: Vec::new(),
            ext2_journal_pending_revokes: Vec::new(),
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
    journal_inode: u32,
    journal_dev: u32,
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
            journal_inode: 0,
            journal_dev: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct Ext2Inode {
    mode: u16,
    uid: u16,
    size: u32,
    atime: u32,
    ctime: u32,
    mtime: u32,
    links_count: u16,
    gid: u16,
    block_count_512: u32,
    blocks: [u32; 15],
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
    if (inode as usize) >= VFS_MAX_INODES {
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
    inode.child_count = 0;
    inode.children.fill(0);
    inode.data.fill(0);
    inode.ext2_ino = 0;
    inode.ext2_children_loaded = false;
    inode.mode = 0;
    inode.uid = 0;
    inode.gid = 0;
    inode.links_count = 0;
    inode.atime = 0;
    inode.mtime = 0;
    inode.ctime = 0;
}

fn reset(state: &mut VfsState) {
    for slot in state.inodes.iter_mut() {
        clear_inode(slot);
    }
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
    state.ext2_image_len = 0;
    state.ext2_image.clear();
    state.ext2_backend = ext2_backend::Ext2Backend::none();
    ext2_clear_dirty_tracking(state);
    state.ext2_cache_max_blocks = EXT2_CACHE_MAX_BLOCKS_DEFAULT;
    state.ext2_cache_dirty_high_watermark_percent = EXT2_CACHE_DIRTY_HIGH_WATERMARK_PERCENT_DEFAULT;
    state.initialized = false;
}

fn setup_root(state: &mut VfsState) {
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
    let _ = mkdir_inner(state, "/etc");

    let _ = mkdir_virtual_inner(state, "/dev");
    let _ = create_virtual_file_inner(state, "/dev/null", NodeBackend::DevNull);
    let _ = create_virtual_file_inner(state, "/dev/zero", NodeBackend::DevZero);
    let _ = create_virtual_file_inner(state, "/dev/console", NodeBackend::DevConsole);
    let _ = create_virtual_file_inner(state, "/dev/urandom", NodeBackend::DevUrandom);
    let _ = create_virtual_file_inner(state, "/dev/tty", NodeBackend::DevTty);
    let _ = create_virtual_file_inner(state, "/dev/ptmx", NodeBackend::DevPtmx);

    let _ = mkdir_virtual_inner(state, "/dev/pts");
    {
        let mut pty_idx: u16 = 0;
        while (pty_idx as usize) < 4 {
            let mut path_buf = [0u8; 16];
            let path_len = format_pts_path(pty_idx, &mut path_buf);
            // Safety: path_buf contains only ASCII digits prefixed by /dev/pts/
            let path = unsafe { core::str::from_utf8_unchecked(&path_buf[..path_len]) };
            if let Ok(inode) = create_virtual_file_inner(state, path, NodeBackend::DevPtsSlave) {
                state.inodes[inode as usize].ext2_ino = pty_idx as u32;
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

fn find_child(state: &VfsState, dir: u16, name: &[u8]) -> Option<u16> {
    let inode = &state.inodes[dir as usize];
    let mut idx = 0usize;
    while idx < inode.child_count as usize {
        let child = inode.children[idx];
        let child_inode = &state.inodes[child as usize];
        if child_inode.used && name_eq(child_inode, name) {
            return Some(child);
        }
        idx += 1;
    }
    None
}

fn find_child_loaded(state: &mut VfsState, dir: u16, name: &[u8]) -> Result<Option<u16>, i64> {
    ext2_ensure_dir_children_loaded(state, dir)?;
    Ok(find_child(state, dir, name))
}

fn add_child(state: &mut VfsState, dir: u16, child: u16) -> Result<(), i64> {
    let parent = &mut state.inodes[dir as usize];
    if parent.file_type != FileType::Directory {
        return Err(ENOTDIR);
    }
    if parent.child_count as usize >= VFS_MAX_CHILDREN {
        return Err(ENOSPC);
    }

    let slot = parent.child_count as usize;
    parent.children[slot] = child;
    parent.child_count = parent.child_count.saturating_add(1);
    Ok(())
}

fn remove_child(state: &mut VfsState, dir: u16, child: u16) -> Result<(), i64> {
    if (dir as usize) >= VFS_MAX_INODES {
        return Err(ENOENT);
    }
    let parent = &mut state.inodes[dir as usize];
    if parent.file_type != FileType::Directory {
        return Err(ENOTDIR);
    }

    let mut idx = 0usize;
    while idx < parent.child_count as usize {
        if parent.children[idx] == child {
            let mut move_idx = idx + 1;
            while move_idx < parent.child_count as usize {
                parent.children[move_idx - 1] = parent.children[move_idx];
                move_idx += 1;
            }
            let last = parent.child_count.saturating_sub(1) as usize;
            parent.children[last] = 0;
            parent.child_count = parent.child_count.saturating_sub(1);
            return Ok(());
        }
        idx += 1;
    }

    Err(ENOENT)
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
    while slot < VFS_MAX_INODES {
        if !state.inodes[slot].used {
            let now = fs_time_tick(state);
            let inode = &mut state.inodes[slot];
            clear_inode(inode);
            inode.used = true;
            inode.file_type = file_type;
            inode.backend = backend;
            inode.parent = parent;
            inode.name_len = name.len() as u8;
            inode.name[..name.len()].copy_from_slice(name);
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
            return Ok(slot as u16);
        }
        slot += 1;
    }

    Err(ENOSPC)
}

fn has_free_vfs_inode(state: &VfsState) -> bool {
    let mut slot = 1usize;
    while slot < VFS_MAX_INODES {
        if !state.inodes[slot].used {
            return true;
        }
        slot += 1;
    }
    false
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

fn lookup_path_from_inner(state: &mut VfsState, start: u16, path: &str) -> Result<u16, i64> {
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
    if current as usize >= VFS_MAX_INODES || !state.inodes[current as usize].used {
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
    if path.is_empty() || path == "/" {
        return Err(EINVAL);
    }

    let bytes = path.as_bytes();
    let absolute = bytes[0] == b'/';
    let mut cursor = if absolute { 1usize } else { 0usize };
    let mut current = if absolute { ROOT_INODE } else { start };
    if current as usize >= VFS_MAX_INODES || !state.inodes[current as usize].used {
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
    if !matches!(parent_backend, NodeBackend::Tmpfs | NodeBackend::Ext2) {
        return Err(EROFS);
    }

    if find_child_loaded(state, parent, &name[..name_len])?.is_some() {
        return Err(EEXIST);
    }

    if parent_backend == NodeBackend::Ext2 {
        if state.inodes[parent as usize].ext2_ino == 0 {
            return Err(EIO);
        }
        if state.inodes[parent as usize].child_count as usize >= VFS_MAX_CHILDREN {
            return Err(ENOSPC);
        }
        if !has_free_vfs_inode(state) {
            return Err(ENOSPC);
        }

        let (ext2_ino, ext2_size) = ext2_create_child_on_disk(
            state,
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
        node.size = ext2_size;
        add_child(state, parent, inode)?;
        touch_inode_metadata(state, parent, true, true, true);
        return Ok(inode);
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
    if !matches!(parent_backend, NodeBackend::Tmpfs | NodeBackend::Ext2) {
        return Err(EROFS);
    }

    if find_child_loaded(state, parent, &name[..name_len])?.is_some() {
        return Err(EEXIST);
    }

    if parent_backend == NodeBackend::Ext2 {
        if state.inodes[parent as usize].ext2_ino == 0 {
            return Err(EIO);
        }
        if state.inodes[parent as usize].child_count as usize >= VFS_MAX_CHILDREN {
            return Err(ENOSPC);
        }
        if !has_free_vfs_inode(state) {
            return Err(ENOSPC);
        }

        let (ext2_ino, ext2_size) = ext2_create_child_on_disk(
            state,
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
        node.size = ext2_size;
        node.ext2_children_loaded = true;
        add_child(state, parent, inode)?;
        touch_inode_metadata(state, parent, true, true, true);
        return Ok(());
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
            | NodeBackend::DevPtsSlave => (STAT_S_IFREG | 0o666) as u16,
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

fn ext2_parse_meta_with_len(
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
    let feature_ro_compat = read_u32_le(image, sb + 100)?;
    let journal_inode = read_u32_le(image, sb + 224)?;
    let journal_dev = read_u32_le(image, sb + 228)?;

    if total_inodes == 0 || total_blocks == 0 || blocks_per_group == 0 || inodes_per_group == 0 {
        return Err(EINVAL);
    }

    let block_size = 1024usize.checked_shl(log_block_size).ok_or(EINVAL)?;
    if block_size < 1024 || block_size > 4096 {
        return Err(EINVAL);
    }

    if (feature_compat & !EXT2_FEATURE_COMPAT_SUPPORTED) != 0 {
        return Err(EINVAL);
    }
    if (feature_incompat & !EXT2_FEATURE_INCOMPAT_SUPPORTED) != 0 {
        return Err(EINVAL);
    }
    if (feature_ro_compat & !EXT2_FEATURE_RO_COMPAT_SUPPORTED) != 0 {
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

    let gdt_size = (group_count as usize)
        .checked_mul(EXT2_GROUP_DESC_SIZE)
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
        journal_inode,
        journal_dev,
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

    let backend = match state.ext2_backend {
        ext2_backend::Ext2Backend::None => return Err(EIO),
        ext2_backend::Ext2Backend::BlockOps(backend) => backend,
    };
    (backend.write_block)(
        backend.ctx,
        block,
        state.ext2_cached_blocks[idx].data.as_slice(),
    )?;
    state.ext2_cached_blocks[idx].dirty = false;
    ext2_unmark_dirty_block(state, block);
    Ok(())
}

fn ext2_cache_evict_one_clock(state: &mut VfsState) -> Result<(), i64> {
    if state.ext2_cached_blocks.is_empty() {
        state.ext2_cache_clock_hand = 0;
        return Ok(());
    }

    loop {
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
            ext2_cache_flush_block(state, block)?;
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
    state.ext2_journal_committed_txs.clear();
    state.ext2_journal_pending_revokes.clear();
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
    state.ext2_journal_committed_txs.clear();
    state.ext2_journal_pending_revokes.clear();
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

    let desc_offset = meta
        .gdt_offset
        .checked_add((group as usize) * EXT2_GROUP_DESC_SIZE)
        .ok_or(EIO)?;
    read_u32_le(image, desc_offset + 8)
}

#[allow(dead_code)]
fn ext2_block_slice<'a>(image: &'a [u8], meta: &Ext2Meta, block: u32) -> Result<&'a [u8], i64> {
    if block == 0 {
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
    if block == 0 {
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
    if block == 0 {
        return Err(EIO);
    }
    let meta = state.ext2_meta;
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
    if block == 0 {
        return Err(EIO);
    }
    let meta = state.ext2_meta;
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

fn ext2_group_desc_offset(meta: &Ext2Meta, group: u32) -> Result<usize, i64> {
    if group >= meta.group_count {
        return Err(EIO);
    }
    meta.gdt_offset
        .checked_add((group as usize) * EXT2_GROUP_DESC_SIZE)
        .ok_or(EIO)
}

#[allow(dead_code)]
fn ext2_group_bitmap_block(
    image: &[u8],
    meta: &Ext2Meta,
    group: u32,
    field_offset: usize,
) -> Result<u32, i64> {
    let desc_offset = ext2_group_desc_offset(meta, group)?;
    read_u32_le(image, desc_offset + field_offset)
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
    let desc_offset = ext2_group_desc_offset(meta, group)?;
    ext2_read_u32_state(state, desc_offset + field_offset)
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
    let desc_offset = ext2_group_desc_offset(meta, group)?;
    ext2_read_u32_state(state, desc_offset + 8)
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
    write_u32_le(image, inode_offset + 8, inode.atime)?;
    write_u32_le(image, inode_offset + 12, inode.ctime)?;
    write_u32_le(image, inode_offset + 16, inode.mtime)?;
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
    let desc_offset = ext2_group_desc_offset(meta, group)?;
    ext2_adjust_counter_u16_state(state, desc_offset + 12, block_delta)?;
    ext2_adjust_counter_u16_state(state, desc_offset + 14, inode_delta)?;
    ext2_adjust_counter_u16_state(state, desc_offset + 16, used_dir_delta)?;
    Ok(())
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

    Ok(used_blocks.saturating_mul(sectors_per_block))
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

fn ext2_recalc_inode_block_count_512_state(
    state: &mut VfsState,
    inode: &Ext2Inode,
) -> Result<u32, i64> {
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

    Ok(used_blocks.saturating_mul(sectors_per_block))
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
    let atime = ext2_read_u32_state(state, inode_offset + 8)?;
    let ctime = ext2_read_u32_state(state, inode_offset + 12)?;
    let mtime = ext2_read_u32_state(state, inode_offset + 16)?;
    let gid = ext2_read_u16_state(state, inode_offset + 24)?;
    let links_count = ext2_read_u16_state(state, inode_offset + 26)?;
    let block_count_512 = ext2_read_u32_state(state, inode_offset + 28)?;

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
    })
}

fn ext2_refresh_alias_nodes(state: &mut VfsState, ext2_ino: u32, inode: &Ext2Inode) {
    let mut idx = 0usize;
    while idx < VFS_MAX_INODES {
        let node = &mut state.inodes[idx];
        if node.used && node.backend == NodeBackend::Ext2 && node.ext2_ino == ext2_ino {
            node.size = inode.size;
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

fn ext2_effective_max_file_size(meta: &Ext2Meta) -> Result<usize, i64> {
    Ok(core::cmp::min(
        TMPFS_MAX_FILE_SIZE,
        ext2_max_addressable_file_size(meta)?,
    ))
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

fn ext2_data_block_for_logical_index_state(
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

fn ext2_detach_data_block(
    state: &mut VfsState,
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

        ext2_write_indirect_entry_from_state(state, table, remaining, 0)?;
        let table_has_entries = ext2_table_has_nonzero_entries_state(state, table)?;
        if !table_has_entries {
            ext2_free_block(state, table)?;
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

        ext2_write_indirect_entry_from_state(state, l1, second, 0)?;
        let l1_has_entries = ext2_table_has_nonzero_entries_state(state, l1)?;
        if !l1_has_entries {
            ext2_free_block(state, l1)?;
            ext2_write_indirect_entry_from_state(state, table, first, 0)?;
        }

        let table_has_entries = ext2_table_has_nonzero_entries_state(state, table)?;
        if !table_has_entries {
            ext2_free_block(state, table)?;
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

        ext2_write_indirect_entry_from_state(state, l2, third, 0)?;
        let l2_has_entries = ext2_table_has_nonzero_entries_state(state, l2)?;
        if !l2_has_entries {
            ext2_free_block(state, l2)?;
            ext2_write_indirect_entry_from_state(state, l1, second, 0)?;
        }

        let l1_has_entries = ext2_table_has_nonzero_entries_state(state, l1)?;
        if !l1_has_entries {
            ext2_free_block(state, l1)?;
            ext2_write_indirect_entry_from_state(state, table, first, 0)?;
        }

        let table_has_entries = ext2_table_has_nonzero_entries_state(state, table)?;
        if !table_has_entries {
            ext2_free_block(state, table)?;
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
    let size = inode.size as usize;
    if offset as usize >= size {
        return Ok(0);
    }

    let mut copied = 0usize;
    let mut pos = offset as usize;
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
    let max_size = ext2_effective_max_file_size(&meta)?;

    let mut inode = ext2_read_inode_from_state(state, ino)?;
    let start = if append {
        inode.size as usize
    } else {
        usize::try_from(offset).map_err(|_| EINVAL)?
    };

    if start > max_size {
        return Err(ENOSPC);
    }

    if src.is_empty() {
        return Ok(WriteResult {
            written: 0,
            new_offset: start as u64,
        });
    }

    let old_size = inode.size as usize;
    if start > old_size {
        ext2_write_zero_range(state, &mut inode, old_size, start)?;
    }

    let room = max_size - start;
    if room == 0 {
        return Err(ENOSPC);
    }

    let count = core::cmp::min(src.len(), room);
    let mut copied = 0usize;
    let mut pos = start;
    while copied < count {
        let block_idx = pos / meta.block_size;
        let block_off = pos % meta.block_size;
        let chunk = core::cmp::min(count - copied, meta.block_size - block_off);
        let block_no = ext2_ensure_data_block(state, &mut inode, block_idx)?;

        {
            let blk = ext2_block_slice_mut_state(state, block_no)?;
            blk[block_off..block_off + chunk].copy_from_slice(&src[copied..copied + chunk]);
        }
        ext2_mark_dirty_block(state, block_no)?;

        copied += chunk;
        pos += chunk;
    }

    let end = start + count;
    if end > inode.size as usize {
        inode.size = end as u32;
    }
    let now = fs_time_tick(state) as u32;
    inode.mtime = now;
    inode.ctime = now;
    inode.atime = now;
    inode.block_count_512 = ext2_recalc_inode_block_count_512_state(state, &inode)?;
    ext2_write_inode_to_state(state, ino, &inode)?;
    ext2_refresh_alias_nodes(state, ino, &inode);

    Ok(WriteResult {
        written: count,
        new_offset: end as u64,
    })
}

fn ext2_free_indirect_tree_blocks(
    state: &mut VfsState,
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
            if depth == 1 {
                ext2_free_block(state, child)?;
            } else {
                ext2_free_indirect_tree_blocks(state, child, depth - 1)?;
            }
        }
        idx += 1;
    }

    ext2_free_block(state, table_block)
}

fn ext2_truncate_to_zero(state: &mut VfsState, ino: u32) -> Result<(), i64> {
    let mut inode = ext2_read_inode_from_state(state, ino)?;

    let mut i = 0usize;
    while i < EXT2_INODE_DIRECT_COUNT {
        let block = inode.blocks[i];
        if block != 0 {
            ext2_free_block(state, block)?;
            inode.blocks[i] = 0;
        }
        i += 1;
    }

    if inode.blocks[12] != 0 {
        ext2_free_indirect_tree_blocks(state, inode.blocks[12], 1)?;
        inode.blocks[12] = 0;
    }
    if inode.blocks[13] != 0 {
        ext2_free_indirect_tree_blocks(state, inode.blocks[13], 2)?;
        inode.blocks[13] = 0;
    }
    if inode.blocks[14] != 0 {
        ext2_free_indirect_tree_blocks(state, inode.blocks[14], 3)?;
        inode.blocks[14] = 0;
    }

    inode.size = 0;
    inode.block_count_512 = 0;
    let now = fs_time_tick(state) as u32;
    inode.mtime = now;
    inode.ctime = now;
    inode.atime = now;
    ext2_write_inode_to_state(state, ino, &inode)?;
    ext2_refresh_alias_nodes(state, ino, &inode);
    Ok(())
}

fn ext2_truncate_to_length(state: &mut VfsState, ino: u32, length: usize) -> Result<(), i64> {
    let meta = state.ext2_meta;
    let max_size = ext2_effective_max_file_size(&meta)?;
    if length > max_size || length > u32::MAX as usize {
        return Err(ENOSPC);
    }

    let mut inode = ext2_read_inode_from_state(state, ino)?;
    let old_size = inode.size as usize;

    if length > old_size {
        ext2_write_zero_range(state, &mut inode, old_size, length)?;
    } else if length < old_size {
        let old_blocks = ext2_logical_blocks_for_size(&meta, old_size)?;
        let new_blocks = ext2_logical_blocks_for_size(&meta, length)?;

        if length > 0 && (length % meta.block_size) != 0 {
            let block_idx = length / meta.block_size;
            let block_off = length % meta.block_size;
            let block_no = ext2_data_block_for_logical_index_state(state, &inode, block_idx)?;
            if block_no != 0 {
                {
                    let blk = ext2_block_slice_mut_state(state, block_no)?;
                    blk[block_off..].fill(0);
                }
                ext2_mark_dirty_block(state, block_no)?;
            }
        }

        let mut idx = new_blocks;
        while idx < old_blocks {
            let removed = ext2_detach_data_block(state, &mut inode, idx)?;
            if removed != 0 {
                ext2_free_block(state, removed)?;
            }
            idx += 1;
        }
    }

    inode.size = length as u32;
    let now = fs_time_tick(state) as u32;
    inode.mtime = now;
    inode.ctime = now;
    inode.atime = now;
    inode.block_count_512 = ext2_recalc_inode_block_count_512_state(state, &inode)?;
    ext2_write_inode_to_state(state, ino, &inode)?;
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
    let mut idx = 0usize;
    while idx < EXT2_INODE_DIRECT_COUNT {
        let block = inode.blocks[idx];
        if block != 0 {
            ext2_free_block(state, block)?;
            inode.blocks[idx] = 0;
        }
        idx += 1;
    }

    if inode.blocks[12] != 0 {
        ext2_free_indirect_tree_blocks(state, inode.blocks[12], 1)?;
        inode.blocks[12] = 0;
    }

    if inode.blocks[13] != 0 {
        ext2_free_indirect_tree_blocks(state, inode.blocks[13], 2)?;
        inode.blocks[13] = 0;
    }

    if inode.blocks[14] != 0 {
        ext2_free_indirect_tree_blocks(state, inode.blocks[14], 3)?;
        inode.blocks[14] = 0;
    }

    inode.size = 0;
    inode.block_count_512 = 0;
    Ok(())
}

fn ext2_patch_dotdot(state: &mut VfsState, dir_ino: u32, parent_ino: u32) -> Result<(), i64> {
    let inode = ext2_read_inode_from_state(state, dir_ino)?;
    if ext2_mode_to_filetype(inode.mode) != Some(FileType::Directory) {
        return Err(ENOTDIR);
    }
    let block = inode.blocks[0];
    if block == 0 {
        return Err(EIO);
    }

    let mut patched = false;
    {
        let blk = ext2_block_slice_mut_state(state, block)?;
        let mut pos = 0usize;
        while pos + 8 <= blk.len() {
            let rec_len = read_u16_le(blk, pos + 4)? as usize;
            let name_len = blk[pos + 6] as usize;
            if rec_len < 8 || (rec_len & 0x3) != 0 || pos + rec_len > blk.len() {
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
        ext2_mark_dirty_block(state, block)?;
        Ok(())
    } else {
        Err(EIO)
    }
}

fn ext2_create_child_on_disk(
    state: &mut VfsState,
    parent_ino: u32,
    name: &[u8],
    file_type: FileType,
) -> Result<(u32, u32), i64> {
    let is_dir = file_type == FileType::Directory;
    let ino = ext2_alloc_inode(state, is_dir)?;
    let meta = state.ext2_meta;
    let now = fs_time_tick(state) as u32;

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
    };

    if is_dir {
        let block = match ext2_alloc_block(state) {
            Ok(v) => v,
            Err(err) => {
                let _ = ext2_free_inode(state, ino, true);
                return Err(err);
            }
        };
        inode.blocks[0] = block;
        inode.size = meta.block_size as u32;
        inode.block_count_512 = ext2_recalc_inode_block_count_512_state(state, &inode)?;

        {
            let blk = ext2_block_slice_mut_state(state, block)?;
            blk.fill(0);
            ext2_write_dirent(blk, 0, ino, 12, FileType::Directory, b".")?;
            ext2_write_dirent(
                blk,
                12,
                parent_ino,
                meta.block_size - 12,
                FileType::Directory,
                b"..",
            )?;
        }
        ext2_mark_dirty_block(state, block)?;
    }

    if let Err(err) = ext2_write_inode_to_state(state, ino, &inode) {
        if inode.blocks[0] != 0 {
            let _ = ext2_free_block(state, inode.blocks[0]);
        }
        let _ = ext2_free_inode(state, ino, is_dir);
        return Err(err);
    }

    if let Err(err) = ext2_add_dir_entry(state, parent_ino, ino, name, file_type) {
        if inode.blocks[0] != 0 {
            let _ = ext2_free_block(state, inode.blocks[0]);
        }
        let _ = ext2_free_inode(state, ino, is_dir);
        return Err(err);
    }

    if is_dir {
        let mut parent = ext2_read_inode_from_state(state, parent_ino)?;
        parent.links_count = parent.links_count.saturating_add(1);
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
    if find_child(state, parent, name).is_some() {
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
        node_ref.size = ext2_inode.size;
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
    if !state.ext2_mounted || (dir as usize) >= VFS_MAX_INODES {
        return Ok(());
    }

    let node = state.inodes[dir as usize];
    if !node.used || node.file_type != FileType::Directory || node.backend != NodeBackend::Ext2 {
        return Ok(());
    }
    if node.ext2_ino == 0 {
        return Err(EIO);
    }
    if node.ext2_children_loaded {
        return Ok(());
    }

    let ext2_inode = ext2_read_inode_from_state(state, node.ext2_ino)?;
    if ext2_mode_to_filetype(ext2_inode.mode) != Some(FileType::Directory) {
        return Err(EIO);
    }
    let mut entry_count = 0usize;
    let mut entry_ino = [0u32; VFS_MAX_CHILDREN];
    let mut entry_name_len = [0u8; VFS_MAX_CHILDREN];
    let mut entry_name = [[0u8; VFS_MAX_NAME]; VFS_MAX_CHILDREN];
    ext2_for_each_dir_entry_state(state, &ext2_inode, |child_ino, _ft, child_name| {
        if entry_count >= VFS_MAX_CHILDREN {
            return Err(ENOSPC);
        }
        if child_name.len() >= VFS_MAX_NAME {
            return Err(ENAMETOOLONG);
        }
        entry_ino[entry_count] = child_ino;
        entry_name_len[entry_count] = child_name.len() as u8;
        entry_name[entry_count][..child_name.len()].copy_from_slice(child_name);
        entry_count += 1;
        Ok(())
    })?;
    let mut idx = 0usize;
    while idx < entry_count {
        let name_len = entry_name_len[idx] as usize;
        ext2_import_node(state, dir, &entry_name[idx][..name_len], entry_ino[idx])?;
        idx += 1;
    }

    let node_ref = &mut state.inodes[dir as usize];
    node_ref.size = ext2_inode.size;
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
    let mut children = [0u16; VFS_MAX_CHILDREN];
    let child_count = state.inodes[inode as usize].child_count as usize;

    let mut idx = 0usize;
    while idx < child_count {
        children[idx] = state.inodes[inode as usize].children[idx];
        idx += 1;
    }

    idx = 0;
    while idx < child_count {
        let child = children[idx];
        if (child as usize) < VFS_MAX_INODES && state.inodes[child as usize].used {
            clear_inode_subtree(state, child);
        }
        idx += 1;
    }

    clear_inode(&mut state.inodes[inode as usize]);
}

fn rollback_ext2_mount_import(state: &mut VfsState, mount_inode: u16) {
    let mut children = [0u16; VFS_MAX_CHILDREN];
    let child_count = state.inodes[mount_inode as usize].child_count as usize;

    let mut idx = 0usize;
    while idx < child_count {
        children[idx] = state.inodes[mount_inode as usize].children[idx];
        idx += 1;
    }

    idx = 0;
    while idx < child_count {
        let child = children[idx];
        if (child as usize) < VFS_MAX_INODES && state.inodes[child as usize].used {
            clear_inode_subtree(state, child);
        }
        idx += 1;
    }

    let mount = &mut state.inodes[mount_inode as usize];
    mount.backend = NodeBackend::Tmpfs;
    mount.ext2_ino = 0;
    mount.ext2_children_loaded = true;
    mount.child_count = 0;
    mount.children = [0; VFS_MAX_CHILDREN];
    mount.size = 0;
    mount.links_count = 2;

    mount_table_remove_by_inode(state, mount_inode);
    state.ext2_mounted = false;
    state.ext2_mount_inode = ROOT_INODE;
    state.ext2_meta = Ext2Meta::empty();
    state.ext2_journal = Ext2JournalState::empty();
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

fn backend_mutable(backend: NodeBackend) -> bool {
    matches!(backend, NodeBackend::Tmpfs | NodeBackend::Ext2)
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
        if !backend_mutable(inode_backend) {
            match inode_backend {
                // Character-device style nodes tolerate O_TRUNC as a no-op.
                NodeBackend::DevNull
                | NodeBackend::DevZero
                | NodeBackend::DevConsole
                | NodeBackend::DevUrandom
                | NodeBackend::DevTty
                | NodeBackend::DevPtmx
                | NodeBackend::DevPtsSlave => {}
                _ => return Err(EROFS),
            }
        }

        match inode_backend {
            NodeBackend::Tmpfs => {
                state.inodes[inode_idx as usize].size = 0;
                touch_inode_metadata(state, inode_idx, true, true, true);
            }
            NodeBackend::Ext2 => {
                if inode_ext2_ino == 0 {
                    return Err(EIO);
                }
                ext2_truncate_to_zero(state, inode_ext2_ino)?;
                state.inodes[inode_idx as usize].size = 0;
                touch_inode_metadata(state, inode_idx, true, true, true);
            }
            _ => {}
        }
    }

    let pts_id = if inode_backend == NodeBackend::DevPtsSlave {
        state.inodes[inode_idx as usize].ext2_ino as u16
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
        pts_id,
    })
}

fn dir_entry_at_offset(
    state: &mut VfsState,
    inode: u16,
    offset: u64,
) -> Result<Option<DirEntry>, i64> {
    if (inode as usize) >= VFS_MAX_INODES {
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
            if idx >= node.child_count as usize {
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
    if (inode as usize) >= VFS_MAX_INODES {
        return Err(ENOENT);
    }
    let node = state.inodes[inode as usize];
    if !node.used {
        return Err(ENOENT);
    }

    let mut stat = FileStat {
        ino: inode_number(&node, inode),
        mode: inode_mode_for_stat(&node),
        uid: node.uid as u32,
        gid: node.gid as u32,
        nlink: core::cmp::max(1, node.links_count as u32),
        size: node.size as u64,
        blksize: 4096,
        blocks: ((node.size as u64) + 511) / 512,
        atime: node.atime,
        mtime: node.mtime,
        ctime: node.ctime,
        file_type: node.file_type,
    };

    if node.backend == NodeBackend::Ext2 {
        if node.ext2_ino == 0 {
            return Err(EIO);
        }
        let ext = ext2_read_inode_from_state(state, node.ext2_ino)?;
        stat.ino = node.ext2_ino as u64;
        stat.mode = ext.mode as u32;
        stat.uid = ext.uid as u32;
        stat.gid = ext.gid as u32;
        stat.nlink = core::cmp::max(1, ext.links_count as u32);
        stat.size = ext.size as u64;
        stat.blksize = state.ext2_meta.block_size as u32;
        stat.blocks = ext.block_count_512 as u64;
        stat.atime = ext.atime as u64;
        stat.mtime = ext.mtime as u64;
        stat.ctime = ext.ctime as u64;
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
    if !backend_mutable(parent_backend) {
        return Err(EROFS);
    }

    let child_ref = &state.inodes[child as usize];
    let child_file_type = child_ref.file_type;
    let child_child_count = child_ref.child_count;
    let child_ext2_ino = child_ref.ext2_ino;
    if remove_dir {
        if child_file_type != FileType::Directory {
            return Err(ENOTDIR);
        }
        if child_child_count != 0 {
            return Err(ENOTEMPTY);
        }
    } else if child_file_type == FileType::Directory {
        return Err(EISDIR);
    }

    if parent_backend == NodeBackend::Ext2 {
        if state.inodes[parent as usize].ext2_ino == 0 || child_ext2_ino == 0 {
            return Err(EIO);
        }
        let parent_ino = state.inodes[parent as usize].ext2_ino;
        let removed = ext2_remove_dir_entry(state, parent_ino, &name[..name_len])?;
        if removed != child_ext2_ino {
            return Err(EIO);
        }

        let mut child_ext = ext2_read_inode_from_state(state, child_ext2_ino)?;
        if remove_dir {
            if !ext2_directory_is_empty(state, child_ext2_ino)? {
                return Err(ENOTEMPTY);
            }
            ext2_free_inode_blocks(state, &mut child_ext)?;
            ext2_free_inode(state, child_ext2_ino, true)?;
            let mut parent_ext = ext2_read_inode_from_state(state, parent_ino)?;
            parent_ext.links_count = parent_ext.links_count.saturating_sub(1);
            parent_ext.mtime = fs_time_tick(state) as u32;
            parent_ext.ctime = parent_ext.mtime;
            ext2_write_inode_to_state(state, parent_ino, &parent_ext)?;
        } else {
            if child_ext.links_count > 1 {
                child_ext.links_count -= 1;
                let now = fs_time_tick(state) as u32;
                child_ext.ctime = now;
                child_ext.mtime = now;
                ext2_write_inode_to_state(state, child_ext2_ino, &child_ext)?;
                ext2_refresh_alias_nodes(state, child_ext2_ino, &child_ext);
            } else {
                ext2_free_inode_blocks(state, &mut child_ext)?;
                ext2_free_inode(state, child_ext2_ino, false)?;
            }
        }
    }

    remove_child(state, parent, child)?;
    clear_inode(&mut state.inodes[child as usize]);
    touch_inode_metadata(state, parent, true, true, true);
    Ok(())
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
    if !backend_mutable(old_backend) {
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

        let removed = ext2_remove_dir_entry(state, old_parent_ino, &old_name[..old_len])?;
        if removed != child_ext2_ino {
            return Err(EIO);
        }
        if let Err(err) = ext2_add_dir_entry(
            state,
            new_parent_ino,
            child_ext2_ino,
            &new_name[..new_len],
            child_file_type,
        ) {
            let _ = ext2_add_dir_entry(
                state,
                old_parent_ino,
                child_ext2_ino,
                &old_name[..old_len],
                child_file_type,
            );
            return Err(err);
        }

        if child_file_type == FileType::Directory && old_parent_ino != new_parent_ino {
            ext2_patch_dotdot(state, child_ext2_ino, new_parent_ino)?;
            let mut old_parent_inode = ext2_read_inode_from_state(state, old_parent_ino)?;
            old_parent_inode.links_count = old_parent_inode.links_count.saturating_sub(1);
            ext2_write_inode_to_state(state, old_parent_ino, &old_parent_inode)?;

            let mut new_parent_inode = ext2_read_inode_from_state(state, new_parent_ino)?;
            new_parent_inode.links_count = new_parent_inode.links_count.saturating_add(1);
            ext2_write_inode_to_state(state, new_parent_ino, &new_parent_inode)?;
        }

        let mut ext_child = ext2_read_inode_from_state(state, child_ext2_ino)?;
        let now = fs_time_tick(state) as u32;
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

    let old_node = state.inodes[old_inode as usize];
    if !old_node.used {
        return Err(ENOENT);
    }
    if old_node.file_type == FileType::Directory {
        return Err(EPERM);
    }

    let new_backend = state.inodes[new_parent as usize].backend;
    if old_node.backend != new_backend {
        return Err(EINVAL);
    }
    if !backend_mutable(old_node.backend) {
        return Err(EROFS);
    }
    if state.inodes[new_parent as usize].child_count as usize >= VFS_MAX_CHILDREN {
        return Err(ENOSPC);
    }
    if !has_free_vfs_inode(state) {
        return Err(ENOSPC);
    }

    match old_node.backend {
        NodeBackend::Tmpfs => {
            let new_inode = alloc_inode(
                state,
                old_node.file_type,
                NodeBackend::Tmpfs,
                new_parent,
                &new_name[..new_len],
            )?;
            {
                let now = fs_time_tick(state);
                let old_links = state.inodes[old_inode as usize]
                    .links_count
                    .saturating_add(1);
                let old = &mut state.inodes[old_inode as usize];
                old.links_count = old_links;
                old.ctime = now;

                let link = &mut state.inodes[new_inode as usize];
                link.size = old_node.size;
                link.data[..old_node.size as usize]
                    .copy_from_slice(&old_node.data[..old_node.size as usize]);
                link.mode = old_node.mode;
                link.uid = old_node.uid;
                link.gid = old_node.gid;
                link.links_count = old_links;
                link.atime = old_node.atime;
                link.mtime = old_node.mtime;
                link.ctime = now;
            }
            add_child(state, new_parent, new_inode)?;
            touch_inode_metadata(state, new_parent, true, true, true);
            Ok(())
        }
        NodeBackend::Ext2 => {
            let old_ext2_ino = old_node.ext2_ino;
            let new_parent_ext2_ino = state.inodes[new_parent as usize].ext2_ino;
            if old_ext2_ino == 0 || new_parent_ext2_ino == 0 {
                return Err(EIO);
            }

            ext2_add_dir_entry(
                state,
                new_parent_ext2_ino,
                old_ext2_ino,
                &new_name[..new_len],
                old_node.file_type,
            )?;

            let mut ext_inode = ext2_read_inode_from_state(state, old_ext2_ino)?;
            ext_inode.links_count = ext_inode.links_count.saturating_add(1);
            ext_inode.ctime = fs_time_tick(state) as u32;
            ext2_write_inode_to_state(state, old_ext2_ino, &ext_inode)?;

            let new_inode = match alloc_inode(
                state,
                old_node.file_type,
                NodeBackend::Ext2,
                new_parent,
                &new_name[..new_len],
            ) {
                Ok(inode) => inode,
                Err(err) => {
                    let _ = ext2_remove_dir_entry(state, new_parent_ext2_ino, &new_name[..new_len]);
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
                let _ = ext2_remove_dir_entry(state, new_parent_ext2_ino, &new_name[..new_len]);
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
    if !backend_mutable(parent_backend) {
        return Err(EROFS);
    }
    if state.inodes[parent as usize].child_count as usize >= VFS_MAX_CHILDREN {
        return Err(ENOSPC);
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
                node.size = target_bytes.len() as u32;
                node.data[..target_bytes.len()].copy_from_slice(target_bytes);
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

            let (ext2_ino, _) =
                ext2_create_child_on_disk(state, parent_ino, &name[..name_len], FileType::Symlink)?;

            if !target_bytes.is_empty() {
                let wrote = match ext2_write_file(state, ext2_ino, 0, target_bytes, false) {
                    Ok(w) => w,
                    Err(err) => {
                        let _ = ext2_remove_dir_entry(state, parent_ino, &name[..name_len]);
                        let mut child = ext2_read_inode_from_state(state, ext2_ino)?;
                        let _ = ext2_free_inode_blocks(state, &mut child);
                        let _ = ext2_free_inode(state, ext2_ino, false);
                        return Err(err);
                    }
                };
                if wrote.written != target_bytes.len() {
                    let _ = ext2_remove_dir_entry(state, parent_ino, &name[..name_len]);
                    let mut child = ext2_read_inode_from_state(state, ext2_ino)?;
                    let _ = ext2_free_inode_blocks(state, &mut child);
                    let _ = ext2_free_inode(state, ext2_ino, false);
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
                    let _ = ext2_remove_dir_entry(state, parent_ino, &name[..name_len]);
                    if let Ok(mut child) = ext2_read_inode_from_state(state, ext2_ino) {
                        let _ = ext2_free_inode_blocks(state, &mut child);
                    }
                    let _ = ext2_free_inode(state, ext2_ino, false);
                    return Err(err);
                }
            };

            let child = ext2_read_inode_from_state(state, ext2_ino)?;
            {
                let node = &mut state.inodes[inode as usize];
                node.ext2_ino = ext2_ino;
                node.size = child.size;
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
                let _ = ext2_remove_dir_entry(state, parent_ino, &name[..name_len]);
                if let Ok(mut child) = ext2_read_inode_from_state(state, ext2_ino) {
                    let _ = ext2_free_inode_blocks(state, &mut child);
                }
                let _ = ext2_free_inode(state, ext2_ino, false);
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
    if inode as usize >= VFS_MAX_INODES {
        return Err(ENOENT);
    }
    if !state.inodes[inode as usize].used {
        return Err(ENOENT);
    }

    let backend = state.inodes[inode as usize].backend;
    if !backend_mutable(backend) {
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
            if ext2_ino == 0 || atime > u32::MAX as u64 || mtime > u32::MAX as u64 {
                return Err(EINVAL);
            }
            let mut ext_inode = ext2_read_inode_from_state(state, ext2_ino)?;
            ext_inode.atime = atime as u32;
            ext_inode.mtime = mtime as u32;
            ext_inode.ctime = ctime as u32;
            ext2_write_inode_to_state(state, ext2_ino, &ext_inode)?;
            ext2_refresh_alias_nodes(state, ext2_ino, &ext_inode);
            Ok(())
        }
        _ => Err(EROFS),
    }
}

fn chmod_inner(state: &mut VfsState, inode: u16, mode: u16) -> Result<(), i64> {
    if inode as usize >= VFS_MAX_INODES {
        return Err(ENOENT);
    }
    if !state.inodes[inode as usize].used {
        return Err(ENOENT);
    }

    let backend = state.inodes[inode as usize].backend;
    if !backend_mutable(backend) {
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
            ext_inode.ctime = ctime as u32;
            ext2_write_inode_to_state(state, ext2_ino, &ext_inode)?;
            ext2_refresh_alias_nodes(state, ext2_ino, &ext_inode);
            Ok(())
        }
        _ => Err(EROFS),
    }
}

fn chown_inner(state: &mut VfsState, inode: u16, uid: u32, gid: u32) -> Result<(), i64> {
    if inode as usize >= VFS_MAX_INODES {
        return Err(ENOENT);
    }
    if !state.inodes[inode as usize].used {
        return Err(ENOENT);
    }

    let backend = state.inodes[inode as usize].backend;
    if !backend_mutable(backend) {
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
            ext_inode.ctime = ctime as u32;
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

pub fn inode_is_dir(inode: u16) -> Result<bool, i64> {
    with_state(|state| {
        ensure_init(state);
        if (inode as usize) >= VFS_MAX_INODES || !state.inodes[inode as usize].used {
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
        unlink_at_inner(state, start, path, false)
    })
}

pub fn rmdir(path: &str) -> Result<(), i64> {
    rmdirat(ROOT_INODE, path)
}

pub fn rmdirat(start: u16, path: &str) -> Result<(), i64> {
    with_state(|state| {
        ensure_init(state);
        unlink_at_inner(state, start, path, true)
    })
}

pub fn rename(old_path: &str, new_path: &str) -> Result<(), i64> {
    renameat(ROOT_INODE, old_path, ROOT_INODE, new_path)
}

pub fn renameat(old_start: u16, old_path: &str, new_start: u16, new_path: &str) -> Result<(), i64> {
    with_state(|state| {
        ensure_init(state);
        rename_at_inner(state, old_start, old_path, new_start, new_path)
    })
}

pub fn linkat(old_start: u16, old_path: &str, new_start: u16, new_path: &str) -> Result<(), i64> {
    with_state(|state| {
        ensure_init(state);
        link_at_inner(state, old_start, old_path, new_start, new_path)
    })
}

pub fn symlinkat(start: u16, target: &str, link_path: &str) -> Result<(), i64> {
    with_state(|state| {
        ensure_init(state);
        symlink_at_inner(state, start, target, link_path)
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
        set_times_inner(state, inode, atime, mtime)
    })
}

pub fn chmod(inode: u16, mode: u16) -> Result<(), i64> {
    with_state(|state| {
        ensure_init(state);
        chmod_inner(state, inode, mode)
    })
}

pub fn chown(inode: u16, uid: u32, gid: u32) -> Result<(), i64> {
    with_state(|state| {
        ensure_init(state);
        chown_inner(state, inode, uid, gid)
    })
}

pub fn read(inode: u16, offset: u64, dst: &mut [u8]) -> Result<usize, i64> {
    with_state(|state| {
        ensure_init(state);

        if inode as usize >= VFS_MAX_INODES {
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
                NodeBackend::DevTty | NodeBackend::DevPtmx | NodeBackend::DevPtsSlave => 0,
            }
        };
        touch_inode_metadata(state, inode, true, false, false);
        Ok(out)
    })
}

pub fn write(inode: u16, offset: u64, src: &[u8], append: bool) -> Result<WriteResult, i64> {
    with_state(|state| {
        ensure_init(state);

        if inode as usize >= VFS_MAX_INODES {
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

                let current_size = node_ref.size as usize;
                if start > current_size {
                    node_ref.data[current_size..start].fill(0);
                }

                let remaining = TMPFS_MAX_FILE_SIZE - start;
                if remaining == 0 {
                    return Err(ENOSPC);
                }

                let count = core::cmp::min(src.len(), remaining);
                node_ref.data[start..start + count].copy_from_slice(&src[..count]);

                start += count;
                if start > node_ref.size as usize {
                    node_ref.size = start as u32;
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
                let result = ext2_write_file(state, ext2_ino, offset, src, append)?;
                state.inodes[inode as usize].size =
                    core::cmp::max(state.inodes[inode as usize].size, result.new_offset as u32);
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
            | NodeBackend::DevPtsSlave => {
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

        if inode as usize >= VFS_MAX_INODES {
            return Err(ENOENT);
        }

        let node = state.inodes[inode as usize];
        if !node.used {
            return Err(ENOENT);
        }
        if node.file_type == FileType::Directory {
            return Err(EISDIR);
        }

        let end = match node.backend {
            NodeBackend::Tmpfs => node.size as u64,
            NodeBackend::Ext2 => {
                if node.ext2_ino == 0 {
                    return Err(EIO);
                }
                ext2_read_inode_from_state(state, node.ext2_ino)?.size as u64
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
            | NodeBackend::DevPtsSlave => current,
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

        match node.backend {
            NodeBackend::Tmpfs => {
                if target > TMPFS_MAX_FILE_SIZE as i128 {
                    return Err(EINVAL);
                }
            }
            NodeBackend::Ext2 => {
                let max_size = ext2_effective_max_file_size(&state.ext2_meta)?;
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
            | NodeBackend::DevPtsSlave => {
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

        if inode as usize >= VFS_MAX_INODES {
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
                ext2_read_inode_from_state(state, node.ext2_ino)?.size as u64
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
            | NodeBackend::DevPtsSlave => 0,
        };

        Ok(size)
    })
}

pub fn truncate(inode: u16) -> Result<(), i64> {
    with_state(|state| {
        ensure_init(state);

        if inode as usize >= VFS_MAX_INODES {
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
        if !backend_mutable(backend) {
            return Err(EROFS);
        }

        match backend {
            NodeBackend::Tmpfs => {
                state.inodes[inode as usize].size = 0;
                state.inodes[inode as usize].data.fill(0);
                touch_inode_metadata(state, inode, false, true, true);
            }
            NodeBackend::Ext2 => {
                if ext2_ino == 0 {
                    return Err(EIO);
                }
                ext2_truncate_to_zero(state, ext2_ino)?;
                state.inodes[inode as usize].size = 0;
                touch_inode_metadata(state, inode, false, true, true);
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

        if inode as usize >= VFS_MAX_INODES {
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
        if !backend_mutable(backend) {
            return Err(EROFS);
        }

        match backend {
            NodeBackend::Tmpfs => {
                let new_len = length as usize;
                if new_len > TMPFS_MAX_FILE_SIZE {
                    return Err(ENOSPC);
                }
                let node = &mut state.inodes[inode as usize];
                let old_size = node.size as usize;
                if new_len < old_size {
                    node.data[new_len..old_size].fill(0);
                } else if new_len > old_size {
                    node.data[old_size..new_len].fill(0);
                }
                node.size = new_len as u32;
                touch_inode_metadata(state, inode, false, true, true);
            }
            NodeBackend::Ext2 => {
                if ext2_ino == 0 {
                    return Err(EIO);
                }
                let new_len = usize::try_from(length).map_err(|_| EINVAL)?;
                ext2_truncate_to_length(state, ext2_ino, new_len)?;
                state.inodes[inode as usize].size = new_len as u32;
                touch_inode_metadata(state, inode, false, true, true);
            }
            _ => {}
        }
        Ok(())
    })
}

pub fn inode_backend_is_ext2(inode: u16) -> bool {
    with_state(|state| {
        if inode as usize >= VFS_MAX_INODES {
            return false;
        }
        state.inodes[inode as usize].backend == NodeBackend::Ext2
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
        let start = (block_index as usize).checked_mul(data.len()).ok_or(EIO)?;
        let end = start.checked_add(data.len()).ok_or(EIO)?;
        if end > ctx.image.len() {
            return Err(EIO);
        }
        ctx.image[start..end].copy_from_slice(data);
        ctx.writes = ctx.writes.saturating_add(1);
        Ok(())
    }

    fn mount_ext2_backend_image(image: &[u8], head_len: usize) -> std::boxed::Box<TestBackendCtx> {
        let mut ctx = std::boxed::Box::new(TestBackendCtx {
            image: std::vec::Vec::from(image),
            reads: 0,
            writes: 0,
        });
        let backend = Ext2BlockBackend {
            ctx: (&mut *ctx as *mut TestBackendCtx) as u64,
            read_block: test_ext2_backend_read,
            write_block: test_ext2_backend_write,
        };
        set_ext2_block_backend(backend).expect("set block backend");
        let head_len = core::cmp::min(head_len, image.len());
        mount_ext2_block_backend("/mnt/ext2", image.len(), &image[..head_len])
            .expect("mount ext2 backend");
        ctx
    }

    fn mount_ext2_backend_image_expect_err(image: &[u8], head_len: usize) -> i64 {
        let mut ctx = std::boxed::Box::new(TestBackendCtx {
            image: std::vec::Vec::from(image),
            reads: 0,
            writes: 0,
        });
        let backend = Ext2BlockBackend {
            ctx: (&mut *ctx as *mut TestBackendCtx) as u64,
            read_block: test_ext2_backend_read,
            write_block: test_ext2_backend_write,
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
            assert_eq!(root.child_count, 0);
            assert!(!root.ext2_children_loaded);
        });

        let _ = open("/mnt/ext2/hello.txt", O_RDONLY, 0).expect("open ext2 file");
        with_state(|state| {
            let root = &state.inodes[ext2_root as usize];
            assert!(root.ext2_children_loaded);
            assert!(root.child_count > 0);
        });
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

        with_state(|state| {
            assert!(state.ext2_journal.present);
            assert_eq!(state.ext2_journal.inode, 8);
            assert_eq!(state.ext2_journal.start, 0);
            assert_eq!(state.ext2_journal.sequence, 56);
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

        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 24), 57);
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

        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 24), 57);
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

        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 24), 58);
        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 28), 1);

        assert_eq!(read_u32_be(&ctx.image, first_descriptor_off), 0xC03B_3998);
        assert_eq!(read_u32_be(&ctx.image, first_descriptor_off + 4), 1);
        assert_eq!(read_u32_be(&ctx.image, first_descriptor_off + 8), 56);

        assert_eq!(read_u32_be(&ctx.image, second_descriptor_off), 0xC03B_3998);
        assert_eq!(read_u32_be(&ctx.image, second_descriptor_off + 4), 1);
        assert_eq!(read_u32_be(&ctx.image, second_descriptor_off + 8), 57);
        assert_eq!(read_u32_be(&ctx.image, second_descriptor_off + 12), target_block);
        assert_eq!(read_u32_be(&ctx.image, second_descriptor_off + 16), 0x0000_000A);

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

        assert_eq!(read_u32_be(&ctx.image, journal_super_off + 24), 58);
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
    fn ext2_mount_rejects_unknown_incompat_features() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let mut bad = std::vec::Vec::from(SAMPLE_EXT2);
        let incompat_off = EXT2_SUPERBLOCK_OFFSET + 96;
        let incompat = read_u32_le(&bad, incompat_off).expect("read feature incompat");
        write_u32_le(&mut bad, incompat_off, incompat | 0x40).expect("set unsupported incompat");

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
            assert_eq!(root.child_count, 0);
            assert!(!root.ext2_children_loaded);
        });

        let opened = open("/mnt/ext2/hello.txt", O_RDONLY, 0).expect("open sample file");
        let mut buf = [0u8; 64];
        let n = read(opened.inode, 0, &mut buf).expect("read sample file");
        assert!(n > 0);
        assert!(ctx.reads > reads_after_mount);
        with_state(|state| {
            let root = &state.inodes[ext2_root as usize];
            assert!(root.ext2_children_loaded);
            assert!(root.child_count > 0);
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

        with_state(|state| {
            assert!(ext2_block_cached(state, 0));
            assert!(!ext2_block_cached(state, 1));
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
}
