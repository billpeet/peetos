use core::cmp;
use core::ptr;

use arch_x86_64::addr::{PhysAddr, VirtAddr, KERNEL_VIRT_BASE, PAGE_SIZE};
use arch_x86_64::idt::InterruptFrame;
use arch_x86_64::pit;
use arch_x86_64::syscall::SyscallFrame;

use crate::ipc;
use crate::memory::{pmm, vmm};
use crate::task::{self, TaskState, TASK_FLAG_USER};

#[cfg(not(test))]
macro_rules! sys_log {
    ($($arg:tt)*) => {
        crate::serial_println!($($arg)*);
    };
}

#[cfg(test)]
macro_rules! sys_log {
    ($($arg:tt)*) => {{
        let _ = core::format_args!($($arg)*);
    }};
}

const EPERM: i64 = 1;
const ENOENT: i64 = 2;
const ESRCH: i64 = 3;
const EIO: i64 = 5;
const EBADF: i64 = 9;
const EAGAIN: i64 = 11;
const ENOMEM: i64 = 12;
const EFAULT: i64 = 14;
const EBUSY: i64 = 16;
const EEXIST: i64 = 17;
const EXDEV: i64 = 18;
const EINVAL: i64 = 22;
const EMFILE: i64 = 24;
const ESPIPE: i64 = 29;
const EROFS: i64 = 30;
const EPIPE: i64 = 32;
const ENOTDIR: i64 = 20;
const EISDIR: i64 = 21;
const ENOTEMPTY: i64 = 39;
const ENOSYS: i64 = 38;
const ENOSPC: i64 = 28;

const SYS_READ: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_OPEN: u64 = 2;
const SYS_CLOSE: u64 = 3;
const SYS_FSTAT: u64 = 5;
const SYS_LSEEK: u64 = 8;
const SYS_BRK: u64 = 12;
const SYS_PIPE: u64 = 22;
const SYS_YIELD: u64 = 24;
const SYS_GETPID: u64 = 39;
const SYS_EXIT: u64 = 60;
const SYS_GETPPID: u64 = 110;
const SYS_GETTID: u64 = 186;
const SYS_FUTEX: u64 = 202;
const SYS_SET_TID_ADDRESS: u64 = 218;
const SYS_GETDENTS64: u64 = 217;
const SYS_EXIT_GROUP: u64 = 231;
const SYS_OPENAT: u64 = 257;
const SYS_MKDIRAT: u64 = 258;
const SYS_NEWFSTATAT: u64 = 262;
const SYS_UNLINKAT: u64 = 263;
const SYS_RENAMEAT: u64 = 264;
const SYS_PIPE2: u64 = 293;

const AT_REMOVEDIR: u32 = 0x200;
const AT_EMPTY_PATH: u32 = 0x1000;

const WRITE_CHUNK_SIZE: usize = 128;
const READ_CHUNK_SIZE: usize = 128;
const UNKNOWN_SYSCALL_LOG_LIMIT: u32 = 8;
const CLEAR_TID_SIZE: usize = 4;

static mut UNKNOWN_SYSCALL_LOG_COUNT: u32 = 0;

#[derive(Clone, Copy)]
struct SyscallArgs {
    num: u64,
    args: [u64; 6],
}

impl SyscallArgs {
    #[inline]
    fn arg(&self, idx: usize) -> u64 {
        self.args[idx]
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SyscallMode {
    AnyTask,
    UserTask,
}

type SyscallHandler = fn(&SyscallContext, &SyscallArgs) -> i64;

#[derive(Clone, Copy)]
struct SyscallDef {
    num: u64,
    name: &'static str,
    mode: SyscallMode,
    handler: SyscallHandler,
}

#[derive(Clone, Copy)]
struct SyscallContext {
    current: *mut task::Task,
}

impl SyscallContext {
    #[inline]
    fn new(current: *mut task::Task) -> Self {
        Self { current }
    }

    #[inline]
    fn has_task(&self) -> bool {
        !self.current.is_null()
    }

    #[inline]
    fn is_user_task(&self) -> bool {
        self.has_task() && unsafe { ((*self.current).flags & TASK_FLAG_USER) != 0 }
    }

    #[inline]
    fn pid(&self) -> Result<i64, i64> {
        if !self.has_task() {
            return Err(-ESRCH);
        }
        Ok(unsafe { (*self.current).pid as i64 })
    }

    #[inline]
    fn ppid(&self) -> Result<i64, i64> {
        if !self.has_task() {
            return Err(-ESRCH);
        }
        Ok(unsafe { (*self.current).ppid as i64 })
    }

    fn user_pml4(&self) -> Result<PhysAddr, i64> {
        if !self.has_task() {
            return Err(-ESRCH);
        }
        let pml4 = unsafe { PhysAddr::new((*self.current).pml4_phys) };
        if pml4.as_u64() == 0 {
            return Err(-EINVAL);
        }
        Ok(pml4)
    }
}

const fn syscall_def(
    num: u64,
    name: &'static str,
    mode: SyscallMode,
    handler: SyscallHandler,
) -> SyscallDef {
    SyscallDef {
        num,
        name,
        mode,
        handler,
    }
}

const SYSCALL_TABLE: &[SyscallDef] = &[
    syscall_def(SYS_READ, "read", SyscallMode::UserTask, sys_read),
    syscall_def(SYS_WRITE, "write", SyscallMode::UserTask, sys_write),
    syscall_def(SYS_OPEN, "open", SyscallMode::UserTask, sys_open),
    syscall_def(SYS_CLOSE, "close", SyscallMode::UserTask, sys_close),
    syscall_def(SYS_FSTAT, "fstat", SyscallMode::UserTask, sys_fstat),
    syscall_def(SYS_LSEEK, "lseek", SyscallMode::UserTask, sys_lseek),
    syscall_def(SYS_PIPE, "pipe", SyscallMode::UserTask, sys_pipe),
    syscall_def(SYS_BRK, "brk", SyscallMode::UserTask, sys_brk),
    syscall_def(SYS_YIELD, "sched_yield", SyscallMode::AnyTask, sys_yield),
    syscall_def(SYS_GETPID, "getpid", SyscallMode::AnyTask, sys_getpid),
    syscall_def(SYS_GETPPID, "getppid", SyscallMode::AnyTask, sys_getppid),
    syscall_def(SYS_GETTID, "gettid", SyscallMode::AnyTask, sys_gettid),
    syscall_def(
        SYS_SET_TID_ADDRESS,
        "set_tid_address",
        SyscallMode::UserTask,
        sys_set_tid_address,
    ),
    syscall_def(SYS_EXIT, "exit", SyscallMode::UserTask, sys_exit),
    syscall_def(
        SYS_EXIT_GROUP,
        "exit_group",
        SyscallMode::UserTask,
        sys_exit_group,
    ),
    syscall_def(SYS_GETDENTS64, "getdents64", SyscallMode::UserTask, sys_getdents64),
    syscall_def(SYS_FUTEX, "futex", SyscallMode::UserTask, sys_futex),
    syscall_def(SYS_OPENAT, "openat", SyscallMode::UserTask, sys_openat),
    syscall_def(SYS_MKDIRAT, "mkdirat", SyscallMode::UserTask, sys_mkdirat),
    syscall_def(SYS_NEWFSTATAT, "newfstatat", SyscallMode::UserTask, sys_newfstatat),
    syscall_def(SYS_UNLINKAT, "unlinkat", SyscallMode::UserTask, sys_unlinkat),
    syscall_def(SYS_RENAMEAT, "renameat", SyscallMode::UserTask, sys_renameat),
    syscall_def(SYS_PIPE2, "pipe2", SyscallMode::UserTask, sys_pipe2),
];

#[inline]
fn to_sysret(value: i64) -> u64 {
    value as u64
}

#[inline]
fn page_align_up(value: u64) -> Option<u64> {
    value
        .checked_add(PAGE_SIZE - 1)
        .map(|v| v & !(PAGE_SIZE - 1))
}

fn lookup_syscall(num: u64) -> Option<&'static SyscallDef> {
    for def in SYSCALL_TABLE {
        if def.num == num {
            return Some(def);
        }
    }
    None
}

fn fs_to_errno(err: i64) -> i64 {
    match err {
        fs::EPERM => EPERM,
        fs::ENOENT => ENOENT,
        fs::EIO => EIO,
        fs::EEXIST => EEXIST,
        fs::EBUSY => EBUSY,
        fs::ENOTDIR => ENOTDIR,
        fs::EISDIR => EISDIR,
        fs::EINVAL => EINVAL,
        fs::ENOSPC => ENOSPC,
        fs::EROFS => EROFS,
        fs::ENOTEMPTY => ENOTEMPTY,
        fs::ENAMETOOLONG => EINVAL,
        _ => EIO,
    }
}

fn ipc_to_errno(err: i64) -> i64 {
    match err {
        ipc::EINVAL => EINVAL,
        ipc::ENOSPC => ENOSPC,
        ipc::EPIPE => EPIPE,
        ipc::EAGAIN => EAGAIN,
        ipc::ENOENT => ENOENT,
        _ => EIO,
    }
}

#[inline]
fn fd_index(fd: i32) -> Result<usize, i64> {
    if fd < 0 || fd as usize >= task::MAX_TASK_FDS {
        return Err(-EBADF);
    }
    Ok(fd as usize)
}

fn fd_slot_is_open(task_ptr: *mut task::Task, idx: usize) -> bool {
    unsafe { (*task_ptr).user_fd_kind[idx] != task::USER_FD_KIND_EMPTY }
}

fn fd_alloc(task_ptr: *mut task::Task, kind: u8, inode: u16, flags: u32) -> Result<i64, i64> {
    if task_ptr.is_null() {
        return Err(-ESRCH);
    }

    for idx in 3..task::MAX_TASK_FDS {
        if unsafe { (*task_ptr).user_fd_kind[idx] == task::USER_FD_KIND_EMPTY } {
            unsafe {
                (*task_ptr).user_fd_kind[idx] = kind;
                (*task_ptr).user_fd_inode[idx] = if kind == task::USER_FD_KIND_VFS
                    || kind == task::USER_FD_KIND_PIPE
                {
                    inode
                } else {
                    task::USER_FD_NONE
                };
                (*task_ptr).user_fd_offset[idx] = 0;
                (*task_ptr).user_fd_flags[idx] = flags;
            }
            return Ok(idx as i64);
        }
    }

    Err(-EMFILE)
}

fn fd_close(task_ptr: *mut task::Task, fd: i32) -> Result<(), i64> {
    let idx = fd_index(fd)?;
    if task_ptr.is_null() {
        return Err(-ESRCH);
    }
    if !fd_slot_is_open(task_ptr, idx) {
        return Err(-EBADF);
    }

    let (kind, inode, flags) = unsafe {
        (
            (*task_ptr).user_fd_kind[idx],
            (*task_ptr).user_fd_inode[idx],
            (*task_ptr).user_fd_flags[idx],
        )
    };

    if kind == task::USER_FD_KIND_PIPE {
        let close_read = fd_can_read(flags);
        let close_write = fd_can_write(flags);
        if inode != task::USER_FD_NONE {
            let _ = ipc::pipe_close(inode, close_read, close_write);
        }
    }

    unsafe {
        (*task_ptr).user_fd_kind[idx] = task::USER_FD_KIND_EMPTY;
        (*task_ptr).user_fd_inode[idx] = task::USER_FD_NONE;
        (*task_ptr).user_fd_offset[idx] = 0;
        (*task_ptr).user_fd_flags[idx] = 0;
    }
    Ok(())
}

fn close_all_fds(task_ptr: *mut task::Task) {
    if task_ptr.is_null() {
        return;
    }

    let mut fd = 0usize;
    while fd < task::MAX_TASK_FDS {
        let _ = fd_close(task_ptr, fd as i32);
        fd += 1;
    }
}

fn fd_access_mode(flags: u32) -> u32 {
    flags & fs::O_ACCMODE
}

fn fd_can_read(flags: u32) -> bool {
    let mode = fd_access_mode(flags);
    mode == fs::O_RDONLY || mode == fs::O_RDWR
}

fn fd_can_write(flags: u32) -> bool {
    let mode = fd_access_mode(flags);
    mode == fs::O_WRONLY || mode == fs::O_RDWR
}

fn read_user_path(path_ptr: u64, out: &mut [u8; fs::VFS_MAX_PATH]) -> Result<&str, i64> {
    let len = match copy_cstr_from_user_current(path_ptr, out) {
        Ok(len) => len,
        Err(err) => return Err(-err),
    };
    match core::str::from_utf8(&out[..len]) {
        Ok(path) => Ok(path),
        Err(_) => Err(-EINVAL),
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxStat {
    st_dev: u64,
    st_ino: u64,
    st_nlink: u64,
    st_mode: u32,
    st_uid: u32,
    st_gid: u32,
    __pad0: u32,
    st_rdev: u64,
    st_size: i64,
    st_blksize: i64,
    st_blocks: i64,
    st_atime: i64,
    st_atime_nsec: i64,
    st_mtime: i64,
    st_mtime_nsec: i64,
    st_ctime: i64,
    st_ctime_nsec: i64,
    __unused: [i64; 3],
}

impl LinuxStat {
    fn from_fs(stat: fs::FileStat) -> Self {
        Self {
            st_dev: 0,
            st_ino: stat.ino,
            st_nlink: stat.nlink as u64,
            st_mode: stat.mode,
            st_uid: stat.uid,
            st_gid: stat.gid,
            __pad0: 0,
            st_rdev: 0,
            st_size: stat.size as i64,
            st_blksize: stat.blksize as i64,
            st_blocks: stat.blocks as i64,
            st_atime: stat.atime as i64,
            st_atime_nsec: 0,
            st_mtime: stat.mtime as i64,
            st_mtime_nsec: 0,
            st_ctime: stat.ctime as i64,
            st_ctime_nsec: 0,
            __unused: [0; 3],
        }
    }
}

fn resolve_dir_inode(ctx: &SyscallContext, dirfd: i32) -> Result<u16, i64> {
    if dirfd == fs::AT_FDCWD {
        if ctx.current.is_null() {
            return Err(-ESRCH);
        }
        return Ok(unsafe { (*ctx.current).user_cwd_inode });
    }

    let idx = fd_index(dirfd)?;
    if !fd_slot_is_open(ctx.current, idx) {
        return Err(-EBADF);
    }

    let (kind, inode) = unsafe { ((*ctx.current).user_fd_kind[idx], (*ctx.current).user_fd_inode[idx]) };
    if kind != task::USER_FD_KIND_VFS {
        return Err(-ENOTDIR);
    }
    match fs::inode_is_dir(inode) {
        Ok(true) => Ok(inode),
        Ok(false) => Err(-ENOTDIR),
        Err(err) => Err(-fs_to_errno(err)),
    }
}

fn resolve_dir_for_path(ctx: &SyscallContext, dirfd: i32, path: &str) -> Result<u16, i64> {
    if path.starts_with('/') {
        Ok(fs::root_inode())
    } else {
        resolve_dir_inode(ctx, dirfd)
    }
}

fn refresh_procfs_runtime() {
    let pmm_stats = pmm::get_stats();
    fs::set_proc_uptime_ticks(pit::ticks());
    fs::set_proc_meminfo(
        pmm_stats.total_bytes,
        pmm_stats.free_bytes,
        pmm_stats.used_bytes,
    );
}

fn user_range_ok(start: u64, len: usize) -> bool {
    if len == 0 {
        return true;
    }

    if start >= KERNEL_VIRT_BASE {
        return false;
    }

    let end = match start.checked_add((len as u64).saturating_sub(1)) {
        Some(end) => end,
        None => return false,
    };

    end < KERNEL_VIRT_BASE
}

fn copy_user_bytes(
    pml4: PhysAddr,
    user_addr: u64,
    kernel_buf: *mut u8,
    len: usize,
    to_user: bool,
) -> Result<(), i64> {
    if len == 0 {
        return Ok(());
    }
    if !user_range_ok(user_addr, len) {
        return Err(EFAULT);
    }

    let mut remaining = len;
    let mut copied = 0usize;

    while remaining > 0 {
        let vaddr = user_addr + copied as u64;
        let page_vaddr = vaddr & !(PAGE_SIZE - 1);
        let page_off = (vaddr & (PAGE_SIZE - 1)) as usize;
        let bytes_in_page = cmp::min(remaining, PAGE_SIZE as usize - page_off);

        let page_phys = match vmm::get_physical_in(pml4, VirtAddr::new(page_vaddr)) {
            Some(phys) => phys,
            None => return Err(EFAULT),
        };

        let page_ptr = (PhysAddr::new(page_phys.as_u64() + page_off as u64)
            .to_virt()
            .as_u64()) as *mut u8;
        let kernel_ptr = unsafe { kernel_buf.add(copied) };

        unsafe {
            if to_user {
                ptr::copy_nonoverlapping(kernel_ptr as *const u8, page_ptr, bytes_in_page);
            } else {
                ptr::copy_nonoverlapping(page_ptr as *const u8, kernel_ptr, bytes_in_page);
            }
        }

        copied += bytes_in_page;
        remaining -= bytes_in_page;
    }

    Ok(())
}

fn copy_from_user_task(
    task_ptr: *mut task::Task,
    user_src: u64,
    dst: &mut [u8],
) -> Result<(), i64> {
    if task_ptr.is_null() {
        return Err(ESRCH);
    }
    let pml4 = unsafe { PhysAddr::new((*task_ptr).pml4_phys) };
    if pml4.as_u64() == 0 {
        return Err(EINVAL);
    }
    copy_user_bytes(pml4, user_src, dst.as_mut_ptr(), dst.len(), false)
}

fn copy_to_user_task(task_ptr: *mut task::Task, user_dst: u64, src: &[u8]) -> Result<(), i64> {
    if task_ptr.is_null() {
        return Err(ESRCH);
    }
    let pml4 = unsafe { PhysAddr::new((*task_ptr).pml4_phys) };
    if pml4.as_u64() == 0 {
        return Err(EINVAL);
    }
    copy_user_bytes(pml4, user_dst, src.as_ptr() as *mut u8, src.len(), true)
}

pub(crate) fn copy_from_user_current(user_src: u64, dst: &mut [u8]) -> Result<(), i64> {
    copy_from_user_task(task::current(), user_src, dst)
}

pub(crate) fn copy_to_user_current(user_dst: u64, src: &[u8]) -> Result<(), i64> {
    copy_to_user_task(task::current(), user_dst, src)
}

pub(crate) fn copy_cstr_from_user_current(user_src: u64, out: &mut [u8]) -> Result<usize, i64> {
    if out.is_empty() {
        return Err(EINVAL);
    }

    for (idx, dst_byte) in out.iter_mut().enumerate() {
        let mut byte = [0u8; 1];
        copy_from_user_current(user_src + idx as u64, &mut byte)?;
        *dst_byte = byte[0];
        if byte[0] == 0 {
            return Ok(idx);
        }
    }

    Err(EINVAL)
}

fn ensure_user_pages_unmapped(pml4: PhysAddr, start: u64, end: u64) -> Result<(), i64> {
    let mut addr = start;
    while addr < end {
        if vmm::get_physical_in(pml4, VirtAddr::new(addr)).is_some() {
            return Err(ENOMEM);
        }
        addr += PAGE_SIZE;
    }
    Ok(())
}

fn map_user_pages(pml4: PhysAddr, start: u64, end: u64) -> Result<(), i64> {
    let mut addr = start;
    while addr < end {
        let phys = pmm::alloc_page();
        if phys.as_u64() == 0 {
            return Err(ENOMEM);
        }

        unsafe {
            ptr::write_bytes(phys.to_virt().as_u64() as *mut u8, 0, PAGE_SIZE as usize);
        }

        if vmm::map_page_in(
            pml4,
            VirtAddr::new(addr),
            phys,
            vmm::PTE_PRESENT | vmm::PTE_WRITE | vmm::PTE_USER | vmm::PTE_NO_EXECUTE,
        ) != 0
        {
            pmm::free_page(phys);
            return Err(ENOMEM);
        }

        addr += PAGE_SIZE;
    }

    Ok(())
}

fn unmap_user_pages(pml4: PhysAddr, start: u64, end: u64) {
    let mut addr = start;
    while addr < end {
        let virt = VirtAddr::new(addr);
        if let Some(phys) = vmm::get_physical_in(pml4, virt) {
            if vmm::unmap_page_in(pml4, virt) == 0 {
                pmm::free_page(PhysAddr::new(phys.as_u64() & !(PAGE_SIZE - 1)));
            }
        }
        addr += PAGE_SIZE;
    }
}

fn set_program_break(ctx: &SyscallContext, requested: u64) -> i64 {
    let pml4 = match ctx.user_pml4() {
        Ok(pml4) => pml4,
        Err(err) => return err,
    };

    let (current, start, stack_bottom) = unsafe {
        (
            (*ctx.current).user_brk,
            (*ctx.current).user_brk_start,
            (*ctx.current).user_stack_bottom,
        )
    };

    if current == 0 || start == 0 || stack_bottom == 0 {
        return -EINVAL;
    }

    if requested == 0 {
        return current as i64;
    }

    let heap_limit = stack_bottom.saturating_sub(PAGE_SIZE);
    if requested < start || requested >= heap_limit || requested >= KERNEL_VIRT_BASE {
        return current as i64;
    }

    let old_pages_end = match page_align_up(current) {
        Some(v) => v,
        None => return current as i64,
    };
    let new_pages_end = match page_align_up(requested) {
        Some(v) => v,
        None => return current as i64,
    };

    if new_pages_end > old_pages_end {
        if ensure_user_pages_unmapped(pml4, old_pages_end, new_pages_end).is_err() {
            return current as i64;
        }
        if map_user_pages(pml4, old_pages_end, new_pages_end).is_err() {
            unmap_user_pages(pml4, old_pages_end, new_pages_end);
            return current as i64;
        }
    } else if new_pages_end < old_pages_end {
        unmap_user_pages(pml4, new_pages_end, old_pages_end);
    }

    unsafe {
        (*ctx.current).user_brk = requested;
    }
    requested as i64
}

fn sys_open(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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

fn sys_openat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let flags = call.arg(2) as u32;
    let mode = call.arg(3) as u32;
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

    let opened = match fs::openat(dir_inode, path, flags, mode) {
        Ok(opened) => opened,
        Err(err) => return -fs_to_errno(err),
    };

    let fd_kind = if opened.is_console {
        task::USER_FD_KIND_CONSOLE
    } else {
        task::USER_FD_KIND_VFS
    };

    match fd_alloc(ctx.current, fd_kind, opened.inode, flags) {
        Ok(fd) => fd,
        Err(err) => err,
    }
}

fn sys_mkdirat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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
        Ok(()) => 0,
        Err(err) => -fs_to_errno(err),
    }
}

fn write_pipe_fds(task_ptr: *mut task::Task, user_dst: u64, read_fd: i32, write_fd: i32) -> Result<(), i64> {
    let mut raw = [0u8; 8];
    raw[..4].copy_from_slice(&read_fd.to_ne_bytes());
    raw[4..8].copy_from_slice(&write_fd.to_ne_bytes());
    copy_to_user_task(task_ptr, user_dst, &raw)
}

fn sys_pipe2(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let user_pipefd = call.arg(0);
    let flags = call.arg(1) as u32;
    if flags != 0 {
        return -EINVAL;
    }

    let pipe_id = match ipc::pipe_open() {
        Ok(id) => id,
        Err(err) => return -ipc_to_errno(err),
    };

    let read_fd = match fd_alloc(ctx.current, task::USER_FD_KIND_PIPE, pipe_id, fs::O_RDONLY) {
        Ok(fd) => fd as i32,
        Err(err) => {
            let _ = ipc::pipe_close(pipe_id, true, true);
            return err;
        }
    };
    let write_fd = match fd_alloc(ctx.current, task::USER_FD_KIND_PIPE, pipe_id, fs::O_WRONLY) {
        Ok(fd) => fd as i32,
        Err(err) => {
            let _ = fd_close(ctx.current, read_fd);
            return err;
        }
    };

    if let Err(err) = write_pipe_fds(ctx.current, user_pipefd, read_fd, write_fd) {
        let _ = fd_close(ctx.current, read_fd);
        let _ = fd_close(ctx.current, write_fd);
        return -err;
    }

    0
}

fn sys_pipe(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let wrapped = SyscallArgs {
        num: SYS_PIPE2,
        args: [call.arg(0), 0, 0, 0, 0, 0],
    };
    sys_pipe2(ctx, &wrapped)
}

fn write_stat_to_user(task_ptr: *mut task::Task, dst: u64, stat: fs::FileStat) -> i64 {
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

fn sys_fstat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let idx = match fd_index(call.arg(0) as i32) {
        Ok(idx) => idx,
        Err(err) => return err,
    };
    if !fd_slot_is_open(ctx.current, idx) {
        return -EBADF;
    }

    let (kind, inode) = unsafe { ((*ctx.current).user_fd_kind[idx], (*ctx.current).user_fd_inode[idx]) };
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

fn sys_newfstatat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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

fn dirent_record_len(name_len: usize) -> usize {
    (19 + name_len + 1 + 7) & !7
}

fn write_u16_ne(dst: &mut [u8], offset: usize, value: u16) {
    let b = value.to_ne_bytes();
    dst[offset] = b[0];
    dst[offset + 1] = b[1];
}

fn write_u64_ne(dst: &mut [u8], offset: usize, value: u64) {
    let b = value.to_ne_bytes();
    dst[offset..offset + 8].copy_from_slice(&b);
}

fn write_i64_ne(dst: &mut [u8], offset: usize, value: i64) {
    let b = value.to_ne_bytes();
    dst[offset..offset + 8].copy_from_slice(&b);
}

fn sys_getdents64(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let idx = match fd_index(call.arg(0) as i32) {
        Ok(idx) => idx,
        Err(err) => return err,
    };
    if !fd_slot_is_open(ctx.current, idx) {
        return -EBADF;
    }

    let (kind, inode, mut offset) = unsafe {
        (
            (*ctx.current).user_fd_kind[idx],
            (*ctx.current).user_fd_inode[idx],
            (*ctx.current).user_fd_offset[idx],
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

        if let Err(err) = copy_to_user_task(ctx.current, call.arg(1) + total as u64, &record[..reclen]) {
            if total > 0 {
                break;
            }
            return -err;
        }

        total += reclen;
        offset = entry.next_offset;
    }

    unsafe {
        (*ctx.current).user_fd_offset[idx] = offset;
    }
    total as i64
}

fn read_user_u32(task_ptr: *mut task::Task, user_src: u64) -> Result<u32, i64> {
    let mut raw = [0u8; 4];
    copy_from_user_task(task_ptr, user_src, &mut raw)?;
    Ok(u32::from_ne_bytes(raw))
}

fn sys_futex(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let uaddr = call.arg(0);
    if !user_range_ok(uaddr, 4) {
        return -EFAULT;
    }

    let op = call.arg(1) as u32;
    let cmd = op & 0x7F;
    let val = call.arg(2) as u32;

    match cmd {
        ipc::FUTEX_WAIT => {
            if call.arg(3) != 0 {
                return -EINVAL;
            }
            let current = match read_user_u32(ctx.current, uaddr) {
                Ok(v) => v,
                Err(err) => return -err,
            };
            if current != val {
                return -EAGAIN;
            }
            match ipc::futex_wait(ctx.current, uaddr) {
                Ok(()) => 0,
                Err(err) => -ipc_to_errno(err),
            }
        }
        ipc::FUTEX_WAKE => ipc::futex_wake(uaddr, val) as i64,
        _ => -EINVAL,
    }
}

fn sys_unlinkat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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
        Ok(()) => 0,
        Err(err) => -fs_to_errno(err),
    }
}

fn sys_renameat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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
        Ok(()) => 0,
        Err(err) => {
            if err == fs::EINVAL && old_dir != new_dir {
                -EXDEV
            } else {
                -fs_to_errno(err)
            }
        }
    }
}

fn sys_close(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    match fd_close(ctx.current, call.arg(0) as i32) {
        Ok(()) => 0,
        Err(err) => err,
    }
}

fn sys_read(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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
                (*ctx.current).user_fd_kind[idx],
                (*ctx.current).user_fd_inode[idx],
                (*ctx.current).user_fd_offset[idx],
                (*ctx.current).user_fd_flags[idx],
            )
        };

        if !fd_can_read(flags) {
            return -EBADF;
        }

        let read_n = if kind == task::USER_FD_KIND_CONSOLE {
            0usize
        } else if kind == task::USER_FD_KIND_VFS {
            refresh_procfs_runtime();
            match fs::read(inode, offset, &mut chunk[..take]) {
                Ok(n) => n,
                Err(err) => {
                    if read_total > 0 {
                        return read_total as i64;
                    }
                    return -fs_to_errno(err);
                }
            }
        } else if kind == task::USER_FD_KIND_PIPE {
            match ipc::pipe_read(ctx.current, inode, &mut chunk[..take]) {
                Ok(n) => n,
                Err(err) => {
                    if read_total > 0 {
                        return read_total as i64;
                    }
                    return -ipc_to_errno(err);
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
            (*ctx.current).user_fd_offset[idx] = offset.saturating_add(read_n as u64);
        }
        read_total += read_n;

        if read_n < take {
            break;
        }
    }

    read_total as i64
}

fn sys_write(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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
    if len == 0 {
        return 0;
    }

    let mut chunk = [0u8; WRITE_CHUNK_SIZE];
    let mut written = 0usize;

    while written < len {
        let take = cmp::min(chunk.len(), len - written);
        let src = call.arg(1) + written as u64;
        let copy_res = copy_from_user_task(ctx.current, src, &mut chunk[..take]);
        if let Err(err) = copy_res {
            if written > 0 {
                return written as i64;
            }
            return -err;
        }

        let (kind, inode, offset, flags) = unsafe {
            (
                (*ctx.current).user_fd_kind[idx],
                (*ctx.current).user_fd_inode[idx],
                (*ctx.current).user_fd_offset[idx],
                (*ctx.current).user_fd_flags[idx],
            )
        };

        if !fd_can_write(flags) {
            return -EBADF;
        }

        if kind == task::USER_FD_KIND_CONSOLE {
            crate::serial::write_bytes(&chunk[..take]);
            unsafe {
                (*ctx.current).user_fd_offset[idx] = offset.saturating_add(take as u64);
            }
            written += take;
            continue;
        }

        if kind == task::USER_FD_KIND_VFS {
            let append = (flags & fs::O_APPEND) != 0;
            let write_res = match fs::write(inode, offset, &chunk[..take], append) {
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

            unsafe {
                (*ctx.current).user_fd_offset[idx] = write_res.new_offset;
            }
            written += write_res.written;
            continue;
        }

        if kind == task::USER_FD_KIND_PIPE {
            let wrote = match ipc::pipe_write(ctx.current, inode, &chunk[..take]) {
                Ok(n) => n,
                Err(err) => {
                    if written > 0 {
                        return written as i64;
                    }
                    return -ipc_to_errno(err);
                }
            };
            if wrote == 0 {
                break;
            }
            unsafe {
                (*ctx.current).user_fd_offset[idx] = offset.saturating_add(wrote as u64);
            }
            written += wrote;
            continue;
        }

        return -EBADF;
    }

    written as i64
}

fn sys_lseek(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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
            (*ctx.current).user_fd_kind[idx],
            (*ctx.current).user_fd_inode[idx],
            (*ctx.current).user_fd_offset[idx],
        )
    };
    if kind == task::USER_FD_KIND_CONSOLE || kind == task::USER_FD_KIND_PIPE {
        return -ESPIPE;
    }
    if kind != task::USER_FD_KIND_VFS {
        return -EBADF;
    }

    let new_offset = match fs::seek(inode, current, call.arg(1) as i64, call.arg(2) as u32) {
        Ok(offset) => offset,
        Err(err) => return -fs_to_errno(err),
    };

    unsafe {
        (*ctx.current).user_fd_offset[idx] = new_offset;
    }
    new_offset as i64
}

fn sys_brk(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    set_program_break(ctx, call.arg(0))
}

fn sys_yield(_ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    crate::sched::request_preempt();
    0
}

fn sys_getpid(ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    match ctx.pid() {
        Ok(pid) => pid,
        Err(err) => err,
    }
}

fn sys_getppid(ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    match ctx.ppid() {
        Ok(ppid) => ppid,
        Err(err) => err,
    }
}

fn sys_gettid(ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    match ctx.pid() {
        Ok(tid) => tid,
        Err(err) => err,
    }
}

fn sys_set_tid_address(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let clear_tid = call.arg(0);
    if clear_tid != 0 && !user_range_ok(clear_tid, CLEAR_TID_SIZE) {
        return -EFAULT;
    }

    unsafe {
        (*ctx.current).user_clear_tid = clear_tid;
    }

    sys_gettid(ctx, call)
}

fn clear_child_tid(ctx: &SyscallContext) {
    let clear_tid = unsafe { (*ctx.current).user_clear_tid };
    if clear_tid == 0 {
        return;
    }

    let zero = [0u8; CLEAR_TID_SIZE];
    let _ = copy_to_user_task(ctx.current, clear_tid, &zero);
    let _ = ipc::futex_wake(clear_tid, 1);
    unsafe {
        (*ctx.current).user_clear_tid = 0;
    }
}

fn sys_exit_common(ctx: &SyscallContext, code: i32) -> i64 {
    if !ctx.has_task() {
        return -ESRCH;
    }

    clear_child_tid(ctx);
    close_all_fds(ctx.current);
    ipc::remove_task_waiters(ctx.current);

    unsafe {
        (*ctx.current).exit_code = code;
        (*ctx.current).state = TaskState::Zombie;
    }
    crate::sched::request_preempt();
    0
}

fn sys_exit(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    sys_exit_common(ctx, call.arg(0) as i32)
}

fn sys_exit_group(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    sys_exit_common(ctx, call.arg(0) as i32)
}

fn log_unknown_syscall(num: u64) {
    unsafe {
        if UNKNOWN_SYSCALL_LOG_COUNT < UNKNOWN_SYSCALL_LOG_LIMIT {
            sys_log!("[SYSCALL] Unhandled syscall {}", num);
            UNKNOWN_SYSCALL_LOG_COUNT += 1;
            if UNKNOWN_SYSCALL_LOG_COUNT == UNKNOWN_SYSCALL_LOG_LIMIT {
                sys_log!("[SYSCALL] Further unknown-syscall logs muted");
            }
        }
    }
}

fn dispatch_syscall(call: SyscallArgs) -> i64 {
    let ctx = SyscallContext::new(task::current());
    let def = match lookup_syscall(call.num) {
        Some(def) => def,
        None => {
            log_unknown_syscall(call.num);
            return -ENOSYS;
        }
    };

    if def.mode == SyscallMode::UserTask && !ctx.is_user_task() {
        return -EPERM;
    }

    let _ = def.name;
    (def.handler)(&ctx, &call)
}

fn call_from_syscall_frame(frame: &SyscallFrame) -> SyscallArgs {
    SyscallArgs {
        num: frame.rax,
        args: [
            frame.rdi, frame.rsi, frame.rdx, frame.r10, frame.r8, frame.r9,
        ],
    }
}

fn call_from_int80_frame(frame: &InterruptFrame) -> SyscallArgs {
    SyscallArgs {
        num: frame.rax,
        args: [
            frame.rdi, frame.rsi, frame.rdx, frame.r10, frame.r8, frame.r9,
        ],
    }
}

pub fn init() {
    unsafe {
        UNKNOWN_SYSCALL_LOG_COUNT = 0;
    }
}

#[no_mangle]
pub extern "C" fn syscall_handler(frame: &mut SyscallFrame) -> u64 {
    to_sysret(dispatch_syscall(call_from_syscall_frame(frame)))
}

pub extern "C" fn syscall_int80_handler(frame: &mut InterruptFrame) {
    frame.rax = to_sysret(dispatch_syscall(call_from_int80_frame(frame)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_range_validation() {
        assert!(user_range_ok(0x1000, 4));
        assert!(!user_range_ok(KERNEL_VIRT_BASE, 1));
        assert!(!user_range_ok(KERNEL_VIRT_BASE - 1, 2));
    }

    #[test]
    fn page_align_up_rounds_correctly() {
        assert_eq!(page_align_up(0), Some(0));
        assert_eq!(page_align_up(1), Some(PAGE_SIZE));
        assert_eq!(page_align_up(PAGE_SIZE), Some(PAGE_SIZE));
    }

    #[test]
    fn syscall_lookup_finds_known_entry() {
        let def = lookup_syscall(SYS_GETPID).expect("syscall should exist");
        assert_eq!(def.name, "getpid");
    }

    #[test]
    fn syscall_lookup_includes_vfs_extensions() {
        assert_eq!(lookup_syscall(SYS_PIPE).map(|d| d.name), Some("pipe"));
        assert_eq!(lookup_syscall(SYS_MKDIRAT).map(|d| d.name), Some("mkdirat"));
        assert_eq!(lookup_syscall(SYS_FUTEX).map(|d| d.name), Some("futex"));
        assert_eq!(lookup_syscall(SYS_NEWFSTATAT).map(|d| d.name), Some("newfstatat"));
        assert_eq!(lookup_syscall(SYS_GETDENTS64).map(|d| d.name), Some("getdents64"));
        assert_eq!(lookup_syscall(SYS_UNLINKAT).map(|d| d.name), Some("unlinkat"));
        assert_eq!(lookup_syscall(SYS_RENAMEAT).map(|d| d.name), Some("renameat"));
        assert_eq!(lookup_syscall(SYS_PIPE2).map(|d| d.name), Some("pipe2"));
    }

    #[test]
    fn unknown_syscall_returns_enosys() {
        let ret = dispatch_syscall(SyscallArgs {
            num: 0xFFFF,
            args: [0; 6],
        });
        assert_eq!(ret, -ENOSYS);
    }

    #[test]
    fn user_only_syscall_requires_user_task() {
        let ret = dispatch_syscall(SyscallArgs {
            num: SYS_BRK,
            args: [0; 6],
        });
        assert_eq!(ret, -EPERM);
    }

    #[test]
    fn set_tid_requires_user_task() {
        let ret = dispatch_syscall(SyscallArgs {
            num: SYS_SET_TID_ADDRESS,
            args: [0; 6],
        });
        assert_eq!(ret, -EPERM);
    }

    #[test]
    fn getpid_without_task_returns_esrch() {
        let ret = dispatch_syscall(SyscallArgs {
            num: SYS_GETPID,
            args: [0; 6],
        });
        assert_eq!(ret, -ESRCH);
    }
}
