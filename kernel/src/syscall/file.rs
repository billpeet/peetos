//! Path resolution, file metadata, and directory-entry syscalls.

use super::*;

#[inline]
pub(super) fn sync_ext2_backing() {
    let _ = crate::vfs::sync_ext2_to_block();
}

/// sync/fsync: commit and checkpoint the journal (no-op difference on ext2).
#[inline]
pub(super) fn sync_ext2_backing_and_checkpoint() {
    let _ = crate::vfs::sync_ext2_to_block_and_checkpoint();
}

#[inline]
pub(super) fn flush_ext2_backing() {
    let _ = crate::vfs::flush_ext2_block_cache();
}

pub(super) fn read_user_path(path_ptr: u64, out: &mut [u8; fs::VFS_MAX_PATH]) -> Result<&str, i64> {
    let len = match copy_cstr_from_user_current(path_ptr, out) {
        Ok(len) => len,
        Err(err) => return Err(-err),
    };
    match core::str::from_utf8(&out[..len]) {
        Ok(path) => Ok(path),
        Err(_) => Err(-EINVAL),
    }
}

pub(super) fn resolve_dir_inode(ctx: &SyscallContext, dirfd: i32) -> Result<u16, i64> {
    if dirfd == fs::AT_FDCWD {
        if ctx.current.is_null() {
            return Err(-ESRCH);
        }
        return Ok(unsafe { (*crate::task::leader_of(ctx.current)).user_cwd_inode });
    }

    let idx = fd_index(dirfd)?;
    if !fd_slot_is_open(ctx.current, idx) {
        return Err(-EBADF);
    }

    let (kind, inode) = unsafe {
        (
            (*crate::task::leader_of(ctx.current)).user_fd_kind[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_inode[idx],
        )
    };
    if kind != task::USER_FD_KIND_VFS {
        return Err(-ENOTDIR);
    }
    match fs::inode_is_dir(inode) {
        Ok(true) => Ok(inode),
        Ok(false) => Err(-ENOTDIR),
        Err(err) => Err(-fs_to_errno(err)),
    }
}

pub(super) fn resolve_dir_for_path(
    ctx: &SyscallContext,
    dirfd: i32,
    path: &str,
) -> Result<u16, i64> {
    if path.starts_with('/') {
        Ok(fs::root_inode())
    } else {
        resolve_dir_inode(ctx, dirfd)
    }
}

pub(super) fn resolve_vfs_fd_inode(ctx: &SyscallContext, fd: i32) -> Result<u16, i64> {
    let idx = fd_index(fd)?;
    if !fd_slot_is_open(ctx.current, idx) {
        return Err(-EBADF);
    }
    let (kind, inode) = unsafe {
        (
            (*crate::task::leader_of(ctx.current)).user_fd_kind[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_inode[idx],
        )
    };
    if kind != task::USER_FD_KIND_VFS {
        return Err(-EBADF);
    }
    Ok(inode)
}

#[inline]
pub(super) fn access_mode_is_valid(mode: u32) -> bool {
    (mode & !(R_OK | W_OK | X_OK)) == 0
}

#[inline]
pub(super) fn access_permission_bits_for(stat: &fs::FileStat, uid: u32, gid: u32) -> u32 {
    if uid == stat.uid {
        return (stat.mode & (STAT_S_IRUSR | STAT_S_IWUSR | STAT_S_IXUSR)) >> 6;
    }
    if gid == stat.gid {
        return (stat.mode & (STAT_S_IRGRP | STAT_S_IWGRP | STAT_S_IXGRP)) >> 3;
    }
    stat.mode & (STAT_S_IROTH | STAT_S_IWOTH | STAT_S_IXOTH)
}

pub(super) fn access_allowed(stat: &fs::FileStat, mode: u32, uid: u32, gid: u32) -> bool {
    if mode == F_OK {
        return true;
    }

    let perms = access_permission_bits_for(stat, uid, gid);
    if (mode & R_OK) != 0 && (perms & 0b100) == 0 {
        return false;
    }
    if (mode & W_OK) != 0 && (perms & 0b010) == 0 {
        return false;
    }
    if (mode & X_OK) != 0 && (perms & 0b001) == 0 {
        return false;
    }
    true
}

pub(super) fn lookup_inode_for_at_path(
    ctx: &SyscallContext,
    dirfd: i32,
    path: &str,
) -> Result<u16, i64> {
    let dir_inode = resolve_dir_for_path(ctx, dirfd, path)?;
    match fs::lookupat(dir_inode, path) {
        Ok(inode) => Ok(inode),
        Err(err) => Err(-fs_to_errno(err)),
    }
}

#[inline]
pub(super) fn path_component_is_dot(name: &[u8]) -> bool {
    name.len() == 1 && name[0] == b'.'
}

#[inline]
pub(super) fn path_component_is_dotdot(name: &[u8]) -> bool {
    name.len() == 2 && name[0] == b'.' && name[1] == b'.'
}

pub(super) fn find_inode_path_from(
    dir_inode: u16,
    target_inode: u16,
    current_len: usize,
    depth: usize,
    path_buf: &mut [u8; fs::VFS_MAX_PATH],
) -> Result<Option<usize>, i64> {
    if dir_inode == target_inode {
        return Ok(Some(current_len));
    }
    if depth >= GETCWD_MAX_DEPTH {
        return Ok(None);
    }

    let mut offset = 0u64;
    loop {
        let entry = match fs::readdir(dir_inode, offset) {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(err) => return Err(-fs_to_errno(err)),
        };
        offset = entry.next_offset;
        if entry.name_len == 0 {
            continue;
        }
        let name = &entry.name[..entry.name_len];
        if path_component_is_dot(name) || path_component_is_dotdot(name) {
            continue;
        }
        let name_str = match core::str::from_utf8(name) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let child_inode = match fs::lookupat(dir_inode, name_str) {
            Ok(inode) => inode,
            Err(_) => continue,
        };

        let next_len;
        if current_len == 0 {
            if 1 + name.len() >= path_buf.len() {
                continue;
            }
            path_buf[0] = b'/';
            path_buf[1..1 + name.len()].copy_from_slice(name);
            next_len = 1 + name.len();
        } else {
            if current_len + 1 + name.len() >= path_buf.len() {
                continue;
            }
            path_buf[current_len] = b'/';
            path_buf[current_len + 1..current_len + 1 + name.len()].copy_from_slice(name);
            next_len = current_len + 1 + name.len();
        }

        if child_inode == target_inode {
            return Ok(Some(next_len));
        }

        match fs::inode_is_dir(child_inode) {
            Ok(true) => {
                if let Some(found_len) =
                    find_inode_path_from(child_inode, target_inode, next_len, depth + 1, path_buf)?
                {
                    return Ok(Some(found_len));
                }
            }
            Ok(false) => {}
            Err(_) => {}
        }
    }

    Ok(None)
}

pub(super) fn build_cwd_path(
    task_ptr: *mut task::Task,
    path_buf: &mut [u8; fs::VFS_MAX_PATH],
) -> Result<usize, i64> {
    if task_ptr.is_null() {
        return Err(-ESRCH);
    }
    let cwd = unsafe { (*crate::task::leader_of(task_ptr)).user_cwd_inode };
    if cwd == fs::root_inode() {
        path_buf[0] = b'/';
        return Ok(1);
    }
    match find_inode_path_from(fs::root_inode(), cwd, 0, 0, path_buf)? {
        Some(len) => Ok(len),
        None => Err(-ENOENT),
    }
}

pub(super) fn refresh_procfs_runtime() {
    let pmm_stats = pmm::get_stats();
    fs::set_proc_uptime_ticks(pit::ticks());
    fs::set_proc_meminfo(
        pmm_stats.total_bytes,
        pmm_stats.free_bytes,
        pmm_stats.used_bytes,
    );
}

pub(super) fn sys_open(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let wrapped = SyscallArgs {
        num: SYS_OPENAT,
        args: [
            fs::AT_FDCWD as u64,
            call.arg(0),
            call.arg(1),
            call.arg(2),
            0,
            0,
        ],
    };
    sys_openat(ctx, &wrapped)
}

pub(super) fn sys_openat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let flags = call.arg(2) as u32;
    let mode = call.arg(3) as u32;
    let fs_flags = flags & !O_CLOEXEC;
    let fd_flags = fd_flags_from_open_flags(flags);
    let mut path_buf = [0u8; fs::VFS_MAX_PATH];
    let path = match read_user_path(call.arg(1), &mut path_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };
    let dir_inode = match resolve_dir_for_path(ctx, call.arg(0) as i32, path) {
        Ok(inode) => inode,
        Err(err) => return err,
    };

    refresh_procfs_runtime();

    let opened = match fs::openat(dir_inode, path, fs_flags, mode) {
        Ok(opened) => opened,
        Err(err) => return -fs_to_errno(err),
    };

    // /dev/tty → route to controlling terminal
    if opened.is_tty {
        let ctty_type = unsafe { (*crate::task::leader_of(ctx.current)).ctty_type };
        return match ctty_type {
            task::CTTY_CONSOLE => {
                match fd_alloc(ctx.current, task::USER_FD_KIND_CONSOLE, 0, fd_flags) {
                    Ok(fd) => fd,
                    Err(err) => err,
                }
            }
            task::CTTY_PTY => {
                let pty_id = unsafe { (*crate::task::leader_of(ctx.current)).ctty_id };
                if crate::pty::is_in_use(pty_id) {
                    if let Err(e) = crate::pty::slave_open(pty_id) {
                        return e as i64;
                    }
                    match fd_alloc(ctx.current, task::USER_FD_KIND_PTY_SLAVE, pty_id, fd_flags) {
                        Ok(fd) => fd,
                        Err(err) => err,
                    }
                } else {
                    -ENXIO
                }
            }
            _ => -ENXIO,
        };
    }

    // /dev/ptmx → allocate a new PTY, return master FD
    if opened.is_ptmx {
        let pty_id = match crate::pty::pty_alloc() {
            Ok(id) => id,
            Err(e) => return e as i64,
        };
        return match fd_alloc(ctx.current, task::USER_FD_KIND_PTY_MASTER, pty_id, fd_flags) {
            Ok(fd) => fd,
            Err(err) => {
                crate::pty::master_close(pty_id);
                err
            }
        };
    }

    // /dev/pts/N → open PTY slave
    if opened.is_pts_slave {
        let pty_id = opened.pts_id;
        if !crate::pty::is_in_use(pty_id) || crate::pty::is_locked(pty_id) {
            return -EIO;
        }
        if let Err(e) = crate::pty::slave_open(pty_id) {
            return e as i64;
        }
        return match fd_alloc(ctx.current, task::USER_FD_KIND_PTY_SLAVE, pty_id, fd_flags) {
            Ok(fd) => fd,
            Err(err) => {
                crate::pty::slave_close(pty_id);
                err
            }
        };
    }

    let fd_kind = if opened.is_console {
        task::USER_FD_KIND_CONSOLE
    } else {
        task::USER_FD_KIND_VFS
    };

    let fd_result = match fd_alloc(ctx.current, fd_kind, opened.inode, fd_flags) {
        Ok(fd) => fd,
        Err(err) => return err,
    };

    if (fs_flags & (fs::O_CREAT | fs::O_TRUNC)) != 0 {
        sync_ext2_backing();
    }

    fd_result
}

pub(super) fn sys_mkdirat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let mut path_buf = [0u8; fs::VFS_MAX_PATH];
    let path = match read_user_path(call.arg(1), &mut path_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };
    let dir_inode = match resolve_dir_for_path(ctx, call.arg(0) as i32, path) {
        Ok(inode) => inode,
        Err(err) => return err,
    };
    let _mode = call.arg(2) as u32;

    match fs::mkdirat(dir_inode, path) {
        Ok(()) => {
            sync_ext2_backing();
            0
        }
        Err(err) => -fs_to_errno(err),
    }
}

pub(super) fn sys_access_common(ctx: &SyscallContext, dirfd: i32, pathname: u64, mode: u32) -> i64 {
    if !access_mode_is_valid(mode) {
        return -EINVAL;
    }

    let mut path_buf = [0u8; fs::VFS_MAX_PATH];
    let path = match read_user_path(pathname, &mut path_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };
    if path.is_empty() {
        return -ENOENT;
    }

    let inode = match lookup_inode_for_at_path(ctx, dirfd, path) {
        Ok(inode) => inode,
        Err(err) => return err,
    };
    let stat = match fs::stat(inode) {
        Ok(stat) => stat,
        Err(err) => return -fs_to_errno(err),
    };

    let (uid, gid) = unsafe { ((*crate::task::leader_of(ctx.current)).user_uid, (*crate::task::leader_of(ctx.current)).user_gid) };
    if access_allowed(&stat, mode, uid, gid) {
        0
    } else {
        -EACCES
    }
}

pub(super) fn sys_access(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    sys_access_common(ctx, fs::AT_FDCWD, call.arg(0), call.arg(1) as u32)
}

pub(super) fn sys_faccessat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    sys_access_common(ctx, call.arg(0) as i32, call.arg(1), call.arg(2) as u32)
}

pub(super) fn sys_faccessat2(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let flags = call.arg(3) as u32;
    if flags & !(AT_EACCESS | AT_SYMLINK_NOFOLLOW) != 0 {
        return -EINVAL;
    }
    sys_access_common(ctx, call.arg(0) as i32, call.arg(1), call.arg(2) as u32)
}

pub(super) fn sys_chdir(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let mut path_buf = [0u8; fs::VFS_MAX_PATH];
    let path = match read_user_path(call.arg(0), &mut path_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };
    if path.is_empty() {
        return -ENOENT;
    }

    let inode = match lookup_inode_for_at_path(ctx, fs::AT_FDCWD, path) {
        Ok(inode) => inode,
        Err(err) => return err,
    };
    match fs::inode_is_dir(inode) {
        Ok(true) => {
            unsafe {
                (*crate::task::leader_of(ctx.current)).user_cwd_inode = inode;
            }
            0
        }
        Ok(false) => -ENOTDIR,
        Err(err) => -fs_to_errno(err),
    }
}

pub(super) fn sys_fchdir(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let inode = match resolve_dir_inode(ctx, call.arg(0) as i32) {
        Ok(inode) => inode,
        Err(err) => return err,
    };
    unsafe {
        (*crate::task::leader_of(ctx.current)).user_cwd_inode = inode;
    }
    0
}

pub(super) fn sys_getcwd(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let user_buf = call.arg(0);
    let user_len = match usize::try_from(call.arg(1)) {
        Ok(len) => len,
        Err(_) => return -EINVAL,
    };
    if user_len == 0 {
        return -EINVAL;
    }

    let mut path = [0u8; fs::VFS_MAX_PATH];
    let path_len = match build_cwd_path(ctx.current, &mut path) {
        Ok(len) => len,
        Err(err) => return err,
    };
    let required = path_len + 1;
    if required > user_len {
        return -ERANGE;
    }

    let mut out = [0u8; fs::VFS_MAX_PATH];
    out[..path_len].copy_from_slice(&path[..path_len]);
    out[path_len] = 0;
    match copy_to_user_task(ctx.current, user_buf, &out[..required]) {
        Ok(()) => user_buf as i64,
        Err(err) => -err,
    }
}

pub(super) fn sys_rename(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let wrapped = SyscallArgs {
        num: SYS_RENAMEAT,
        args: [
            fs::AT_FDCWD as u64,
            call.arg(0),
            fs::AT_FDCWD as u64,
            call.arg(1),
            0,
            0,
        ],
    };
    sys_renameat(ctx, &wrapped)
}

pub(super) fn sys_link(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let wrapped = SyscallArgs {
        num: SYS_LINKAT,
        args: [
            fs::AT_FDCWD as u64,
            call.arg(0),
            fs::AT_FDCWD as u64,
            call.arg(1),
            0,
            0,
        ],
    };
    sys_linkat(ctx, &wrapped)
}

pub(super) fn sys_linkat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let flags = call.arg(4) as u32;
    if (flags & !AT_SYMLINK_FOLLOW) != 0 {
        return -EINVAL;
    }

    let mut old_buf = [0u8; fs::VFS_MAX_PATH];
    let mut new_buf = [0u8; fs::VFS_MAX_PATH];
    let old_path = match read_user_path(call.arg(1), &mut old_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };
    let new_path = match read_user_path(call.arg(3), &mut new_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };
    if old_path.is_empty() || new_path.is_empty() {
        return -ENOENT;
    }

    let old_dir = match resolve_dir_for_path(ctx, call.arg(0) as i32, old_path) {
        Ok(inode) => inode,
        Err(err) => return err,
    };
    let new_dir = match resolve_dir_for_path(ctx, call.arg(2) as i32, new_path) {
        Ok(inode) => inode,
        Err(err) => return err,
    };

    match fs::linkat(old_dir, old_path, new_dir, new_path) {
        Ok(()) => {
            sync_ext2_backing();
            0
        }
        Err(err) => -fs_to_errno(err),
    }
}

pub(super) fn sys_symlinkat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let mut target_buf = [0u8; fs::VFS_MAX_PATH];
    let mut link_buf = [0u8; fs::VFS_MAX_PATH];
    let target = match read_user_path(call.arg(0), &mut target_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };
    let link_path = match read_user_path(call.arg(2), &mut link_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };
    if link_path.is_empty() {
        return -ENOENT;
    }

    let dir = match resolve_dir_for_path(ctx, call.arg(1) as i32, link_path) {
        Ok(inode) => inode,
        Err(err) => return err,
    };

    match fs::symlinkat(dir, target, link_path) {
        Ok(()) => {
            sync_ext2_backing();
            0
        }
        Err(err) => -fs_to_errno(err),
    }
}

pub(super) fn sys_readlinkat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let dirfd = call.arg(0) as i32;
    let user_buf = call.arg(2);
    let buf_len = match usize::try_from(call.arg(3)) {
        Ok(len) => len,
        Err(_) => return -EINVAL,
    };
    if buf_len == 0 {
        return -EINVAL;
    }

    let mut path_buf = [0u8; fs::VFS_MAX_PATH];
    let path = match read_user_path(call.arg(1), &mut path_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };
    if path.is_empty() {
        return -ENOENT;
    }

    // Baseline procfs convenience: many userland paths probe /proc/self/exe.
    if path == "/proc/self/exe" || path == "proc/self/exe" {
        let target = b"/proc/self/exe";
        let count = cmp::min(target.len(), buf_len);
        return match copy_to_user_task(ctx.current, user_buf, &target[..count]) {
            Ok(()) => count as i64,
            Err(err) => -err,
        };
    }

    let dir = match resolve_dir_for_path(ctx, dirfd, path) {
        Ok(inode) => inode,
        Err(err) => return err,
    };

    let mut link_buf = [0u8; fs::VFS_MAX_PATH];
    match fs::readlinkat(dir, path, &mut link_buf) {
        Ok(link_len) => {
            let count = cmp::min(link_len, buf_len);
            match copy_to_user_task(ctx.current, user_buf, &link_buf[..count]) {
                Ok(()) => count as i64,
                Err(err) => -err,
            }
        }
        Err(err) => -fs_to_errno(err),
    }
}

pub(super) fn sys_readlink(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let compat = SyscallArgs {
        num: SYS_READLINKAT,
        args: [
            fs::AT_FDCWD as i64 as u64,
            call.arg(0),
            call.arg(1),
            call.arg(2),
            0,
            0,
        ],
    };
    sys_readlinkat(ctx, &compat)
}

pub(super) fn sys_fchmodat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let dirfd = call.arg(0) as i32;
    let path_ptr = call.arg(1);
    let mode = call.arg(2) as u32;

    let mut path_buf = [0u8; fs::VFS_MAX_PATH];
    let path = match read_user_path(path_ptr, &mut path_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };

    let inode = match lookup_inode_for_at_path(ctx, dirfd, path) {
        Ok(inode) => inode,
        Err(err) => return err,
    };

    match fs::chmod(inode, (mode & 0o7777) as u16) {
        Ok(()) => {
            sync_ext2_backing();
            0
        }
        Err(err) => -fs_to_errno(err),
    }
}

pub(super) fn sys_fchownat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let dirfd = call.arg(0) as i32;
    let path_ptr = call.arg(1);
    let owner = call.arg(2) as u32;
    let group = call.arg(3) as u32;
    let flags = call.arg(4) as u32;

    if (flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH)) != 0 {
        return -EINVAL;
    }

    let inode = if path_ptr == 0 || {
        let mut probe = [0u8; 1];
        copy_from_user_task(ctx.current, path_ptr, &mut probe).is_ok() && probe[0] == 0
    } {
        if (flags & AT_EMPTY_PATH) == 0 {
            return -ENOENT;
        }
        match resolve_vfs_fd_inode(ctx, dirfd) {
            Ok(inode) => inode,
            Err(err) => return err,
        }
    } else {
        let mut path_buf = [0u8; fs::VFS_MAX_PATH];
        let path = match read_user_path(path_ptr, &mut path_buf) {
            Ok(path) => path,
            Err(err) => return err,
        };
        match lookup_inode_for_at_path(ctx, dirfd, path) {
            Ok(inode) => inode,
            Err(err) => return err,
        }
    };

    match fs::chown(inode, owner, group) {
        Ok(()) => {
            sync_ext2_backing();
            0
        }
        Err(err) => -fs_to_errno(err),
    }
}

pub(super) fn sys_mknodat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let dirfd = call.arg(0) as i32;
    let path_ptr = call.arg(1);
    let mode = call.arg(2) as u32;
    let _dev = call.arg(3);

    let file_type = mode & 0xF000;
    // Only support S_IFREG (regular file create) and mode==0 (bare create);
    // reject S_IFCHR, S_IFBLK, S_IFIFO as unsupported.
    if file_type != 0x8000 && file_type != 0 {
        return -EINVAL;
    }

    let mut path_buf = [0u8; fs::VFS_MAX_PATH];
    let path = match read_user_path(path_ptr, &mut path_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };

    let dir_inode = match resolve_dir_for_path(ctx, dirfd, path) {
        Ok(inode) => inode,
        Err(err) => return err,
    };

    let opened = match fs::openat(dir_inode, path, fs::O_CREAT | fs::O_EXCL | fs::O_WRONLY, 0) {
        Ok(opened) => opened,
        Err(err) => return -fs_to_errno(err),
    };

    let perm = (mode & 0o7777) as u16;
    if perm != 0 {
        let _ = fs::chmod(opened.inode, perm);
    }

    sync_ext2_backing();
    0
}

pub(super) fn sys_utimensat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let dirfd = call.arg(0) as i32;
    let path_ptr = call.arg(1);
    let times_ptr = call.arg(2);
    let flags = call.arg(3) as u32;
    if (flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH)) != 0 {
        return -EINVAL;
    }

    let mut path_buf = [0u8; fs::VFS_MAX_PATH];
    let path = if path_ptr == 0 {
        ""
    } else {
        match read_user_path(path_ptr, &mut path_buf) {
            Ok(path) => path,
            Err(err) => return err,
        }
    };

    let inode = if path.is_empty() {
        if (flags & AT_EMPTY_PATH) == 0 {
            return -ENOENT;
        }
        match resolve_vfs_fd_inode(ctx, dirfd) {
            Ok(inode) => inode,
            Err(err) => return err,
        }
    } else {
        match lookup_inode_for_at_path(ctx, dirfd, path) {
            Ok(inode) => inode,
            Err(err) => return err,
        }
    };

    let current = match fs::stat(inode) {
        Ok(stat) => stat,
        Err(err) => return -fs_to_errno(err),
    };
    let now = pit::ticks();

    let (atime, mtime) = if times_ptr == 0 {
        (now, now)
    } else {
        let specs = match read_struct_from_user::<[LinuxTimeSpec; 2]>(ctx.current, times_ptr) {
            Ok(specs) => specs,
            Err(err) => return -err,
        };
        let atime = match timespec_to_unix_seconds(specs[0], now, current.atime) {
            Ok(value) => value,
            Err(err) => return err,
        };
        let mtime = match timespec_to_unix_seconds(specs[1], now, current.mtime) {
            Ok(value) => value,
            Err(err) => return err,
        };
        (atime, mtime)
    };

    match fs::set_times(inode, atime, mtime) {
        Ok(()) => {
            sync_ext2_backing();
            0
        }
        Err(err) => -fs_to_errno(err),
    }
}

pub(super) fn sys_utimes(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let path_ptr = call.arg(0);
    let times_ptr = call.arg(1);

    let mut path_buf = [0u8; fs::VFS_MAX_PATH];
    let path = match read_user_path(path_ptr, &mut path_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };

    let inode = match lookup_inode_for_at_path(ctx, fs::AT_FDCWD, path) {
        Ok(inode) => inode,
        Err(err) => return err,
    };

    let now = pit::ticks();
    let (atime, mtime) = if times_ptr == 0 {
        (now, now)
    } else {
        let tv = match read_struct_from_user::<[LinuxTimeVal; 2]>(ctx.current, times_ptr) {
            Ok(tv) => tv,
            Err(err) => return -err,
        };
        (tv[0].tv_sec as u64, tv[1].tv_sec as u64)
    };

    match fs::set_times(inode, atime, mtime) {
        Ok(()) => {
            sync_ext2_backing();
            0
        }
        Err(err) => -fs_to_errno(err),
    }
}

pub(super) fn sys_futimesat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let dirfd = call.arg(0) as i32;
    let path_ptr = call.arg(1);
    let times_ptr = call.arg(2);

    let mut path_buf = [0u8; fs::VFS_MAX_PATH];
    let path = match read_user_path(path_ptr, &mut path_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };

    let inode = match lookup_inode_for_at_path(ctx, dirfd, path) {
        Ok(inode) => inode,
        Err(err) => return err,
    };

    let now = pit::ticks();
    let (atime, mtime) = if times_ptr == 0 {
        (now, now)
    } else {
        let tv = match read_struct_from_user::<[LinuxTimeVal; 2]>(ctx.current, times_ptr) {
            Ok(tv) => tv,
            Err(err) => return -err,
        };
        (tv[0].tv_sec as u64, tv[1].tv_sec as u64)
    };

    match fs::set_times(inode, atime, mtime) {
        Ok(()) => {
            sync_ext2_backing();
            0
        }
        Err(err) => -fs_to_errno(err),
    }
}

pub(super) fn stat_path(
    ctx: &SyscallContext,
    path_ptr: u64,
    follow_symlinks: bool,
) -> Result<fs::FileStat, i64> {
    let mut path_buf = [0u8; fs::VFS_MAX_PATH];
    let path = read_user_path(path_ptr, &mut path_buf)?;
    if path.is_empty() {
        return Err(-ENOENT);
    }

    let dir_inode = resolve_dir_for_path(ctx, fs::AT_FDCWD, path)?;
    let inode = if follow_symlinks {
        fs::lookupat(dir_inode, path).map_err(|err| -fs_to_errno(err))?
    } else {
        // VFS symlink support is still pending, so lstat currently resolves
        // through lookupat with the same behavior as stat.
        fs::lookupat(dir_inode, path).map_err(|err| -fs_to_errno(err))?
    };
    fs::stat(inode).map_err(|err| -fs_to_errno(err))
}

pub(super) fn write_stat_to_user(task_ptr: *mut task::Task, dst: u64, stat: fs::FileStat) -> i64 {
    let raw = LinuxStat::from_fs(stat);
    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&raw as *const LinuxStat) as *const u8,
            core::mem::size_of::<LinuxStat>(),
        )
    };
    match copy_to_user_task(task_ptr, dst, bytes) {
        Ok(()) => 0,
        Err(err) => -err,
    }
}

pub(super) fn sys_stat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let stat = match stat_path(ctx, call.arg(0), true) {
        Ok(stat) => stat,
        Err(err) => return err,
    };
    write_stat_to_user(ctx.current, call.arg(1), stat)
}

pub(super) fn sys_lstat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let stat = match stat_path(ctx, call.arg(0), false) {
        Ok(stat) => stat,
        Err(err) => return err,
    };
    write_stat_to_user(ctx.current, call.arg(1), stat)
}

pub(super) fn sys_fstat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let idx = match fd_index(call.arg(0) as i32) {
        Ok(idx) => idx,
        Err(err) => return err,
    };
    if !fd_slot_is_open(ctx.current, idx) {
        return -EBADF;
    }

    let (kind, inode) = unsafe {
        (
            (*crate::task::leader_of(ctx.current)).user_fd_kind[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_inode[idx],
        )
    };
    if kind == task::USER_FD_KIND_PIPE {
        let pipe_stat = LinuxStat {
            st_dev: 0,
            st_ino: inode as u64 + 1,
            st_nlink: 1,
            st_mode: 0x1000 | 0o600,
            st_uid: 0,
            st_gid: 0,
            __pad0: 0,
            st_rdev: 0,
            st_size: 0,
            st_blksize: 4096,
            st_blocks: 0,
            st_atime: 0,
            st_atime_nsec: 0,
            st_mtime: 0,
            st_mtime_nsec: 0,
            st_ctime: 0,
            st_ctime_nsec: 0,
            __unused: [0; 3],
        };
        let bytes = unsafe {
            core::slice::from_raw_parts(
                (&pipe_stat as *const LinuxStat) as *const u8,
                core::mem::size_of::<LinuxStat>(),
            )
        };
        return match copy_to_user_task(ctx.current, call.arg(1), bytes) {
            Ok(()) => 0,
            Err(err) => -err,
        };
    }
    if kind != task::USER_FD_KIND_VFS {
        return -EBADF;
    }
    let stat = match fs::stat(inode) {
        Ok(stat) => stat,
        Err(err) => return -fs_to_errno(err),
    };
    write_stat_to_user(ctx.current, call.arg(1), stat)
}

pub(super) fn sys_newfstatat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let flags = call.arg(3) as u32;
    if flags & !AT_EMPTY_PATH != 0 {
        return -EINVAL;
    }
    let mut path_buf = [0u8; fs::VFS_MAX_PATH];
    let path = match read_user_path(call.arg(1), &mut path_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };
    let dir_inode = match resolve_dir_for_path(ctx, call.arg(0) as i32, path) {
        Ok(inode) => inode,
        Err(err) => return err,
    };

    let inode = if path.is_empty() {
        if (flags & AT_EMPTY_PATH) == 0 {
            return -ENOENT;
        }
        dir_inode
    } else {
        match fs::lookupat(dir_inode, path) {
            Ok(inode) => inode,
            Err(err) => return -fs_to_errno(err),
        }
    };
    let stat = match fs::stat(inode) {
        Ok(stat) => stat,
        Err(err) => return -fs_to_errno(err),
    };
    write_stat_to_user(ctx.current, call.arg(2), stat)
}

pub(super) fn dirent_record_len(name_len: usize) -> usize {
    (19 + name_len + 1 + 7) & !7
}

pub(super) fn write_u16_ne(dst: &mut [u8], offset: usize, value: u16) {
    let b = value.to_ne_bytes();
    dst[offset] = b[0];
    dst[offset + 1] = b[1];
}

pub(super) fn write_u64_ne(dst: &mut [u8], offset: usize, value: u64) {
    let b = value.to_ne_bytes();
    dst[offset..offset + 8].copy_from_slice(&b);
}

pub(super) fn write_i64_ne(dst: &mut [u8], offset: usize, value: i64) {
    let b = value.to_ne_bytes();
    dst[offset..offset + 8].copy_from_slice(&b);
}

pub(super) fn sys_getdents64(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let idx = match fd_index(call.arg(0) as i32) {
        Ok(idx) => idx,
        Err(err) => return err,
    };
    if !fd_slot_is_open(ctx.current, idx) {
        return -EBADF;
    }

    let (kind, inode, mut offset) = unsafe {
        (
            (*crate::task::leader_of(ctx.current)).user_fd_kind[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_inode[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_offset[idx],
        )
    };
    if kind != task::USER_FD_KIND_VFS {
        return -EBADF;
    }
    match fs::inode_is_dir(inode) {
        Ok(true) => {}
        Ok(false) => return -ENOTDIR,
        Err(err) => return -fs_to_errno(err),
    }

    let max_len = call.arg(2) as usize;
    if max_len == 0 {
        return 0;
    }

    let mut total = 0usize;
    let mut record = [0u8; 128];
    loop {
        let entry = match fs::readdir(inode, offset) {
            Ok(Some(v)) => v,
            Ok(None) => break,
            Err(err) => {
                if total > 0 {
                    break;
                }
                return -fs_to_errno(err);
            }
        };
        let reclen = dirent_record_len(entry.name_len);
        if reclen > record.len() {
            return -EIO;
        }
        if total + reclen > max_len {
            break;
        }

        record[..reclen].fill(0);
        write_u64_ne(&mut record, 0, entry.ino);
        write_i64_ne(&mut record, 8, entry.next_offset as i64);
        write_u16_ne(&mut record, 16, reclen as u16);
        record[18] = entry.d_type;
        record[19..19 + entry.name_len].copy_from_slice(&entry.name[..entry.name_len]);
        record[19 + entry.name_len] = 0;

        if let Err(err) =
            copy_to_user_task(ctx.current, call.arg(1) + total as u64, &record[..reclen])
        {
            if total > 0 {
                break;
            }
            return -err;
        }

        total += reclen;
        offset = entry.next_offset;
    }

    unsafe {
        (*crate::task::leader_of(ctx.current)).user_fd_offset[idx] = offset;
    }
    total as i64
}

pub(super) fn sys_unlinkat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let flags = call.arg(2) as u32;
    if flags & !AT_REMOVEDIR != 0 {
        return -EINVAL;
    }
    let mut path_buf = [0u8; fs::VFS_MAX_PATH];
    let path = match read_user_path(call.arg(1), &mut path_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };
    let dir_inode = match resolve_dir_for_path(ctx, call.arg(0) as i32, path) {
        Ok(inode) => inode,
        Err(err) => return err,
    };
    let res = if (flags & AT_REMOVEDIR) != 0 {
        fs::rmdirat(dir_inode, path)
    } else {
        fs::unlinkat(dir_inode, path)
    };
    match res {
        Ok(()) => {
            sync_ext2_backing();
            0
        }
        Err(err) => -fs_to_errno(err),
    }
}

pub(super) fn sys_unlink(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let compat = SyscallArgs {
        num: SYS_UNLINKAT,
        args: [fs::AT_FDCWD as i64 as u64, call.arg(0), 0, 0, 0, 0],
    };
    sys_unlinkat(ctx, &compat)
}

pub(super) fn sys_renameat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let mut old_buf = [0u8; fs::VFS_MAX_PATH];
    let mut new_buf = [0u8; fs::VFS_MAX_PATH];
    let old_path = match read_user_path(call.arg(1), &mut old_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };
    let new_path = match read_user_path(call.arg(3), &mut new_buf) {
        Ok(path) => path,
        Err(err) => return err,
    };
    let old_dir = match resolve_dir_for_path(ctx, call.arg(0) as i32, old_path) {
        Ok(inode) => inode,
        Err(err) => return err,
    };
    let new_dir = match resolve_dir_for_path(ctx, call.arg(2) as i32, new_path) {
        Ok(inode) => inode,
        Err(err) => return err,
    };

    match fs::renameat(old_dir, old_path, new_dir, new_path) {
        Ok(()) => {
            sync_ext2_backing();
            0
        }
        Err(err) => {
            if err == fs::EINVAL && old_dir != new_dir {
                -EXDEV
            } else {
                -fs_to_errno(err)
            }
        }
    }
}

pub(super) fn sys_umask(_ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let new_mask = (call.arg(0) as u32) & 0o777;
    let old = PROCESS_UMASK.swap(new_mask, Ordering::Relaxed);
    old as i64
}

// ---------------------------------------------------------------------------
// ftruncate (77)
// ---------------------------------------------------------------------------
pub(super) fn sys_ftruncate(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let length = call.arg(1);

    let idx = match fd_index(fd) {
        Ok(idx) => idx,
        Err(err) => return err,
    };
    if !fd_slot_is_open(ctx.current, idx) {
        return -EBADF;
    }

    let (kind, inode, fd_flags) = unsafe {
        (
            (*crate::task::leader_of(ctx.current)).user_fd_kind[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_inode[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_flags[idx],
        )
    };
    if kind != task::USER_FD_KIND_VFS {
        return -EBADF;
    }
    let seals = fd_seals(fd_flags, inode);
    if seals != 0 {
        let size = match fs::file_size(inode) {
            Ok(size) => size,
            Err(err) => return -fs_to_errno(err),
        };
        if let Err(err) = seals_allow_resize(seals, size, length) {
            return err;
        }
    }

    let result = if length == 0 {
        fs::truncate(inode)
    } else {
        fs::truncate_to(inode, length)
    };

    match result {
        Ok(()) => {
            vm_page_cache_invalidate_inode(inode);
            sync_ext2_backing();
            0
        }
        Err(err) => -fs_to_errno(err),
    }
}

// ---------------------------------------------------------------------------
// statfs (137) / fstatfs (138)
// ---------------------------------------------------------------------------
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct LinuxStatfs {
    pub(super) f_type: i64,
    pub(super) f_bsize: i64,
    pub(super) f_blocks: i64,
    pub(super) f_bfree: i64,
    pub(super) f_bavail: i64,
    pub(super) f_files: i64,
    pub(super) f_ffree: i64,
    pub(super) f_fsid: [i32; 2],
    pub(super) f_namelen: i64,
    pub(super) f_frsize: i64,
    pub(super) f_flags: i64,
    pub(super) f_spare: [i64; 4],
}

pub(super) fn fill_statfs_for_inode(inode: u16) -> LinuxStatfs {
    let is_ext2 = fs::inode_backend_is_ext2(inode);
    if is_ext2 {
        let bsize = fs::ext2_block_size() as i64;
        let blocks = fs::ext2_total_blocks() as i64;
        LinuxStatfs {
            f_type: EXT2_SUPER_MAGIC_STATFS,
            f_bsize: bsize,
            f_blocks: blocks,
            f_bfree: blocks / 4,
            f_bavail: blocks / 4,
            f_files: fs::ext2_total_inodes() as i64,
            f_ffree: 16,
            f_fsid: [0; 2],
            f_namelen: 255,
            f_frsize: bsize,
            f_flags: 0,
            f_spare: [0; 4],
        }
    } else {
        LinuxStatfs {
            f_type: TMPFS_MAGIC,
            f_bsize: 4096,
            f_blocks: 1024,
            f_bfree: 512,
            f_bavail: 512,
            f_files: 256,
            f_ffree: 128,
            f_fsid: [0; 2],
            f_namelen: 255,
            f_frsize: 4096,
            f_flags: 0,
            f_spare: [0; 4],
        }
    }
}

pub(super) fn sys_statfs(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let mut path_buf = [0u8; fs::VFS_MAX_PATH];
    let path = match read_user_path(call.arg(0), &mut path_buf) {
        Ok(p) => p,
        Err(err) => return err,
    };
    if path.is_empty() {
        return -ENOENT;
    }

    let dir_inode = match resolve_dir_for_path(ctx, fs::AT_FDCWD, path) {
        Ok(ino) => ino,
        Err(err) => return err,
    };
    let inode = match fs::lookupat(dir_inode, path) {
        Ok(ino) => ino,
        Err(err) => return -fs_to_errno(err),
    };

    let sfs = fill_statfs_for_inode(inode);
    match write_struct_to_user(ctx.current, call.arg(1), &sfs) {
        Ok(()) => 0,
        Err(err) => -err,
    }
}

pub(super) fn sys_fstatfs(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let idx = match fd_index(fd) {
        Ok(idx) => idx,
        Err(err) => return err,
    };
    if !fd_slot_is_open(ctx.current, idx) {
        return -EBADF;
    }

    let (kind, inode) = unsafe {
        (
            (*crate::task::leader_of(ctx.current)).user_fd_kind[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_inode[idx],
        )
    };
    if kind != task::USER_FD_KIND_VFS {
        // For pipes, return tmpfs-like statfs.
        let sfs = fill_statfs_for_inode(0);
        return match write_struct_to_user(ctx.current, call.arg(1), &sfs) {
            Ok(()) => 0,
            Err(err) => -err,
        };
    }

    let sfs = fill_statfs_for_inode(inode);
    match write_struct_to_user(ctx.current, call.arg(1), &sfs) {
        Ok(()) => 0,
        Err(err) => -err,
    }
}

// ---------------------------------------------------------------------------
// fchmod (91)
// ---------------------------------------------------------------------------
pub(super) fn sys_fchmod(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let mode = call.arg(1) as u32;

    let idx = match fd_index(fd) {
        Ok(idx) => idx,
        Err(err) => return err,
    };
    if !fd_slot_is_open(ctx.current, idx) {
        return -EBADF;
    }

    let (kind, inode) = unsafe {
        (
            (*crate::task::leader_of(ctx.current)).user_fd_kind[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_inode[idx],
        )
    };
    if kind != task::USER_FD_KIND_VFS {
        return -EBADF;
    }

    match fs::chmod(inode, (mode & 0o7777) as u16) {
        Ok(()) => {
            sync_ext2_backing();
            0
        }
        Err(err) => -fs_to_errno(err),
    }
}

// ---------------------------------------------------------------------------
// fchown (93)
// ---------------------------------------------------------------------------
pub(super) fn sys_fchown(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let owner = call.arg(1) as u32;
    let group = call.arg(2) as u32;

    let idx = match fd_index(fd) {
        Ok(idx) => idx,
        Err(err) => return err,
    };
    if !fd_slot_is_open(ctx.current, idx) {
        return -EBADF;
    }

    let (kind, inode) = unsafe {
        (
            (*crate::task::leader_of(ctx.current)).user_fd_kind[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_inode[idx],
        )
    };
    if kind != task::USER_FD_KIND_VFS {
        return -EBADF;
    }

    match fs::chown(inode, owner, group) {
        Ok(()) => {
            sync_ext2_backing();
            0
        }
        Err(err) => -fs_to_errno(err),
    }
}

// ---------------------------------------------------------------------------
// truncate (76)
// ---------------------------------------------------------------------------
pub(super) fn sys_truncate(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let path_ptr = call.arg(0);
    let length = call.arg(1);

    let mut path_buf = [0u8; fs::VFS_MAX_PATH];
    let path = match read_user_path(path_ptr, &mut path_buf) {
        Ok(p) => p,
        Err(err) => return err,
    };
    if path.is_empty() {
        return -ENOENT;
    }

    let dir_inode = match resolve_dir_for_path(ctx, fs::AT_FDCWD, path) {
        Ok(ino) => ino,
        Err(err) => return err,
    };
    let inode = match fs::lookupat(dir_inode, path) {
        Ok(ino) => ino,
        Err(err) => return -fs_to_errno(err),
    };

    let result = if length == 0 {
        fs::truncate(inode)
    } else {
        fs::truncate_to(inode, length)
    };

    match result {
        Ok(()) => {
            vm_page_cache_invalidate_inode(inode);
            sync_ext2_backing();
            0
        }
        Err(err) => -fs_to_errno(err),
    }
}

// ---------------------------------------------------------------------------
// sync (162)
// ---------------------------------------------------------------------------
pub(super) fn sys_sync(_ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    if let Err(err) = vm_page_cache_writeback_all() {
        return err;
    }
    sync_ext2_backing_and_checkpoint();
    flush_ext2_backing();
    0
}

pub(super) fn sys_fsync(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let idx = match fd_index(fd) {
        Ok(i) => i,
        Err(err) => return err,
    };
    if !fd_slot_is_open(ctx.current, idx) {
        return -EBADF;
    }

    let (kind, inode) = unsafe {
        (
            (*crate::task::leader_of(ctx.current)).user_fd_kind[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_inode[idx],
        )
    };
    if kind == task::USER_FD_KIND_VFS {
        if let Err(err) = vm_page_cache_writeback_inode(inode) {
            return err;
        }
        sync_ext2_backing_and_checkpoint();
        flush_ext2_backing();
        return 0;
    }
    if kind == task::USER_FD_KIND_PIPE
        || kind == task::USER_FD_KIND_CONSOLE
        || kind == task::USER_FD_KIND_SOCKET
        || kind == task::USER_FD_KIND_PTY_MASTER
        || kind == task::USER_FD_KIND_PTY_SLAVE
    {
        return 0;
    }

    -EBADF
}
