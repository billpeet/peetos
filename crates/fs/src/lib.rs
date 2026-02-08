#![no_std]

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

const ROOT_INODE: u16 = 0;
const PROCFS_BUFFER_MAX: usize = 512;
const PROC_TICK_HZ: u64 = 100;

const EXT2_SUPERBLOCK_OFFSET: usize = 1024;
const EXT2_SUPER_MAGIC: u16 = 0xEF53;
const EXT2_ROOT_INODE: u32 = 2;
const EXT2_GROUP_DESC_SIZE: usize = 32;
const EXT2_INODE_DIRECT_COUNT: usize = 12;
const EXT2_MAX_RECURSION: u8 = 16;
const EXT2_MAX_IMAGE_SIZE: usize = 512 * 1024;

const EXT2_FT_REG_FILE: u8 = 1;
const EXT2_FT_DIR: u8 = 2;

const EXT2_S_IFMT: u16 = 0xF000;
const EXT2_S_IFDIR: u16 = 0x4000;
const EXT2_S_IFREG: u16 = 0x8000;
const EXT2_DEF_FILE_MODE: u16 = EXT2_S_IFREG | 0o644;
const EXT2_DEF_DIR_MODE: u16 = EXT2_S_IFDIR | 0o755;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FileType {
    None = 0,
    Regular = 1,
    Directory = 2,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenResult {
    pub inode: u16,
    pub file_type: FileType,
    pub is_console: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriteResult {
    pub written: usize,
    pub new_offset: u64,
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
        }
    }
}

struct VfsState {
    initialized: bool,
    inodes: [Inode; VFS_MAX_INODES],
    proc_uptime_ticks: u64,
    proc_total_bytes: u64,
    proc_free_bytes: u64,
    proc_used_bytes: u64,
    ext2_mounted: bool,
    ext2_mount_inode: u16,
    ext2_meta: Ext2Meta,
    ext2_image_len: usize,
    ext2_image: [u8; EXT2_MAX_IMAGE_SIZE],
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
            ext2_mounted: false,
            ext2_mount_inode: ROOT_INODE,
            ext2_meta: Ext2Meta::empty(),
            ext2_image_len: 0,
            ext2_image: [0; EXT2_MAX_IMAGE_SIZE],
        }
    }
}

static mut VFS_STATE: VfsState = VfsState::new();

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
        }
    }
}

#[derive(Clone, Copy)]
struct Ext2Inode {
    mode: u16,
    size: u32,
    links_count: u16,
    block_count_512: u32,
    blocks: [u32; 15],
}

fn state() -> &'static mut VfsState {
    unsafe { &mut VFS_STATE }
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
}

fn reset(state: &mut VfsState) {
    for slot in state.inodes.iter_mut() {
        clear_inode(slot);
    }
    state.proc_uptime_ticks = 0;
    state.proc_total_bytes = 0;
    state.proc_free_bytes = 0;
    state.proc_used_bytes = 0;
    state.ext2_mounted = false;
    state.ext2_mount_inode = ROOT_INODE;
    state.ext2_meta = Ext2Meta::empty();
    state.ext2_image_len = 0;
    state.ext2_image.fill(0);
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

    let _ = mkdir_virtual_inner(state, "/proc");
    let _ = create_virtual_file_inner(state, "/proc/uptime", NodeBackend::ProcUptime);
    let _ = create_virtual_file_inner(state, "/proc/meminfo", NodeBackend::ProcMeminfo);
    let _ = create_virtual_file_inner(state, "/proc/mounts", NodeBackend::ProcMounts);
    let _ = create_virtual_file_inner(state, "/proc/version", NodeBackend::ProcVersion);
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
            let inode = &mut state.inodes[slot];
            clear_inode(inode);
            inode.used = true;
            inode.file_type = file_type;
            inode.backend = backend;
            inode.parent = parent;
            inode.name_len = name.len() as u8;
            inode.name[..name.len()].copy_from_slice(name);
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

fn lookup_path_inner(state: &VfsState, path: &str) -> Result<u16, i64> {
    if path.is_empty() {
        return Err(EINVAL);
    }

    if path == "/" {
        return Ok(ROOT_INODE);
    }

    let bytes = path.as_bytes();
    let mut cursor = if bytes[0] == b'/' { 1usize } else { 0usize };
    let mut current = ROOT_INODE;
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

        let child = find_child(state, current, &name[..len]).ok_or(ENOENT)?;
        current = child;
    }

    Ok(current)
}

fn resolve_parent_inner(
    state: &VfsState,
    path: &str,
    out_name: &mut [u8; VFS_MAX_NAME],
) -> Result<(u16, usize), i64> {
    if path.is_empty() || path == "/" {
        return Err(EINVAL);
    }

    let bytes = path.as_bytes();
    let mut cursor = if bytes[0] == b'/' { 1usize } else { 0usize };
    let mut current = ROOT_INODE;
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

        let child = find_child(state, current, &component[..len]).ok_or(ENOENT)?;
        if state.inodes[child as usize].file_type != FileType::Directory {
            return Err(ENOTDIR);
        }
        current = child;
    }
}

fn create_regular_inner(state: &mut VfsState, path: &str) -> Result<u16, i64> {
    let mut name = [0u8; VFS_MAX_NAME];
    let (parent, name_len) = resolve_parent_inner(state, path, &mut name)?;

    let parent_backend = state.inodes[parent as usize].backend;
    if !matches!(parent_backend, NodeBackend::Tmpfs | NodeBackend::Ext2) {
        return Err(EROFS);
    }

    if find_child(state, parent, &name[..name_len]).is_some() {
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
        return Ok(inode);
    }

    let inode = alloc_inode(state, FileType::Regular, NodeBackend::Tmpfs, parent, &name[..name_len])?;
    add_child(state, parent, inode)?;
    Ok(inode)
}

fn mkdir_inner(state: &mut VfsState, path: &str) -> Result<(), i64> {
    let mut name = [0u8; VFS_MAX_NAME];
    let (parent, name_len) = resolve_parent_inner(state, path, &mut name)?;

    let parent_backend = state.inodes[parent as usize].backend;
    if !matches!(parent_backend, NodeBackend::Tmpfs | NodeBackend::Ext2) {
        return Err(EROFS);
    }

    if find_child(state, parent, &name[..name_len]).is_some() {
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
        add_child(state, parent, inode)?;
        return Ok(());
    }

    let inode = alloc_inode(state, FileType::Directory, NodeBackend::Tmpfs, parent, &name[..name_len])?;
    add_child(state, parent, inode)?;
    Ok(())
}

fn mkdir_virtual_inner(state: &mut VfsState, path: &str) -> Result<(), i64> {
    let mut name = [0u8; VFS_MAX_NAME];
    let (parent, name_len) = resolve_parent_inner(state, path, &mut name)?;

    if find_child(state, parent, &name[..name_len]).is_some() {
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

    if find_child(state, parent, &name[..name_len]).is_some() {
        return Err(EEXIST);
    }

    let inode = alloc_inode(state, FileType::Regular, backend, parent, &name[..name_len])?;
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

fn has_ext2_mount(state: &VfsState) -> bool {
    if !state.ext2_mounted || (state.ext2_mount_inode as usize) >= VFS_MAX_INODES {
        return false;
    }
    let mount = &state.inodes[state.ext2_mount_inode as usize];
    mount.used && mount.file_type == FileType::Directory && mount.backend == NodeBackend::Ext2
}

fn format_proc_payload(state: &VfsState, backend: NodeBackend, out: &mut [u8]) -> usize {
    let mut cursor = 0usize;
    match backend {
        NodeBackend::ProcVersion => {
            append_str(out, &mut cursor, "PeetOS-Rust 0.1\n");
        }
        NodeBackend::ProcMounts => {
            append_str(out, &mut cursor, "tmpfs / tmpfs rw 0 0\n");
            append_str(out, &mut cursor, "devfs /dev devfs rw 0 0\n");
            append_str(out, &mut cursor, "proc /proc procfs ro 0 0\n");
            if has_ext2_mount(state) {
                append_str(out, &mut cursor, "ext2 /mnt/ext2 ext2 rw 0 0\n");
            }
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

fn ext2_parse_meta(image: &[u8]) -> Result<Ext2Meta, i64> {
    if image.len() < EXT2_SUPERBLOCK_OFFSET + 1024 {
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

    if total_inodes == 0 || total_blocks == 0 || blocks_per_group == 0 || inodes_per_group == 0 {
        return Err(EINVAL);
    }

    let block_size = 1024usize
        .checked_shl(log_block_size)
        .ok_or(EINVAL)?;
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

    let gdt_size = (group_count as usize)
        .checked_mul(EXT2_GROUP_DESC_SIZE)
        .ok_or(EIO)?;
    if gdt_offset
        .checked_add(gdt_size)
        .filter(|end| *end <= image.len())
        .is_none()
    {
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
    })
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

fn ext2_read_inode(image: &[u8], meta: &Ext2Meta, ino: u32) -> Result<Ext2Inode, i64> {
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
    let inode_offset = inode_table_offset
        .checked_add((local as usize) * meta.inode_size)
        .ok_or(EIO)?;
    if inode_offset
        .checked_add(meta.inode_size)
        .filter(|end| *end <= image.len())
        .is_none()
    {
        return Err(EIO);
    }

    let mode = read_u16_le(image, inode_offset)?;
    let size = read_u32_le(image, inode_offset + 4)?;
    let links_count = read_u16_le(image, inode_offset + 26)?;
    let block_count_512 = read_u32_le(image, inode_offset + 28)?;

    let mut blocks = [0u32; 15];
    let mut i = 0usize;
    while i < 15 {
        blocks[i] = read_u32_le(image, inode_offset + 40 + i * 4)?;
        i += 1;
    }

    Ok(Ext2Inode {
        mode,
        size,
        links_count,
        block_count_512,
        blocks,
    })
}

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

fn ext2_group_desc_offset(meta: &Ext2Meta, group: u32) -> Result<usize, i64> {
    if group >= meta.group_count {
        return Err(EIO);
    }
    meta.gdt_offset
        .checked_add((group as usize) * EXT2_GROUP_DESC_SIZE)
        .ok_or(EIO)
}

fn ext2_group_bitmap_block(
    image: &[u8],
    meta: &Ext2Meta,
    group: u32,
    field_offset: usize,
) -> Result<u32, i64> {
    let desc_offset = ext2_group_desc_offset(meta, group)?;
    read_u32_le(image, desc_offset + field_offset)
}

fn ext2_group_block_bitmap_block(image: &[u8], meta: &Ext2Meta, group: u32) -> Result<u32, i64> {
    ext2_group_bitmap_block(image, meta, group, 0)
}

fn ext2_group_inode_bitmap_block(image: &[u8], meta: &Ext2Meta, group: u32) -> Result<u32, i64> {
    ext2_group_bitmap_block(image, meta, group, 4)
}

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
    write_u32_le(image, inode_offset + 4, inode.size)?;
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

fn ext2_recalc_inode_block_count_512(meta: &Ext2Meta, inode: &Ext2Inode) -> u32 {
    let sectors_per_block = (meta.block_size / 512) as u32;
    let mut used = 0u32;
    let mut i = 0usize;
    while i < inode.blocks.len() {
        if inode.blocks[i] != 0 {
            used = used.saturating_add(sectors_per_block);
        }
        i += 1;
    }
    used
}

fn ext2_alloc_block(state: &mut VfsState) -> Result<u32, i64> {
    if !state.ext2_mounted {
        return Err(EINVAL);
    }

    let meta = state.ext2_meta;
    let image = &mut state.ext2_image[..state.ext2_image_len];

    let mut group = 0u32;
    while group < meta.group_count {
        let bitmap_block = ext2_group_block_bitmap_block(image, &meta, group)?;
        let mut found = None;

        {
            let bitmap = ext2_block_slice_mut(image, &meta, bitmap_block)?;
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
                    ext2_bitmap_set(bitmap, bit, true)?;
                    found = Some(block_num);
                    break;
                }
                bit += 1;
            }
        }

        if let Some(block_num) = found {
            ext2_adjust_super_counters(image, -1, 0)?;
            ext2_adjust_group_counters(image, &meta, group, -1, 0, 0)?;
            let blk = ext2_block_slice_mut(image, &meta, block_num)?;
            blk.fill(0);
            return Ok(block_num);
        }

        group += 1;
    }

    Err(ENOSPC)
}

fn ext2_free_block(state: &mut VfsState, block_num: u32) -> Result<(), i64> {
    if !state.ext2_mounted {
        return Err(EINVAL);
    }

    let meta = state.ext2_meta;
    let (group, bit) = ext2_block_group_and_bit(&meta, block_num)?;
    let image = &mut state.ext2_image[..state.ext2_image_len];
    let bitmap_block = ext2_group_block_bitmap_block(image, &meta, group)?;
    let mut was_set = false;

    {
        let bitmap = ext2_block_slice_mut(image, &meta, bitmap_block)?;
        if ext2_bitmap_get(bitmap, bit)? {
            ext2_bitmap_set(bitmap, bit, false)?;
            was_set = true;
        }
    }

    if was_set {
        ext2_adjust_super_counters(image, 1, 0)?;
        ext2_adjust_group_counters(image, &meta, group, 1, 0, 0)?;
    }

    Ok(())
}

fn ext2_alloc_inode(state: &mut VfsState, is_dir: bool) -> Result<u32, i64> {
    if !state.ext2_mounted {
        return Err(EINVAL);
    }

    let meta = state.ext2_meta;
    let image = &mut state.ext2_image[..state.ext2_image_len];
    let mut group = 0u32;

    while group < meta.group_count {
        let bitmap_block = ext2_group_inode_bitmap_block(image, &meta, group)?;
        let mut found = None;

        {
            let bitmap = ext2_block_slice_mut(image, &meta, bitmap_block)?;
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
                    ext2_bitmap_set(bitmap, bit, true)?;
                    found = Some(ino);
                    break;
                }
                bit += 1;
            }
        }

        if let Some(ino) = found {
            ext2_adjust_super_counters(image, 0, -1)?;
            ext2_adjust_group_counters(image, &meta, group, 0, -1, if is_dir { 1 } else { 0 })?;

            let inode_offset = ext2_inode_offset_in_image(image, &meta, ino)?;
            if inode_offset
                .checked_add(meta.inode_size)
                .filter(|end| *end <= image.len())
                .is_none()
            {
                return Err(EIO);
            }
            image[inode_offset..inode_offset + meta.inode_size].fill(0);

            return Ok(ino);
        }

        group += 1;
    }

    Err(ENOSPC)
}

fn ext2_dirent_file_type(file_type: FileType) -> u8 {
    match file_type {
        FileType::Directory => EXT2_FT_DIR,
        FileType::Regular => EXT2_FT_REG_FILE,
        _ => 0,
    }
}

fn ext2_dirent_len(name_len: usize) -> usize {
    (8 + name_len + 3) & !3
}

fn ext2_write_dirent(
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

fn ext2_free_inode(state: &mut VfsState, ino: u32, is_dir: bool) -> Result<(), i64> {
    if !state.ext2_mounted {
        return Err(EINVAL);
    }
    let meta = state.ext2_meta;
    let (group, bit) = ext2_inode_group_and_bit(&meta, ino)?;
    let image = &mut state.ext2_image[..state.ext2_image_len];
    let bitmap_block = ext2_group_inode_bitmap_block(image, &meta, group)?;
    let mut was_set = false;

    {
        let bitmap = ext2_block_slice_mut(image, &meta, bitmap_block)?;
        if ext2_bitmap_get(bitmap, bit)? {
            ext2_bitmap_set(bitmap, bit, false)?;
            was_set = true;
        }
    }

    if was_set {
        ext2_adjust_super_counters(image, 0, 1)?;
        ext2_adjust_group_counters(image, &meta, group, 0, 1, if is_dir { -1 } else { 0 })?;
    }

    let inode_offset = ext2_inode_offset_in_image(image, &meta, ino)?;
    if inode_offset
        .checked_add(meta.inode_size)
        .filter(|end| *end <= image.len())
        .is_none()
    {
        return Err(EIO);
    }
    image[inode_offset..inode_offset + meta.inode_size].fill(0);
    Ok(())
}

fn ext2_read_inode_from_state(state: &VfsState, ino: u32) -> Result<Ext2Inode, i64> {
    if !state.ext2_mounted {
        return Err(EINVAL);
    }
    let image = &state.ext2_image[..state.ext2_image_len];
    ext2_read_inode(image, &state.ext2_meta, ino)
}

fn ext2_write_inode_to_state(state: &mut VfsState, ino: u32, inode: &Ext2Inode) -> Result<(), i64> {
    if !state.ext2_mounted {
        return Err(EINVAL);
    }
    let meta = state.ext2_meta;
    let image = &mut state.ext2_image[..state.ext2_image_len];
    ext2_write_inode(image, &meta, ino, inode)
}

fn ext2_ensure_data_block(
    state: &mut VfsState,
    inode: &mut Ext2Inode,
    block_idx: usize,
) -> Result<u32, i64> {
    if block_idx >= EXT2_INODE_DIRECT_COUNT {
        return Err(ENOSPC);
    }

    if inode.blocks[block_idx] != 0 {
        return Ok(inode.blocks[block_idx]);
    }

    let block = ext2_alloc_block(state)?;
    inode.blocks[block_idx] = block;
    Ok(block)
}

fn ext2_write_zero_range(
    state: &mut VfsState,
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
        if block_idx >= EXT2_INODE_DIRECT_COUNT {
            return Err(ENOSPC);
        }

        let block_no = ext2_ensure_data_block(state, inode, block_idx)?;
        let block_off = start % meta.block_size;
        let chunk = core::cmp::min(end - start, meta.block_size - block_off);

        let image = &mut state.ext2_image[..state.ext2_image_len];
        let blk = ext2_block_slice_mut(image, &meta, block_no)?;
        blk[block_off..block_off + chunk].fill(0);
        start += chunk;
    }

    Ok(())
}

fn ext2_read_file(
    state: &VfsState,
    ino: u32,
    offset: u64,
    dst: &mut [u8],
) -> Result<usize, i64> {
    if dst.is_empty() {
        return Ok(0);
    }

    let meta = state.ext2_meta;
    let image = &state.ext2_image[..state.ext2_image_len];
    let inode = ext2_read_inode(image, &meta, ino)?;
    let size = inode.size as usize;
    if offset as usize >= size {
        return Ok(0);
    }

    let mut copied = 0usize;
    let mut pos = offset as usize;
    while copied < dst.len() && pos < size {
        let block_idx = pos / meta.block_size;
        if block_idx >= EXT2_INODE_DIRECT_COUNT {
            return Err(EIO);
        }

        let block_off = pos % meta.block_size;
        let remain_file = size - pos;
        let chunk = core::cmp::min(meta.block_size - block_off, core::cmp::min(remain_file, dst.len() - copied));
        let block_no = inode.blocks[block_idx];

        if block_no == 0 {
            dst[copied..copied + chunk].fill(0);
        } else {
            let blk = ext2_block_slice(image, &meta, block_no)?;
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
    let max_size = core::cmp::min(TMPFS_MAX_FILE_SIZE, EXT2_INODE_DIRECT_COUNT * meta.block_size);

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

        let image = &mut state.ext2_image[..state.ext2_image_len];
        let blk = ext2_block_slice_mut(image, &meta, block_no)?;
        blk[block_off..block_off + chunk].copy_from_slice(&src[copied..copied + chunk]);

        copied += chunk;
        pos += chunk;
    }

    let end = start + count;
    if end > inode.size as usize {
        inode.size = end as u32;
    }
    inode.block_count_512 = ext2_recalc_inode_block_count_512(&meta, &inode);
    ext2_write_inode_to_state(state, ino, &inode)?;

    Ok(WriteResult {
        written: count,
        new_offset: end as u64,
    })
}

fn ext2_truncate_to_zero(state: &mut VfsState, ino: u32) -> Result<(), i64> {
    let meta = state.ext2_meta;
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

    inode.size = 0;
    inode.block_count_512 = 0;
    let _ = meta; // keep read/write behavior explicit for future extensions
    ext2_write_inode_to_state(state, ino, &inode)?;
    Ok(())
}

fn ext2_for_each_dir_entry<F>(
    image: &[u8],
    meta: &Ext2Meta,
    inode: &Ext2Inode,
    mut f: F,
) -> Result<(), i64>
where
    F: FnMut(u32, u8, &[u8]) -> Result<(), i64>,
{
    let mut block_idx = 0usize;
    while block_idx < EXT2_INODE_DIRECT_COUNT {
        let block_num = inode.blocks[block_idx];
        block_idx += 1;

        if block_num == 0 {
            continue;
        }

        let block = ext2_block_slice(image, meta, block_num)?;
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

fn ext2_add_dir_entry(
    state: &mut VfsState,
    parent_ino: u32,
    child_ino: u32,
    name: &[u8],
    child_type: FileType,
) -> Result<(), i64> {
    if name.is_empty() || name.len() >= 255 {
        return Err(ENAMETOOLONG);
    }

    let meta = state.ext2_meta;
    let mut parent = ext2_read_inode_from_state(state, parent_ino)?;
    if ext2_mode_to_filetype(parent.mode) != Some(FileType::Directory) {
        return Err(ENOTDIR);
    }

    let needed = ext2_dirent_len(name.len());
    let mut idx = 0usize;
    while idx < EXT2_INODE_DIRECT_COUNT {
        let block_no = parent.blocks[idx];
        if block_no == 0 {
            let new_block = ext2_alloc_block(state)?;
            parent.blocks[idx] = new_block;

            let image = &mut state.ext2_image[..state.ext2_image_len];
            let blk = ext2_block_slice_mut(image, &meta, new_block)?;
            blk.fill(0);
            ext2_write_dirent(blk, 0, child_ino, meta.block_size, child_type, name)?;

            let min_size = ((idx + 1) * meta.block_size) as u32;
            if parent.size < min_size {
                parent.size = min_size;
            }
            parent.block_count_512 = ext2_recalc_inode_block_count_512(&meta, &parent);
            ext2_write_inode_to_state(state, parent_ino, &parent)?;
            return Ok(());
        }

        let mut inserted = false;
        {
            let image = &mut state.ext2_image[..state.ext2_image_len];
            let blk = ext2_block_slice_mut(image, &meta, block_no)?;
            let mut pos = 0usize;
            while pos + 8 <= blk.len() {
                let entry_ino = read_u32_le(blk, pos)?;
                let rec_len = read_u16_le(blk, pos + 4)? as usize;
                if rec_len < 8 || (rec_len & 0x3) != 0 || pos + rec_len > blk.len() {
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
            return Ok(());
        }

        idx += 1;
    }

    Err(ENOSPC)
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

    let mut inode = Ext2Inode {
        mode: match file_type {
            FileType::Regular => EXT2_DEF_FILE_MODE,
            FileType::Directory => EXT2_DEF_DIR_MODE,
            FileType::None => return Err(EINVAL),
        },
        size: 0,
        links_count: if is_dir { 2 } else { 1 },
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
        inode.block_count_512 = ext2_recalc_inode_block_count_512(&meta, &inode);

        {
            let image = &mut state.ext2_image[..state.ext2_image_len];
            let blk = ext2_block_slice_mut(image, &meta, block)?;
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
        _ => None,
    }
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
    image: &[u8],
    meta: &Ext2Meta,
    inode_num: u32,
    depth: u8,
) -> Result<(), i64> {
    if depth > EXT2_MAX_RECURSION {
        return Err(EIO);
    }

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

    let ext2_inode = ext2_read_inode(image, meta, inode_num)?;
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
    }

    match file_type {
        FileType::Regular => {}
        FileType::Directory => {
            ext2_for_each_dir_entry(image, meta, &ext2_inode, |child_ino, _ft, child_name| {
                ext2_import_node(
                    state,
                    node,
                    child_name,
                    image,
                    meta,
                    child_ino,
                    depth.saturating_add(1),
                )
            })?;
        }
        FileType::None => {}
    }

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
    mount.child_count = 0;
    mount.children = [0; VFS_MAX_CHILDREN];

    state.ext2_mounted = false;
    state.ext2_mount_inode = ROOT_INODE;
    state.ext2_meta = Ext2Meta::empty();
    state.ext2_image_len = 0;
    state.ext2_image.fill(0);
}

pub fn init() {
    let state = state();
    reset(state);
    ensure_init(state);
}

pub fn set_proc_uptime_ticks(ticks: u64) {
    let state = state();
    ensure_init(state);
    state.proc_uptime_ticks = ticks;
}

pub fn set_proc_meminfo(total_bytes: u64, free_bytes: u64, used_bytes: u64) {
    let state = state();
    ensure_init(state);
    state.proc_total_bytes = total_bytes;
    state.proc_free_bytes = free_bytes;
    state.proc_used_bytes = used_bytes;
}

pub fn mount_ext2_image(mount_path: &str, image: &[u8]) -> Result<(), i64> {
    let state = state();
    ensure_init(state);

    let mount_inode = ensure_dir_path_tmpfs(state, mount_path)?;
    if state.inodes[mount_inode as usize].file_type != FileType::Directory {
        return Err(ENOTDIR);
    }

    if state.inodes[mount_inode as usize].child_count != 0 {
        return Err(EEXIST);
    }

    if image.is_empty() || image.len() > EXT2_MAX_IMAGE_SIZE {
        return Err(ENOSPC);
    }

    state.ext2_image[..image.len()].copy_from_slice(image);
    state.ext2_image_len = image.len();

    let meta = ext2_parse_meta(&state.ext2_image[..state.ext2_image_len])?;
    if meta.blocks_per_group == 0 {
        state.ext2_image_len = 0;
        return Err(EINVAL);
    }

    let root = ext2_read_inode(&state.ext2_image[..state.ext2_image_len], &meta, EXT2_ROOT_INODE)?;
    if ext2_mode_to_filetype(root.mode) != Some(FileType::Directory) {
        state.ext2_image_len = 0;
        return Err(EIO);
    }

    state.ext2_meta = meta;
    state.ext2_mounted = true;
    state.ext2_mount_inode = mount_inode;

    match ext2_for_each_dir_entry(image, &meta, &root, |child_ino, _file_type, name| {
        ext2_import_node(state, mount_inode, name, image, &meta, child_ino, 0)
    }) {
        Ok(()) => {
            let mount = &mut state.inodes[mount_inode as usize];
            mount.backend = NodeBackend::Ext2;
            mount.ext2_ino = EXT2_ROOT_INODE;
            mount.size = root.size;
            Ok(())
        }
        Err(err) => {
            rollback_ext2_mount_import(state, mount_inode);
            Err(err)
        }
    }
}

pub fn export_ext2_image(out: &mut [u8]) -> Result<usize, i64> {
    let state = state();
    ensure_init(state);
    if !state.ext2_mounted {
        return Err(ENOENT);
    }
    if out.len() < state.ext2_image_len {
        return Err(ENOSPC);
    }
    out[..state.ext2_image_len].copy_from_slice(&state.ext2_image[..state.ext2_image_len]);
    Ok(state.ext2_image_len)
}

pub fn mkdir(path: &str) -> Result<(), i64> {
    let state = state();
    ensure_init(state);
    mkdir_inner(state, path)
}

pub fn open(path: &str, flags: u32, _mode: u32) -> Result<OpenResult, i64> {
    let state = state();
    ensure_init(state);

    let access = flags & O_ACCMODE;
    if access > O_RDWR {
        return Err(EINVAL);
    }

    let inode_idx = match lookup_path_inner(state, path) {
        Ok(inode) => {
            if (flags & O_CREAT) != 0 && (flags & O_EXCL) != 0 {
                return Err(EEXIST);
            }
            inode
        }
        Err(ENOENT) if (flags & O_CREAT) != 0 => create_regular_inner(state, path)?,
        Err(err) => return Err(err),
    };

    let inode_info = state.inodes[inode_idx as usize];
    if inode_info.file_type == FileType::Directory {
        if access != O_RDONLY {
            return Err(EISDIR);
        }
    } else if (flags & O_DIRECTORY) != 0 {
        return Err(ENOTDIR);
    }

    if (flags & O_TRUNC) != 0 && inode_info.file_type == FileType::Regular && access != O_RDONLY {
        if !matches!(inode_info.backend, NodeBackend::Tmpfs | NodeBackend::Ext2) {
            return Err(EROFS);
        }

        match inode_info.backend {
            NodeBackend::Tmpfs => {
                state.inodes[inode_idx as usize].size = 0;
            }
            NodeBackend::Ext2 => {
                if inode_info.ext2_ino == 0 {
                    return Err(EIO);
                }
                ext2_truncate_to_zero(state, inode_info.ext2_ino)?;
                state.inodes[inode_idx as usize].size = 0;
            }
            _ => {}
        }
    }

    let opened = state.inodes[inode_idx as usize];

    Ok(OpenResult {
        inode: inode_idx,
        file_type: opened.file_type,
        is_console: opened.backend == NodeBackend::DevConsole,
    })
}

pub fn read(inode: u16, offset: u64, dst: &mut [u8]) -> Result<usize, i64> {
    let state = state();
    ensure_init(state);

    if inode as usize >= VFS_MAX_INODES {
        return Err(ENOENT);
    }

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
            Ok(count)
        }
        NodeBackend::Ext2 => {
            if node.ext2_ino == 0 {
                return Err(EIO);
            }
            ext2_read_file(state, node.ext2_ino, offset, dst)
        }
        NodeBackend::DevNull => Ok(0),
        NodeBackend::DevZero => {
            dst.fill(0);
            Ok(dst.len())
        }
        NodeBackend::DevConsole => Ok(0),
        NodeBackend::ProcUptime
        | NodeBackend::ProcMeminfo
        | NodeBackend::ProcMounts
        | NodeBackend::ProcVersion => read_proc(node, state, offset, dst),
    }
}

pub fn write(inode: u16, offset: u64, src: &[u8], append: bool) -> Result<WriteResult, i64> {
    let state = state();
    ensure_init(state);

    if inode as usize >= VFS_MAX_INODES {
        return Err(ENOENT);
    }

    let node = &state.inodes[inode as usize];
    if !node.used {
        return Err(ENOENT);
    }
    if node.file_type == FileType::Directory {
        return Err(EISDIR);
    }

    match node.backend {
        NodeBackend::Tmpfs => {
            let node = &mut state.inodes[inode as usize];
            let mut start = if append {
                node.size as usize
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

            let current_size = node.size as usize;
            if start > current_size {
                node.data[current_size..start].fill(0);
            }

            let remaining = TMPFS_MAX_FILE_SIZE - start;
            if remaining == 0 {
                return Err(ENOSPC);
            }

            let count = core::cmp::min(src.len(), remaining);
            node.data[start..start + count].copy_from_slice(&src[..count]);

            start += count;
            if start > node.size as usize {
                node.size = start as u32;
            }

            Ok(WriteResult {
                written: count,
                new_offset: start as u64,
            })
        }
        NodeBackend::Ext2 => {
            if node.ext2_ino == 0 {
                return Err(EIO);
            }
            let result = ext2_write_file(state, node.ext2_ino, offset, src, append)?;
            state.inodes[inode as usize].size = core::cmp::max(
                state.inodes[inode as usize].size,
                result.new_offset as u32,
            );
            Ok(result)
        }
        NodeBackend::ProcUptime
        | NodeBackend::ProcMeminfo
        | NodeBackend::ProcMounts
        | NodeBackend::ProcVersion => Err(EROFS),
        NodeBackend::DevNull | NodeBackend::DevZero | NodeBackend::DevConsole => {
            let next = offset.saturating_add(src.len() as u64);
            Ok(WriteResult {
                written: src.len(),
                new_offset: next,
            })
        }
    }
}

pub fn seek(inode: u16, current: u64, offset: i64, whence: u32) -> Result<u64, i64> {
    let state = state();
    ensure_init(state);

    if inode as usize >= VFS_MAX_INODES {
        return Err(ENOENT);
    }

    let node = &state.inodes[inode as usize];
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
        NodeBackend::DevNull | NodeBackend::DevZero | NodeBackend::DevConsole => current,
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
            let max_size =
                core::cmp::min(TMPFS_MAX_FILE_SIZE, EXT2_INODE_DIRECT_COUNT * state.ext2_meta.block_size);
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
        NodeBackend::DevNull | NodeBackend::DevZero | NodeBackend::DevConsole => {
            if target > u64::MAX as i128 {
                return Err(EINVAL);
            }
        }
    }

    Ok(target as u64)
}

pub fn file_size(inode: u16) -> Result<u64, i64> {
    let state = state();
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
        NodeBackend::DevNull | NodeBackend::DevZero | NodeBackend::DevConsole => 0,
    };

    Ok(size)
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());
    static SAMPLE_EXT2: &[u8] = include_bytes!("../../../user/sample_ext2.img");

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

        mount_ext2_image("/mnt/ext2", SAMPLE_EXT2).expect("mount sample ext2");
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
    }

    #[test]
    fn ext2_mount_invalid_image_rolls_back_mount_state() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        let mut bad = std::vec::Vec::from(SAMPLE_EXT2);
        bad[EXT2_SUPERBLOCK_OFFSET + 56] = 0;
        bad[EXT2_SUPERBLOCK_OFFSET + 57] = 0;

        let err = mount_ext2_image("/mnt/ext2", &bad).unwrap_err();
        assert_eq!(err, EINVAL);

        let created =
            open("/mnt/ext2/fallback.txt", O_CREAT | O_RDWR, 0).expect("tmpfs path still writable");
        let _ = write(created.inode, 0, b"ok", false).expect("write fallback");
    }

    #[test]
    fn ext2_mutations_persist_in_exported_image() {
        let _guard = TEST_LOCK.lock().expect("lock should work");
        init();

        mount_ext2_image("/mnt/ext2", SAMPLE_EXT2).expect("mount sample ext2");

        let created = open("/mnt/ext2/persist.txt", O_CREAT | O_RDWR, 0).expect("create persist");
        let _ = write(created.inode, 0, b"persist", false).expect("write persist");
        mkdir("/mnt/ext2/persist_dir").expect("mkdir persist dir");
        let hello = open("/mnt/ext2/hello.txt", O_WRONLY | O_TRUNC, 0).expect("truncate hello");
        let _ = write(hello.inode, 0, b"rewritten", false).expect("rewrite hello");

        let mut exported = [0u8; EXT2_MAX_IMAGE_SIZE];
        let image_len = export_ext2_image(&mut exported).expect("export image");

        let remount_image = std::vec::Vec::from(&exported[..image_len]);
        init();
        mount_ext2_image("/mnt/ext2", &remount_image).expect("remount exported image");

        let persist = open("/mnt/ext2/persist.txt", O_RDONLY, 0).expect("open persisted file");
        let mut persist_buf = [0u8; 16];
        let persist_n = read(persist.inode, 0, &mut persist_buf).expect("read persisted file");
        assert_eq!(&persist_buf[..persist_n], b"persist");

        let persisted_hello = open("/mnt/ext2/hello.txt", O_RDONLY, 0).expect("open rewritten hello");
        let mut hello_buf = [0u8; 16];
        let hello_n = read(persisted_hello.inode, 0, &mut hello_buf).expect("read rewritten hello");
        assert_eq!(&hello_buf[..hello_n], b"rewritten");

        let nested = open("/mnt/ext2/persist_dir/new.txt", O_CREAT | O_RDWR, 0)
            .expect("create in persisted dir");
        let _ = write(nested.inode, 0, b"x", false).expect("write nested");
    }
}
