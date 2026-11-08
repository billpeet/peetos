//! read/write family of syscalls.

use super::*;

pub(super) fn read_vfs_bytes(inode: u16, offset: u64, dst: &mut [u8]) -> Result<usize, i64> {
    if let Some(result) = crate::shmem::read(inode, offset, dst) {
        return result;
    }
    vm_page_cache_read(inode, offset, dst)
}

// The vfs_* wrappers route memfd inodes (backed by `crate::shmem`) away from
// tmpfs and otherwise defer to the fs crate. They keep the fs crate's error
// convention (fs error codes, mapped with `fs_to_errno` by callers).

/// Map a shmem errno to the closest fs error code.
fn shmem_err_to_fs(err: i64) -> i64 {
    if err == -EFBIG || err == -ENOMEM {
        fs::ENOSPC
    } else {
        fs::EIO
    }
}

pub(super) fn vfs_write(inode: u16, offset: u64, src: &[u8], append: bool) -> Result<fs::WriteResult, i64> {
    if let Some(size) = crate::shmem::size(inode) {
        let start = if append { size } else { offset };
        return match crate::shmem::write(inode, start, src) {
            Some(Ok(written)) => Ok(fs::WriteResult {
                written,
                new_offset: start + written as u64,
            }),
            Some(Err(err)) => Err(shmem_err_to_fs(err)),
            None => Err(fs::EIO),
        };
    }
    fs::write(inode, offset, src, append)
}

pub(super) fn vfs_stat(inode: u16) -> Result<fs::FileStat, i64> {
    let mut stat = fs::stat(inode)?;
    if let Some(size) = crate::shmem::size(inode) {
        stat.size = size;
        stat.blocks = size.div_ceil(512);
    }
    Ok(stat)
}

pub(super) fn vfs_file_size(inode: u16) -> Result<u64, i64> {
    if let Some(size) = crate::shmem::size(inode) {
        return Ok(size);
    }
    fs::file_size(inode)
}

pub(super) fn vfs_truncate_to(inode: u16, length: u64) -> Result<(), i64> {
    if let Some(result) = crate::shmem::truncate(inode, length) {
        return result.map_err(shmem_err_to_fs);
    }
    if length == 0 {
        fs::truncate(inode)
    } else {
        fs::truncate_to(inode, length)
    }
}

pub(super) fn input_device_read(
    task_ptr: *mut task::Task,
    inode: u16,
    dst: &mut [u8],
    nonblocking: bool,
) -> Result<usize, i64> {
    let Some(device_id) = fs::inode_input_device_id(inode) else {
        return Err(-EINVAL);
    };
    crate::input::read_device(task_ptr, device_id, dst, nonblocking).map_err(|err| -err)
}

pub(super) fn sys_copy_file_range(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let in_fd = call.arg(0) as i32;
    let in_off_ptr = call.arg(1);
    let out_fd = call.arg(2) as i32;
    let out_off_ptr = call.arg(3);
    let len = match usize::try_from(call.arg(4)) {
        Ok(v) => v,
        Err(_) => return -EINVAL,
    };
    let flags = call.arg(5);
    if flags != 0 {
        return -EINVAL;
    }
    if len == 0 {
        return 0;
    }

    let in_idx = match fd_index(in_fd) {
        Ok(idx) => idx,
        Err(err) => return err,
    };
    let out_idx = match fd_index(out_fd) {
        Ok(idx) => idx,
        Err(err) => return err,
    };
    if !fd_slot_is_open(ctx.current, in_idx) || !fd_slot_is_open(ctx.current, out_idx) {
        return -EBADF;
    }

    let (in_kind, in_ino, in_fd_off, in_flags) = unsafe {
        (
            (*crate::task::leader_of(ctx.current)).user_fd_kind[in_idx],
            (*crate::task::leader_of(ctx.current)).user_fd_inode[in_idx],
            (*crate::task::leader_of(ctx.current)).user_fd_offset[in_idx],
            (*crate::task::leader_of(ctx.current)).user_fd_flags[in_idx],
        )
    };
    let (out_kind, out_ino, out_fd_off, out_flags) = unsafe {
        (
            (*crate::task::leader_of(ctx.current)).user_fd_kind[out_idx],
            (*crate::task::leader_of(ctx.current)).user_fd_inode[out_idx],
            (*crate::task::leader_of(ctx.current)).user_fd_offset[out_idx],
            (*crate::task::leader_of(ctx.current)).user_fd_flags[out_idx],
        )
    };
    if in_kind != task::USER_FD_KIND_VFS || out_kind != task::USER_FD_KIND_VFS {
        return -EINVAL;
    }
    if !fd_can_read(in_flags) || !fd_can_write(out_flags) {
        return -EBADF;
    }

    let mut in_off = if in_off_ptr != 0 {
        let off = match read_struct_from_user::<i64>(ctx.current, in_off_ptr) {
            Ok(v) => v,
            Err(err) => return -err,
        };
        if off < 0 {
            return -EINVAL;
        }
        off as u64
    } else {
        in_fd_off
    };
    let mut out_off = if out_off_ptr != 0 {
        let off = match read_struct_from_user::<i64>(ctx.current, out_off_ptr) {
            Ok(v) => v,
            Err(err) => return -err,
        };
        if off < 0 {
            return -EINVAL;
        }
        off as u64
    } else {
        out_fd_off
    };

    let mut copied = 0usize;
    let mut buf = [0u8; READ_CHUNK_SIZE];
    while copied < len {
        let take = cmp::min(buf.len(), len - copied);
        let read_n = match read_vfs_bytes(in_ino, in_off, &mut buf[..take]) {
            Ok(n) => n,
            Err(err) => {
                if copied > 0 {
                    break;
                }
                return err;
            }
        };
        if read_n == 0 {
            break;
        }

        let write_res = match vfs_write(out_ino, out_off, &buf[..read_n], false) {
            Ok(w) => w,
            Err(err) => {
                if copied > 0 {
                    break;
                }
                return -fs_to_errno(err);
            }
        };
        if write_res.written == 0 {
            break;
        }

        let wrote = write_res.written;
        vm_page_cache_invalidate_inode_range(out_ino, out_off, wrote as u64);
        in_off = in_off.saturating_add(wrote as u64);
        out_off = write_res.new_offset;
        copied += wrote;

        if wrote < read_n {
            break;
        }
    }

    if in_off_ptr == 0 {
        unsafe {
            (*crate::task::leader_of(ctx.current)).user_fd_offset[in_idx] = in_off;
        }
    } else if in_off <= i64::MAX as u64 {
        if let Err(err) = write_struct_to_user(ctx.current, in_off_ptr, &(in_off as i64)) {
            return -err;
        }
    }
    if out_off_ptr == 0 {
        unsafe {
            (*crate::task::leader_of(ctx.current)).user_fd_offset[out_idx] = out_off;
        }
    } else if out_off <= i64::MAX as u64 {
        if let Err(err) = write_struct_to_user(ctx.current, out_off_ptr, &(out_off as i64)) {
            return -err;
        }
    }

    if copied > 0 {
        sync_ext2_backing();
    }
    copied as i64
}

pub(super) fn sys_read(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let idx = match fd_index(fd) {
        Ok(idx) => idx,
        Err(err) => return err,
    };

    if !fd_slot_is_open(ctx.current, idx) {
        return -EBADF;
    }

    let len = match usize::try_from(call.arg(2)) {
        Ok(v) => v,
        Err(_) => return -EINVAL,
    };
    let (slot_kind, slot_inode, slot_flags) = unsafe {
        (
            (*crate::task::leader_of(ctx.current)).user_fd_kind[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_inode[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_flags[idx],
        )
    };
    if is_evfd_kind(slot_kind) {
        // eventfd/timerfd/signalfd reads are single atomic records, not a
        // byte stream, so they bypass the chunked loop below.
        return evfd_read(ctx, slot_kind, slot_inode, slot_flags, call.arg(1), len);
    }
    if len == 0 {
        return 0;
    }

    let mut read_total = 0usize;
    let mut chunk = [0u8; READ_CHUNK_SIZE];

    loop {
        if read_total >= len {
            break;
        }
        let take = cmp::min(chunk.len(), len - read_total);

        let (kind, inode, offset, flags) = unsafe {
            (
                (*crate::task::leader_of(ctx.current)).user_fd_kind[idx],
                (*crate::task::leader_of(ctx.current)).user_fd_inode[idx],
                (*crate::task::leader_of(ctx.current)).user_fd_offset[idx],
                (*crate::task::leader_of(ctx.current)).user_fd_flags[idx],
            )
        };

        if !fd_can_read(flags) {
            return -EBADF;
        }

        let read_n = if kind == task::USER_FD_KIND_CONSOLE {
            match crate::tty::read_console(ctx.current, &mut chunk[..take]) {
                Ok(n) => n,
                Err(err) => {
                    if read_total > 0 {
                        return read_total as i64;
                    }
                    return -err;
                }
            }
        } else if kind == task::USER_FD_KIND_VFS {
            let nb = (fd_open_flags(flags) & O_NONBLOCK) != 0;
            let read_result = if fs::inode_input_device_id(inode).is_some() {
                input_device_read(ctx.current, inode, &mut chunk[..take], nb)
            } else {
                read_vfs_bytes(inode, offset, &mut chunk[..take])
            };
            match read_result {
                Ok(n) => n,
                Err(err) => {
                    if read_total > 0 {
                        return read_total as i64;
                    }
                    return err;
                }
            }
        } else if kind == task::USER_FD_KIND_PIPE {
            let nb = (fd_open_flags(flags) & O_NONBLOCK) != 0;
            match ipc::pipe_read(ctx.current, inode, &mut chunk[..take], nb) {
                Ok(n) => n,
                Err(err) => {
                    if read_total > 0 {
                        return read_total as i64;
                    }
                    return -ipc_to_errno(err);
                }
            }
        } else if kind == task::USER_FD_KIND_SOCKET {
            let nb = (fd_open_flags(flags) & O_NONBLOCK) != 0;
            match socket::socket_recv(inode, &mut chunk[..take], nb) {
                Ok(n) => n,
                Err(err) => {
                    if read_total > 0 {
                        return read_total as i64;
                    }
                    return err;
                }
            }
        } else if kind == task::USER_FD_KIND_PTY_MASTER {
            let nb = (fd_open_flags(flags) & O_NONBLOCK) != 0;
            match crate::pty::master_read(ctx.current, inode, &mut chunk[..take], nb) {
                Ok(n) => n,
                Err(err) => {
                    if read_total > 0 {
                        return read_total as i64;
                    }
                    return -err;
                }
            }
        } else if kind == task::USER_FD_KIND_PTY_SLAVE {
            let nb = (fd_open_flags(flags) & O_NONBLOCK) != 0;
            match crate::pty::slave_read(ctx.current, inode, &mut chunk[..take], nb) {
                Ok(n) => n,
                Err(err) => {
                    if read_total > 0 {
                        return read_total as i64;
                    }
                    return -err;
                }
            }
        } else {
            return -EBADF;
        };

        if read_n == 0 {
            break;
        }

        match copy_to_user_task(
            ctx.current,
            call.arg(1) + read_total as u64,
            &chunk[..read_n],
        ) {
            Ok(()) => {}
            Err(err) => {
                if read_total > 0 {
                    return read_total as i64;
                }
                return -err;
            }
        }

        unsafe {
            (*crate::task::leader_of(ctx.current)).user_fd_offset[idx] = offset.saturating_add(read_n as u64);
        }
        read_total += read_n;

        if read_n < take {
            break;
        }
    }

    read_total as i64
}

pub(super) fn write_from_user_fd(
    ctx: &SyscallContext,
    idx: usize,
    user_src: u64,
    len: usize,
    wrote_vfs: &mut bool,
) -> Result<usize, i64> {
    if len == 0 {
        return Ok(0);
    }
    let mut chunk = [0u8; WRITE_CHUNK_SIZE];
    let mut written = 0usize;

    while written < len {
        let take = cmp::min(chunk.len(), len - written);
        let src = user_src + written as u64;
        let copy_res = copy_from_user_task(ctx.current, src, &mut chunk[..take]);
        if let Err(err) = copy_res {
            if written > 0 {
                return Ok(written);
            }
            return Err(-err);
        }

        let (kind, inode, offset, flags) = unsafe {
            (
                (*crate::task::leader_of(ctx.current)).user_fd_kind[idx],
                (*crate::task::leader_of(ctx.current)).user_fd_inode[idx],
                (*crate::task::leader_of(ctx.current)).user_fd_offset[idx],
                (*crate::task::leader_of(ctx.current)).user_fd_flags[idx],
            )
        };

        if !fd_can_write(flags) {
            if written > 0 {
                return Ok(written);
            }
            return Err(-EBADF);
        }

        if kind == task::USER_FD_KIND_CONSOLE {
            crate::serial::write_bytes(&chunk[..take]);
            unsafe {
                (*crate::task::leader_of(ctx.current)).user_fd_offset[idx] = offset.saturating_add(take as u64);
            }
            written += take;
            continue;
        }

        if kind == task::USER_FD_KIND_VFS {
            if fs::inode_input_device_id(inode).is_some() {
                if written > 0 {
                    return Ok(written);
                }
                return Err(-EINVAL);
            }
            let append = (fd_open_flags(flags) & fs::O_APPEND) != 0;
            let seals = fd_seals(flags, inode);
            if seals != 0 {
                let size = vfs_file_size(inode).unwrap_or(0);
                let start = if append { size } else { offset };
                let end = start.saturating_add(take as u64);
                let sealed = seals_allow_write(seals)
                    .and_then(|()| seals_allow_resize(seals, size, end.max(size)));
                if let Err(err) = sealed {
                    if written > 0 {
                        return Ok(written);
                    }
                    return Err(err);
                }
            }
            let write_res = match vfs_write(inode, offset, &chunk[..take], append) {
                Ok(result) => result,
                Err(err) => {
                    if written > 0 {
                        return Ok(written);
                    }
                    return Err(-fs_to_errno(err));
                }
            };

            if write_res.written == 0 {
                break;
            }

            vm_page_cache_invalidate_inode_range(inode, offset, write_res.written as u64);
            unsafe {
                (*crate::task::leader_of(ctx.current)).user_fd_offset[idx] = write_res.new_offset;
            }
            if write_res.written > 0 {
                *wrote_vfs = true;
            }
            written += write_res.written;
            continue;
        }

        if kind == task::USER_FD_KIND_PIPE {
            let nb = (fd_open_flags(flags) & O_NONBLOCK) != 0;
            let wrote = match ipc::pipe_write(ctx.current, inode, &chunk[..take], nb) {
                Ok(n) => n,
                Err(err) => {
                    if err == ipc::EPIPE {
                        let _ = signal::send(ctx.current, signal::SIGPIPE);
                    }
                    if written > 0 {
                        return Ok(written);
                    }
                    return Err(-ipc_to_errno(err));
                }
            };
            if wrote == 0 {
                break;
            }
            unsafe {
                (*crate::task::leader_of(ctx.current)).user_fd_offset[idx] = offset.saturating_add(wrote as u64);
            }
            written += wrote;
            continue;
        }

        if kind == task::USER_FD_KIND_SOCKET {
            let nb = (fd_open_flags(flags) & O_NONBLOCK) != 0;
            let wrote = match socket::socket_send(inode, &chunk[..take], nb) {
                Ok(n) => n,
                Err(err) => {
                    if written > 0 {
                        return Ok(written);
                    }
                    return Err(err);
                }
            };
            if wrote == 0 {
                break;
            }
            written += wrote;
            continue;
        }

        if kind == task::USER_FD_KIND_PTY_MASTER {
            let nb = (fd_open_flags(flags) & O_NONBLOCK) != 0;
            let wrote = match crate::pty::master_write(ctx.current, inode, &chunk[..take], nb) {
                Ok(n) => n,
                Err(err) => {
                    if written > 0 {
                        return Ok(written);
                    }
                    return Err(-err);
                }
            };
            if wrote == 0 {
                break;
            }
            written += wrote;
            continue;
        }

        if kind == task::USER_FD_KIND_PTY_SLAVE {
            let nb = (fd_open_flags(flags) & O_NONBLOCK) != 0;
            let wrote = match crate::pty::slave_write(ctx.current, inode, &chunk[..take], nb) {
                Ok(n) => n,
                Err(err) => {
                    if written > 0 {
                        return Ok(written);
                    }
                    return Err(-err);
                }
            };
            if wrote == 0 {
                break;
            }
            written += wrote;
            continue;
        }

        if written > 0 {
            return Ok(written);
        }
        return Err(-EBADF);
    }

    Ok(written)
}

pub(super) fn sys_write(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let idx = match fd_index(fd) {
        Ok(idx) => idx,
        Err(err) => return err,
    };

    if !fd_slot_is_open(ctx.current, idx) {
        return -EBADF;
    }

    let len = match usize::try_from(call.arg(2)) {
        Ok(v) => v,
        Err(_) => return -EINVAL,
    };

    let (slot_kind, slot_inode, slot_flags) = unsafe {
        (
            (*crate::task::leader_of(ctx.current)).user_fd_kind[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_inode[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_flags[idx],
        )
    };
    if is_evfd_kind(slot_kind) {
        return evfd_write(ctx, slot_kind, slot_inode, slot_flags, call.arg(1), len);
    }

    let mut wrote_vfs = false;
    let written = match write_from_user_fd(ctx, idx, call.arg(1), len, &mut wrote_vfs) {
        Ok(n) => n,
        Err(err) => return err,
    };

    if wrote_vfs && written > 0 {
        sync_ext2_backing();
    }

    written as i64
}

pub(super) fn sys_writev(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let idx = match fd_index(fd) {
        Ok(idx) => idx,
        Err(err) => return err,
    };

    if !fd_slot_is_open(ctx.current, idx) {
        return -EBADF;
    }

    let iovcnt = call.arg(2);
    if iovcnt > WRITE_IOV_MAX as u64 {
        return -EINVAL;
    }

    let mut total = 0usize;
    let mut wrote_vfs = false;
    let iov_ptr = call.arg(1);

    for i in 0..(iovcnt as usize) {
        let entry_ptr = iov_ptr + (i * size_of::<LinuxIovec>()) as u64;
        let iov = match read_struct_from_user::<LinuxIovec>(ctx.current, entry_ptr) {
            Ok(v) => v,
            Err(err) => {
                if total > 0 {
                    break;
                }
                return -err;
            }
        };

        let len = match usize::try_from(iov.iov_len) {
            Ok(v) => v,
            Err(_) => return -EINVAL,
        };
        if len == 0 {
            continue;
        }
        if total > (i64::MAX as usize).saturating_sub(len) {
            return -EINVAL;
        }

        let wrote = match write_from_user_fd(ctx, idx, iov.iov_base, len, &mut wrote_vfs) {
            Ok(n) => n,
            Err(err) => {
                if total > 0 {
                    break;
                }
                return err;
            }
        };

        total += wrote;
        if wrote < len {
            break;
        }
    }

    if wrote_vfs && total > 0 {
        sync_ext2_backing();
    }

    total as i64
}

pub(super) fn sys_lseek(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let idx = match fd_index(fd) {
        Ok(idx) => idx,
        Err(err) => return err,
    };
    if !fd_slot_is_open(ctx.current, idx) {
        return -EBADF;
    }

    let (kind, inode, current) = unsafe {
        (
            (*crate::task::leader_of(ctx.current)).user_fd_kind[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_inode[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_offset[idx],
        )
    };
    if kind == task::USER_FD_KIND_CONSOLE
        || kind == task::USER_FD_KIND_PIPE
        || kind == task::USER_FD_KIND_PTY_MASTER
        || kind == task::USER_FD_KIND_PTY_SLAVE
    {
        return -ESPIPE;
    }
    if kind != task::USER_FD_KIND_VFS {
        return -EBADF;
    }

    if let Some(size) = crate::shmem::size(inode) {
        let base = match call.arg(2) as u32 {
            0 => 0i128,
            1 => current as i128,
            2 => size as i128,
            _ => return -EINVAL,
        };
        let next = base + (call.arg(1) as i64) as i128;
        if next < 0 || next > i64::MAX as i128 {
            return -EINVAL;
        }
        unsafe {
            (*crate::task::leader_of(ctx.current)).user_fd_offset[idx] = next as u64;
        }
        return next as i64;
    }

    let new_offset = match fs::seek(inode, current, call.arg(1) as i64, call.arg(2) as u32) {
        Ok(offset) => offset,
        Err(err) => return -fs_to_errno(err),
    };

    unsafe {
        (*crate::task::leader_of(ctx.current)).user_fd_offset[idx] = new_offset;
    }
    new_offset as i64
}

// ---------------------------------------------------------------------------
// pread64 (17)
// ---------------------------------------------------------------------------
pub(super) fn sys_pread64(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let buf_ptr = call.arg(1);
    let count = call.arg(2);
    let offset = call.arg(3);

    let idx = match fd_index(fd) {
        Ok(idx) => idx,
        Err(err) => return err,
    };
    if !fd_slot_is_open(ctx.current, idx) {
        return -EBADF;
    }

    let len = match usize::try_from(count) {
        Ok(v) => v,
        Err(_) => return -EINVAL,
    };
    if len == 0 {
        return 0;
    }

    let (kind, inode, flags) = unsafe {
        (
            (*crate::task::leader_of(ctx.current)).user_fd_kind[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_inode[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_flags[idx],
        )
    };

    if !fd_can_read(flags) {
        return -EBADF;
    }
    if kind == task::USER_FD_KIND_PIPE
        || kind == task::USER_FD_KIND_CONSOLE
        || kind == task::USER_FD_KIND_PTY_MASTER
        || kind == task::USER_FD_KIND_PTY_SLAVE
    {
        return -ESPIPE;
    }
    if kind != task::USER_FD_KIND_VFS {
        return -EBADF;
    }

    let mut read_total = 0usize;
    let mut chunk = [0u8; READ_CHUNK_SIZE];
    let mut pos = offset;

    loop {
        if read_total >= len {
            break;
        }
        let take = cmp::min(chunk.len(), len - read_total);

        let read_n = match read_vfs_bytes(inode, pos, &mut chunk[..take]) {
            Ok(n) => n,
            Err(err) => {
                if read_total > 0 {
                    return read_total as i64;
                }
                return err;
            }
        };
        if read_n == 0 {
            break;
        }

        match copy_to_user_task(ctx.current, buf_ptr + read_total as u64, &chunk[..read_n]) {
            Ok(()) => {}
            Err(err) => {
                if read_total > 0 {
                    return read_total as i64;
                }
                return -err;
            }
        }

        pos = pos.saturating_add(read_n as u64);
        read_total += read_n;

        if read_n < take {
            break;
        }
    }

    read_total as i64
}

// ---------------------------------------------------------------------------
// pwrite64 (18)
// ---------------------------------------------------------------------------
pub(super) fn sys_pwrite64(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let buf_ptr = call.arg(1);
    let count = call.arg(2);
    let offset = call.arg(3);

    let idx = match fd_index(fd) {
        Ok(idx) => idx,
        Err(err) => return err,
    };
    if !fd_slot_is_open(ctx.current, idx) {
        return -EBADF;
    }

    let len = match usize::try_from(count) {
        Ok(v) => v,
        Err(_) => return -EINVAL,
    };
    if len == 0 {
        return 0;
    }

    let (kind, inode, flags) = unsafe {
        (
            (*crate::task::leader_of(ctx.current)).user_fd_kind[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_inode[idx],
            (*crate::task::leader_of(ctx.current)).user_fd_flags[idx],
        )
    };

    if !fd_can_write(flags) {
        return -EBADF;
    }
    if kind == task::USER_FD_KIND_PIPE
        || kind == task::USER_FD_KIND_CONSOLE
        || kind == task::USER_FD_KIND_PTY_MASTER
        || kind == task::USER_FD_KIND_PTY_SLAVE
    {
        return -ESPIPE;
    }
    if kind != task::USER_FD_KIND_VFS {
        return -EBADF;
    }

    let mut chunk = [0u8; WRITE_CHUNK_SIZE];
    let mut written = 0usize;
    let mut pos = offset;

    while written < len {
        let take = cmp::min(chunk.len(), len - written);
        if let Err(err) =
            copy_from_user_task(ctx.current, buf_ptr + written as u64, &mut chunk[..take])
        {
            if written > 0 {
                return written as i64;
            }
            return -err;
        }

        let write_res = match vfs_write(inode, pos, &chunk[..take], false) {
            Ok(result) => result,
            Err(err) => {
                if written > 0 {
                    return written as i64;
                }
                return -fs_to_errno(err);
            }
        };

        if write_res.written == 0 {
            break;
        }

        vm_page_cache_invalidate_inode_range(inode, pos, write_res.written as u64);
        pos = write_res.new_offset;
        written += write_res.written;
    }

    if written > 0 {
        sync_ext2_backing();
    }

    written as i64
}

// ---------------------------------------------------------------------------
// readv (19)
// ---------------------------------------------------------------------------
pub(super) fn sys_readv(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let iov_ptr = call.arg(1);
    let iovcnt = call.arg(2);

    if iovcnt > WRITE_IOV_MAX as u64 {
        return -EINVAL;
    }

    let idx = match fd_index(fd) {
        Ok(idx) => idx,
        Err(err) => return err,
    };
    if !fd_slot_is_open(ctx.current, idx) {
        return -EBADF;
    }

    let mut total = 0usize;

    for i in 0..(iovcnt as usize) {
        let entry_ptr = iov_ptr + (i * size_of::<LinuxIovec>()) as u64;
        let iov = match read_struct_from_user::<LinuxIovec>(ctx.current, entry_ptr) {
            Ok(v) => v,
            Err(err) => {
                if total > 0 {
                    break;
                }
                return -err;
            }
        };

        let len = match usize::try_from(iov.iov_len) {
            Ok(v) => v,
            Err(_) => return -EINVAL,
        };
        if len == 0 {
            continue;
        }
        if total > (i64::MAX as usize).saturating_sub(len) {
            return -EINVAL;
        }

        // Build a synthetic SyscallArgs for the read sub-operation.
        let sub_call = SyscallArgs {
            num: SYS_READ,
            args: [fd as u64, iov.iov_base, len as u64, 0, 0, 0],
        };
        let n = sys_read(ctx, &sub_call);
        if n < 0 {
            if total > 0 {
                break;
            }
            return n;
        }
        total += n as usize;
        if (n as usize) < len {
            break;
        }
    }

    total as i64
}
