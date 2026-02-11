use core::cmp;
use core::mem::{size_of, MaybeUninit};
use core::ptr;

use arch_x86_64::addr::{PhysAddr, VirtAddr, KERNEL_VIRT_BASE, PAGE_SIZE};
use arch_x86_64::idt::InterruptFrame;
use arch_x86_64::pit;
use arch_x86_64::syscall::SyscallFrame;

use crate::ipc;
use crate::memory::{pmm, vmm};
use crate::signal;
use crate::shm;
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
const SYS_RT_SIGACTION: u64 = 13;
const SYS_RT_SIGPROCMASK: u64 = 14;
const SYS_RT_SIGRETURN: u64 = 15;
const SYS_BRK: u64 = 12;
const SYS_PIPE: u64 = 22;
const SYS_YIELD: u64 = 24;
const SYS_SHMGET: u64 = 29;
const SYS_SHMAT: u64 = 30;
const SYS_SHMCTL: u64 = 31;
const SYS_GETPID: u64 = 39;
const SYS_EXIT: u64 = 60;
const SYS_KILL: u64 = 62;
const SYS_SHMDT: u64 = 67;
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
const SIGNAL_FRAME_MAGIC_SYSCALL: u64 = 0x5352_4653_5953_0001;
const SIGNAL_FRAME_MAGIC_INT80: u64 = 0x5352_4649_3830_0001;

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
    syscall_def(
        SYS_RT_SIGACTION,
        "rt_sigaction",
        SyscallMode::UserTask,
        sys_rt_sigaction,
    ),
    syscall_def(
        SYS_RT_SIGPROCMASK,
        "rt_sigprocmask",
        SyscallMode::UserTask,
        sys_rt_sigprocmask,
    ),
    syscall_def(
        SYS_RT_SIGRETURN,
        "rt_sigreturn",
        SyscallMode::UserTask,
        sys_rt_sigreturn,
    ),
    syscall_def(SYS_PIPE, "pipe", SyscallMode::UserTask, sys_pipe),
    syscall_def(SYS_SHMGET, "shmget", SyscallMode::UserTask, sys_shmget),
    syscall_def(SYS_SHMAT, "shmat", SyscallMode::UserTask, sys_shmat),
    syscall_def(SYS_SHMCTL, "shmctl", SyscallMode::UserTask, sys_shmctl),
    syscall_def(SYS_BRK, "brk", SyscallMode::UserTask, sys_brk),
    syscall_def(SYS_YIELD, "sched_yield", SyscallMode::AnyTask, sys_yield),
    syscall_def(SYS_GETPID, "getpid", SyscallMode::AnyTask, sys_getpid),
    syscall_def(SYS_KILL, "kill", SyscallMode::UserTask, sys_kill),
    syscall_def(SYS_SHMDT, "shmdt", SyscallMode::UserTask, sys_shmdt),
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
    syscall_def(
        SYS_GETDENTS64,
        "getdents64",
        SyscallMode::UserTask,
        sys_getdents64,
    ),
    syscall_def(SYS_FUTEX, "futex", SyscallMode::UserTask, sys_futex),
    syscall_def(SYS_OPENAT, "openat", SyscallMode::UserTask, sys_openat),
    syscall_def(SYS_MKDIRAT, "mkdirat", SyscallMode::UserTask, sys_mkdirat),
    syscall_def(
        SYS_NEWFSTATAT,
        "newfstatat",
        SyscallMode::UserTask,
        sys_newfstatat,
    ),
    syscall_def(
        SYS_UNLINKAT,
        "unlinkat",
        SyscallMode::UserTask,
        sys_unlinkat,
    ),
    syscall_def(
        SYS_RENAMEAT,
        "renameat",
        SyscallMode::UserTask,
        sys_renameat,
    ),
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

#[inline]
fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
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

fn shm_to_errno(err: i64) -> i64 {
    match err {
        shm::ENOENT => ENOENT,
        shm::ENOMEM => ENOMEM,
        shm::EEXIST => EEXIST,
        shm::EINVAL => EINVAL,
        shm::ENOSPC => ENOSPC,
        _ => EIO,
    }
}

fn signal_to_errno(err: i64) -> i64 {
    match err {
        signal::EINVAL => EINVAL,
        signal::ESRCH => ESRCH,
        signal::ENOSYS => ENOSYS,
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
                (*task_ptr).user_fd_inode[idx] =
                    if kind == task::USER_FD_KIND_VFS || kind == task::USER_FD_KIND_PIPE {
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

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxSigAction {
    sa_handler: u64,
    sa_flags: u64,
    sa_restorer: u64,
    sa_mask: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UserSignalFrameSyscall {
    magic: u64,
    saved_mask: u64,
    signum: u64,
    r9: u64,
    r8: u64,
    r10: u64,
    rdx: u64,
    rsi: u64,
    rdi: u64,
    rax: u64,
    r11: u64,
    rcx: u64,
    rsp: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UserSignalFrameInt80 {
    magic: u64,
    saved_mask: u64,
    signum: u64,
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    r11: u64,
    r10: u64,
    r9: u64,
    r8: u64,
    rbp: u64,
    rdi: u64,
    rsi: u64,
    rdx: u64,
    rcx: u64,
    rbx: u64,
    rax: u64,
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
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

    let (kind, inode) = unsafe {
        (
            (*ctx.current).user_fd_kind[idx],
            (*ctx.current).user_fd_inode[idx],
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

fn write_struct_to_user<T: Copy>(
    task_ptr: *mut task::Task,
    user_dst: u64,
    value: &T,
) -> Result<(), i64> {
    let bytes =
        unsafe { core::slice::from_raw_parts((value as *const T) as *const u8, size_of::<T>()) };
    copy_to_user_task(task_ptr, user_dst, bytes)
}

fn read_struct_from_user<T: Copy>(task_ptr: *mut task::Task, user_src: u64) -> Result<T, i64> {
    let mut raw = MaybeUninit::<T>::uninit();
    let bytes = unsafe { core::slice::from_raw_parts_mut(raw.as_mut_ptr() as *mut u8, size_of::<T>()) };
    copy_from_user_task(task_ptr, user_src, bytes)?;
    Ok(unsafe { raw.assume_init() })
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

fn write_pipe_fds(
    task_ptr: *mut task::Task,
    user_dst: u64,
    read_fd: i32,
    write_fd: i32,
) -> Result<(), i64> {
    let mut raw = [0u8; 8];
    raw[..4].copy_from_slice(&read_fd.to_ne_bytes());
    raw[4..8].copy_from_slice(&write_fd.to_ne_bytes());
    copy_to_user_task(task_ptr, user_dst, &raw)
}

fn read_sigaction_from_user(task_ptr: *mut task::Task, user_src: u64) -> Result<LinuxSigAction, i64> {
    let mut raw = [0u8; core::mem::size_of::<LinuxSigAction>()];
    copy_from_user_task(task_ptr, user_src, &mut raw)?;
    let ptr = raw.as_ptr() as *const LinuxSigAction;
    Ok(unsafe { ptr.read_unaligned() })
}

fn write_sigaction_to_user(
    task_ptr: *mut task::Task,
    user_dst: u64,
    action: LinuxSigAction,
) -> Result<(), i64> {
    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&action as *const LinuxSigAction) as *const u8,
            core::mem::size_of::<LinuxSigAction>(),
        )
    };
    copy_to_user_task(task_ptr, user_dst, bytes)
}

fn read_u64_from_user(task_ptr: *mut task::Task, user_src: u64) -> Result<u64, i64> {
    let mut raw = [0u8; 8];
    copy_from_user_task(task_ptr, user_src, &mut raw)?;
    Ok(u64::from_ne_bytes(raw))
}

fn write_u64_to_user(task_ptr: *mut task::Task, user_dst: u64, value: u64) -> Result<(), i64> {
    copy_to_user_task(task_ptr, user_dst, &value.to_ne_bytes())
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

fn sys_kill(_ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let pid = call.arg(0) as i64;
    let sig = call.arg(1) as i64;

    if pid <= 0 {
        return -EINVAL;
    }
    if sig < 0 {
        return -EINVAL;
    }

    let target = task::get_by_pid(pid as i32);
    if target.is_null() {
        return -ESRCH;
    }
    if sig == 0 {
        return 0;
    }

    match signal::send(target, sig as u32) {
        Ok(()) => 0,
        Err(err) => -signal_to_errno(err),
    }
}

fn sys_rt_sigaction(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let signum = call.arg(0) as u32;
    let act_ptr = call.arg(1);
    let oldact_ptr = call.arg(2);
    let sigsetsize = call.arg(3);

    if sigsetsize != 8 {
        return -EINVAL;
    }

    if oldact_ptr != 0 {
        let old = match signal::read_action(ctx.current, signum) {
            Ok(old) => old,
            Err(err) => return -signal_to_errno(err),
        };
        let old_linux = LinuxSigAction {
            sa_handler: old.handler,
            sa_flags: old.flags,
            sa_restorer: old.restorer,
            sa_mask: old.mask,
        };
        if let Err(err) = write_sigaction_to_user(ctx.current, oldact_ptr, old_linux) {
            return -err;
        }
    }

    if act_ptr != 0 {
        let act = match read_sigaction_from_user(ctx.current, act_ptr) {
            Ok(act) => act,
            Err(err) => return -err,
        };
        let new_action = signal::SigAction {
            handler: act.sa_handler,
            flags: act.sa_flags,
            restorer: act.sa_restorer,
            mask: act.sa_mask,
        };
        if let Err(err) = signal::write_action(ctx.current, signum, new_action) {
            return -signal_to_errno(err);
        }
    }

    0
}

fn sys_rt_sigprocmask(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let how = call.arg(0) as i32;
    let set_ptr = call.arg(1);
    let oldset_ptr = call.arg(2);
    let sigsetsize = call.arg(3);

    if sigsetsize != 8 {
        return -EINVAL;
    }

    if oldset_ptr != 0 {
        let mask = signal::current_mask(ctx.current);
        if let Err(err) = write_u64_to_user(ctx.current, oldset_ptr, mask) {
            return -err;
        }
    }

    if set_ptr != 0 {
        let set = match read_u64_from_user(ctx.current, set_ptr) {
            Ok(set) => set,
            Err(err) => return -err,
        };
        if let Err(err) = signal::update_mask(ctx.current, how, set) {
            return -signal_to_errno(err);
        }
    }

    0
}

fn sys_rt_sigreturn(_ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    // rt_sigreturn requires direct access to the live trap frame and is
    // handled in the entry stubs before table dispatch.
    -ENOSYS
}

fn sys_shmget(_ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let key = call.arg(0) as i32;
    let size = call.arg(1);
    let flags = call.arg(2) as i32;
    match shm::shm_get(key, size, flags) {
        Ok(shmid) => shmid as i64,
        Err(err) => -shm_to_errno(err),
    }
}

fn sys_shmat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let shmid = call.arg(0) as i32;
    let addr = call.arg(1);
    let flags = call.arg(2) as i32;
    match shm::shm_attach(ctx.current, shmid, addr, flags) {
        Ok(vaddr) => vaddr as i64,
        Err(err) => -shm_to_errno(err),
    }
}

fn sys_shmdt(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let addr = call.arg(0);
    match shm::shm_detach(ctx.current, addr) {
        Ok(()) => 0,
        Err(err) => -shm_to_errno(err),
    }
}

fn sys_shmctl(_ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let shmid = call.arg(0) as i32;
    let cmd = call.arg(1) as i32;
    match shm::shm_ctl(shmid, cmd) {
        Ok(()) => 0,
        Err(err) => -shm_to_errno(err),
    }
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

    let (kind, inode) = unsafe {
        (
            (*ctx.current).user_fd_kind[idx],
            (*ctx.current).user_fd_inode[idx],
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
                    if err == ipc::EPIPE {
                        let _ = signal::send(ctx.current, signal::SIGPIPE);
                    }
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
    shm::cleanup_task(ctx.current);
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

fn install_user_signal_frame_syscall(
    task_ptr: *mut task::Task,
    frame: &mut SyscallFrame,
    delivery: signal::UserDelivery,
) -> Result<(), i64> {
    let frame_size = size_of::<UserSignalFrameSyscall>() as u64;
    let frame_rsp = frame.rsp.checked_sub(frame_size).ok_or(EFAULT)?;
    let frame_rsp = align_down(frame_rsp, 16);
    let return_rsp = frame_rsp.checked_sub(size_of::<u64>() as u64).ok_or(EFAULT)?;

    let signal_frame = UserSignalFrameSyscall {
        magic: SIGNAL_FRAME_MAGIC_SYSCALL,
        saved_mask: delivery.saved_mask,
        signum: delivery.signum as u64,
        r9: frame.r9,
        r8: frame.r8,
        r10: frame.r10,
        rdx: frame.rdx,
        rsi: frame.rsi,
        rdi: frame.rdi,
        rax: frame.rax,
        r11: frame.r11,
        rcx: frame.rcx,
        rsp: frame.rsp,
    };

    write_struct_to_user(task_ptr, frame_rsp, &signal_frame)?;
    write_u64_to_user(task_ptr, return_rsp, delivery.restorer)?;

    frame.rcx = delivery.handler;
    frame.rsp = return_rsp;
    frame.rdi = delivery.signum as u64;
    Ok(())
}

fn install_user_signal_frame_int80(
    task_ptr: *mut task::Task,
    frame: &mut InterruptFrame,
    delivery: signal::UserDelivery,
) -> Result<(), i64> {
    let frame_size = size_of::<UserSignalFrameInt80>() as u64;
    let frame_rsp = frame.rsp.checked_sub(frame_size).ok_or(EFAULT)?;
    let frame_rsp = align_down(frame_rsp, 16);
    let return_rsp = frame_rsp.checked_sub(size_of::<u64>() as u64).ok_or(EFAULT)?;

    let signal_frame = UserSignalFrameInt80 {
        magic: SIGNAL_FRAME_MAGIC_INT80,
        saved_mask: delivery.saved_mask,
        signum: delivery.signum as u64,
        r15: frame.r15,
        r14: frame.r14,
        r13: frame.r13,
        r12: frame.r12,
        r11: frame.r11,
        r10: frame.r10,
        r9: frame.r9,
        r8: frame.r8,
        rbp: frame.rbp,
        rdi: frame.rdi,
        rsi: frame.rsi,
        rdx: frame.rdx,
        rcx: frame.rcx,
        rbx: frame.rbx,
        rax: frame.rax,
        rip: frame.rip,
        cs: frame.cs,
        rflags: frame.rflags,
        rsp: frame.rsp,
        ss: frame.ss,
    };

    write_struct_to_user(task_ptr, frame_rsp, &signal_frame)?;
    write_u64_to_user(task_ptr, return_rsp, delivery.restorer)?;

    frame.rip = delivery.handler;
    frame.rsp = return_rsp;
    frame.rdi = delivery.signum as u64;
    Ok(())
}

fn restore_user_signal_frame_syscall(task_ptr: *mut task::Task, frame: &mut SyscallFrame) -> i64 {
    let signal_frame = match read_struct_from_user::<UserSignalFrameSyscall>(task_ptr, frame.rsp) {
        Ok(signal_frame) => signal_frame,
        Err(err) => return -err,
    };
    if signal_frame.magic != SIGNAL_FRAME_MAGIC_SYSCALL {
        return -EINVAL;
    }
    if !user_range_ok(signal_frame.rsp, 1) || !user_range_ok(signal_frame.rcx, 1) {
        return -EFAULT;
    }
    if let Err(err) = signal::restore_mask(task_ptr, signal_frame.saved_mask) {
        return -signal_to_errno(err);
    }

    frame.r9 = signal_frame.r9;
    frame.r8 = signal_frame.r8;
    frame.r10 = signal_frame.r10;
    frame.rdx = signal_frame.rdx;
    frame.rsi = signal_frame.rsi;
    frame.rdi = signal_frame.rdi;
    frame.rax = signal_frame.rax;
    frame.r11 = signal_frame.r11;
    frame.rcx = signal_frame.rcx;
    frame.rsp = signal_frame.rsp;
    0
}

fn restore_user_signal_frame_int80(task_ptr: *mut task::Task, frame: &mut InterruptFrame) -> i64 {
    let signal_frame = match read_struct_from_user::<UserSignalFrameInt80>(task_ptr, frame.rsp) {
        Ok(signal_frame) => signal_frame,
        Err(err) => return -err,
    };
    if signal_frame.magic != SIGNAL_FRAME_MAGIC_INT80 {
        return -EINVAL;
    }
    if !user_range_ok(signal_frame.rsp, 1) || !user_range_ok(signal_frame.rip, 1) {
        return -EFAULT;
    }
    if let Err(err) = signal::restore_mask(task_ptr, signal_frame.saved_mask) {
        return -signal_to_errno(err);
    }

    frame.r15 = signal_frame.r15;
    frame.r14 = signal_frame.r14;
    frame.r13 = signal_frame.r13;
    frame.r12 = signal_frame.r12;
    frame.r11 = signal_frame.r11;
    frame.r10 = signal_frame.r10;
    frame.r9 = signal_frame.r9;
    frame.r8 = signal_frame.r8;
    frame.rbp = signal_frame.rbp;
    frame.rdi = signal_frame.rdi;
    frame.rsi = signal_frame.rsi;
    frame.rdx = signal_frame.rdx;
    frame.rcx = signal_frame.rcx;
    frame.rbx = signal_frame.rbx;
    frame.rax = signal_frame.rax;
    frame.rip = signal_frame.rip;
    frame.cs = signal_frame.cs;
    frame.rflags = signal_frame.rflags;
    frame.rsp = signal_frame.rsp;
    frame.ss = signal_frame.ss;
    0
}

fn can_process_pending_signals(current: *mut task::Task) -> bool {
    if current.is_null() {
        return false;
    }
    unsafe { (*current).state != TaskState::Zombie && (*current).state != TaskState::Dead }
}

fn apply_pending_signals_syscall(frame: &mut SyscallFrame) -> Option<i64> {
    let current = task::current();
    if !can_process_pending_signals(current) {
        return None;
    }

    loop {
        match signal::consume_next(current) {
            signal::DeliveryAction::None => return None,
            signal::DeliveryAction::Ignore => continue,
            signal::DeliveryAction::Terminate(code) => {
                let ctx = SyscallContext::new(current);
                return Some(sys_exit_common(&ctx, code));
            }
            signal::DeliveryAction::Handle(delivery) => {
                if install_user_signal_frame_syscall(current, frame, delivery).is_err() {
                    let ctx = SyscallContext::new(current);
                    return Some(sys_exit_common(&ctx, 128 + signal::SIGSEGV as i32));
                }
                return None;
            }
        }
    }
}

fn apply_pending_signals_int80(frame: &mut InterruptFrame) -> Option<i64> {
    let current = task::current();
    if !can_process_pending_signals(current) {
        return None;
    }

    loop {
        match signal::consume_next(current) {
            signal::DeliveryAction::None => return None,
            signal::DeliveryAction::Ignore => continue,
            signal::DeliveryAction::Terminate(code) => {
                let ctx = SyscallContext::new(current);
                return Some(sys_exit_common(&ctx, code));
            }
            signal::DeliveryAction::Handle(delivery) => {
                if install_user_signal_frame_int80(current, frame, delivery).is_err() {
                    let ctx = SyscallContext::new(current);
                    return Some(sys_exit_common(&ctx, 128 + signal::SIGSEGV as i32));
                }
                return None;
            }
        }
    }
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
    let ctx = SyscallContext::new(task::current());
    if frame.rax == SYS_RT_SIGRETURN {
        if !ctx.is_user_task() {
            return to_sysret(-EPERM);
        }
        let restore = restore_user_signal_frame_syscall(ctx.current, frame);
        if restore < 0 {
            return to_sysret(restore);
        }
        if let Some(ret) = apply_pending_signals_syscall(frame) {
            frame.rax = to_sysret(ret);
        }
        return frame.rax;
    }

    let result = dispatch_syscall(call_from_syscall_frame(frame));
    frame.rax = to_sysret(result);
    if let Some(ret) = apply_pending_signals_syscall(frame) {
        frame.rax = to_sysret(ret);
    }
    frame.rax
}

pub extern "C" fn syscall_int80_handler(frame: &mut InterruptFrame) {
    let ctx = SyscallContext::new(task::current());
    if frame.rax == SYS_RT_SIGRETURN {
        frame.rax = if !ctx.is_user_task() {
            to_sysret(-EPERM)
        } else {
            let restore = restore_user_signal_frame_int80(ctx.current, frame);
            if restore < 0 {
                to_sysret(restore)
            } else {
                frame.rax
            }
        };
        if let Some(ret) = apply_pending_signals_int80(frame) {
            frame.rax = to_sysret(ret);
        }
        return;
    }

    frame.rax = to_sysret(dispatch_syscall(call_from_int80_frame(frame)));
    if let Some(ret) = apply_pending_signals_int80(frame) {
        frame.rax = to_sysret(ret);
    }
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
        assert_eq!(lookup_syscall(SYS_KILL).map(|d| d.name), Some("kill"));
        assert_eq!(
            lookup_syscall(SYS_RT_SIGACTION).map(|d| d.name),
            Some("rt_sigaction")
        );
        assert_eq!(
            lookup_syscall(SYS_RT_SIGPROCMASK).map(|d| d.name),
            Some("rt_sigprocmask")
        );
        assert_eq!(
            lookup_syscall(SYS_RT_SIGRETURN).map(|d| d.name),
            Some("rt_sigreturn")
        );
        assert_eq!(lookup_syscall(SYS_SHMGET).map(|d| d.name), Some("shmget"));
        assert_eq!(lookup_syscall(SYS_SHMAT).map(|d| d.name), Some("shmat"));
        assert_eq!(lookup_syscall(SYS_SHMDT).map(|d| d.name), Some("shmdt"));
        assert_eq!(lookup_syscall(SYS_SHMCTL).map(|d| d.name), Some("shmctl"));
        assert_eq!(lookup_syscall(SYS_MKDIRAT).map(|d| d.name), Some("mkdirat"));
        assert_eq!(lookup_syscall(SYS_FUTEX).map(|d| d.name), Some("futex"));
        assert_eq!(
            lookup_syscall(SYS_NEWFSTATAT).map(|d| d.name),
            Some("newfstatat")
        );
        assert_eq!(
            lookup_syscall(SYS_GETDENTS64).map(|d| d.name),
            Some("getdents64")
        );
        assert_eq!(
            lookup_syscall(SYS_UNLINKAT).map(|d| d.name),
            Some("unlinkat")
        );
        assert_eq!(
            lookup_syscall(SYS_RENAMEAT).map(|d| d.name),
            Some("renameat")
        );
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
