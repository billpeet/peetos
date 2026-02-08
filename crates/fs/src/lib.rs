#![no_std]

pub const VFS_MAX_PATH: usize = 256;
pub const VFS_MAX_NAME: usize = 64;
pub const TMPFS_MAX_INODES: usize = 128;
pub const TMPFS_MAX_CHILDREN: usize = 32;
pub const TMPFS_MAX_FILE_SIZE: usize = 16 * 1024;

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

pub const ENOENT: i64 = 2;
pub const EIO: i64 = 5;
pub const EEXIST: i64 = 17;
pub const ENOTDIR: i64 = 20;
pub const EISDIR: i64 = 21;
pub const EINVAL: i64 = 22;
pub const ENOSPC: i64 = 28;
pub const ENAMETOOLONG: i64 = 36;

const ROOT_INODE: u16 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FileType {
    None = 0,
    Regular = 1,
    Directory = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenResult {
    pub inode: u16,
    pub file_type: FileType,
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
    parent: u16,
    name: [u8; VFS_MAX_NAME],
    name_len: u8,
    size: u32,
    child_count: u8,
    children: [u16; TMPFS_MAX_CHILDREN],
    data: [u8; TMPFS_MAX_FILE_SIZE],
}

impl Inode {
    const fn empty() -> Self {
        Self {
            used: false,
            file_type: FileType::None,
            parent: ROOT_INODE,
            name: [0; VFS_MAX_NAME],
            name_len: 0,
            size: 0,
            child_count: 0,
            children: [0; TMPFS_MAX_CHILDREN],
            data: [0; TMPFS_MAX_FILE_SIZE],
        }
    }
}

struct VfsState {
    initialized: bool,
    inodes: [Inode; TMPFS_MAX_INODES],
}

impl VfsState {
    const fn new() -> Self {
        Self {
            initialized: false,
            inodes: [Inode::empty(); TMPFS_MAX_INODES],
        }
    }
}

static mut VFS_STATE: VfsState = VfsState::new();

fn state() -> &'static mut VfsState {
    unsafe { &mut VFS_STATE }
}

fn reset(state: &mut VfsState) {
    for slot in state.inodes.iter_mut() {
        *slot = Inode::empty();
    }
    state.initialized = false;
}

fn setup_root(state: &mut VfsState) {
    let root = &mut state.inodes[ROOT_INODE as usize];
    *root = Inode::empty();
    root.used = true;
    root.file_type = FileType::Directory;
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
    let _ = mkdir_inner(state, "/dev");
    let _ = mkdir_inner(state, "/etc");
    let _ = create_regular_inner(state, "/dev/console");
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
    if parent.child_count as usize >= TMPFS_MAX_CHILDREN {
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
    parent: u16,
    name: &[u8],
) -> Result<u16, i64> {
    if name.is_empty() || name.len() >= VFS_MAX_NAME {
        return Err(ENAMETOOLONG);
    }

    let mut slot = 1usize;
    while slot < TMPFS_MAX_INODES {
        if !state.inodes[slot].used {
            let inode = &mut state.inodes[slot];
            *inode = Inode::empty();
            inode.used = true;
            inode.file_type = file_type;
            inode.parent = parent;
            inode.name_len = name.len() as u8;
            inode.name[..name.len()].copy_from_slice(name);
            return Ok(slot as u16);
        }
        slot += 1;
    }

    Err(ENOSPC)
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

    if find_child(state, parent, &name[..name_len]).is_some() {
        return Err(EEXIST);
    }

    let inode = alloc_inode(state, FileType::Regular, parent, &name[..name_len])?;
    add_child(state, parent, inode)?;
    Ok(inode)
}

fn mkdir_inner(state: &mut VfsState, path: &str) -> Result<(), i64> {
    let mut name = [0u8; VFS_MAX_NAME];
    let (parent, name_len) = resolve_parent_inner(state, path, &mut name)?;

    if find_child(state, parent, &name[..name_len]).is_some() {
        return Err(EEXIST);
    }

    let inode = alloc_inode(state, FileType::Directory, parent, &name[..name_len])?;
    add_child(state, parent, inode)?;
    Ok(())
}

pub fn init() {
    let state = state();
    reset(state);
    ensure_init(state);
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

    let inode = &mut state.inodes[inode_idx as usize];
    if inode.file_type == FileType::Directory {
        if access != O_RDONLY {
            return Err(EISDIR);
        }
    } else if (flags & O_DIRECTORY) != 0 {
        return Err(ENOTDIR);
    }

    if (flags & O_TRUNC) != 0 && inode.file_type == FileType::Regular && access != O_RDONLY {
        inode.size = 0;
    }

    Ok(OpenResult {
        inode: inode_idx,
        file_type: inode.file_type,
    })
}

pub fn read(inode: u16, offset: u64, dst: &mut [u8]) -> Result<usize, i64> {
    let state = state();
    ensure_init(state);

    if inode as usize >= TMPFS_MAX_INODES {
        return Err(ENOENT);
    }

    let node = &state.inodes[inode as usize];
    if !node.used {
        return Err(ENOENT);
    }
    if node.file_type == FileType::Directory {
        return Err(EISDIR);
    }

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

pub fn write(inode: u16, offset: u64, src: &[u8], append: bool) -> Result<WriteResult, i64> {
    let state = state();
    ensure_init(state);

    if inode as usize >= TMPFS_MAX_INODES {
        return Err(ENOENT);
    }

    let node = &mut state.inodes[inode as usize];
    if !node.used {
        return Err(ENOENT);
    }
    if node.file_type == FileType::Directory {
        return Err(EISDIR);
    }

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

pub fn seek(inode: u16, current: u64, offset: i64, whence: u32) -> Result<u64, i64> {
    let state = state();
    ensure_init(state);

    if inode as usize >= TMPFS_MAX_INODES {
        return Err(ENOENT);
    }

    let node = &state.inodes[inode as usize];
    if !node.used {
        return Err(ENOENT);
    }
    if node.file_type == FileType::Directory {
        return Err(EISDIR);
    }

    let base = match whence {
        SEEK_SET => 0i128,
        SEEK_CUR => current as i128,
        SEEK_END => node.size as i128,
        _ => return Err(EINVAL),
    };

    let target = base + offset as i128;
    if target < 0 || target > TMPFS_MAX_FILE_SIZE as i128 {
        return Err(EINVAL);
    }

    Ok(target as u64)
}

pub fn file_size(inode: u16) -> Result<u64, i64> {
    let state = state();
    ensure_init(state);

    if inode as usize >= TMPFS_MAX_INODES {
        return Err(ENOENT);
    }

    let node = &state.inodes[inode as usize];
    if !node.used {
        return Err(ENOENT);
    }

    Ok(node.size as u64)
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

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
}
