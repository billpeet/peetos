use core::cmp;
use core::mem::{size_of, MaybeUninit};
use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering};

use arch_x86_64::addr::{PhysAddr, VirtAddr, KERNEL_VIRT_BASE, PAGE_SIZE};
use arch_x86_64::gdt;
use arch_x86_64::idt::InterruptFrame;
use arch_x86_64::pit;
use arch_x86_64::syscall::{self as arch_syscall, SyscallFrame};

use crate::framebuffer;
use crate::ipc;
use crate::memory::{heap, pmm, vmm};
use crate::shm;
use crate::signal;
use crate::socket;
use crate::task::{self, TaskState, TASK_FLAG_USER};
use crate::tty;
use crate::user;

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
const EINTR: i64 = 4;
const EIO: i64 = 5;
const ENXIO: i64 = 6;
const E2BIG: i64 = 7;
const EBADF: i64 = 9;
const EAGAIN: i64 = 11;
const ENOMEM: i64 = 12;
const EACCES: i64 = 13;
const EFAULT: i64 = 14;
const EBUSY: i64 = 16;
const EEXIST: i64 = 17;
const EXDEV: i64 = 18;
const ECHILD: i64 = 10;
const EINVAL: i64 = 22;
const ENOTTY: i64 = 25;
const EMFILE: i64 = 24;
const ESPIPE: i64 = 29;
const EROFS: i64 = 30;
const EPIPE: i64 = 32;
const ERANGE: i64 = 34;
const ENOTDIR: i64 = 20;
const EISDIR: i64 = 21;
const ENFILE: i64 = 23;
const ENOTEMPTY: i64 = 39;
const ENOSYS: i64 = 38;
const ENOSPC: i64 = 28;
const ENOTSOCK: i64 = 88;
const EPROTONOSUPPORT: i64 = 93;
const EOPNOTSUPP: i64 = 95;
const EAFNOSUPPORT: i64 = 97;
const EADDRINUSE: i64 = 98;
const ENOTCONN: i64 = 107;
const ETIMEDOUT: i64 = 110;
const ECONNREFUSED: i64 = 111;
const EINPROGRESS: i64 = 115;

const SYS_READ: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_OPEN: u64 = 2;
const SYS_CLOSE: u64 = 3;
const SYS_STAT: u64 = 4;
const SYS_FSTAT: u64 = 5;
const SYS_LSTAT: u64 = 6;
const SYS_LSEEK: u64 = 8;
const SYS_MMAP: u64 = 9;
const SYS_MPROTECT: u64 = 10;
const SYS_MUNMAP: u64 = 11;
const SYS_RT_SIGACTION: u64 = 13;
const SYS_RT_SIGPROCMASK: u64 = 14;
const SYS_RT_SIGRETURN: u64 = 15;
const SYS_IOCTL: u64 = 16;
const SYS_MSYNC: u64 = 26;
const SYS_WRITEV: u64 = 20;
const SYS_ACCESS: u64 = 21;
const SYS_SELECT: u64 = 23;
const SYS_DUP: u64 = 32;
const SYS_DUP2: u64 = 33;
const SYS_NANOSLEEP: u64 = 35;
const SYS_CLONE: u64 = 56;
const SYS_FORK: u64 = 57;
const SYS_VFORK: u64 = 58;
const SYS_BRK: u64 = 12;
const SYS_WAIT4: u64 = 61;
const SYS_UNAME: u64 = 63;
const SYS_PIPE: u64 = 22;
const SYS_YIELD: u64 = 24;
const SYS_SHMGET: u64 = 29;
const SYS_SHMAT: u64 = 30;
const SYS_SHMCTL: u64 = 31;
const SYS_FCNTL: u64 = 72;
const SYS_FSYNC: u64 = 74;
const SYS_GETCWD: u64 = 79;
const SYS_CHDIR: u64 = 80;
const SYS_FCHDIR: u64 = 81;
const SYS_RENAME: u64 = 82;
const SYS_LINK: u64 = 86;
const SYS_UNLINK: u64 = 87;
const SYS_READLINK: u64 = 89;
const SYS_UMASK: u64 = 95;
const SYS_GETTIMEOFDAY: u64 = 96;
const SYS_GETUID: u64 = 102;
const SYS_GETGID: u64 = 104;
const SYS_GETEUID: u64 = 107;
const SYS_GETEGID: u64 = 108;
const SYS_SETPGID: u64 = 109;
const SYS_GETPID: u64 = 39;
const SYS_EXECVE: u64 = 59;
const SYS_EXIT: u64 = 60;
const SYS_KILL: u64 = 62;
const SYS_SHMDT: u64 = 67;
const SYS_GETPPID: u64 = 110;
const SYS_SETSID: u64 = 112;
const SYS_GETSID: u64 = 124;
const SYS_ARCH_PRCTL: u64 = 158;
const SYS_GETTID: u64 = 186;
const SYS_FUTEX: u64 = 202;
const SYS_SET_TID_ADDRESS: u64 = 218;
const SYS_GETDENTS64: u64 = 217;
const SYS_EXIT_GROUP: u64 = 231;
const SYS_UTIMES: u64 = 235;
const SYS_CLOCK_GETTIME: u64 = 228;
const SYS_OPENAT: u64 = 257;
const SYS_MKDIRAT: u64 = 258;
const SYS_FCHOWNAT: u64 = 260;
const SYS_FUTIMESAT: u64 = 261;
const SYS_NEWFSTATAT: u64 = 262;
const SYS_UNLINKAT: u64 = 263;
const SYS_RENAMEAT: u64 = 264;
const SYS_LINKAT: u64 = 265;
const SYS_SYMLINKAT: u64 = 266;
const SYS_READLINKAT: u64 = 267;
const SYS_FCHMODAT: u64 = 268;
const SYS_FACCESSAT: u64 = 269;
const SYS_MKNODAT: u64 = 259;
const SYS_UTIMENSAT: u64 = 280;
const SYS_DUP3: u64 = 292;
const SYS_PIPE2: u64 = 293;
const SYS_FTRUNCATE: u64 = 77;
const SYS_GETRUSAGE: u64 = 98;
const SYS_SYSINFO: u64 = 99;
const SYS_STATFS: u64 = 137;
const SYS_FSTATFS: u64 = 138;
const SYS_PRLIMIT64: u64 = 302;
const SYS_GETRANDOM: u64 = 318;
const SYS_MEMFD_CREATE: u64 = 319;
const SYS_EPOLL_WAIT: u64 = 232;
const SYS_EPOLL_PWAIT: u64 = 281;
const SYS_EPOLL_CTL: u64 = 233;
const SYS_EPOLL_CREATE1: u64 = 291;
const SYS_PREAD64: u64 = 17;
const SYS_PWRITE64: u64 = 18;
const SYS_READV: u64 = 19;
const SYS_TRUNCATE: u64 = 76;
const SYS_FCHMOD: u64 = 91;
const SYS_FCHOWN: u64 = 93;
const SYS_GETPGID: u64 = 121;
const SYS_SYNC: u64 = 162;
const SYS_SCHED_GETAFFINITY: u64 = 204;
const SYS_COPY_FILE_RANGE: u64 = 326;
const SYS_FACCESSAT2: u64 = 439;

// Poll syscall numbers
const SYS_POLL: u64 = 7;
const SYS_PPOLL: u64 = 271;

// Poll event bits (Linux ABI)
const POLLIN: i16 = 0x0001;
const POLLPRI: i16 = 0x0002;
const POLLOUT: i16 = 0x0004;
const POLLERR: i16 = 0x0008;
const POLLHUP: i16 = 0x0010;
const POLLNVAL: i16 = 0x0020;

// Socket syscall numbers
const SYS_SOCKET: u64 = 41;
const SYS_CONNECT: u64 = 42;
const SYS_ACCEPT: u64 = 43;
const SYS_SENDTO: u64 = 44;
const SYS_RECVFROM: u64 = 45;
const SYS_SENDMSG: u64 = 46;
const SYS_RECVMSG: u64 = 47;
const SYS_SHUTDOWN: u64 = 48;
const SYS_BIND: u64 = 49;
const SYS_LISTEN: u64 = 50;
const SYS_GETSOCKNAME: u64 = 51;
const SYS_GETPEERNAME: u64 = 52;
const SYS_SOCKETPAIR: u64 = 53;
const SYS_SETSOCKOPT: u64 = 54;
const SYS_GETSOCKOPT: u64 = 55;
const SYS_ACCEPT4: u64 = 288;

const AT_REMOVEDIR: u32 = 0x200;
const AT_EMPTY_PATH: u32 = 0x1000;
const AT_EACCESS: u32 = 0x200;
const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
const AT_SYMLINK_FOLLOW: u32 = 0x400;

const F_OK: u32 = 0;
const X_OK: u32 = 1;
const W_OK: u32 = 2;
const R_OK: u32 = 4;

const STAT_S_IRUSR: u32 = 0o400;
const STAT_S_IWUSR: u32 = 0o200;
const STAT_S_IXUSR: u32 = 0o100;
const STAT_S_IRGRP: u32 = 0o040;
const STAT_S_IWGRP: u32 = 0o020;
const STAT_S_IXGRP: u32 = 0o010;
const STAT_S_IROTH: u32 = 0o004;
const STAT_S_IWOTH: u32 = 0o002;
const STAT_S_IXOTH: u32 = 0o001;

const CLOCK_REALTIME: i32 = 0;
const CLOCK_MONOTONIC: i32 = 1;
const WNOHANG: u32 = 0x1;
const SIGCHLD_EXIT_SIGNAL: u64 = 17;
const UTIME_NOW: i64 = 0x3fff_ffff;
const UTIME_OMIT: i64 = 0x3fff_fffe;

const CLONE_CSIGNAL: u64 = 0x0000_00FF;
const CLONE_VM: u64 = 0x0000_0100;
const CLONE_VFORK: u64 = 0x0000_4000;

const TIOCGWINSZ: u64 = 0x5413;
const TIOCSWINSZ: u64 = 0x5414;
const TIOCGPGRP: u64 = 0x540F;
const TIOCSPGRP: u64 = 0x5410;
const TIOCSCTTY: u64 = 0x540E;
const TIOCNOTTY: u64 = 0x5422;
const TIOCSPTLCK: u64 = 0x40045431;
const TIOCGPTN: u64 = 0x80045430;
const TCGETS: u64 = 0x5401;
const TCSETS: u64 = 0x5402;
const TCSETSW: u64 = 0x5403;
const TCSETSF: u64 = 0x5404;
const FBIOGETINFO: u64 = 0x5001;

const ARCH_SET_GS: u64 = 0x1001;
const ARCH_SET_FS: u64 = 0x1002;
const ARCH_GET_FS: u64 = 0x1003;
const ARCH_GET_GS: u64 = 0x1004;

const PROT_NONE: u32 = 0x0;
const PROT_READ: u32 = 0x1;
const PROT_WRITE: u32 = 0x2;
const PROT_EXEC: u32 = 0x4;

const MAP_SHARED: u32 = 0x01;
const MAP_PRIVATE: u32 = 0x02;
const MAP_FIXED: u32 = 0x10;
const MAP_ANONYMOUS: u32 = 0x20;
const MAP_DENYWRITE: u32 = 0x0800;
const MAP_EXECUTABLE: u32 = 0x1000;
const MAP_FIXED_NOREPLACE: u32 = 0x100000;
const MS_ASYNC: u32 = 0x1;
const MS_INVALIDATE: u32 = 0x2;
const MS_SYNC: u32 = 0x4;
const O_CLOEXEC: u32 = 0x80000;
const O_NONBLOCK: u32 = 0x800;
const MFD_CLOEXEC: u32 = 0x0001;
const MFD_ALLOW_SEALING: u32 = 0x0002;

// Socket constants
const AF_UNIX: u32 = 1;
const AF_INET: u32 = 2;
const SOCK_TYPE_MASK: u32 = 0x0f;
const SOCK_STREAM: u32 = 1;
const SOCK_DGRAM: u32 = 2;
const SOCK_CLOEXEC: u32 = 0x80000;
const SOCK_NONBLOCK: u32 = 0x800;
const SOCKADDR_IN_SIZE: usize = 16;
const SOCKADDR_UN_FAMILY_SIZE: usize = 2;
const SOCKADDR_UN_PATH_MAX: usize = 108;
const SOCKADDR_UN_SIZE: usize = SOCKADDR_UN_FAMILY_SIZE + SOCKADDR_UN_PATH_MAX;
const MSG_DONTWAIT: u32 = 0x40;
const MSG_CTRUNC: u32 = 0x08;
const MSG_CMSG_CLOEXEC: u32 = 0x40000000;
const SCM_RIGHTS: u32 = 1;
const EPOLL_CLOEXEC: u32 = O_CLOEXEC;
const EPOLLIN: u32 = 0x0000_0001;
const EPOLLPRI: u32 = 0x0000_0002;
const EPOLLOUT: u32 = 0x0000_0004;
const EPOLLERR: u32 = 0x0000_0008;
const EPOLLHUP: u32 = 0x0000_0010;
const EPOLL_CTL_ADD: i32 = 1;
const EPOLL_CTL_DEL: i32 = 2;
const EPOLL_CTL_MOD: i32 = 3;

// Socket option levels/names
const SOL_SOCKET: u32 = 1;
const SO_ERROR: u32 = 4;
const SO_REUSEADDR: u32 = 2;
const SO_KEEPALIVE: u32 = 9;
const IPPROTO_ICMP: u32 = 1;
const IPPROTO_IP: u32 = 0;
const IPPROTO_TCP: u32 = 6;
const IPPROTO_UDP: u32 = 17;
const TCP_NODELAY: u32 = 1;

const F_DUPFD: u32 = 0;
const F_GETFD: u32 = 1;
const F_SETFD: u32 = 2;
const F_GETFL: u32 = 3;
const F_SETFL: u32 = 4;
const F_DUPFD_CLOEXEC: u32 = 1030;
const FD_CLOEXEC: u32 = 1;
const FD_INTERNAL_CLOEXEC: u32 = 1 << 31;
const FD_INTERNAL_MEMFD: u32 = 1 << 30;
const FD_INTERNAL_MASK: u32 = FD_INTERNAL_CLOEXEC | FD_INTERNAL_MEMFD;
const USER_CANON_MAX: u64 = 0x0000_7FFF_FFFF_FFFF;

const MMAP_BASE_ADDR: u64 = 0x0000_0001_0000_0000;
const MMAP_BRK_GAP: u64 = 0x0020_0000;
const MMAP_STACK_GUARD: u64 = 0x0020_0000;

const WRITE_CHUNK_SIZE: usize = 128;
const READ_CHUNK_SIZE: usize = 128;
const WRITE_IOV_MAX: usize = 1024;
const UNKNOWN_SYSCALL_LOG_LIMIT: u32 = 8;
const CLEAR_TID_SIZE: usize = 4;
const EXEC_MAX_ARGV: usize = 64;
const EXEC_MAX_ENVP: usize = 64;
const EXEC_MAX_STR_LEN: usize = 256;
const EXEC_MAX_IMAGE_SIZE: usize = 4 * 1024 * 1024;
const GETCWD_MAX_DEPTH: usize = 32;
const SIGNAL_FRAME_MAGIC_SYSCALL: u64 = 0x5352_4653_5953_0001;
const SIGNAL_FRAME_MAGIC_INT80: u64 = 0x5352_4649_3830_0001;
const UTS_FIELD_LEN: usize = 65;
const MMAP_MAX_REGIONS_PER_TASK: usize = 32;
const MMAP_MAX_PAGES_PER_TASK: u64 = 8192;
const VM_PAGE_CACHE_MAX_PAGES: usize = 1024;
const USER_VM_RECLAIM_TARGET_PAGES: usize = 16;
const MEMFD_MAX_BUFFERS_PER_TASK: usize = 16;
const MEMFD_NAME_MAX: usize = 249;
const MEMFD_CREATE_MAX_RETRIES: usize = 64;
const MEMFD_STORAGE_DIR: &str = "/temp/.memfd";
const MEMFD_PATH_PREFIX: &[u8] = b"/temp/.memfd/mfd-";
const EPOLL_MAX_INSTANCES: usize = 32;
const EPOLL_MAX_WATCHES: usize = 64;
const EPOLL_MAX_WATCHES_PER_TASK: usize = 64;
const EPOLL_EVENT_SIZE: usize = 12;
const EPOLL_SUPPORTED_EVENTS_MASK: u32 = EPOLLIN | EPOLLPRI | EPOLLOUT | EPOLLERR | EPOLLHUP;

const PAGE_FAULT_ERROR_PRESENT: u64 = 1 << 0;
const PAGE_FAULT_ERROR_WRITE: u64 = 1 << 1;
const PAGE_FAULT_ERROR_USER: u64 = 1 << 2;
const PAGE_FAULT_ERROR_INSTR: u64 = 1 << 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct VmMapping {
    start: u64,
    len: u64,
    file_offset: u64,
    file_inode: u16,
    prot: u32,
    map_shared: bool,
    map_shared_can_write: bool,
}

impl VmMapping {
    const EMPTY: Self = Self {
        start: 0,
        len: 0,
        file_offset: 0,
        file_inode: task::USER_FD_NONE,
        prot: 0,
        map_shared: false,
        map_shared_can_write: false,
    };

    #[inline]
    fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    fn end(&self) -> u64 {
        self.start.saturating_add(self.len)
    }
}

static mut UNKNOWN_SYSCALL_LOG_COUNT: u32 = 0;
static PROCESS_UMASK: AtomicU32 = AtomicU32::new(0o022);
static MEMFD_COUNTER: AtomicU32 = AtomicU32::new(1);
static mut TASK_VM_OWNER_PID: [i32; task::MAX_TASKS] = [task::INVALID_PID; task::MAX_TASKS];
static mut TASK_VM_MAPPINGS: [[VmMapping; MMAP_MAX_REGIONS_PER_TASK]; task::MAX_TASKS] =
    [[VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK]; task::MAX_TASKS];
#[derive(Clone, Copy)]
struct VmPageCacheEntry {
    inode: u16,
    page_off: u64,
    page_phys: u64,
    loaded_len: u16,
    valid: bool,
    referenced: bool,
    mapped_count: u16,
    dirty: bool,
}

impl VmPageCacheEntry {
    const EMPTY: Self = Self {
        inode: task::USER_FD_NONE,
        page_off: 0,
        page_phys: 0,
        loaded_len: 0,
        valid: false,
        referenced: false,
        mapped_count: 0,
        dirty: false,
    };
}

static mut VM_PAGE_CACHE: [VmPageCacheEntry; VM_PAGE_CACHE_MAX_PAGES] =
    [VmPageCacheEntry::EMPTY; VM_PAGE_CACHE_MAX_PAGES];
static mut VM_PAGE_CACHE_CLOCK_HAND: usize = 0;

#[derive(Clone, Copy)]
struct EpollWatch {
    used: bool,
    owner_pid: i32,
    #[cfg(test)]
    owner_task: *mut task::Task,
    fd: i32,
    kind: u8,
    inode: u16,
    events: u32,
    data: u64,
}

impl EpollWatch {
    const EMPTY: Self = Self {
        used: false,
        owner_pid: task::INVALID_PID,
        #[cfg(test)]
        owner_task: ptr::null_mut(),
        fd: -1,
        kind: task::USER_FD_KIND_EMPTY,
        inode: task::USER_FD_NONE,
        events: 0,
        data: 0,
    };
}

#[derive(Clone, Copy)]
struct EpollInstance {
    used: bool,
    refs: u16,
    watch_count: u16,
    watches: [EpollWatch; EPOLL_MAX_WATCHES],
}

impl EpollInstance {
    const EMPTY: Self = Self {
        used: false,
        refs: 0,
        watch_count: 0,
        watches: [EpollWatch::EMPTY; EPOLL_MAX_WATCHES],
    };

    fn reset(&mut self) {
        self.used = false;
        self.refs = 0;
        self.watch_count = 0;
        self.watches = [EpollWatch::EMPTY; EPOLL_MAX_WATCHES];
    }
}

static mut EPOLL_INSTANCES: [EpollInstance; EPOLL_MAX_INSTANCES] =
    [EpollInstance::EMPTY; EPOLL_MAX_INSTANCES];

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
    syscall_def(SYS_STAT, "stat", SyscallMode::UserTask, sys_stat),
    syscall_def(SYS_FSTAT, "fstat", SyscallMode::UserTask, sys_fstat),
    syscall_def(SYS_LSTAT, "lstat", SyscallMode::UserTask, sys_lstat),
    syscall_def(SYS_LSEEK, "lseek", SyscallMode::UserTask, sys_lseek),
    syscall_def(SYS_MMAP, "mmap", SyscallMode::UserTask, sys_mmap),
    syscall_def(
        SYS_MPROTECT,
        "mprotect",
        SyscallMode::UserTask,
        sys_mprotect,
    ),
    syscall_def(SYS_MUNMAP, "munmap", SyscallMode::UserTask, sys_munmap),
    syscall_def(SYS_MSYNC, "msync", SyscallMode::UserTask, sys_msync),
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
    syscall_def(SYS_IOCTL, "ioctl", SyscallMode::UserTask, sys_ioctl),
    syscall_def(SYS_WRITEV, "writev", SyscallMode::UserTask, sys_writev),
    syscall_def(SYS_ACCESS, "access", SyscallMode::UserTask, sys_access),
    syscall_def(SYS_DUP, "dup", SyscallMode::UserTask, sys_dup),
    syscall_def(SYS_DUP2, "dup2", SyscallMode::UserTask, sys_dup2),
    syscall_def(
        SYS_NANOSLEEP,
        "nanosleep",
        SyscallMode::UserTask,
        sys_nanosleep,
    ),
    syscall_def(SYS_PIPE, "pipe", SyscallMode::UserTask, sys_pipe),
    syscall_def(SYS_CLONE, "clone", SyscallMode::UserTask, sys_clone),
    syscall_def(SYS_WAIT4, "wait4", SyscallMode::UserTask, sys_wait4),
    syscall_def(SYS_UNAME, "uname", SyscallMode::UserTask, sys_uname),
    syscall_def(SYS_SHMGET, "shmget", SyscallMode::UserTask, sys_shmget),
    syscall_def(SYS_SHMAT, "shmat", SyscallMode::UserTask, sys_shmat),
    syscall_def(SYS_SHMCTL, "shmctl", SyscallMode::UserTask, sys_shmctl),
    syscall_def(SYS_BRK, "brk", SyscallMode::UserTask, sys_brk),
    syscall_def(SYS_YIELD, "sched_yield", SyscallMode::AnyTask, sys_yield),
    syscall_def(SYS_FORK, "fork", SyscallMode::UserTask, sys_fork),
    syscall_def(SYS_VFORK, "vfork", SyscallMode::UserTask, sys_vfork),
    syscall_def(SYS_GETCWD, "getcwd", SyscallMode::UserTask, sys_getcwd),
    syscall_def(SYS_CHDIR, "chdir", SyscallMode::UserTask, sys_chdir),
    syscall_def(SYS_FCHDIR, "fchdir", SyscallMode::UserTask, sys_fchdir),
    syscall_def(SYS_RENAME, "rename", SyscallMode::UserTask, sys_rename),
    syscall_def(SYS_LINK, "link", SyscallMode::UserTask, sys_link),
    syscall_def(
        SYS_READLINK,
        "readlink",
        SyscallMode::UserTask,
        sys_readlink,
    ),
    syscall_def(
        SYS_GETTIMEOFDAY,
        "gettimeofday",
        SyscallMode::UserTask,
        sys_gettimeofday,
    ),
    syscall_def(SYS_GETUID, "getuid", SyscallMode::AnyTask, sys_getuid),
    syscall_def(SYS_GETGID, "getgid", SyscallMode::AnyTask, sys_getgid),
    syscall_def(SYS_GETEUID, "geteuid", SyscallMode::AnyTask, sys_geteuid),
    syscall_def(SYS_GETEGID, "getegid", SyscallMode::AnyTask, sys_getegid),
    syscall_def(SYS_GETPID, "getpid", SyscallMode::AnyTask, sys_getpid),
    syscall_def(SYS_SETPGID, "setpgid", SyscallMode::UserTask, sys_setpgid),
    syscall_def(SYS_EXECVE, "execve", SyscallMode::UserTask, sys_execve),
    syscall_def(SYS_KILL, "kill", SyscallMode::UserTask, sys_kill),
    syscall_def(SYS_SHMDT, "shmdt", SyscallMode::UserTask, sys_shmdt),
    syscall_def(SYS_UMASK, "umask", SyscallMode::UserTask, sys_umask),
    syscall_def(SYS_GETPPID, "getppid", SyscallMode::AnyTask, sys_getppid),
    syscall_def(SYS_SETSID, "setsid", SyscallMode::UserTask, sys_setsid),
    syscall_def(SYS_GETSID, "getsid", SyscallMode::UserTask, sys_getsid),
    syscall_def(SYS_FCNTL, "fcntl", SyscallMode::UserTask, sys_fcntl),
    syscall_def(SYS_FSYNC, "fsync", SyscallMode::UserTask, sys_fsync),
    syscall_def(SYS_UNLINK, "unlink", SyscallMode::UserTask, sys_unlink),
    syscall_def(
        SYS_ARCH_PRCTL,
        "arch_prctl",
        SyscallMode::UserTask,
        sys_arch_prctl,
    ),
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
    syscall_def(SYS_UTIMES, "utimes", SyscallMode::UserTask, sys_utimes),
    syscall_def(
        SYS_GETDENTS64,
        "getdents64",
        SyscallMode::UserTask,
        sys_getdents64,
    ),
    syscall_def(
        SYS_CLOCK_GETTIME,
        "clock_gettime",
        SyscallMode::UserTask,
        sys_clock_gettime,
    ),
    syscall_def(SYS_FUTEX, "futex", SyscallMode::UserTask, sys_futex),
    syscall_def(SYS_OPENAT, "openat", SyscallMode::UserTask, sys_openat),
    syscall_def(SYS_MKDIRAT, "mkdirat", SyscallMode::UserTask, sys_mkdirat),
    syscall_def(SYS_MKNODAT, "mknodat", SyscallMode::UserTask, sys_mknodat),
    syscall_def(
        SYS_FCHOWNAT,
        "fchownat",
        SyscallMode::UserTask,
        sys_fchownat,
    ),
    syscall_def(
        SYS_NEWFSTATAT,
        "newfstatat",
        SyscallMode::UserTask,
        sys_newfstatat,
    ),
    syscall_def(
        SYS_FUTIMESAT,
        "futimesat",
        SyscallMode::UserTask,
        sys_futimesat,
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
    syscall_def(SYS_LINKAT, "linkat", SyscallMode::UserTask, sys_linkat),
    syscall_def(
        SYS_SYMLINKAT,
        "symlinkat",
        SyscallMode::UserTask,
        sys_symlinkat,
    ),
    syscall_def(
        SYS_READLINKAT,
        "readlinkat",
        SyscallMode::UserTask,
        sys_readlinkat,
    ),
    syscall_def(
        SYS_FCHMODAT,
        "fchmodat",
        SyscallMode::UserTask,
        sys_fchmodat,
    ),
    syscall_def(
        SYS_FACCESSAT,
        "faccessat",
        SyscallMode::UserTask,
        sys_faccessat,
    ),
    syscall_def(
        SYS_UTIMENSAT,
        "utimensat",
        SyscallMode::UserTask,
        sys_utimensat,
    ),
    syscall_def(SYS_DUP3, "dup3", SyscallMode::UserTask, sys_dup3),
    syscall_def(SYS_PIPE2, "pipe2", SyscallMode::UserTask, sys_pipe2),
    syscall_def(
        SYS_COPY_FILE_RANGE,
        "copy_file_range",
        SyscallMode::UserTask,
        sys_copy_file_range,
    ),
    syscall_def(
        SYS_FACCESSAT2,
        "faccessat2",
        SyscallMode::UserTask,
        sys_faccessat2,
    ),
    syscall_def(
        SYS_FTRUNCATE,
        "ftruncate",
        SyscallMode::UserTask,
        sys_ftruncate,
    ),
    syscall_def(
        SYS_GETRANDOM,
        "getrandom",
        SyscallMode::UserTask,
        sys_getrandom,
    ),
    syscall_def(
        SYS_MEMFD_CREATE,
        "memfd_create",
        SyscallMode::UserTask,
        sys_memfd_create,
    ),
    syscall_def(SYS_SYSINFO, "sysinfo", SyscallMode::UserTask, sys_sysinfo),
    syscall_def(
        SYS_PRLIMIT64,
        "prlimit64",
        SyscallMode::UserTask,
        sys_prlimit64,
    ),
    syscall_def(
        SYS_GETRUSAGE,
        "getrusage",
        SyscallMode::UserTask,
        sys_getrusage,
    ),
    syscall_def(SYS_STATFS, "statfs", SyscallMode::UserTask, sys_statfs),
    syscall_def(SYS_FSTATFS, "fstatfs", SyscallMode::UserTask, sys_fstatfs),
    syscall_def(SYS_FCHMOD, "fchmod", SyscallMode::UserTask, sys_fchmod),
    syscall_def(SYS_FCHOWN, "fchown", SyscallMode::UserTask, sys_fchown),
    syscall_def(
        SYS_TRUNCATE,
        "truncate",
        SyscallMode::UserTask,
        sys_truncate,
    ),
    syscall_def(SYS_SYNC, "sync", SyscallMode::AnyTask, sys_sync),
    syscall_def(SYS_PREAD64, "pread64", SyscallMode::UserTask, sys_pread64),
    syscall_def(
        SYS_PWRITE64,
        "pwrite64",
        SyscallMode::UserTask,
        sys_pwrite64,
    ),
    syscall_def(SYS_READV, "readv", SyscallMode::UserTask, sys_readv),
    syscall_def(
        SYS_SCHED_GETAFFINITY,
        "sched_getaffinity",
        SyscallMode::UserTask,
        sys_sched_getaffinity,
    ),
    syscall_def(SYS_GETPGID, "getpgid", SyscallMode::UserTask, sys_getpgid),
    // Poll/select syscalls
    syscall_def(SYS_SELECT, "select", SyscallMode::UserTask, sys_select),
    syscall_def(SYS_POLL, "poll", SyscallMode::UserTask, sys_poll),
    syscall_def(SYS_PPOLL, "ppoll", SyscallMode::UserTask, sys_ppoll),
    syscall_def(
        SYS_EPOLL_CREATE1,
        "epoll_create1",
        SyscallMode::UserTask,
        sys_epoll_create1,
    ),
    syscall_def(
        SYS_EPOLL_CTL,
        "epoll_ctl",
        SyscallMode::UserTask,
        sys_epoll_ctl,
    ),
    syscall_def(
        SYS_EPOLL_WAIT,
        "epoll_wait",
        SyscallMode::UserTask,
        sys_epoll_wait,
    ),
    syscall_def(
        SYS_EPOLL_PWAIT,
        "epoll_pwait",
        SyscallMode::UserTask,
        sys_epoll_pwait,
    ),
    // Socket syscalls
    syscall_def(SYS_SOCKET, "socket", SyscallMode::UserTask, sys_socket),
    syscall_def(SYS_CONNECT, "connect", SyscallMode::UserTask, sys_connect),
    syscall_def(SYS_ACCEPT, "accept", SyscallMode::UserTask, sys_accept),
    syscall_def(SYS_SENDTO, "sendto", SyscallMode::UserTask, sys_sendto),
    syscall_def(
        SYS_RECVFROM,
        "recvfrom",
        SyscallMode::UserTask,
        sys_recvfrom,
    ),
    syscall_def(SYS_SENDMSG, "sendmsg", SyscallMode::UserTask, sys_sendmsg),
    syscall_def(SYS_RECVMSG, "recvmsg", SyscallMode::UserTask, sys_recvmsg),
    syscall_def(
        SYS_SHUTDOWN,
        "shutdown",
        SyscallMode::UserTask,
        sys_shutdown,
    ),
    syscall_def(SYS_BIND, "bind", SyscallMode::UserTask, sys_bind),
    syscall_def(SYS_LISTEN, "listen", SyscallMode::UserTask, sys_listen),
    syscall_def(
        SYS_GETSOCKNAME,
        "getsockname",
        SyscallMode::UserTask,
        sys_getsockname,
    ),
    syscall_def(
        SYS_GETPEERNAME,
        "getpeername",
        SyscallMode::UserTask,
        sys_getpeername,
    ),
    syscall_def(
        SYS_SOCKETPAIR,
        "socketpair",
        SyscallMode::UserTask,
        sys_socketpair,
    ),
    syscall_def(
        SYS_SETSOCKOPT,
        "setsockopt",
        SyscallMode::UserTask,
        sys_setsockopt,
    ),
    syscall_def(
        SYS_GETSOCKOPT,
        "getsockopt",
        SyscallMode::UserTask,
        sys_getsockopt,
    ),
    syscall_def(SYS_ACCEPT4, "accept4", SyscallMode::UserTask, sys_accept4),
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

#[inline]
fn sync_ext2_backing() {
    let _ = crate::vfs::sync_ext2_to_block();
}

#[inline]
fn flush_ext2_backing() {
    let _ = crate::vfs::flush_ext2_block_cache();
}

#[inline]
fn vm_page_cache_eligible_inode(inode: u16) -> bool {
    fs::inode_backend_is_ext2(inode)
}

#[inline]
fn mmap_mapping_is_framebuffer(region: &VmMapping) -> bool {
    region.file_inode != task::USER_FD_NONE && fs::inode_is_framebuffer(region.file_inode)
}

#[inline]
fn mmap_region_is_reclaimable(region: &VmMapping) -> bool {
    if region.file_inode == task::USER_FD_NONE {
        return false;
    }
    if mmap_mapping_is_framebuffer(region) {
        return false;
    }
    if region.map_shared {
        return true;
    }
    (region.prot & PROT_WRITE) == 0
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum UnmappedPageRelease {
    Freed,
    CacheBacked,
    Device,
}

fn release_unmapped_user_page(mapping: Option<VmMapping>, phys: PhysAddr) -> UnmappedPageRelease {
    if let Some(mapping) = mapping {
        if mmap_mapping_is_framebuffer(&mapping) {
            return UnmappedPageRelease::Device;
        }
        if mapping.map_shared
            && mapping.file_inode != task::USER_FD_NONE
            && vm_page_cache_release_mapped_phys(phys)
        {
            return UnmappedPageRelease::CacheBacked;
        }
    }

    pmm::free_page(phys);
    UnmappedPageRelease::Freed
}

fn vm_page_cache_reclaim_unmapped_pages(target_pages: usize) -> usize {
    if target_pages == 0 {
        return 0;
    }
    let mut reclaimed = 0usize;
    for idx in 0..VM_PAGE_CACHE_MAX_PAGES {
        if reclaimed >= target_pages {
            break;
        }
        let (valid, page_phys, mapped_count) = unsafe {
            let entry = VM_PAGE_CACHE[idx];
            (entry.valid, entry.page_phys, entry.mapped_count)
        };
        if !valid || mapped_count != 0 || page_phys == 0 {
            continue;
        }
        vm_page_cache_free_entry(idx);
        let now_empty = unsafe { !VM_PAGE_CACHE[idx].valid };
        if now_empty {
            reclaimed += 1;
        }
    }
    reclaimed
}

fn reclaim_user_vm_pages(target_pages: usize) -> usize {
    if target_pages == 0 {
        return 0;
    }

    let mut reclaimed = vm_page_cache_reclaim_unmapped_pages(target_pages);
    if reclaimed >= target_pages {
        return reclaimed;
    }

    for task_idx in 0..task::MAX_TASKS {
        let task_ptr = task::get_by_index(task_idx);
        if task_ptr.is_null() {
            continue;
        }
        let (is_user, pml4) = unsafe {
            (
                ((*task_ptr).flags & TASK_FLAG_USER) != 0,
                PhysAddr::new((*task_ptr).pml4_phys),
            )
        };
        if !is_user || pml4.as_u64() == 0 {
            continue;
        }

        let slot = match mmap_prepare_task_slot(task_ptr) {
            Some(slot) => slot,
            None => continue,
        };

        for region_idx in 0..MMAP_MAX_REGIONS_PER_TASK {
            let region = unsafe { TASK_VM_MAPPINGS[slot][region_idx] };
            if region.is_empty() || !mmap_region_is_reclaimable(&region) {
                continue;
            }

            let mut page = region.start;
            while page < region.end() {
                let virt = VirtAddr::new(page);
                let phys = match vmm::get_physical_in(pml4, virt) {
                    Some(phys) => PhysAddr::new(phys.as_u64() & !(PAGE_SIZE - 1)),
                    None => {
                        page = page.saturating_add(PAGE_SIZE);
                        continue;
                    }
                };

                if vmm::unmap_page_in(pml4, virt) == 0 {
                    if release_unmapped_user_page(Some(region), phys) == UnmappedPageRelease::Freed
                    {
                        reclaimed += 1;
                    }

                    if reclaimed >= target_pages {
                        return reclaimed;
                    }
                }
                page = page.saturating_add(PAGE_SIZE);
            }
        }
    }

    reclaimed += vm_page_cache_reclaim_unmapped_pages(target_pages.saturating_sub(reclaimed));
    reclaimed
}

fn alloc_user_vm_page_with_reclaim() -> PhysAddr {
    let page = pmm::alloc_page();
    if page.as_u64() != 0 {
        return page;
    }
    let _ = reclaim_user_vm_pages(USER_VM_RECLAIM_TARGET_PAGES);
    pmm::alloc_page()
}

fn vm_page_cache_free_entry(idx: usize) {
    let should_writeback = unsafe {
        let entry = VM_PAGE_CACHE[idx];
        entry.valid && entry.mapped_count == 0 && entry.dirty
    };
    if should_writeback {
        let _ = vm_page_cache_writeback_idx(idx);
    }
    unsafe {
        let entry = &mut VM_PAGE_CACHE[idx];
        if entry.valid && entry.mapped_count != 0 {
            return;
        }
        if entry.valid && entry.page_phys != 0 {
            pmm::free_page(PhysAddr::new(entry.page_phys));
        }
        *entry = VmPageCacheEntry::EMPTY;
    }
}

fn vm_page_cache_reset() {
    for idx in 0..VM_PAGE_CACHE_MAX_PAGES {
        vm_page_cache_free_entry(idx);
    }
    unsafe {
        VM_PAGE_CACHE_CLOCK_HAND = 0;
    }
}

fn vm_page_cache_lookup_idx(inode: u16, page_off: u64) -> Option<usize> {
    unsafe {
        for idx in 0..VM_PAGE_CACHE_MAX_PAGES {
            let entry = &mut VM_PAGE_CACHE[idx];
            if entry.valid && entry.inode == inode && entry.page_off == page_off {
                entry.referenced = true;
                return Some(idx);
            }
        }
    }
    None
}

fn vm_page_cache_acquire_slot() -> Result<usize, i64> {
    for idx in 0..VM_PAGE_CACHE_MAX_PAGES {
        unsafe {
            if !VM_PAGE_CACHE[idx].valid {
                return Ok(idx);
            }
        }
    }

    let mut scanned = 0usize;
    while scanned < VM_PAGE_CACHE_MAX_PAGES * 2 {
        let idx = unsafe {
            let idx = VM_PAGE_CACHE_CLOCK_HAND;
            VM_PAGE_CACHE_CLOCK_HAND = (VM_PAGE_CACHE_CLOCK_HAND + 1) % VM_PAGE_CACHE_MAX_PAGES;
            idx
        };
        unsafe {
            if VM_PAGE_CACHE[idx].referenced {
                VM_PAGE_CACHE[idx].referenced = false;
                scanned += 1;
                continue;
            }
            if VM_PAGE_CACHE[idx].mapped_count != 0 {
                scanned += 1;
                continue;
            }
        }
        vm_page_cache_writeback_idx(idx)?;
        vm_page_cache_free_entry(idx);
        return Ok(idx);
    }
    Err(-ENOMEM)
}

fn vm_page_cache_fill_entry(idx: usize, inode: u16, page_off: u64) -> Result<(), i64> {
    let page_phys = alloc_user_vm_page_with_reclaim();
    if page_phys.as_u64() == 0 {
        return Err(-ENOMEM);
    }

    let page_ptr = page_phys.to_virt().as_u64() as *mut u8;
    unsafe {
        ptr::write_bytes(page_ptr, 0, PAGE_SIZE as usize);
    }
    let page_buf = unsafe { core::slice::from_raw_parts_mut(page_ptr, PAGE_SIZE as usize) };

    let mut loaded = 0usize;
    while loaded < page_buf.len() {
        let read_n = fs::read(inode, page_off + loaded as u64, &mut page_buf[loaded..])
            .map_err(|err| -fs_to_errno(err))?;
        if read_n == 0 {
            break;
        }
        loaded += read_n;
    }

    unsafe {
        VM_PAGE_CACHE[idx] = VmPageCacheEntry {
            inode,
            page_off,
            page_phys: page_phys.as_u64(),
            loaded_len: loaded as u16,
            valid: true,
            referenced: true,
            mapped_count: 0,
            dirty: false,
        };
    }
    Ok(())
}

fn vm_page_cache_get_entry(inode: u16, page_off: u64) -> Result<VmPageCacheEntry, i64> {
    if let Some(idx) = vm_page_cache_lookup_idx(inode, page_off) {
        unsafe {
            return Ok(VM_PAGE_CACHE[idx]);
        }
    }

    let idx = vm_page_cache_acquire_slot()?;
    vm_page_cache_fill_entry(idx, inode, page_off)?;
    unsafe { Ok(VM_PAGE_CACHE[idx]) }
}

fn vm_page_cache_pin_mapped_page(
    inode: u16,
    page_off: u64,
    writable: bool,
) -> Result<VmPageCacheEntry, i64> {
    let idx = if let Some(idx) = vm_page_cache_lookup_idx(inode, page_off) {
        idx
    } else {
        let idx = vm_page_cache_acquire_slot()?;
        vm_page_cache_fill_entry(idx, inode, page_off)?;
        idx
    };

    unsafe {
        let entry = &mut VM_PAGE_CACHE[idx];
        entry.mapped_count = entry.mapped_count.saturating_add(1);
        entry.referenced = true;
        if writable {
            entry.dirty = true;
        }
        Ok(*entry)
    }
}

pub(crate) fn vm_page_cache_release_mapped_phys(phys: PhysAddr) -> bool {
    let page_phys = phys.as_u64() & !(PAGE_SIZE - 1);
    unsafe {
        for idx in 0..VM_PAGE_CACHE_MAX_PAGES {
            let entry = &mut VM_PAGE_CACHE[idx];
            if !entry.valid || entry.page_phys != page_phys {
                continue;
            }
            if entry.mapped_count != 0 {
                entry.mapped_count -= 1;
            }
            entry.referenced = true;
            return true;
        }
    }
    false
}

fn vm_page_cache_mark_dirty_by_phys(phys: PhysAddr) -> bool {
    let page_phys = phys.as_u64() & !(PAGE_SIZE - 1);
    unsafe {
        for idx in 0..VM_PAGE_CACHE_MAX_PAGES {
            let entry = &mut VM_PAGE_CACHE[idx];
            if !entry.valid || entry.page_phys != page_phys {
                continue;
            }
            entry.dirty = true;
            entry.referenced = true;
            return true;
        }
    }
    false
}

fn vm_page_cache_writeback_idx(idx: usize) -> Result<usize, i64> {
    let (inode, page_off, page_phys, loaded_len, valid, dirty) = unsafe {
        let entry = VM_PAGE_CACHE[idx];
        (
            entry.inode,
            entry.page_off,
            entry.page_phys,
            entry.loaded_len,
            entry.valid,
            entry.dirty,
        )
    };

    if !valid || !dirty {
        return Ok(0);
    }
    if loaded_len == 0 {
        unsafe {
            if VM_PAGE_CACHE[idx].valid {
                VM_PAGE_CACHE[idx].dirty = false;
            }
        }
        return Ok(0);
    }

    let total = loaded_len as usize;
    let page_ptr = PhysAddr::new(page_phys).to_virt().as_u64() as *const u8;
    let page = unsafe { core::slice::from_raw_parts(page_ptr, PAGE_SIZE as usize) };
    let mut written = 0usize;
    while written < total {
        let write_res = fs::write(
            inode,
            page_off.saturating_add(written as u64),
            &page[written..total],
            false,
        )
        .map_err(|err| -fs_to_errno(err))?;
        if write_res.written == 0 {
            return Err(-EIO);
        }
        written += write_res.written;
    }

    unsafe {
        let entry = &mut VM_PAGE_CACHE[idx];
        if entry.valid
            && entry.page_phys == page_phys
            && entry.inode == inode
            && entry.page_off == page_off
        {
            entry.dirty = false;
            if written > entry.loaded_len as usize {
                entry.loaded_len = written as u16;
            }
        }
    }
    Ok(written)
}

fn vm_page_cache_writeback_page(inode: u16, page_off: u64) -> Result<usize, i64> {
    unsafe {
        for idx in 0..VM_PAGE_CACHE_MAX_PAGES {
            let entry = VM_PAGE_CACHE[idx];
            if entry.valid && entry.inode == inode && entry.page_off == page_off {
                return vm_page_cache_writeback_idx(idx);
            }
        }
    }
    Ok(0)
}

fn vm_page_cache_writeback_inode(inode: u16) -> Result<usize, i64> {
    let mut written = 0usize;
    for idx in 0..VM_PAGE_CACHE_MAX_PAGES {
        let should_flush = unsafe { VM_PAGE_CACHE[idx].valid && VM_PAGE_CACHE[idx].inode == inode };
        if should_flush {
            written = written.saturating_add(vm_page_cache_writeback_idx(idx)?);
        }
    }
    Ok(written)
}

fn vm_page_cache_writeback_all() -> Result<usize, i64> {
    let mut written = 0usize;
    for idx in 0..VM_PAGE_CACHE_MAX_PAGES {
        written = written.saturating_add(vm_page_cache_writeback_idx(idx)?);
    }
    Ok(written)
}

fn vm_page_cache_read(inode: u16, offset: u64, dst: &mut [u8]) -> Result<usize, i64> {
    if dst.is_empty() {
        return Ok(0);
    }
    if !vm_page_cache_eligible_inode(inode) {
        refresh_procfs_runtime();
        return fs::read(inode, offset, dst).map_err(|err| -fs_to_errno(err));
    }

    let mut copied = 0usize;
    while copied < dst.len() {
        let file_off = offset.saturating_add(copied as u64);
        let page_off = align_down(file_off, PAGE_SIZE);
        let in_page = (file_off - page_off) as usize;
        let entry = vm_page_cache_get_entry(inode, page_off)?;
        let loaded = entry.loaded_len as usize;
        if in_page >= loaded {
            break;
        }

        let available = loaded - in_page;
        let take = cmp::min(available, dst.len() - copied);
        let page_ptr = PhysAddr::new(entry.page_phys).to_virt().as_u64() as *const u8;
        unsafe {
            ptr::copy_nonoverlapping(page_ptr.add(in_page), dst.as_mut_ptr().add(copied), take);
        }
        copied += take;
        if take < PAGE_SIZE as usize - in_page {
            break;
        }
    }
    Ok(copied)
}

fn vm_page_cache_invalidate_inode(inode: u16) {
    for idx in 0..VM_PAGE_CACHE_MAX_PAGES {
        unsafe {
            if VM_PAGE_CACHE[idx].valid && VM_PAGE_CACHE[idx].inode == inode {
                vm_page_cache_free_entry(idx);
            }
        }
    }
}

fn vm_page_cache_invalidate_inode_range(inode: u16, offset: u64, len: u64) {
    if len == 0 {
        return;
    }
    let start = align_down(offset, PAGE_SIZE);
    let end = match offset.checked_add(len) {
        Some(v) => page_align_up(v).unwrap_or(u64::MAX),
        None => u64::MAX,
    };

    for idx in 0..VM_PAGE_CACHE_MAX_PAGES {
        let should_invalidate = unsafe {
            let entry = VM_PAGE_CACHE[idx];
            if !entry.valid || entry.inode != inode {
                false
            } else {
                let page_end = entry.page_off.saturating_add(PAGE_SIZE);
                start < page_end && entry.page_off < end
            }
        };
        if should_invalidate {
            vm_page_cache_free_entry(idx);
        }
    }
}

fn read_vfs_bytes(inode: u16, offset: u64, dst: &mut [u8]) -> Result<usize, i64> {
    vm_page_cache_read(inode, offset, dst)
}

fn input_device_read(
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

#[inline]
fn fd_flags_from_open_flags(flags: u32) -> u32 {
    let mut fd_flags = flags & !O_CLOEXEC;
    if (flags & O_CLOEXEC) != 0 {
        fd_flags |= FD_INTERNAL_CLOEXEC;
    }
    fd_flags
}

#[inline]
fn fd_open_flags(flags: u32) -> u32 {
    flags & !FD_INTERNAL_MASK
}

#[inline]
fn fd_internal_flags(flags: u32) -> u32 {
    flags & FD_INTERNAL_MASK
}

#[inline]
fn fd_is_memfd(flags: u32) -> bool {
    (flags & FD_INTERNAL_MEMFD) != 0
}

#[inline]
fn fd_has_cloexec(flags: u32) -> bool {
    (flags & FD_INTERNAL_CLOEXEC) != 0
}

#[inline]
fn fd_set_cloexec(flags: u32, enabled: bool) -> u32 {
    if enabled {
        flags | FD_INTERNAL_CLOEXEC
    } else {
        flags & !FD_INTERNAL_CLOEXEC
    }
}

fn fd_find_free_from(task_ptr: *mut task::Task, start: usize) -> Option<usize> {
    if task_ptr.is_null() || start >= task::MAX_TASK_FDS {
        return None;
    }
    for idx in start..task::MAX_TASK_FDS {
        if unsafe { (*task_ptr).user_fd_kind[idx] == task::USER_FD_KIND_EMPTY } {
            return Some(idx);
        }
    }
    None
}

fn task_memfd_contains_inode(task_ptr: *mut task::Task, inode: u16) -> bool {
    if task_ptr.is_null() || inode == task::USER_FD_NONE {
        return false;
    }

    let mut idx = 0usize;
    while idx < task::MAX_TASK_FDS {
        let is_match = unsafe {
            fd_slot_is_open(task_ptr, idx)
                && fd_is_memfd((*task_ptr).user_fd_flags[idx])
                && (*task_ptr).user_fd_inode[idx] == inode
        };
        if is_match {
            return true;
        }
        idx += 1;
    }
    false
}

fn task_memfd_object_count(task_ptr: *mut task::Task) -> usize {
    if task_ptr.is_null() {
        return 0;
    }

    let mut count = 0usize;
    let mut seen = [task::USER_FD_NONE; task::MAX_TASK_FDS];
    let mut idx = 0usize;
    while idx < task::MAX_TASK_FDS {
        let (open, flags, inode) = unsafe {
            (
                fd_slot_is_open(task_ptr, idx),
                (*task_ptr).user_fd_flags[idx],
                (*task_ptr).user_fd_inode[idx],
            )
        };
        if open && fd_is_memfd(flags) && inode != task::USER_FD_NONE {
            let mut known = false;
            let mut seen_idx = 0usize;
            while seen_idx < count {
                if seen[seen_idx] == inode {
                    known = true;
                    break;
                }
                seen_idx += 1;
            }
            if !known && count < seen.len() {
                seen[count] = inode;
                count += 1;
            }
        }
        idx += 1;
    }
    count
}

fn enforce_memfd_object_limit_for_inode(task_ptr: *mut task::Task, inode: u16) -> Result<(), i64> {
    if task_ptr.is_null() {
        return Err(-ESRCH);
    }
    if inode == task::USER_FD_NONE {
        return Err(-EINVAL);
    }
    if task_memfd_contains_inode(task_ptr, inode) {
        return Ok(());
    }
    if task_memfd_object_count(task_ptr) >= MEMFD_MAX_BUFFERS_PER_TASK {
        return Err(-EMFILE);
    }
    Ok(())
}

fn enforce_memfd_create_limit(task_ptr: *mut task::Task) -> Result<(), i64> {
    if task_ptr.is_null() {
        return Err(-ESRCH);
    }
    if task_memfd_object_count(task_ptr) >= MEMFD_MAX_BUFFERS_PER_TASK {
        return Err(-EMFILE);
    }
    Ok(())
}

fn fd_write_slot(
    task_ptr: *mut task::Task,
    idx: usize,
    kind: u8,
    inode: u16,
    offset: u64,
    flags: u32,
) {
    unsafe {
        (*task_ptr).user_fd_kind[idx] = kind;
        (*task_ptr).user_fd_inode[idx] = if kind == task::USER_FD_KIND_VFS
            || kind == task::USER_FD_KIND_PIPE
            || kind == task::USER_FD_KIND_SOCKET
            || kind == task::USER_FD_KIND_PTY_MASTER
            || kind == task::USER_FD_KIND_PTY_SLAVE
            || kind == task::USER_FD_KIND_EPOLL
        {
            inode
        } else {
            task::USER_FD_NONE
        };
        (*task_ptr).user_fd_offset[idx] = offset;
        (*task_ptr).user_fd_flags[idx] = flags;
    }
}

fn fd_alloc(task_ptr: *mut task::Task, kind: u8, inode: u16, flags: u32) -> Result<i64, i64> {
    if task_ptr.is_null() {
        return Err(-ESRCH);
    }

    let idx = match fd_find_free_from(task_ptr, 3) {
        Some(idx) => idx,
        None => return Err(-EMFILE),
    };
    fd_write_slot(task_ptr, idx, kind, inode, 0, flags);
    Ok(idx as i64)
}

fn fd_dup_from(
    task_ptr: *mut task::Task,
    old_fd: i32,
    min_fd: usize,
    cloexec: bool,
) -> Result<i64, i64> {
    if task_ptr.is_null() {
        return Err(-ESRCH);
    }
    let src_idx = fd_index(old_fd)?;
    if !fd_slot_is_open(task_ptr, src_idx) {
        return Err(-EBADF);
    }

    let dst_idx = match fd_find_free_from(task_ptr, min_fd) {
        Some(idx) => idx,
        None => return Err(-EMFILE),
    };

    let (kind, inode, offset, src_flags) = unsafe {
        (
            (*task_ptr).user_fd_kind[src_idx],
            (*task_ptr).user_fd_inode[src_idx],
            (*task_ptr).user_fd_offset[src_idx],
            (*task_ptr).user_fd_flags[src_idx],
        )
    };
    if fd_is_memfd(src_flags) {
        enforce_memfd_object_limit_for_inode(task_ptr, inode)?;
    }
    let new_flags = fd_set_cloexec(src_flags, cloexec);
    fd_write_slot(task_ptr, dst_idx, kind, inode, offset, new_flags);
    if kind == task::USER_FD_KIND_PTY_MASTER && inode != task::USER_FD_NONE {
        crate::pty::master_retain(inode);
    }
    if kind == task::USER_FD_KIND_PTY_SLAVE && inode != task::USER_FD_NONE {
        crate::pty::slave_retain(inode);
    }
    if kind == task::USER_FD_KIND_SOCKET && inode != task::USER_FD_NONE {
        socket::socket_retain(inode)?;
    }
    if kind == task::USER_FD_KIND_EPOLL && inode != task::USER_FD_NONE {
        epoll_retain(inode)?;
    }
    Ok(dst_idx as i64)
}

fn fd_dup_to(
    task_ptr: *mut task::Task,
    old_fd: i32,
    new_fd: i32,
    cloexec: bool,
    reject_same: bool,
) -> Result<i64, i64> {
    if task_ptr.is_null() {
        return Err(-ESRCH);
    }
    let src_idx = fd_index(old_fd)?;
    let dst_idx = fd_index(new_fd)?;
    if !fd_slot_is_open(task_ptr, src_idx) {
        return Err(-EBADF);
    }

    if src_idx == dst_idx {
        if reject_same {
            return Err(-EINVAL);
        }
        return Ok(new_fd as i64);
    }

    if fd_slot_is_open(task_ptr, dst_idx) {
        fd_close(task_ptr, new_fd)?;
    }

    let (kind, inode, offset, src_flags) = unsafe {
        (
            (*task_ptr).user_fd_kind[src_idx],
            (*task_ptr).user_fd_inode[src_idx],
            (*task_ptr).user_fd_offset[src_idx],
            (*task_ptr).user_fd_flags[src_idx],
        )
    };
    if fd_is_memfd(src_flags) {
        enforce_memfd_object_limit_for_inode(task_ptr, inode)?;
    }
    let new_flags = fd_set_cloexec(src_flags, cloexec);
    fd_write_slot(task_ptr, dst_idx, kind, inode, offset, new_flags);
    if kind == task::USER_FD_KIND_PTY_MASTER && inode != task::USER_FD_NONE {
        crate::pty::master_retain(inode);
    }
    if kind == task::USER_FD_KIND_PTY_SLAVE && inode != task::USER_FD_NONE {
        crate::pty::slave_retain(inode);
    }
    if kind == task::USER_FD_KIND_SOCKET && inode != task::USER_FD_NONE {
        socket::socket_retain(inode)?;
    }
    if kind == task::USER_FD_KIND_EPOLL && inode != task::USER_FD_NONE {
        epoll_retain(inode)?;
    }
    Ok(new_fd as i64)
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
    let owner_pid = unsafe { (*task_ptr).pid };

    epoll_drop_watches_for_fd(owner_pid, fd, kind, inode);

    if kind == task::USER_FD_KIND_PIPE {
        let close_read = fd_can_read(flags);
        let close_write = fd_can_write(flags);
        if inode != task::USER_FD_NONE {
            let _ = ipc::pipe_close(inode, close_read, close_write);
        }
    } else if kind == task::USER_FD_KIND_SOCKET {
        if inode != task::USER_FD_NONE {
            let _ = socket::socket_close(inode);
        }
    } else if kind == task::USER_FD_KIND_PTY_MASTER {
        if inode != task::USER_FD_NONE {
            crate::pty::master_close(inode);
        }
    } else if kind == task::USER_FD_KIND_PTY_SLAVE {
        if inode != task::USER_FD_NONE {
            crate::pty::slave_close(inode);
        }
    } else if kind == task::USER_FD_KIND_EPOLL {
        if inode != task::USER_FD_NONE {
            let _ = epoll_close(inode);
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

fn close_cloexec_fds(task_ptr: *mut task::Task) {
    if task_ptr.is_null() {
        return;
    }

    let mut fd = 0usize;
    while fd < task::MAX_TASK_FDS {
        let should_close = unsafe {
            fd_slot_is_open(task_ptr, fd) && fd_has_cloexec((*task_ptr).user_fd_flags[fd])
        };
        if should_close {
            let _ = fd_close(task_ptr, fd as i32);
        }
        fd += 1;
    }
}

fn fd_access_mode(flags: u32) -> u32 {
    fd_open_flags(flags) & fs::O_ACCMODE
}

fn fd_can_read(flags: u32) -> bool {
    let mode = fd_access_mode(flags);
    mode == fs::O_RDONLY || mode == fs::O_RDWR
}

fn fd_can_write(flags: u32) -> bool {
    let mode = fd_access_mode(flags);
    mode == fs::O_WRONLY || mode == fs::O_RDWR
}

#[inline]
fn fcntl_setfl_mutable_mask(kind: u8) -> u32 {
    match kind {
        task::USER_FD_KIND_VFS => fs::O_APPEND | O_NONBLOCK,
        task::USER_FD_KIND_PIPE
        | task::USER_FD_KIND_SOCKET
        | task::USER_FD_KIND_PTY_MASTER
        | task::USER_FD_KIND_PTY_SLAVE => O_NONBLOCK,
        _ => 0,
    }
}

fn fd_apply_fcntl_setfl(current_flags: u32, kind: u8, requested_flags: u32) -> u32 {
    let current_open = fd_open_flags(current_flags);
    let preserved_access = current_open & fs::O_ACCMODE;
    let mutable = fcntl_setfl_mutable_mask(kind);
    let mut next_open = current_open & !mutable;
    next_open |= requested_flags & mutable;
    next_open = (next_open & !fs::O_ACCMODE) | preserved_access;
    fd_internal_flags(current_flags) | next_open
}

#[inline]
fn socket_type_base_and_open_flags(raw_type: u32) -> Result<(u32, u32), i64> {
    let allowed = SOCK_TYPE_MASK | SOCK_CLOEXEC | SOCK_NONBLOCK;
    if (raw_type & !allowed) != 0 {
        return Err(-EINVAL);
    }
    let base_type = raw_type & SOCK_TYPE_MASK;
    if base_type != SOCK_STREAM && base_type != SOCK_DGRAM {
        return Err(-EPROTONOSUPPORT);
    }
    let mut open_flags = fs::O_RDWR;
    if (raw_type & SOCK_NONBLOCK) != 0 {
        open_flags |= O_NONBLOCK;
    }
    if (raw_type & SOCK_CLOEXEC) != 0 {
        open_flags |= O_CLOEXEC;
    }
    Ok((base_type, open_flags))
}

#[inline]
fn accept4_open_flags(flags: u32) -> Result<u32, i64> {
    if (flags & !(SOCK_CLOEXEC | SOCK_NONBLOCK)) != 0 {
        return Err(-EINVAL);
    }
    let mut open_flags = fs::O_RDWR;
    if (flags & SOCK_NONBLOCK) != 0 {
        open_flags |= O_NONBLOCK;
    }
    if (flags & SOCK_CLOEXEC) != 0 {
        open_flags |= O_CLOEXEC;
    }
    Ok(open_flags)
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
struct LinuxUtsName {
    sysname: [u8; UTS_FIELD_LEN],
    nodename: [u8; UTS_FIELD_LEN],
    release: [u8; UTS_FIELD_LEN],
    version: [u8; UTS_FIELD_LEN],
    machine: [u8; UTS_FIELD_LEN],
    domainname: [u8; UTS_FIELD_LEN],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxIovec {
    iov_base: u64,
    iov_len: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxMsgHdr {
    msg_name: u64,
    msg_namelen: u32,
    _pad0: u32,
    msg_iov: u64,
    msg_iovlen: u64,
    msg_control: u64,
    msg_controllen: u64,
    msg_flags: u32,
    _pad1: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxCmsgHdr {
    cmsg_len: u64,
    cmsg_level: i32,
    cmsg_type: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxTimeVal {
    tv_sec: i64,
    tv_usec: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxTimeSpec {
    tv_sec: i64,
    tv_nsec: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxWinSize {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
}

use crate::tty::LinuxTermios;

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
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    rbp: u64,
    rbx: u64,
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

#[derive(Clone, Copy)]
struct ExecCString {
    len: usize,
    bytes: [u8; EXEC_MAX_STR_LEN],
}

impl ExecCString {
    const EMPTY: Self = Self {
        len: 0,
        bytes: [0; EXEC_MAX_STR_LEN],
    };

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
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

fn write_uts_field(dst: &mut [u8; UTS_FIELD_LEN], value: &[u8]) {
    let len = cmp::min(value.len(), UTS_FIELD_LEN - 1);
    dst[..len].copy_from_slice(&value[..len]);
    dst[len] = 0;
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

fn resolve_vfs_fd_inode(ctx: &SyscallContext, fd: i32) -> Result<u16, i64> {
    let idx = fd_index(fd)?;
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
        return Err(-EBADF);
    }
    Ok(inode)
}

fn timespec_to_unix_seconds(spec: LinuxTimeSpec, now: u64, current: u64) -> Result<u64, i64> {
    match spec.tv_nsec {
        UTIME_NOW => Ok(now),
        UTIME_OMIT => Ok(current),
        nsec => {
            if spec.tv_sec < 0 || nsec < 0 || nsec >= 1_000_000_000 {
                return Err(-EINVAL);
            }
            Ok(spec.tv_sec as u64)
        }
    }
}

#[inline]
fn access_mode_is_valid(mode: u32) -> bool {
    (mode & !(R_OK | W_OK | X_OK)) == 0
}

#[inline]
fn access_permission_bits_for(stat: &fs::FileStat, uid: u32, gid: u32) -> u32 {
    if uid == stat.uid {
        return (stat.mode & (STAT_S_IRUSR | STAT_S_IWUSR | STAT_S_IXUSR)) >> 6;
    }
    if gid == stat.gid {
        return (stat.mode & (STAT_S_IRGRP | STAT_S_IWGRP | STAT_S_IXGRP)) >> 3;
    }
    stat.mode & (STAT_S_IROTH | STAT_S_IWOTH | STAT_S_IXOTH)
}

fn access_allowed(stat: &fs::FileStat, mode: u32, uid: u32, gid: u32) -> bool {
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

fn lookup_inode_for_at_path(ctx: &SyscallContext, dirfd: i32, path: &str) -> Result<u16, i64> {
    let dir_inode = resolve_dir_for_path(ctx, dirfd, path)?;
    match fs::lookupat(dir_inode, path) {
        Ok(inode) => Ok(inode),
        Err(err) => Err(-fs_to_errno(err)),
    }
}

#[inline]
fn path_component_is_dot(name: &[u8]) -> bool {
    name.len() == 1 && name[0] == b'.'
}

#[inline]
fn path_component_is_dotdot(name: &[u8]) -> bool {
    name.len() == 2 && name[0] == b'.' && name[1] == b'.'
}

fn find_inode_path_from(
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

fn build_cwd_path(
    task_ptr: *mut task::Task,
    path_buf: &mut [u8; fs::VFS_MAX_PATH],
) -> Result<usize, i64> {
    if task_ptr.is_null() {
        return Err(-ESRCH);
    }
    let cwd = unsafe { (*task_ptr).user_cwd_inode };
    if cwd == fs::root_inode() {
        path_buf[0] = b'/';
        return Ok(1);
    }
    match find_inode_path_from(fs::root_inode(), cwd, 0, 0, path_buf)? {
        Some(len) => Ok(len),
        None => Err(-ENOENT),
    }
}

#[inline]
fn ticks_to_seconds_nanos(ticks: u64) -> (i64, i64) {
    let hz = pit::TIMER_FREQUENCY as u64;
    let sec = ticks / hz;
    let rem_ticks = ticks % hz;
    let nsec = (rem_ticks as u128 * 1_000_000_000u128 / hz as u128) as u64;
    (sec as i64, nsec as i64)
}

fn timespec_to_sleep_ticks(spec: &LinuxTimeSpec) -> Result<u64, i64> {
    if spec.tv_sec < 0 || spec.tv_nsec < 0 || spec.tv_nsec >= 1_000_000_000 {
        return Err(-EINVAL);
    }

    let secs = spec.tv_sec as u128;
    let nanos = spec.tv_nsec as u128;
    let total_nanos = secs
        .checked_mul(1_000_000_000u128)
        .and_then(|v| v.checked_add(nanos))
        .ok_or(-EINVAL)?;
    if total_nanos == 0 {
        return Ok(0);
    }

    let hz = pit::TIMER_FREQUENCY as u128;
    let ticks = total_nanos
        .checked_mul(hz)
        .and_then(|v| v.checked_add(1_000_000_000u128 - 1))
        .map(|v| v / 1_000_000_000u128)
        .ok_or(-EINVAL)?;
    if ticks > u64::MAX as u128 {
        return Err(-EINVAL);
    }
    Ok(ticks as u64)
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

    if start > USER_CANON_MAX || start >= KERNEL_VIRT_BASE {
        return false;
    }

    let end = match start.checked_add((len as u64).saturating_sub(1)) {
        Some(end) => end,
        None => return false,
    };

    end <= USER_CANON_MAX && end < KERNEL_VIRT_BASE
}

fn arch_prctl_set_fs_addr_ok(addr: u64) -> bool {
    addr <= USER_CANON_MAX && addr < KERNEL_VIRT_BASE
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
    let bytes =
        unsafe { core::slice::from_raw_parts_mut(raw.as_mut_ptr() as *mut u8, size_of::<T>()) };
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

fn copy_cstr_from_user_task(
    task_ptr: *mut task::Task,
    user_src: u64,
    out: &mut [u8],
) -> Result<usize, i64> {
    if out.is_empty() {
        return Err(-EINVAL);
    }

    for (idx, dst_byte) in out.iter_mut().enumerate() {
        let mut byte = [0u8; 1];
        if let Err(err) = copy_from_user_task(task_ptr, user_src + idx as u64, &mut byte) {
            return Err(-err);
        }
        *dst_byte = byte[0];
        if byte[0] == 0 {
            return Ok(idx);
        }
    }

    Err(-E2BIG)
}

fn read_exec_cstr_vector(
    task_ptr: *mut task::Task,
    vector_ptr: u64,
    out: &mut [ExecCString],
) -> Result<usize, i64> {
    if vector_ptr == 0 {
        return Ok(0);
    }

    for idx in 0..out.len() {
        let slot_ptr = vector_ptr + (idx * size_of::<u64>()) as u64;
        let entry_ptr = match read_u64_from_user(task_ptr, slot_ptr) {
            Ok(ptr) => ptr,
            Err(err) => return Err(-err),
        };
        if entry_ptr == 0 {
            return Ok(idx);
        }

        let len = copy_cstr_from_user_task(task_ptr, entry_ptr, &mut out[idx].bytes)?;
        out[idx].len = len + 1;
    }

    Err(-E2BIG)
}

struct ExecImage {
    ptr: *mut u8,
    len: usize,
}

impl ExecImage {
    fn as_slice(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.ptr as *const u8, self.len) }
    }
}

fn read_exec_image_from_inode(inode: u16) -> Result<ExecImage, i64> {
    let stat = fs::stat(inode).map_err(|err| -fs_to_errno(err))?;
    if stat.file_type != fs::FileType::Regular {
        return Err(-EACCES);
    }

    let size = usize::try_from(stat.size).map_err(|_| -EINVAL)?;
    if size == 0 || size > EXEC_MAX_IMAGE_SIZE {
        return Err(-EINVAL);
    }

    let ptr = heap::kmalloc(size);
    if ptr.is_null() {
        return Err(-ENOMEM);
    }

    let image = ExecImage { ptr, len: size };
    let dst = unsafe { core::slice::from_raw_parts_mut(ptr, size) };
    let mut copied = 0usize;
    while copied < dst.len() {
        let read = fs::read(inode, copied as u64, &mut dst[copied..]).map_err(|err| {
            heap::kfree(ptr);
            -fs_to_errno(err)
        })?;
        if read == 0 {
            heap::kfree(ptr);
            return Err(-EIO);
        }
        copied += read;
    }

    Ok(image)
}

fn execve_common(
    ctx: &SyscallContext,
    filename_ptr: u64,
    argv_ptr: u64,
    envp_ptr: u64,
) -> Result<user::ExecImageStart, i64> {
    let mut path_buf = [0u8; fs::VFS_MAX_PATH];
    let path = read_user_path(filename_ptr, &mut path_buf)?;
    if path.is_empty() {
        return Err(-ENOENT);
    }
    if path.len() + 1 > fs::VFS_MAX_PATH {
        return Err(-EINVAL);
    }

    let argv_buf_size = size_of::<ExecCString>() * EXEC_MAX_ARGV;
    let argv_buf_ptr = heap::kmalloc(argv_buf_size) as *mut ExecCString;
    if argv_buf_ptr.is_null() {
        return Err(-ENOMEM);
    }
    unsafe {
        ptr::write_bytes(argv_buf_ptr as *mut u8, 0, argv_buf_size);
    }
    let argv_buf = unsafe { core::slice::from_raw_parts_mut(argv_buf_ptr, EXEC_MAX_ARGV) };
    let argc = match read_exec_cstr_vector(ctx.current, argv_ptr, argv_buf) {
        Ok(argc) => argc,
        Err(err) => {
            heap::kfree(argv_buf_ptr as *mut u8);
            return Err(err);
        }
    };

    let env_buf_size = size_of::<ExecCString>() * EXEC_MAX_ENVP;
    let env_buf_ptr = heap::kmalloc(env_buf_size) as *mut ExecCString;
    if env_buf_ptr.is_null() {
        heap::kfree(argv_buf_ptr as *mut u8);
        return Err(-ENOMEM);
    }
    unsafe {
        ptr::write_bytes(env_buf_ptr as *mut u8, 0, env_buf_size);
    }
    let env_buf = unsafe { core::slice::from_raw_parts_mut(env_buf_ptr, EXEC_MAX_ENVP) };
    let envc = match read_exec_cstr_vector(ctx.current, envp_ptr, env_buf) {
        Ok(envc) => envc,
        Err(err) => {
            heap::kfree(env_buf_ptr as *mut u8);
            heap::kfree(argv_buf_ptr as *mut u8);
            return Err(err);
        }
    };

    let mut argv = [&[][..]; EXEC_MAX_ARGV];
    for idx in 0..argc {
        argv[idx] = argv_buf[idx].as_bytes();
    }
    let mut env = [&[][..]; EXEC_MAX_ENVP];
    for idx in 0..envc {
        env[idx] = env_buf[idx].as_bytes();
    }

    let dir_inode = match resolve_dir_for_path(ctx, fs::AT_FDCWD, path) {
        Ok(d) => d,
        Err(err) => {
            heap::kfree(env_buf_ptr as *mut u8);
            heap::kfree(argv_buf_ptr as *mut u8);
            return Err(err);
        }
    };
    let inode = match fs::lookupat(dir_inode, path) {
        Ok(i) => i,
        Err(err) => {
            heap::kfree(env_buf_ptr as *mut u8);
            heap::kfree(argv_buf_ptr as *mut u8);
            return Err(-fs_to_errno(err));
        }
    };
    let image = match read_exec_image_from_inode(inode) {
        Ok(image) => image,
        Err(err) => {
            heap::kfree(env_buf_ptr as *mut u8);
            heap::kfree(argv_buf_ptr as *mut u8);
            return Err(err);
        }
    };
    let execfn_len = path.len();
    path_buf[execfn_len] = 0;
    let execfn = &path_buf[..execfn_len + 1];
    let result = user::exec_current_image(image.as_slice(), &argv[..argc], &env[..envc], execfn);
    heap::kfree(env_buf_ptr as *mut u8);
    heap::kfree(argv_buf_ptr as *mut u8);
    heap::kfree(image.ptr);
    if result.is_ok() {
        mmap_clear_task_regions(ctx.current);
        close_cloexec_fds(ctx.current);
    }
    result
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
        let phys = alloc_user_vm_page_with_reclaim();
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

fn unmap_user_pages_in_regions(
    pml4: PhysAddr,
    start: u64,
    end: u64,
    regions: Option<&[VmMapping; MMAP_MAX_REGIONS_PER_TASK]>,
) {
    let mut addr = start;
    while addr < end {
        let virt = VirtAddr::new(addr);
        if let Some(phys) = vmm::get_physical_in(pml4, virt) {
            if vmm::unmap_page_in(pml4, virt) == 0 {
                let phys_page = PhysAddr::new(phys.as_u64() & !(PAGE_SIZE - 1));
                let mapping = regions.and_then(|entries| mmap_regions_find_mapping(entries, addr));
                let _ = release_unmapped_user_page(mapping, phys_page);
            }
        }
        addr += PAGE_SIZE;
    }
}

fn unmap_user_pages(pml4: PhysAddr, start: u64, end: u64) {
    unmap_user_pages_in_regions(pml4, start, end, None);
}

fn mmap_flags_are_supported(flags: u32) -> bool {
    let supported = MAP_SHARED
        | MAP_PRIVATE
        | MAP_FIXED
        | MAP_ANONYMOUS
        | MAP_DENYWRITE
        | MAP_EXECUTABLE
        | MAP_FIXED_NOREPLACE;
    (flags & !supported) == 0
}

fn mmap_parse_sharing_mode(flags: u32) -> Result<(bool, bool), i64> {
    let is_private = (flags & MAP_PRIVATE) != 0;
    let is_shared = (flags & MAP_SHARED) != 0;
    if is_private == is_shared {
        return Err(-EINVAL);
    }
    Ok((is_private, is_shared))
}

fn msync_flags_are_valid(flags: u32) -> bool {
    let supported = MS_ASYNC | MS_SYNC | MS_INVALIDATE;
    if (flags & !supported) != 0 {
        return false;
    }
    if (flags & MS_ASYNC) != 0 && (flags & MS_SYNC) != 0 {
        return false;
    }
    true
}

fn mmap_prot_to_pte_flags(prot: u32) -> Result<u64, i64> {
    if (prot & !(PROT_READ | PROT_WRITE | PROT_EXEC)) != 0 {
        return Err(-EINVAL);
    }

    let mut flags = vmm::PTE_PRESENT;
    if prot != PROT_NONE {
        flags |= vmm::PTE_USER;
    }
    if (prot & PROT_WRITE) != 0 {
        flags |= vmm::PTE_WRITE;
    }
    if (prot & PROT_EXEC) == 0 {
        flags |= vmm::PTE_NO_EXECUTE;
    }

    Ok(flags)
}

fn mmap_task_slot(task_ptr: *mut task::Task) -> Option<usize> {
    if task_ptr.is_null() {
        return None;
    }
    for idx in 0..task::MAX_TASKS {
        if task::get_by_index(idx) == task_ptr {
            return Some(idx);
        }
    }
    None
}

fn mmap_prepare_task_slot(task_ptr: *mut task::Task) -> Option<usize> {
    let slot = mmap_task_slot(task_ptr)?;
    unsafe {
        let pid = (*task_ptr).pid;
        if TASK_VM_OWNER_PID[slot] != pid {
            TASK_VM_OWNER_PID[slot] = pid;
            TASK_VM_MAPPINGS[slot] = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
        }
    }
    Some(slot)
}

fn mmap_clear_task_regions(task_ptr: *mut task::Task) {
    let slot = match mmap_task_slot(task_ptr) {
        Some(slot) => slot,
        None => return,
    };
    unsafe {
        TASK_VM_OWNER_PID[slot] = task::INVALID_PID;
        TASK_VM_MAPPINGS[slot] = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
    }
}

fn mmap_regions_find_free(regions: &[VmMapping; MMAP_MAX_REGIONS_PER_TASK]) -> Option<usize> {
    for (idx, region) in regions.iter().enumerate() {
        if region.is_empty() {
            return Some(idx);
        }
    }
    None
}

fn mmap_regions_overlaps(
    regions: &[VmMapping; MMAP_MAX_REGIONS_PER_TASK],
    start: u64,
    end: u64,
) -> bool {
    for region in regions {
        if region.is_empty() {
            continue;
        }
        if start < region.end() && region.start < end {
            return true;
        }
    }
    false
}

fn mmap_regions_find_mapping(
    regions: &[VmMapping; MMAP_MAX_REGIONS_PER_TASK],
    addr: u64,
) -> Option<VmMapping> {
    for region in regions {
        if region.is_empty() {
            continue;
        }
        if addr >= region.start && addr < region.end() {
            return Some(*region);
        }
    }
    None
}

fn mmap_regions_insert(
    regions: &mut [VmMapping; MMAP_MAX_REGIONS_PER_TASK],
    mapping: VmMapping,
) -> Result<(), i64> {
    if mapping.is_empty() {
        return Err(-EINVAL);
    }
    if mmap_regions_overlaps(regions, mapping.start, mapping.end()) {
        return Err(-EINVAL);
    }

    let slot = match mmap_regions_find_free(regions) {
        Some(slot) => slot,
        None => return Err(-ENOMEM),
    };
    regions[slot] = mapping;
    Ok(())
}

#[inline]
fn mmap_region_allows_prot(region: &VmMapping, prot: u32) -> bool {
    if region.map_shared && (prot & PROT_WRITE) != 0 && !region.map_shared_can_write {
        return false;
    }
    true
}

fn mmap_regions_split_at(
    regions: &mut [VmMapping; MMAP_MAX_REGIONS_PER_TASK],
    cut: u64,
) -> Result<(), i64> {
    for idx in 0..MMAP_MAX_REGIONS_PER_TASK {
        let region = regions[idx];
        if region.is_empty() || cut <= region.start || cut >= region.end() {
            continue;
        }

        let free_idx = match mmap_regions_find_free(regions) {
            Some(slot) => slot,
            None => return Err(-ENOMEM),
        };

        let mut right = region;
        right.start = cut;
        right.len = region.end() - cut;
        if right.file_inode != task::USER_FD_NONE {
            right.file_offset = region.file_offset.saturating_add(cut - region.start);
        }

        regions[idx].len = cut - region.start;
        regions[free_idx] = right;
        return Ok(());
    }
    Ok(())
}

fn mmap_regions_range_mapped(
    regions: &[VmMapping; MMAP_MAX_REGIONS_PER_TASK],
    start: u64,
    end: u64,
) -> bool {
    let mut cursor = start;
    while cursor < end {
        let region = match mmap_regions_find_mapping(regions, cursor) {
            Some(region) => region,
            None => return false,
        };
        let next = cmp::min(region.end(), end);
        if next <= cursor {
            return false;
        }
        cursor = next;
    }
    true
}

fn mmap_regions_unmap_range(
    regions: &mut [VmMapping; MMAP_MAX_REGIONS_PER_TASK],
    start: u64,
    end: u64,
) -> Result<(), i64> {
    let mut next = *regions;
    mmap_regions_unmap_range_in_place(&mut next, start, end)?;
    *regions = next;
    Ok(())
}

fn mmap_regions_unmap_range_in_place(
    regions: &mut [VmMapping; MMAP_MAX_REGIONS_PER_TASK],
    start: u64,
    end: u64,
) -> Result<(), i64> {
    for idx in 0..MMAP_MAX_REGIONS_PER_TASK {
        let region = regions[idx];
        if region.is_empty() {
            continue;
        }

        let region_start = region.start;
        let region_end = region.end();
        if end <= region_start || start >= region_end {
            continue;
        }

        if start <= region_start && end >= region_end {
            regions[idx] = VmMapping::EMPTY;
            continue;
        }

        if start <= region_start {
            let delta = end - region_start;
            regions[idx].start = end;
            regions[idx].len = region_end - end;
            if region.file_inode != task::USER_FD_NONE {
                regions[idx].file_offset = region.file_offset.saturating_add(delta);
            }
            continue;
        }

        if end >= region_end {
            regions[idx].len = start - region_start;
            continue;
        }

        let free_idx = match mmap_regions_find_free(regions) {
            Some(slot) => slot,
            None => return Err(-ENOMEM),
        };
        let mut right = region;
        right.start = end;
        right.len = region_end - end;
        if right.file_inode != task::USER_FD_NONE {
            right.file_offset = region.file_offset.saturating_add(end - region_start);
        }

        regions[idx].len = start - region_start;
        regions[free_idx] = right;
    }
    Ok(())
}

fn mmap_regions_protect_range(
    regions: &mut [VmMapping; MMAP_MAX_REGIONS_PER_TASK],
    start: u64,
    end: u64,
    prot: u32,
) -> Result<(), i64> {
    let mut next = *regions;
    mmap_regions_protect_range_in_place(&mut next, start, end, prot)?;
    *regions = next;
    Ok(())
}

fn mmap_regions_protect_range_in_place(
    regions: &mut [VmMapping; MMAP_MAX_REGIONS_PER_TASK],
    start: u64,
    end: u64,
    prot: u32,
) -> Result<(), i64> {
    if !mmap_regions_range_mapped(regions, start, end) {
        return Err(-ENOMEM);
    }

    mmap_regions_split_at(regions, start)?;
    mmap_regions_split_at(regions, end)?;

    for region in regions.iter() {
        if region.is_empty() {
            continue;
        }
        if region.start >= start && region.end() <= end && !mmap_region_allows_prot(region, prot) {
            return Err(-EINVAL);
        }
    }

    for region in regions.iter_mut() {
        if region.is_empty() {
            continue;
        }
        if region.start >= start && region.end() <= end {
            region.prot = prot;
        }
    }
    Ok(())
}

fn mmap_task_overlaps(task_ptr: *mut task::Task, start: u64, end: u64) -> bool {
    let slot = match mmap_prepare_task_slot(task_ptr) {
        Some(slot) => slot,
        None => return false,
    };
    unsafe { mmap_regions_overlaps(&TASK_VM_MAPPINGS[slot], start, end) }
}

fn mmap_range_available(task_ptr: *mut task::Task, pml4: PhysAddr, start: u64, end: u64) -> bool {
    if mmap_task_overlaps(task_ptr, start, end) {
        return false;
    }

    let mut addr = start;
    while addr < end {
        if vmm::get_physical_in(pml4, VirtAddr::new(addr)).is_some() {
            return false;
        }
        addr += PAGE_SIZE;
    }
    true
}

fn mmap_find_range(ctx: &SyscallContext, pml4: PhysAddr, hint: u64, len: u64) -> Result<u64, i64> {
    let (brk, stack_bottom) =
        unsafe { ((*ctx.current).user_brk, (*ctx.current).user_stack_bottom) };
    if stack_bottom == 0 {
        return Err(-ENOMEM);
    }

    let brk_start = page_align_up(brk.saturating_add(MMAP_BRK_GAP)).unwrap_or(MMAP_BASE_ADDR);
    let search_base = cmp::max(MMAP_BASE_ADDR, brk_start);
    let search_end = stack_bottom.saturating_sub(MMAP_STACK_GUARD);
    if search_end <= search_base {
        return Err(-ENOMEM);
    }

    let mut cursor = if hint == 0 {
        search_base
    } else {
        cmp::max(search_base, align_down(hint, PAGE_SIZE))
    };

    while cursor < search_end {
        let end = match cursor.checked_add(len) {
            Some(end) => end,
            None => break,
        };
        if end > search_end || end >= KERNEL_VIRT_BASE || cursor < PAGE_SIZE {
            break;
        }
        if mmap_range_available(ctx.current, pml4, cursor, end) {
            return Ok(cursor);
        }
        cursor = match cursor.checked_add(PAGE_SIZE) {
            Some(next) => next,
            None => break,
        };
    }

    Err(-ENOMEM)
}

fn mmap_file_inode(
    task_ptr: *mut task::Task,
    fd: i32,
    require_readable: bool,
    require_writable: bool,
) -> Result<(u16, u32), i64> {
    let idx = fd_index(fd)?;
    if !fd_slot_is_open(task_ptr, idx) {
        return Err(-EBADF);
    }

    let (kind, inode, fd_flags) = unsafe {
        (
            (*task_ptr).user_fd_kind[idx],
            (*task_ptr).user_fd_inode[idx],
            (*task_ptr).user_fd_flags[idx],
        )
    };
    if kind != task::USER_FD_KIND_VFS {
        return Err(-EBADF);
    }
    if require_readable && !fd_can_read(fd_flags) {
        return Err(-EACCES);
    }
    if require_writable && !fd_can_write(fd_flags) {
        return Err(-EACCES);
    }

    Ok((inode, fd_flags))
}

fn mmap_validate_shared_write_window(inode: u16, offset: u64, len: u64) -> Result<(), i64> {
    let end = offset.checked_add(len).ok_or(-EINVAL)?;
    let stat = fs::stat(inode).map_err(|err| -fs_to_errno(err))?;
    if end > stat.size {
        return Err(-EINVAL);
    }
    Ok(())
}

fn mmap_validate_framebuffer_window(
    offset: u64,
    len: u64,
    prot: u32,
    is_shared: bool,
) -> Result<(), i64> {
    if !is_shared || (prot & PROT_EXEC) != 0 {
        return Err(-EINVAL);
    }
    match framebuffer::mmap_size() {
        Some(_) if framebuffer::mmap_range_valid(offset, len) => Ok(()),
        Some(_) => Err(-EINVAL),
        None => Err(-ENXIO),
    }
}

fn mmap_regions_total_pages(regions: &[VmMapping; MMAP_MAX_REGIONS_PER_TASK]) -> Result<u64, i64> {
    let mut pages = 0u64;
    for region in regions {
        if region.is_empty() {
            continue;
        }
        pages = pages.checked_add(region.len / PAGE_SIZE).ok_or(-ENOMEM)?;
    }
    Ok(pages)
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

fn sys_mmap(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let addr_hint = call.arg(0);
    let length = call.arg(1);
    let prot = call.arg(2) as u32;
    let flags = call.arg(3) as u32;
    let fd_raw = call.arg(4) as i64;
    let offset = call.arg(5);

    if length == 0 {
        return -EINVAL;
    }
    if !mmap_flags_are_supported(flags) {
        return -EINVAL;
    }

    let (_is_private, is_shared) = match mmap_parse_sharing_mode(flags) {
        Ok(mode) => mode,
        Err(err) => return err,
    };

    let is_anon = (flags & MAP_ANONYMOUS) != 0;
    let is_fixed_noreplace = (flags & MAP_FIXED_NOREPLACE) != 0;
    let is_fixed = (flags & MAP_FIXED) != 0 || is_fixed_noreplace;

    let len = match page_align_up(length) {
        Some(v) if v > 0 => v,
        _ => return -EINVAL,
    };
    let pml4 = match ctx.user_pml4() {
        Ok(v) => v,
        Err(err) => return err,
    };

    let file_src = if is_anon {
        if is_shared {
            return -EINVAL;
        }
        if fd_raw != -1 {
            return -EINVAL;
        }
        if offset != 0 {
            return -EINVAL;
        }
        None
    } else {
        if fd_raw < 0 || fd_raw > i32::MAX as i64 {
            return -EBADF;
        }
        if (offset & (PAGE_SIZE - 1)) != 0 {
            return -EINVAL;
        }
        if offset.checked_add(len).is_none() {
            return -EINVAL;
        }
        let fd = fd_raw as i32;
        let require_writable = is_shared && (prot & PROT_WRITE) != 0;
        match mmap_file_inode(ctx.current, fd, true, require_writable) {
            Ok((inode, fd_flags)) => {
                let shared_can_write = is_shared && fd_can_write(fd_flags);
                if fs::inode_is_framebuffer(inode) {
                    if let Err(err) = mmap_validate_framebuffer_window(offset, len, prot, is_shared)
                    {
                        return err;
                    }
                } else if is_shared && (prot & PROT_WRITE) != 0 {
                    if let Err(err) = mmap_validate_shared_write_window(inode, offset, len) {
                        return err;
                    }
                }
                Some((inode, offset, shared_can_write))
            }
            Err(err) => return err,
        }
    };

    let slot = match mmap_prepare_task_slot(ctx.current) {
        Some(slot) => slot,
        None => return -ESRCH,
    };
    let mut projected_regions = unsafe { TASK_VM_MAPPINGS[slot] };

    let start = if is_fixed {
        if (addr_hint & (PAGE_SIZE - 1)) != 0 || addr_hint < MMAP_BASE_ADDR {
            return -EINVAL;
        }
        let end = match addr_hint.checked_add(len) {
            Some(v) => v,
            None => return -EINVAL,
        };
        if end >= KERNEL_VIRT_BASE {
            return -EINVAL;
        }

        if is_fixed_noreplace {
            if !mmap_range_available(ctx.current, pml4, addr_hint, end) {
                return -EEXIST;
            }
        } else {
            let unmap_result = mmap_regions_unmap_range(&mut projected_regions, addr_hint, end);
            if let Err(err) = unmap_result {
                return err;
            }
        }
        addr_hint
    } else {
        let hint = if addr_hint == 0 {
            0
        } else {
            align_down(addr_hint, PAGE_SIZE)
        };
        match mmap_find_range(ctx, pml4, hint, len) {
            Ok(addr) => addr,
            Err(err) => return err,
        }
    };

    let mapped_pages = len / PAGE_SIZE;
    let projected_pages = match mmap_regions_total_pages(&projected_regions) {
        Ok(pages) => pages,
        Err(err) => return err,
    };
    if projected_pages.saturating_add(mapped_pages) > MMAP_MAX_PAGES_PER_TASK {
        return -ENOMEM;
    }

    let mapping = VmMapping {
        start,
        len,
        file_offset: file_src.map(|(_, off, _)| off).unwrap_or(0),
        file_inode: file_src
            .map(|(inode, _, _)| inode)
            .unwrap_or(task::USER_FD_NONE),
        prot,
        map_shared: is_shared,
        map_shared_can_write: file_src.map(|(_, _, can)| can).unwrap_or(false),
    };
    let insert_result = mmap_regions_insert(&mut projected_regions, mapping);
    match insert_result {
        Ok(()) => {
            if is_fixed && !is_fixed_noreplace {
                let end = match start.checked_add(len) {
                    Some(v) => v,
                    None => return -EINVAL,
                };
                let old_regions = unsafe { TASK_VM_MAPPINGS[slot] };
                unmap_user_pages_in_regions(pml4, start, end, Some(&old_regions));
            }
            unsafe {
                TASK_VM_MAPPINGS[slot] = projected_regions;
            }
            start as i64
        }
        Err(err) => err,
    }
}

fn sys_munmap(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let addr = call.arg(0);
    let length = call.arg(1);
    if length == 0 {
        return -EINVAL;
    }
    if (addr & (PAGE_SIZE - 1)) != 0 || addr < MMAP_BASE_ADDR {
        return -EINVAL;
    }

    let len = match page_align_up(length) {
        Some(v) if v > 0 => v,
        _ => return -EINVAL,
    };
    let end = match addr.checked_add(len) {
        Some(v) => v,
        None => return -EINVAL,
    };
    if end >= KERNEL_VIRT_BASE {
        return -EINVAL;
    }

    let pml4 = match ctx.user_pml4() {
        Ok(v) => v,
        Err(err) => return err,
    };
    let slot = match mmap_prepare_task_slot(ctx.current) {
        Some(slot) => slot,
        None => return -ESRCH,
    };
    let old_regions = unsafe { TASK_VM_MAPPINGS[slot] };
    let mut next_regions = old_regions;
    let unmap_result = mmap_regions_unmap_range(&mut next_regions, addr, end);
    if let Err(err) = unmap_result {
        return err;
    }
    unmap_user_pages_in_regions(pml4, addr, end, Some(&old_regions));
    unsafe {
        TASK_VM_MAPPINGS[slot] = next_regions;
    }
    0
}

fn sys_mprotect(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let addr = call.arg(0);
    let length = call.arg(1);
    let prot = call.arg(2) as u32;

    if length == 0 {
        return 0;
    }
    if (addr & (PAGE_SIZE - 1)) != 0 || addr < MMAP_BASE_ADDR {
        return -EINVAL;
    }

    let len = match page_align_up(length) {
        Some(v) if v > 0 => v,
        _ => return -EINVAL,
    };
    let end = match addr.checked_add(len) {
        Some(v) => v,
        None => return -EINVAL,
    };
    if end >= KERNEL_VIRT_BASE {
        return -EINVAL;
    }

    let pte_flags = match mmap_prot_to_pte_flags(prot) {
        Ok(v) => v,
        Err(err) => return err,
    };
    let pml4 = match ctx.user_pml4() {
        Ok(v) => v,
        Err(err) => return err,
    };
    let slot = match mmap_prepare_task_slot(ctx.current) {
        Some(slot) => slot,
        None => return -ESRCH,
    };
    let protect_result =
        unsafe { mmap_regions_protect_range(&mut TASK_VM_MAPPINGS[slot], addr, end, prot) };
    if let Err(err) = protect_result {
        return err;
    }

    let mut page = addr;
    while page < end {
        let phys = match vmm::get_physical_in(pml4, VirtAddr::new(page)) {
            Some(phys) => PhysAddr::new(phys.as_u64() & !(PAGE_SIZE - 1)),
            None => {
                page += PAGE_SIZE;
                continue;
            }
        };

        if vmm::map_page_in(pml4, VirtAddr::new(page), phys, pte_flags) != 0 {
            return -ENOMEM;
        }
        let mapping = unsafe { mmap_regions_find_mapping(&TASK_VM_MAPPINGS[slot], page) };
        if let Some(mapping) = mapping {
            if mapping.map_shared
                && mapping.file_inode != task::USER_FD_NONE
                && (mapping.prot & PROT_WRITE) != 0
            {
                let _ = vm_page_cache_mark_dirty_by_phys(phys);
            }
        }
        page += PAGE_SIZE;
    }

    0
}

fn mmap_prot_allows_fault_access(prot: u32, error_code: u64) -> bool {
    if prot == PROT_NONE {
        return false;
    }
    if (error_code & PAGE_FAULT_ERROR_WRITE) != 0 {
        return (prot & PROT_WRITE) != 0;
    }
    if (error_code & PAGE_FAULT_ERROR_INSTR) != 0 {
        return (prot & PROT_EXEC) != 0;
    }
    (prot & (PROT_READ | PROT_WRITE)) != 0
}

fn mmap_fault_map_page(pml4: PhysAddr, page_addr: u64, mapping: VmMapping) -> Result<(), i64> {
    if vmm::get_physical_in(pml4, VirtAddr::new(page_addr)).is_some() {
        return Ok(());
    }

    if mmap_mapping_is_framebuffer(&mapping) {
        let map_delta = page_addr.checked_sub(mapping.start).ok_or(-EINVAL)?;
        let page_off = mapping.file_offset.checked_add(map_delta).ok_or(-EINVAL)?;
        let page_phys = framebuffer::mmap_phys_page(page_off).ok_or(-EINVAL)?;
        let pte_flags = mmap_prot_to_pte_flags(mapping.prot)?;
        if vmm::map_page_in(
            pml4,
            VirtAddr::new(page_addr),
            PhysAddr::new(page_phys),
            pte_flags,
        ) != 0
        {
            return Err(-ENOMEM);
        }
        return Ok(());
    }

    if mapping.map_shared && mapping.file_inode != task::USER_FD_NONE {
        let map_delta = page_addr.checked_sub(mapping.start).ok_or(-EINVAL)?;
        let file_off = mapping.file_offset.checked_add(map_delta).ok_or(-EINVAL)?;
        let page_off = align_down(file_off, PAGE_SIZE);
        let writable = (mapping.prot & PROT_WRITE) != 0;
        let entry = vm_page_cache_pin_mapped_page(mapping.file_inode, page_off, writable)?;

        let pte_flags = mmap_prot_to_pte_flags(mapping.prot)?;
        if vmm::map_page_in(
            pml4,
            VirtAddr::new(page_addr),
            PhysAddr::new(entry.page_phys),
            pte_flags,
        ) != 0
        {
            let _ = vm_page_cache_release_mapped_phys(PhysAddr::new(entry.page_phys));
            return Err(-ENOMEM);
        }
        return Ok(());
    }

    let page_phys = alloc_user_vm_page_with_reclaim();
    if page_phys.as_u64() == 0 {
        return Err(-ENOMEM);
    }
    let page_ptr = page_phys.to_virt().as_u64() as *mut u8;
    unsafe {
        ptr::write_bytes(page_ptr, 0, PAGE_SIZE as usize);
    }

    if mapping.file_inode != task::USER_FD_NONE {
        let map_delta = page_addr.checked_sub(mapping.start).ok_or(-EINVAL)?;
        let file_off = mapping.file_offset.checked_add(map_delta).ok_or(-EINVAL)?;
        let to_read = cmp::min(PAGE_SIZE, mapping.end().saturating_sub(page_addr)) as usize;
        if to_read > 0 {
            let page_slice =
                unsafe { core::slice::from_raw_parts_mut(page_ptr, PAGE_SIZE as usize) };
            let _ = read_vfs_bytes(mapping.file_inode, file_off, &mut page_slice[..to_read])?;
        }
    }

    let pte_flags = mmap_prot_to_pte_flags(mapping.prot)?;
    if vmm::map_page_in(pml4, VirtAddr::new(page_addr), page_phys, pte_flags) != 0 {
        pmm::free_page(page_phys);
        return Err(-ENOMEM);
    }
    Ok(())
}

fn mmap_clone_task_regions(parent: *mut task::Task, child: *mut task::Task) -> Result<(), i64> {
    let parent_slot = match mmap_prepare_task_slot(parent) {
        Some(slot) => slot,
        None => return Err(-ESRCH),
    };
    let child_slot = match mmap_task_slot(child) {
        Some(slot) => slot,
        None => return Err(-ESRCH),
    };

    unsafe {
        TASK_VM_OWNER_PID[child_slot] = (*child).pid;
        TASK_VM_MAPPINGS[child_slot] = TASK_VM_MAPPINGS[parent_slot];
    }
    Ok(())
}

fn mmap_fork_reconcile_shared_file_mappings(task_ptr: *mut task::Task) -> Result<(), i64> {
    if task_ptr.is_null() {
        return Err(-ESRCH);
    }
    let pml4 = unsafe { PhysAddr::new((*task_ptr).pml4_phys) };
    if pml4.as_u64() == 0 {
        return Err(-EINVAL);
    }
    let slot = mmap_prepare_task_slot(task_ptr).ok_or(-ESRCH)?;
    let regions = unsafe { TASK_VM_MAPPINGS[slot] };

    for region in regions {
        if region.is_empty() || !region.map_shared || region.file_inode == task::USER_FD_NONE {
            continue;
        }

        let mut page = region.start;
        while page < region.end() {
            let virt = VirtAddr::new(page);
            let phys = match vmm::get_physical_in(pml4, virt) {
                Some(phys) => PhysAddr::new(phys.as_u64() & !(PAGE_SIZE - 1)),
                None => {
                    page = page.saturating_add(PAGE_SIZE);
                    continue;
                }
            };

            if vmm::unmap_page_in(pml4, virt) != 0 {
                return Err(-ENOMEM);
            }
            pmm::free_page(phys);
            page = page.saturating_add(PAGE_SIZE);
        }
    }

    Ok(())
}

pub fn try_handle_page_fault(fault_addr: u64, error_code: u64) -> bool {
    if (error_code & PAGE_FAULT_ERROR_USER) == 0 {
        return false;
    }
    if (error_code & PAGE_FAULT_ERROR_PRESENT) != 0 {
        return false;
    }
    if fault_addr < PAGE_SIZE || fault_addr >= KERNEL_VIRT_BASE || fault_addr > USER_CANON_MAX {
        return false;
    }

    let current = task::current();
    if current.is_null() {
        return false;
    }
    let is_user = unsafe { ((*current).flags & TASK_FLAG_USER) != 0 };
    if !is_user {
        return false;
    }

    let pml4 = unsafe { PhysAddr::new((*current).pml4_phys) };
    if pml4.as_u64() == 0 {
        return false;
    }

    let slot = match mmap_prepare_task_slot(current) {
        Some(slot) => slot,
        None => return false,
    };
    let page_addr = align_down(fault_addr, PAGE_SIZE);
    let mapping = unsafe { mmap_regions_find_mapping(&TASK_VM_MAPPINGS[slot], page_addr) };
    let mapping = match mapping {
        Some(mapping) => mapping,
        None => return false,
    };
    if !mmap_prot_allows_fault_access(mapping.prot, error_code) {
        return false;
    }

    mmap_fault_map_page(pml4, page_addr, mapping).is_ok()
}

pub fn terminate_current_user_fault(error_code: u64, fault_addr: u64) -> bool {
    if (error_code & PAGE_FAULT_ERROR_USER) == 0 {
        return false;
    }

    let current = task::current();
    if current.is_null() {
        return false;
    }
    let is_user = unsafe { ((*current).flags & TASK_FLAG_USER) != 0 };
    if !is_user {
        return false;
    }

    let pid = unsafe { (*current).pid };
    sys_log!(
        "[SYSCALL] pid {} terminated after page fault at 0x{:x} (err=0x{:x})",
        pid,
        fault_addr,
        error_code
    );

    let ctx = SyscallContext::new(current);
    let _ = sys_exit_common(&ctx, 128 + signal::SIGSEGV as i32);
    true
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
        let ctty_type = unsafe { (*ctx.current).ctty_type };
        return match ctty_type {
            task::CTTY_CONSOLE => {
                match fd_alloc(ctx.current, task::USER_FD_KIND_CONSOLE, 0, fd_flags) {
                    Ok(fd) => fd,
                    Err(err) => err,
                }
            }
            task::CTTY_PTY => {
                let pty_id = unsafe { (*ctx.current).ctty_id };
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

fn memfd_validate_flags(flags: u32) -> Result<bool, i64> {
    let supported = MFD_CLOEXEC | MFD_ALLOW_SEALING;
    if (flags & !supported) != 0 {
        return Err(-EINVAL);
    }
    if (flags & MFD_ALLOW_SEALING) != 0 {
        return Err(-ENOSYS);
    }
    Ok((flags & MFD_CLOEXEC) != 0)
}

fn memfd_storage_dir_ready() -> Result<(), i64> {
    match fs::lookup(MEMFD_STORAGE_DIR) {
        Ok(inode) => match fs::inode_is_dir(inode) {
            Ok(true) => Ok(()),
            Ok(false) => Err(-ENOTDIR),
            Err(err) => Err(-fs_to_errno(err)),
        },
        Err(fs::ENOENT) => match fs::mkdir(MEMFD_STORAGE_DIR) {
            Ok(()) => Ok(()),
            Err(fs::EEXIST) => Ok(()),
            Err(err) => Err(-fs_to_errno(err)),
        },
        Err(err) => Err(-fs_to_errno(err)),
    }
}

fn memfd_name_sanitize(raw: &[u8], out: &mut [u8; MEMFD_NAME_MAX]) -> usize {
    let mut len = 0usize;
    for &byte in raw {
        if len >= out.len() {
            break;
        }
        out[len] = match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => byte,
            _ => b'_',
        };
        len += 1;
    }
    if len == 0 {
        out[0] = b'a';
        out[1] = b'n';
        out[2] = b'o';
        out[3] = b'n';
        4
    } else {
        len
    }
}

fn memfd_push_bytes(dst: &mut [u8], len: &mut usize, src: &[u8]) -> Result<(), i64> {
    let end = match len.checked_add(src.len()) {
        Some(v) => v,
        None => return Err(-EINVAL),
    };
    if end > dst.len() {
        return Err(-EINVAL);
    }
    dst[*len..end].copy_from_slice(src);
    *len = end;
    Ok(())
}

fn memfd_push_u32_decimal(dst: &mut [u8], len: &mut usize, mut value: u32) -> Result<(), i64> {
    let mut tmp = [0u8; 10];
    let mut digits = 0usize;
    loop {
        tmp[digits] = b'0' + (value % 10) as u8;
        digits += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    let mut idx = digits;
    while idx > 0 {
        idx -= 1;
        memfd_push_bytes(dst, len, &tmp[idx..idx + 1])?;
    }
    Ok(())
}

fn memfd_build_path<'a>(
    pid: i32,
    sequence: u32,
    raw_name: &[u8],
    out: &'a mut [u8; fs::VFS_MAX_PATH],
) -> Result<&'a str, i64> {
    let mut len = 0usize;
    memfd_push_bytes(out, &mut len, MEMFD_PATH_PREFIX)?;
    let pid_u32 = if pid > 0 { pid as u32 } else { 0 };
    memfd_push_u32_decimal(out, &mut len, pid_u32)?;
    memfd_push_bytes(out, &mut len, b"-")?;
    memfd_push_u32_decimal(out, &mut len, sequence)?;
    memfd_push_bytes(out, &mut len, b"-")?;

    let mut sanitized = [0u8; MEMFD_NAME_MAX];
    let sanitized_len = memfd_name_sanitize(raw_name, &mut sanitized);
    memfd_push_bytes(out, &mut len, &sanitized[..sanitized_len])?;

    core::str::from_utf8(&out[..len]).map_err(|_| -EINVAL)
}

fn sys_memfd_create(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let cloexec = match memfd_validate_flags(call.arg(1) as u32) {
        Ok(value) => value,
        Err(err) => return err,
    };

    let mut name_buf = [0u8; MEMFD_NAME_MAX + 1];
    let name_len = match copy_cstr_from_user_current(call.arg(0), &mut name_buf) {
        Ok(len) => len,
        Err(err) => return -err,
    };
    if let Err(err) = enforce_memfd_create_limit(ctx.current) {
        return err;
    }

    if let Err(err) = memfd_storage_dir_ready() {
        return err;
    }

    let pid = unsafe { (*ctx.current).pid };
    let mut path_buf = [0u8; fs::VFS_MAX_PATH];

    for _ in 0..MEMFD_CREATE_MAX_RETRIES {
        let sequence = MEMFD_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = match memfd_build_path(pid, sequence, &name_buf[..name_len], &mut path_buf) {
            Ok(path) => path,
            Err(err) => return err,
        };

        let opened = match fs::open(path, fs::O_CREAT | fs::O_EXCL | fs::O_RDWR, 0o600) {
            Ok(opened) => opened,
            Err(fs::EEXIST) => continue,
            Err(fs::ENOSPC) => return -ENFILE,
            Err(err) => return -fs_to_errno(err),
        };

        let mut open_flags = fs::O_RDWR;
        if cloexec {
            open_flags |= O_CLOEXEC;
        }
        let fd_flags = fd_flags_from_open_flags(open_flags) | FD_INTERNAL_MEMFD;
        return match fd_alloc(ctx.current, task::USER_FD_KIND_VFS, opened.inode, fd_flags) {
            Ok(fd) => fd,
            Err(err) => {
                let _ = fs::unlink(path);
                err
            }
        };
    }

    -EAGAIN
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
        Ok(()) => {
            sync_ext2_backing();
            0
        }
        Err(err) => -fs_to_errno(err),
    }
}

fn sys_access_common(ctx: &SyscallContext, dirfd: i32, pathname: u64, mode: u32) -> i64 {
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

    let (uid, gid) = unsafe { ((*ctx.current).user_uid, (*ctx.current).user_gid) };
    if access_allowed(&stat, mode, uid, gid) {
        0
    } else {
        -EACCES
    }
}

fn sys_access(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    sys_access_common(ctx, fs::AT_FDCWD, call.arg(0), call.arg(1) as u32)
}

fn sys_faccessat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    sys_access_common(ctx, call.arg(0) as i32, call.arg(1), call.arg(2) as u32)
}

fn sys_faccessat2(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let flags = call.arg(3) as u32;
    if flags & !(AT_EACCESS | AT_SYMLINK_NOFOLLOW) != 0 {
        return -EINVAL;
    }
    sys_access_common(ctx, call.arg(0) as i32, call.arg(1), call.arg(2) as u32)
}

fn sys_chdir(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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
                (*ctx.current).user_cwd_inode = inode;
            }
            0
        }
        Ok(false) => -ENOTDIR,
        Err(err) => -fs_to_errno(err),
    }
}

fn sys_fchdir(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let inode = match resolve_dir_inode(ctx, call.arg(0) as i32) {
        Ok(inode) => inode,
        Err(err) => return err,
    };
    unsafe {
        (*ctx.current).user_cwd_inode = inode;
    }
    0
}

fn sys_getcwd(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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

fn sys_rename(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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

fn sys_link(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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

fn sys_gettimeofday(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let tv_ptr = call.arg(0);
    let _tz_ptr = call.arg(1);

    if tv_ptr != 0 {
        let ticks = pit::ticks();
        let (sec, nsec) = ticks_to_seconds_nanos(ticks);
        let tv = LinuxTimeVal {
            tv_sec: sec,
            tv_usec: nsec / 1_000,
        };
        if let Err(err) = write_struct_to_user(ctx.current, tv_ptr, &tv) {
            return -err;
        }
    }
    0
}

fn sys_clock_gettime(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let clock_id = call.arg(0) as i32;
    if clock_id != CLOCK_REALTIME && clock_id != CLOCK_MONOTONIC {
        return -EINVAL;
    }

    let ticks = pit::ticks();
    let (sec, nsec) = ticks_to_seconds_nanos(ticks);
    let ts = LinuxTimeSpec {
        tv_sec: sec,
        tv_nsec: nsec,
    };
    match write_struct_to_user(ctx.current, call.arg(1), &ts) {
        Ok(()) => 0,
        Err(err) => -err,
    }
}

fn sys_nanosleep(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let req = match read_struct_from_user::<LinuxTimeSpec>(ctx.current, call.arg(0)) {
        Ok(req) => req,
        Err(err) => return -err,
    };
    let ticks = match timespec_to_sleep_ticks(&req) {
        Ok(ticks) => ticks,
        Err(err) => return err,
    };
    if ticks > 0 {
        crate::sched::sleep(ticks);
    }

    if call.arg(1) != 0 {
        let rem = LinuxTimeSpec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        if let Err(err) = write_struct_to_user(ctx.current, call.arg(1), &rem) {
            return -err;
        }
    }
    0
}

#[inline]
fn log_stub_enosys(name: &str) {
    if cfg!(any(
        feature = "musl-ext2-cmd-smoke",
        feature = "musl-ext2-quickjs-smoke",
        feature = "musl-ext2-micropython-smoke",
        feature = "musl-ext2-dropbear-smoke"
    )) {
        sys_log!("[SYSCALL] {} -> -ENOSYS", name);
    }
}

fn sys_linkat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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

fn sys_symlinkat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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

fn sys_readlinkat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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

fn sys_readlink(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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

fn sys_fchmodat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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

fn sys_fchownat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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

fn sys_mknodat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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

fn sys_utimensat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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

fn sys_utimes(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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

fn sys_futimesat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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

fn sys_copy_file_range(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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
            (*ctx.current).user_fd_kind[in_idx],
            (*ctx.current).user_fd_inode[in_idx],
            (*ctx.current).user_fd_offset[in_idx],
            (*ctx.current).user_fd_flags[in_idx],
        )
    };
    let (out_kind, out_ino, out_fd_off, out_flags) = unsafe {
        (
            (*ctx.current).user_fd_kind[out_idx],
            (*ctx.current).user_fd_inode[out_idx],
            (*ctx.current).user_fd_offset[out_idx],
            (*ctx.current).user_fd_flags[out_idx],
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

        let write_res = match fs::write(out_ino, out_off, &buf[..read_n], false) {
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
            (*ctx.current).user_fd_offset[in_idx] = in_off;
        }
    } else if in_off <= i64::MAX as u64 {
        if let Err(err) = write_struct_to_user(ctx.current, in_off_ptr, &(in_off as i64)) {
            return -err;
        }
    }
    if out_off_ptr == 0 {
        unsafe {
            (*ctx.current).user_fd_offset[out_idx] = out_off;
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

fn read_sigaction_from_user(
    task_ptr: *mut task::Task,
    user_src: u64,
) -> Result<LinuxSigAction, i64> {
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

fn read_i32_from_user(task_ptr: *mut task::Task, user_src: u64) -> Result<i32, i64> {
    let mut raw = [0u8; 4];
    copy_from_user_task(task_ptr, user_src, &mut raw)?;
    Ok(i32::from_ne_bytes(raw))
}

fn write_i32_to_user(task_ptr: *mut task::Task, user_dst: u64, value: i32) -> Result<(), i64> {
    copy_to_user_task(task_ptr, user_dst, &value.to_ne_bytes())
}

#[inline]
fn wait_status_from_exit_code(code: i32) -> i32 {
    (code & 0xFF) << 8
}

fn default_termios() -> LinuxTermios {
    tty::default_termios()
}

fn process_group_exists_in_session(sid: i32, pgid: i32) -> bool {
    for idx in 0..task::MAX_TASKS {
        let task_ptr = task::get_by_index(idx);
        if task_ptr.is_null() {
            continue;
        }
        let (task_sid, task_pgid, state) =
            unsafe { ((*task_ptr).sid, (*task_ptr).pgid, (*task_ptr).state) };
        if state == TaskState::Dead {
            continue;
        }
        if task_sid == sid && task_pgid == pgid {
            return true;
        }
    }
    false
}

fn process_group_exists(pgid: i32) -> bool {
    for idx in 0..task::MAX_TASKS {
        let task_ptr = task::get_by_index(idx);
        if task_ptr.is_null() {
            continue;
        }
        let (task_pgid, state) = unsafe { ((*task_ptr).pgid, (*task_ptr).state) };
        if state == TaskState::Dead {
            continue;
        }
        if task_pgid == pgid {
            return true;
        }
    }
    false
}

fn ioctl_console(ctx: &SyscallContext, request: u64, argp: u64) -> i64 {
    match request {
        TIOCGWINSZ => {
            if argp == 0 {
                return -EFAULT;
            }
            let ws = LinuxWinSize {
                ws_row: 25,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            match write_struct_to_user(ctx.current, argp, &ws) {
                Ok(()) => 0,
                Err(err) => -err,
            }
        }
        TCGETS => {
            if argp == 0 {
                return -EFAULT;
            }
            let tio = tty::console_termios();
            match write_struct_to_user(ctx.current, argp, &tio) {
                Ok(()) => 0,
                Err(err) => -err,
            }
        }
        TCSETS | TCSETSW | TCSETSF => {
            if argp == 0 {
                return -EFAULT;
            }
            match read_struct_from_user::<LinuxTermios>(ctx.current, argp) {
                Ok(tio) => {
                    tty::set_console_termios(&tio);
                    0
                }
                Err(err) => -err,
            }
        }
        TIOCGPGRP => {
            if argp == 0 {
                return -EFAULT;
            }
            let pgrp = unsafe { (*ctx.current).pgid };
            match write_i32_to_user(ctx.current, argp, pgrp) {
                Ok(()) => 0,
                Err(err) => -err,
            }
        }
        TIOCSPGRP => {
            if argp == 0 {
                return -EFAULT;
            }
            let requested = match read_i32_from_user(ctx.current, argp) {
                Ok(value) => value,
                Err(err) => return -err,
            };
            if requested <= 0 {
                return -EINVAL;
            }
            let sid = unsafe { (*ctx.current).sid };
            if !process_group_exists_in_session(sid, requested) {
                return -EPERM;
            }
            0
        }
        _ => -ENOTTY,
    }
}

fn ioctl_pty(ctx: &SyscallContext, pty_id: u16, request: u64, argp: u64) -> i64 {
    match request {
        TIOCGWINSZ => {
            if argp == 0 {
                return -EFAULT;
            }
            let (rows, cols) = crate::pty::get_winsize(pty_id);
            let ws = LinuxWinSize {
                ws_row: rows,
                ws_col: cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            match write_struct_to_user(ctx.current, argp, &ws) {
                Ok(()) => 0,
                Err(err) => -err,
            }
        }
        TIOCSWINSZ => {
            if argp == 0 {
                return -EFAULT;
            }
            match read_struct_from_user::<LinuxWinSize>(ctx.current, argp) {
                Ok(ws) => {
                    crate::pty::set_winsize(pty_id, ws.ws_row, ws.ws_col);
                    0
                }
                Err(err) => -err,
            }
        }
        TCGETS => {
            if argp == 0 {
                return -EFAULT;
            }
            let tio = crate::pty::get_termios(pty_id);
            match write_struct_to_user(ctx.current, argp, &tio) {
                Ok(()) => 0,
                Err(err) => -err,
            }
        }
        TCSETS | TCSETSW | TCSETSF => {
            if argp == 0 {
                return -EFAULT;
            }
            match read_struct_from_user::<LinuxTermios>(ctx.current, argp) {
                Ok(tio) => {
                    crate::pty::set_termios(pty_id, &tio);
                    0
                }
                Err(err) => -err,
            }
        }
        TIOCSCTTY => {
            unsafe {
                (*ctx.current).ctty_type = task::CTTY_PTY;
                (*ctx.current).ctty_id = pty_id;
            }
            0
        }
        TIOCNOTTY => {
            unsafe {
                (*ctx.current).ctty_type = task::CTTY_NONE;
                (*ctx.current).ctty_id = 0;
            }
            0
        }
        TIOCGPGRP => {
            if argp == 0 {
                return -EFAULT;
            }
            let pgrp = unsafe { (*ctx.current).pgid };
            match write_i32_to_user(ctx.current, argp, pgrp) {
                Ok(()) => 0,
                Err(err) => -err,
            }
        }
        TIOCSPGRP => {
            if argp == 0 {
                return -EFAULT;
            }
            let requested = match read_i32_from_user(ctx.current, argp) {
                Ok(value) => value,
                Err(err) => return -err,
            };
            if requested <= 0 {
                return -EINVAL;
            }
            let sid = unsafe { (*ctx.current).sid };
            if !process_group_exists_in_session(sid, requested) {
                return -EPERM;
            }
            0
        }
        TIOCSPTLCK => {
            // Lock/unlock the slave side. argp points to an int: 0 = unlock, nonzero = lock.
            if argp == 0 {
                return -EFAULT;
            }
            let lock_val = match read_i32_from_user(ctx.current, argp) {
                Ok(v) => v,
                Err(err) => return -err,
            };
            if lock_val == 0 {
                crate::pty::unlock(pty_id);
            }
            // lock_val != 0: re-lock (not typically used, ignore for simplicity)
            0
        }
        TIOCGPTN => {
            // Return the PTY number (slave index)
            if argp == 0 {
                return -EFAULT;
            }
            match write_i32_to_user(ctx.current, argp, pty_id as i32) {
                Ok(()) => 0,
                Err(err) => -err,
            }
        }
        _ => -ENOTTY,
    }
}

fn ioctl_framebuffer(ctx: &SyscallContext, request: u64, argp: u64) -> i64 {
    match request {
        FBIOGETINFO => {
            if argp == 0 {
                return -EFAULT;
            }
            let info = match framebuffer::device_info() {
                Some(info) => info,
                None => return -ENXIO,
            };
            match write_struct_to_user(ctx.current, argp, &info) {
                Ok(()) => 0,
                Err(err) => -err,
            }
        }
        _ => -ENOTTY,
    }
}

fn fork_cleanup_child(task_ptr: *mut task::Task) {
    if task_ptr.is_null() {
        return;
    }
    let owner_pid = unsafe { (*task_ptr).pid };
    mmap_clear_task_regions(task_ptr);
    close_all_fds(task_ptr);
    epoll_drop_watches_for_owner(owner_pid);
    teardown_user_address_space(task_ptr);
    task::task_destroy(task_ptr);
}

fn clone_parent_fds_for_child(parent: *mut task::Task, child: *mut task::Task) -> Result<(), i64> {
    unsafe {
        (*child).user_fd_kind = (*parent).user_fd_kind;
        (*child).user_fd_inode = (*parent).user_fd_inode;
        (*child).user_fd_offset = (*parent).user_fd_offset;
        (*child).user_fd_flags = (*parent).user_fd_flags;
    }

    for idx in 0..task::MAX_TASK_FDS {
        let (kind, inode, flags) = unsafe {
            (
                (*child).user_fd_kind[idx],
                (*child).user_fd_inode[idx],
                (*child).user_fd_flags[idx],
            )
        };
        if kind == task::USER_FD_KIND_PIPE && inode != task::USER_FD_NONE {
            let retain_read = fd_can_read(flags);
            let retain_write = fd_can_write(flags);
            ipc::pipe_retain(inode, retain_read, retain_write).map_err(|err| -ipc_to_errno(err))?;
        }
        if kind == task::USER_FD_KIND_PTY_MASTER && inode != task::USER_FD_NONE {
            crate::pty::master_retain(inode);
        }
        if kind == task::USER_FD_KIND_PTY_SLAVE && inode != task::USER_FD_NONE {
            crate::pty::slave_retain(inode);
        }
        if kind == task::USER_FD_KIND_SOCKET && inode != task::USER_FD_NONE {
            socket::socket_retain(inode)?;
        }
        if kind == task::USER_FD_KIND_EPOLL && inode != task::USER_FD_NONE {
            epoll_retain(inode)?;
        }
    }

    Ok(())
}

fn create_fork_child(ctx: &SyscallContext, child_frame: &InterruptFrame) -> Result<i64, i64> {
    let parent = ctx.current;
    if parent.is_null() {
        return Err(-ESRCH);
    }
    let parent_pml4 = unsafe { PhysAddr::new((*parent).pml4_phys) };
    if parent_pml4.as_u64() == 0 {
        return Err(-EINVAL);
    }

    let child = task::task_alloc("fork", TASK_FLAG_USER);
    if child.is_null() {
        return Err(-ENOMEM);
    }

    let child_pml4 = vmm::create_address_space();
    if child_pml4.as_u64() == 0 {
        task::task_destroy(child);
        return Err(-ENOMEM);
    }
    if vmm::clone_user_space(parent_pml4, child_pml4) != 0 {
        vmm::clear_user_space(child_pml4);
        vmm::destroy_address_space(child_pml4);
        task::task_destroy(child);
        return Err(-ENOMEM);
    }

    unsafe {
        (*child).pml4_phys = child_pml4.as_u64();

        (*child).user_entry = (*parent).user_entry;
        (*child).user_stack_bottom = (*parent).user_stack_bottom;
        (*child).user_stack_top = (*parent).user_stack_top;
        (*child).user_stack_ptr = (*parent).user_stack_ptr;
        (*child).user_argc = (*parent).user_argc;
        (*child).user_argv = (*parent).user_argv;
        (*child).user_envp = (*parent).user_envp;
        (*child).user_auxv = (*parent).user_auxv;
        (*child).user_fs_base = (*parent).user_fs_base;
        (*child).user_clear_tid = 0;
        (*child).user_brk_start = (*parent).user_brk_start;
        (*child).user_brk = (*parent).user_brk;
        (*child).user_phdr_addr = (*parent).user_phdr_addr;
        (*child).user_phdr_count = (*parent).user_phdr_count;
        (*child).user_phdr_size = (*parent).user_phdr_size;
        (*child).user_base_addr = (*parent).user_base_addr;
        (*child).user_interp_base = (*parent).user_interp_base;
        (*child).user_is_dynamic = (*parent).user_is_dynamic;
        (*child).user_uid = (*parent).user_uid;
        (*child).user_gid = (*parent).user_gid;
        (*child).user_cwd_inode = (*parent).user_cwd_inode;
        (*child).pgid = (*parent).pgid;
        (*child).sid = (*parent).sid;
        (*child).ctty_type = (*parent).ctty_type;
        (*child).ctty_id = (*parent).ctty_id;
        (*child).signal_handlers = (*parent).signal_handlers;
        (*child).signal_flags = (*parent).signal_flags;
        (*child).signal_mask_saved = (*parent).signal_mask_saved;
        (*child).signal_restorer = (*parent).signal_restorer;
        (*child).signal_blocked = (*parent).signal_blocked;
        (*child).sig_pending = 0;
        (*child).user_shm_id = [0; task::MAX_TASK_SHM_ATTACH];
        (*child).user_shm_addr = [0; task::MAX_TASK_SHM_ATTACH];
        (*child).user_shm_size = [0; task::MAX_TASK_SHM_ATTACH];
        (*child).user_shm_flags = [0; task::MAX_TASK_SHM_ATTACH];
    }

    mmap_clone_task_regions(parent, child)?;
    mmap_fork_reconcile_shared_file_mappings(child)?;

    if let Err(err) = clone_parent_fds_for_child(parent, child) {
        fork_cleanup_child(child);
        return Err(err);
    }

    unsafe {
        let mut frame_addr = (*child).kernel_stack_top - size_of::<InterruptFrame>() as u64;
        frame_addr &= !0xF;
        let frame_ptr = frame_addr as *mut InterruptFrame;
        ptr::write(frame_ptr, *child_frame);
        (*child).saved_rsp = frame_addr;
        (*child).state = TaskState::Ready;
    }

    crate::sched::add_task(child);
    Ok(unsafe { (*child).pid as i64 })
}

fn build_child_frame_from_syscall_frame(frame: &SyscallFrame) -> InterruptFrame {
    InterruptFrame {
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
        rax: 0,
        int_no: 0,
        error_code: 0,
        rip: frame.rcx,
        cs: gdt::USER_CS as u64,
        rflags: frame.r11,
        rsp: frame.rsp,
        ss: gdt::USER_DS as u64,
    }
}

fn build_child_frame_from_int80_frame(frame: &InterruptFrame) -> InterruptFrame {
    let mut child = *frame;
    child.rax = 0;
    child.int_no = 0;
    child.error_code = 0;
    child
}

fn validate_clone_args(call: &SyscallArgs) -> Result<u64, i64> {
    let flags = call.arg(0);
    let child_stack = call.arg(1);
    let parent_tid = call.arg(2);
    let child_tid = call.arg(3);
    let tls = call.arg(4);

    if (flags & !(CLONE_CSIGNAL | CLONE_VM | CLONE_VFORK)) != 0 {
        return Err(-EINVAL);
    }
    if (flags & CLONE_CSIGNAL) != SIGCHLD_EXIT_SIGNAL {
        return Err(-EINVAL);
    }
    let clone_vm = (flags & CLONE_VM) != 0;
    let clone_vfork = (flags & CLONE_VFORK) != 0;
    if clone_vm != clone_vfork {
        return Err(-EINVAL);
    }
    if parent_tid != 0 || child_tid != 0 || tls != 0 {
        return Err(-EINVAL);
    }
    if child_stack != 0 && !user_range_ok(child_stack, 1) {
        return Err(-EFAULT);
    }
    Ok(child_stack)
}

fn build_clone_child_frame_from_syscall(
    frame: &SyscallFrame,
    child_stack: u64,
) -> Result<InterruptFrame, i64> {
    let mut child = build_child_frame_from_syscall_frame(frame);
    if child_stack != 0 {
        child.rsp = child_stack;
    }
    Ok(child)
}

fn build_clone_child_frame_from_int80(
    frame: &InterruptFrame,
    child_stack: u64,
) -> Result<InterruptFrame, i64> {
    let mut child = build_child_frame_from_int80_frame(frame);
    if child_stack != 0 {
        child.rsp = child_stack;
    }
    Ok(child)
}

fn sys_pipe2(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let user_pipefd = call.arg(0);
    let flags = call.arg(1) as u32;
    if (flags & !(O_CLOEXEC | O_NONBLOCK)) != 0 {
        return -EINVAL;
    }
    let mut read_open_flags = fs::O_RDONLY;
    let mut write_open_flags = fs::O_WRONLY;
    if (flags & O_NONBLOCK) != 0 {
        read_open_flags |= O_NONBLOCK;
        write_open_flags |= O_NONBLOCK;
    }
    if (flags & O_CLOEXEC) != 0 {
        read_open_flags |= O_CLOEXEC;
        write_open_flags |= O_CLOEXEC;
    }
    let read_flags = fd_flags_from_open_flags(read_open_flags);
    let write_flags = fd_flags_from_open_flags(write_open_flags);

    let pipe_id = match ipc::pipe_open() {
        Ok(id) => id,
        Err(err) => return -ipc_to_errno(err),
    };

    let read_fd = match fd_alloc(ctx.current, task::USER_FD_KIND_PIPE, pipe_id, read_flags) {
        Ok(fd) => fd as i32,
        Err(err) => {
            let _ = ipc::pipe_close(pipe_id, true, true);
            return err;
        }
    };
    let write_fd = match fd_alloc(ctx.current, task::USER_FD_KIND_PIPE, pipe_id, write_flags) {
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

fn stat_path(
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

fn sys_stat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let stat = match stat_path(ctx, call.arg(0), true) {
        Ok(stat) => stat,
        Err(err) => return err,
    };
    write_stat_to_user(ctx.current, call.arg(1), stat)
}

fn sys_lstat(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let stat = match stat_path(ctx, call.arg(0), false) {
        Ok(stat) => stat,
        Err(err) => return err,
    };
    write_stat_to_user(ctx.current, call.arg(1), stat)
}

fn sys_uname(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let mut uts = LinuxUtsName {
        sysname: [0; UTS_FIELD_LEN],
        nodename: [0; UTS_FIELD_LEN],
        release: [0; UTS_FIELD_LEN],
        version: [0; UTS_FIELD_LEN],
        machine: [0; UTS_FIELD_LEN],
        domainname: [0; UTS_FIELD_LEN],
    };
    write_uts_field(&mut uts.sysname, b"PeetOS");
    write_uts_field(&mut uts.nodename, b"peetos");
    write_uts_field(&mut uts.release, b"0.1.0");
    write_uts_field(&mut uts.version, b"#1");
    write_uts_field(&mut uts.machine, b"x86_64");
    write_uts_field(&mut uts.domainname, b"localdomain");

    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&uts as *const LinuxUtsName) as *const u8,
            core::mem::size_of::<LinuxUtsName>(),
        )
    };
    match copy_to_user_task(ctx.current, call.arg(0), bytes) {
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
        Ok(()) => {
            sync_ext2_backing();
            0
        }
        Err(err) => -fs_to_errno(err),
    }
}

fn sys_unlink(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let compat = SyscallArgs {
        num: SYS_UNLINKAT,
        args: [fs::AT_FDCWD as i64 as u64, call.arg(0), 0, 0, 0, 0],
    };
    sys_unlinkat(ctx, &compat)
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

fn sys_close(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    match fd_close(ctx.current, call.arg(0) as i32) {
        Ok(()) => 0,
        Err(err) => err,
    }
}

fn parse_dup_min_fd(arg: u64) -> Result<usize, i64> {
    let min = i64::try_from(arg).map_err(|_| -EINVAL)?;
    if min < 0 {
        return Err(-EINVAL);
    }
    let min = min as usize;
    if min >= task::MAX_TASK_FDS {
        return Err(-EINVAL);
    }
    Ok(min)
}

fn sys_dup(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    match fd_dup_from(ctx.current, call.arg(0) as i32, 0, false) {
        Ok(fd) => fd,
        Err(err) => err,
    }
}

fn sys_dup2(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    match fd_dup_to(
        ctx.current,
        call.arg(0) as i32,
        call.arg(1) as i32,
        false,
        false,
    ) {
        Ok(fd) => fd,
        Err(err) => err,
    }
}

fn sys_dup3(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let flags = call.arg(2) as u32;
    if (flags & !O_CLOEXEC) != 0 {
        return -EINVAL;
    }
    let cloexec = (flags & O_CLOEXEC) != 0;
    match fd_dup_to(
        ctx.current,
        call.arg(0) as i32,
        call.arg(1) as i32,
        cloexec,
        true,
    ) {
        Ok(fd) => fd,
        Err(err) => err,
    }
}

fn sys_fcntl(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let cmd = call.arg(1) as u32;
    let arg = call.arg(2);
    let idx = match fd_index(fd) {
        Ok(idx) => idx,
        Err(err) => return err,
    };
    if !fd_slot_is_open(ctx.current, idx) {
        return -EBADF;
    }

    match cmd {
        F_GETFD => {
            let flags = unsafe { (*ctx.current).user_fd_flags[idx] };
            if fd_has_cloexec(flags) {
                FD_CLOEXEC as i64
            } else {
                0
            }
        }
        F_SETFD => {
            let cloexec = (arg as u32 & FD_CLOEXEC) != 0;
            unsafe {
                let flags = (*ctx.current).user_fd_flags[idx];
                (*ctx.current).user_fd_flags[idx] = fd_set_cloexec(flags, cloexec);
            }
            0
        }
        F_GETFL => {
            let flags = unsafe { (*ctx.current).user_fd_flags[idx] };
            fd_open_flags(flags) as i64
        }
        F_SETFL => {
            unsafe {
                let current_flags = (*ctx.current).user_fd_flags[idx];
                let kind = (*ctx.current).user_fd_kind[idx];
                let next_flags = fd_apply_fcntl_setfl(current_flags, kind, arg as u32);
                (*ctx.current).user_fd_flags[idx] = next_flags;
            }
            0
        }
        F_DUPFD => {
            let min_fd = match parse_dup_min_fd(arg) {
                Ok(v) => v,
                Err(err) => return err,
            };
            match fd_dup_from(ctx.current, fd, min_fd, false) {
                Ok(new_fd) => new_fd,
                Err(err) => err,
            }
        }
        F_DUPFD_CLOEXEC => {
            let min_fd = match parse_dup_min_fd(arg) {
                Ok(v) => v,
                Err(err) => return err,
            };
            match fd_dup_from(ctx.current, fd, min_fd, true) {
                Ok(new_fd) => new_fd,
                Err(err) => err,
            }
        }
        _ => -EINVAL,
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
            (*ctx.current).user_fd_offset[idx] = offset.saturating_add(read_n as u64);
        }
        read_total += read_n;

        if read_n < take {
            break;
        }
    }

    read_total as i64
}

fn write_from_user_fd(
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
                (*ctx.current).user_fd_kind[idx],
                (*ctx.current).user_fd_inode[idx],
                (*ctx.current).user_fd_offset[idx],
                (*ctx.current).user_fd_flags[idx],
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
                (*ctx.current).user_fd_offset[idx] = offset.saturating_add(take as u64);
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
            let write_res = match fs::write(inode, offset, &chunk[..take], append) {
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
                (*ctx.current).user_fd_offset[idx] = write_res.new_offset;
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
                (*ctx.current).user_fd_offset[idx] = offset.saturating_add(wrote as u64);
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

fn sys_writev(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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

fn sys_ioctl(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let idx = match fd_index(call.arg(0) as i32) {
        Ok(idx) => idx,
        Err(err) => return err,
    };
    if !fd_slot_is_open(ctx.current, idx) {
        return -EBADF;
    }

    let kind = unsafe { (*ctx.current).user_fd_kind[idx] };
    let inode = unsafe { (*ctx.current).user_fd_inode[idx] };
    let request = call.arg(1);
    let argp = call.arg(2);
    if kind == task::USER_FD_KIND_CONSOLE {
        ioctl_console(ctx, request, argp)
    } else if kind == task::USER_FD_KIND_PTY_MASTER || kind == task::USER_FD_KIND_PTY_SLAVE {
        ioctl_pty(ctx, inode, request, argp)
    } else if kind == task::USER_FD_KIND_VFS && fs::inode_is_framebuffer(inode) {
        ioctl_framebuffer(ctx, request, argp)
    } else {
        -ENOTTY
    }
}

fn child_pid_matches(wait_pid: i64, child_pid: i32) -> bool {
    if wait_pid == -1 {
        return true;
    }
    if wait_pid > 0 {
        return child_pid == wait_pid as i32;
    }
    false
}

fn find_waitable_child(
    parent_pid: i32,
    wait_pid: i64,
    found_match: &mut bool,
) -> Option<*mut task::Task> {
    for idx in 0..task::MAX_TASKS {
        let child = task::get_by_index(idx);
        if child.is_null() {
            continue;
        }
        let (child_ppid, child_pid, child_state) =
            unsafe { ((*child).ppid, (*child).pid, (*child).state) };
        if child_ppid != parent_pid {
            continue;
        }
        if !child_pid_matches(wait_pid, child_pid) {
            continue;
        }
        *found_match = true;
        if child_state == TaskState::Zombie {
            return Some(child);
        }
    }
    None
}

fn sys_wait4(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let wait_pid = call.arg(0) as i64;
    let user_status = call.arg(1);
    let options = call.arg(2) as u32;
    let user_rusage = call.arg(3);

    if wait_pid < -1 || wait_pid == 0 {
        return -EINVAL;
    }
    if (options & !WNOHANG) != 0 {
        return -EINVAL;
    }
    if user_rusage != 0 {
        return -ENOSYS;
    }
    if ctx.current.is_null() {
        return -ESRCH;
    }

    let parent_pid = unsafe { (*ctx.current).pid };
    loop {
        let mut found_match = false;
        let child = find_waitable_child(parent_pid, wait_pid, &mut found_match);
        if let Some(child_task) = child {
            let (pid, exit_code, child_pml4) = unsafe {
                (
                    (*child_task).pid,
                    (*child_task).exit_code,
                    PhysAddr::new((*child_task).pml4_phys),
                )
            };
            let status = wait_status_from_exit_code(exit_code);

            if user_status != 0 {
                if let Err(err) = write_i32_to_user(ctx.current, user_status, status) {
                    return -err;
                }
            }

            if child_pml4.as_u64() != 0 {
                vmm::clear_user_space(child_pml4);
                vmm::destroy_address_space(child_pml4);
                unsafe {
                    (*child_task).pml4_phys = 0;
                }
            }
            task::task_destroy(child_task);
            return pid as i64;
        }

        if !found_match {
            return -ECHILD;
        }
        if (options & WNOHANG) != 0 {
            return 0;
        }
        if !crate::sched::is_running() {
            return -EAGAIN;
        }
        crate::sched::yield_now();
    }
}

fn sys_fork(_ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    // fork needs direct trap-frame access and is handled in entry stubs.
    -ENOSYS
}

fn sys_clone(_ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    // clone needs direct trap-frame access and is handled in entry stubs.
    -ENOSYS
}

fn sys_vfork(_ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    // vfork is handled in entry stubs as a clone/fork compatibility path.
    -ENOSYS
}

fn sys_getpid(ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    match ctx.pid() {
        Ok(pid) => pid,
        Err(err) => err,
    }
}

fn pid_from_syscall_arg_or_current(raw: u64, current_pid: i32) -> Result<i32, i64> {
    let pid = raw as i64;
    if pid < 0 || pid > i32::MAX as i64 {
        return Err(-EINVAL);
    }
    if pid == 0 {
        return Ok(current_pid);
    }
    Ok(pid as i32)
}

fn pgid_from_syscall_arg_or_target(raw: u64, target_pid: i32) -> Result<i32, i64> {
    let pgid = raw as i64;
    if pgid < 0 || pgid > i32::MAX as i64 {
        return Err(-EINVAL);
    }
    if pgid == 0 {
        return Ok(target_pid);
    }
    Ok(pgid as i32)
}

fn lookup_user_task(pid: i32) -> *mut task::Task {
    let task_ptr = task::get_by_pid(pid);
    if task_ptr.is_null() {
        return ptr::null_mut();
    }
    let is_user = unsafe { ((*task_ptr).flags & TASK_FLAG_USER) != 0 };
    if !is_user {
        return ptr::null_mut();
    }
    task_ptr
}

fn sys_setpgid(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    if !ctx.has_task() {
        return -ESRCH;
    }

    let caller_pid = unsafe { (*ctx.current).pid };
    let caller_sid = unsafe { (*ctx.current).sid };

    let target_pid = match pid_from_syscall_arg_or_current(call.arg(0), caller_pid) {
        Ok(pid) => pid,
        Err(err) => return err,
    };
    let new_pgid = match pgid_from_syscall_arg_or_target(call.arg(1), target_pid) {
        Ok(pgid) => pgid,
        Err(err) => return err,
    };
    if new_pgid <= 0 {
        return -EINVAL;
    }

    let target = lookup_user_task(target_pid);
    if target.is_null() {
        return -ESRCH;
    }

    let (target_pid_value, target_ppid, target_sid, target_state) = unsafe {
        (
            (*target).pid,
            (*target).ppid,
            (*target).sid,
            (*target).state,
        )
    };

    if target_state == TaskState::Zombie || target_state == TaskState::Dead {
        return -ESRCH;
    }
    if target_pid_value != caller_pid && target_ppid != caller_pid {
        return -ESRCH;
    }
    if target_sid != caller_sid {
        return -EPERM;
    }
    if target_pid_value == target_sid {
        return -EPERM;
    }
    if new_pgid != target_pid_value && !process_group_exists_in_session(target_sid, new_pgid) {
        return -EPERM;
    }

    unsafe {
        (*target).pgid = new_pgid;
    }
    0
}

fn sys_setsid(ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    if !ctx.has_task() {
        return -ESRCH;
    }

    let pid = unsafe { (*ctx.current).pid };
    let pgid = unsafe { (*ctx.current).pgid };
    if pgid == pid || process_group_exists(pid) {
        return -EPERM;
    }

    unsafe {
        (*ctx.current).sid = pid;
        (*ctx.current).pgid = pid;
        (*ctx.current).ctty_type = task::CTTY_NONE;
        (*ctx.current).ctty_id = 0;
    }
    pid as i64
}

fn sys_getsid(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    if !ctx.has_task() {
        return -ESRCH;
    }

    let caller_pid = unsafe { (*ctx.current).pid };
    let target_pid = match pid_from_syscall_arg_or_current(call.arg(0), caller_pid) {
        Ok(pid) => pid,
        Err(err) => return err,
    };
    let target = lookup_user_task(target_pid);
    if target.is_null() {
        return -ESRCH;
    }
    unsafe { (*target).sid as i64 }
}

fn sys_getuid(ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    if !ctx.has_task() {
        return -ESRCH;
    }
    unsafe { (*ctx.current).user_uid as i64 }
}

fn sys_geteuid(ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    sys_getuid(ctx, _call)
}

fn sys_getgid(ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    if !ctx.has_task() {
        return -ESRCH;
    }
    unsafe { (*ctx.current).user_gid as i64 }
}

fn sys_getegid(ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    sys_getgid(ctx, _call)
}

fn sys_umask(_ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let new_mask = (call.arg(0) as u32) & 0o777;
    let old = PROCESS_UMASK.swap(new_mask, Ordering::Relaxed);
    old as i64
}

fn sys_execve(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    match execve_common(ctx, call.arg(0), call.arg(1), call.arg(2)) {
        Ok(_) => 0,
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

fn sys_arch_prctl(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let code = call.arg(0);
    let addr = call.arg(1);

    match code {
        ARCH_SET_FS => {
            if !arch_prctl_set_fs_addr_ok(addr) {
                return -EINVAL;
            }

            unsafe {
                (*ctx.current).user_fs_base = addr;
            }
            arch_syscall::set_user_fs_base(addr);
            0
        }
        ARCH_GET_FS => {
            if !user_range_ok(addr, size_of::<u64>()) {
                return -EFAULT;
            }

            let fs_base = unsafe { (*ctx.current).user_fs_base };
            match write_u64_to_user(ctx.current, addr, fs_base) {
                Ok(()) => 0,
                Err(err) => -err,
            }
        }
        ARCH_SET_GS | ARCH_GET_GS => -EINVAL,
        _ => -EINVAL,
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

fn teardown_user_address_space(task_ptr: *mut task::Task) {
    if task_ptr.is_null() {
        return;
    }
    let pml4 = unsafe { PhysAddr::new((*task_ptr).pml4_phys) };
    if pml4.as_u64() == 0 {
        return;
    }
    vmm::clear_user_space(pml4);
    vmm::destroy_address_space(pml4);
    unsafe {
        (*task_ptr).pml4_phys = 0;
    }
}

fn sys_exit_common(ctx: &SyscallContext, code: i32) -> i64 {
    if !ctx.has_task() {
        return -ESRCH;
    }

    let owner_pid = unsafe { (*ctx.current).pid };
    clear_child_tid(ctx);
    mmap_clear_task_regions(ctx.current);
    close_all_fds(ctx.current);
    epoll_drop_watches_for_owner(owner_pid);
    shm::cleanup_task(ctx.current);
    teardown_user_address_space(ctx.current);
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

// ---------------------------------------------------------------------------
// ftruncate (77)
// ---------------------------------------------------------------------------
fn sys_ftruncate(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let length = call.arg(1);

    let idx = match fd_index(fd) {
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
    if kind != task::USER_FD_KIND_VFS {
        return -EBADF;
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
// getrandom (318)
// ---------------------------------------------------------------------------
fn sys_getrandom(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let buf_ptr = call.arg(0);
    let buflen = call.arg(1) as usize;
    let _flags = call.arg(2);

    if buf_ptr == 0 {
        return -EFAULT;
    }

    let cap = cmp::min(buflen, 256);
    let mut tmp = [0u8; 256];

    let mut state = 0x9E37_79B9_7F4A_7C15u64 ^ pit::ticks().wrapping_mul(0xA076_1D64_78BD_642F);
    let pid = if ctx.has_task() {
        unsafe { (*ctx.current).pid as u64 }
    } else {
        0
    };
    state ^= pid.wrapping_mul(0x517C_C1B7_2722_0A95);
    if state == 0 {
        state = 1;
    }

    for chunk in tmp[..cap].chunks_mut(8) {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let sample = state.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes();
        chunk.copy_from_slice(&sample[..chunk.len()]);
    }

    match copy_to_user_task(ctx.current, buf_ptr, &tmp[..cap]) {
        Ok(()) => cap as i64,
        Err(err) => -err,
    }
}

// ---------------------------------------------------------------------------
// sysinfo (99)
// ---------------------------------------------------------------------------
#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxSysinfo {
    uptime: i64,
    loads: [u64; 3],
    totalram: u64,
    freeram: u64,
    sharedram: u64,
    bufferram: u64,
    totalswap: u64,
    freeswap: u64,
    procs: u16,
    pad: u16,
    pad2: u32,
    totalhigh: u64,
    freehigh: u64,
    mem_unit: u32,
    _padding: [u8; 4],
}

fn sys_sysinfo(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let out_ptr = call.arg(0);
    if out_ptr == 0 {
        return -EFAULT;
    }

    let pmm = pmm::get_stats();

    let info = LinuxSysinfo {
        uptime: pit::uptime_seconds() as i64,
        loads: [0; 3],
        totalram: pmm.total_bytes,
        freeram: pmm.free_bytes,
        sharedram: 0,
        bufferram: 0,
        totalswap: 0,
        freeswap: 0,
        procs: 1,
        pad: 0,
        pad2: 0,
        totalhigh: 0,
        freehigh: 0,
        mem_unit: 1,
        _padding: [0; 4],
    };

    match write_struct_to_user(ctx.current, out_ptr, &info) {
        Ok(()) => 0,
        Err(err) => -err,
    }
}

// ---------------------------------------------------------------------------
// prlimit64 (302) — stub returning success
// ---------------------------------------------------------------------------
fn sys_prlimit64(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let new_limit_ptr = call.arg(2);
    let old_limit_ptr = call.arg(3);

    // If old_limit_ptr is set, write a zeroed rlimit (cur=RLIM_INFINITY, max=RLIM_INFINITY).
    if old_limit_ptr != 0 {
        let rlimit = [u64::MAX, u64::MAX]; // [rlim_cur, rlim_max]
        if let Err(err) = write_struct_to_user(ctx.current, old_limit_ptr, &rlimit) {
            return -err;
        }
    }
    let _ = new_limit_ptr;
    0
}

// ---------------------------------------------------------------------------
// getrusage (98) — stub returning zeroed struct
// ---------------------------------------------------------------------------
fn sys_getrusage(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let _who = call.arg(0) as i32;
    let usage_ptr = call.arg(1);

    if usage_ptr == 0 {
        return -EFAULT;
    }

    // struct rusage is 144 bytes on x86-64.
    let zeroed = [0u8; 144];
    match copy_to_user_task(ctx.current, usage_ptr, &zeroed) {
        Ok(()) => 0,
        Err(err) => -err,
    }
}

// ---------------------------------------------------------------------------
// statfs (137) / fstatfs (138)
// ---------------------------------------------------------------------------
#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxStatfs {
    f_type: i64,
    f_bsize: i64,
    f_blocks: i64,
    f_bfree: i64,
    f_bavail: i64,
    f_files: i64,
    f_ffree: i64,
    f_fsid: [i32; 2],
    f_namelen: i64,
    f_frsize: i64,
    f_flags: i64,
    f_spare: [i64; 4],
}

const TMPFS_MAGIC: i64 = 0x01021994;
const EXT2_SUPER_MAGIC_STATFS: i64 = 0xEF53;

fn fill_statfs_for_inode(inode: u16) -> LinuxStatfs {
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

fn sys_statfs(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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

fn sys_fstatfs(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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
            (*ctx.current).user_fd_kind[idx],
            (*ctx.current).user_fd_inode[idx],
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
fn sys_fchmod(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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
            (*ctx.current).user_fd_kind[idx],
            (*ctx.current).user_fd_inode[idx],
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
fn sys_fchown(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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
            (*ctx.current).user_fd_kind[idx],
            (*ctx.current).user_fd_inode[idx],
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
fn sys_truncate(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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
// msync (26)
// ---------------------------------------------------------------------------
fn sys_msync(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let addr = call.arg(0);
    let length = call.arg(1);
    let flags = call.arg(2) as u32;

    if length == 0 {
        return 0;
    }
    if (addr & (PAGE_SIZE - 1)) != 0 || addr < MMAP_BASE_ADDR {
        return -EINVAL;
    }
    if !msync_flags_are_valid(flags) {
        return -EINVAL;
    }

    let len = match page_align_up(length) {
        Some(v) if v > 0 => v,
        _ => return -EINVAL,
    };
    let end = match addr.checked_add(len) {
        Some(v) => v,
        None => return -EINVAL,
    };
    if end >= KERNEL_VIRT_BASE {
        return -EINVAL;
    }

    let slot = match mmap_prepare_task_slot(ctx.current) {
        Some(slot) => slot,
        None => return -ESRCH,
    };

    if unsafe { !mmap_regions_range_mapped(&TASK_VM_MAPPINGS[slot], addr, end) } {
        return -ENOMEM;
    }

    let mut page = addr;
    while page < end {
        let mapping = unsafe { mmap_regions_find_mapping(&TASK_VM_MAPPINGS[slot], page) };
        let mapping = match mapping {
            Some(mapping) => mapping,
            None => return -ENOMEM,
        };

        if mapping.map_shared
            && mapping.file_inode != task::USER_FD_NONE
            && (mapping.prot & PROT_WRITE) != 0
        {
            let map_delta = match page.checked_sub(mapping.start) {
                Some(v) => v,
                None => return -EINVAL,
            };
            let file_off = match mapping.file_offset.checked_add(map_delta) {
                Some(v) => v,
                None => return -EINVAL,
            };
            let page_off = align_down(file_off, PAGE_SIZE);
            if let Err(err) = vm_page_cache_writeback_page(mapping.file_inode, page_off) {
                return err;
            }
        }

        page = page.saturating_add(PAGE_SIZE);
    }

    sync_ext2_backing();
    flush_ext2_backing();
    0
}

// ---------------------------------------------------------------------------
// sync (162)
// ---------------------------------------------------------------------------
fn sys_sync(_ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    if let Err(err) = vm_page_cache_writeback_all() {
        return err;
    }
    sync_ext2_backing();
    flush_ext2_backing();
    0
}

fn sys_fsync(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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
            (*ctx.current).user_fd_kind[idx],
            (*ctx.current).user_fd_inode[idx],
        )
    };
    if kind == task::USER_FD_KIND_VFS {
        if let Err(err) = vm_page_cache_writeback_inode(inode) {
            return err;
        }
        sync_ext2_backing();
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

// ---------------------------------------------------------------------------
// pread64 (17)
// ---------------------------------------------------------------------------
fn sys_pread64(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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
            (*ctx.current).user_fd_kind[idx],
            (*ctx.current).user_fd_inode[idx],
            (*ctx.current).user_fd_flags[idx],
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
fn sys_pwrite64(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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
            (*ctx.current).user_fd_kind[idx],
            (*ctx.current).user_fd_inode[idx],
            (*ctx.current).user_fd_flags[idx],
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

        let write_res = match fs::write(inode, pos, &chunk[..take], false) {
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
fn sys_readv(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
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

// ---------------------------------------------------------------------------
// sched_getaffinity (204)
// ---------------------------------------------------------------------------
fn sys_sched_getaffinity(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let _pid = call.arg(0) as i32;
    let cpusetsize = call.arg(1) as usize;
    let mask_ptr = call.arg(2);

    if mask_ptr == 0 || cpusetsize == 0 {
        return -EINVAL;
    }

    // Single-CPU stub: set bit 0 in the affinity mask.
    let write_len = cmp::min(cpusetsize, 128);
    let mut mask = [0u8; 128];
    mask[0] = 1; // CPU 0 present

    match copy_to_user_task(ctx.current, mask_ptr, &mask[..write_len]) {
        Ok(()) => write_len as i64,
        Err(err) => -err,
    }
}

// ---------------------------------------------------------------------------
// getpgid (121)
// ---------------------------------------------------------------------------
fn sys_getpgid(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let pid = call.arg(0) as i32;

    if pid < 0 {
        return -EINVAL;
    }

    if pid == 0 {
        // Return current task's pgid.
        let pgid = unsafe { (*ctx.current).pgid };
        return pgid as i64;
    }

    // For pid > 0, look up that task's pgid.
    let target = task::get_by_pid(pid);
    if target.is_null() {
        return -ESRCH;
    }
    let pgid = unsafe { (*target).pgid };
    pgid as i64
}

// ---------------------------------------------------------------------------
// Socket syscall helpers
// ---------------------------------------------------------------------------

/// Parse a sockaddr_in from user memory. Layout: {family: u16, port: u16 (net order), addr: [u8;4], zero: [u8;8]}
fn parse_sockaddr_in(
    task_ptr: *mut task::Task,
    addr_ptr: u64,
    addr_len: usize,
) -> Result<(net::Ipv4Addr, u16), i64> {
    if addr_len < SOCKADDR_IN_SIZE {
        return Err(-EINVAL);
    }
    let mut buf = [0u8; SOCKADDR_IN_SIZE];
    copy_from_user_task(task_ptr, addr_ptr, &mut buf)?;
    let family = (buf[0] as u16) | ((buf[1] as u16) << 8);
    if family != AF_INET as u16 {
        return Err(-EAFNOSUPPORT);
    }
    // Port is in network byte order (big endian)
    let port = ((buf[2] as u16) << 8) | (buf[3] as u16);
    let ip = net::Ipv4Addr::new(buf[4], buf[5], buf[6], buf[7]);
    Ok((ip, port))
}

/// Parse a sockaddr_un from user memory. Layout: {family: u16, sun_path: [u8;108]}
fn parse_sockaddr_un(
    task_ptr: *mut task::Task,
    addr_ptr: u64,
    addr_len: usize,
) -> Result<([u8; SOCKADDR_UN_PATH_MAX], usize), i64> {
    if addr_len < SOCKADDR_UN_FAMILY_SIZE {
        return Err(-EINVAL);
    }

    let copy_len = addr_len.min(SOCKADDR_UN_SIZE);
    let mut buf = [0u8; SOCKADDR_UN_SIZE];
    copy_from_user_task(task_ptr, addr_ptr, &mut buf[..copy_len])?;

    let family = (buf[0] as u16) | ((buf[1] as u16) << 8);
    if family != AF_UNIX as u16 {
        return Err(-EAFNOSUPPORT);
    }
    if copy_len <= SOCKADDR_UN_FAMILY_SIZE {
        return Err(-EINVAL);
    }

    let mut path_len = copy_len - SOCKADDR_UN_FAMILY_SIZE;
    while path_len > 0 && buf[SOCKADDR_UN_FAMILY_SIZE + path_len - 1] == 0 {
        path_len -= 1;
    }
    if path_len == 0 {
        return Err(-EINVAL);
    }
    if buf[SOCKADDR_UN_FAMILY_SIZE] == 0 {
        // Abstract namespace sockets are deferred for now.
        return Err(-EOPNOTSUPP);
    }

    let mut path = [0u8; SOCKADDR_UN_PATH_MAX];
    path[..path_len]
        .copy_from_slice(&buf[SOCKADDR_UN_FAMILY_SIZE..SOCKADDR_UN_FAMILY_SIZE + path_len]);
    Ok((path, path_len))
}

/// Write a sockaddr_in to user memory.
fn write_sockaddr_in(
    task_ptr: *mut task::Task,
    addr_ptr: u64,
    addrlen_ptr: u64,
    ip: &net::Ipv4Addr,
    port: u16,
) -> Result<(), i64> {
    if addr_ptr == 0 {
        return Ok(());
    }
    let mut buf = [0u8; SOCKADDR_IN_SIZE];
    buf[0] = AF_INET as u8;
    buf[1] = 0; // AF_INET fits in one byte on little-endian
                // Port in network byte order (big endian)
    buf[2] = (port >> 8) as u8;
    buf[3] = (port & 0xFF) as u8;
    buf[4] = ip.0[0];
    buf[5] = ip.0[1];
    buf[6] = ip.0[2];
    buf[7] = ip.0[3];

    copy_to_user_task(task_ptr, addr_ptr, &buf)?;

    if addrlen_ptr != 0 {
        let len_val = SOCKADDR_IN_SIZE as u32;
        let len_bytes = len_val.to_le_bytes();
        copy_to_user_task(task_ptr, addrlen_ptr, &len_bytes)?;
    }
    Ok(())
}

/// Write a sockaddr_un to user memory.
fn write_sockaddr_un(
    task_ptr: *mut task::Task,
    addr_ptr: u64,
    addrlen_ptr: u64,
    path: &[u8],
) -> Result<(), i64> {
    if addr_ptr == 0 {
        return Ok(());
    }

    let path_len = path.len().min(SOCKADDR_UN_PATH_MAX);
    let out_len = SOCKADDR_UN_FAMILY_SIZE + path_len;
    let mut buf = [0u8; SOCKADDR_UN_SIZE];
    buf[0] = AF_UNIX as u8;
    buf[1] = 0;
    if path_len > 0 {
        buf[SOCKADDR_UN_FAMILY_SIZE..SOCKADDR_UN_FAMILY_SIZE + path_len]
            .copy_from_slice(&path[..path_len]);
    }

    copy_to_user_task(task_ptr, addr_ptr, &buf[..out_len])?;

    if addrlen_ptr != 0 {
        let len_val = out_len as u32;
        let len_bytes = len_val.to_le_bytes();
        copy_to_user_task(task_ptr, addrlen_ptr, &len_bytes)?;
    }
    Ok(())
}

/// Get socket_id from an FD that is a socket. Returns socket inode (= socket table id).
fn fd_to_socket_id(task_ptr: *mut task::Task, fd: i32) -> Result<(usize, u16), i64> {
    let idx = fd_index(fd)?;
    if !fd_slot_is_open(task_ptr, idx) {
        return Err(-EBADF);
    }
    let (kind, inode) = unsafe {
        (
            (*task_ptr).user_fd_kind[idx],
            (*task_ptr).user_fd_inode[idx],
        )
    };
    if kind != task::USER_FD_KIND_SOCKET {
        return Err(-ENOTSOCK);
    }
    Ok((idx, inode))
}

// ---------------------------------------------------------------------------
// Socket syscall handlers
// ---------------------------------------------------------------------------

fn sys_socket(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let domain = call.arg(0) as u32;
    let raw_type = call.arg(1) as u32;
    let protocol = call.arg(2) as u32;

    if domain != AF_INET && domain != AF_UNIX {
        return -EAFNOSUPPORT;
    }

    let (base_type, open_flags) = match socket_type_base_and_open_flags(raw_type) {
        Ok(v) => v,
        Err(err) => return err,
    };

    let normalized_protocol = if domain == AF_INET {
        let proto = if protocol == IPPROTO_IP { 0 } else { protocol };
        if proto != 0 && proto != IPPROTO_TCP && proto != IPPROTO_UDP && proto != IPPROTO_ICMP {
            return -EPROTONOSUPPORT;
        }
        proto
    } else {
        // AF_UNIX baseline: stream sockets only, protocol must be 0.
        if base_type != SOCK_STREAM || protocol != 0 {
            return -EPROTONOSUPPORT;
        }
        0
    };

    let socket_id = match socket::socket_create(domain, base_type, normalized_protocol) {
        Ok(id) => id,
        Err(e) => return e,
    };

    let fd_flags = fd_flags_from_open_flags(open_flags);

    match fd_alloc(ctx.current, task::USER_FD_KIND_SOCKET, socket_id, fd_flags) {
        Ok(fd) => fd,
        Err(e) => {
            let _ = socket::socket_close(socket_id);
            e
        }
    }
}

fn sys_bind(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let addr_ptr = call.arg(1);
    let addr_len = call.arg(2) as usize;

    let (_, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let domain = match socket::socket_domain(sock_id) {
        Ok(v) => v,
        Err(e) => return e,
    };

    match domain {
        AF_INET => {
            let (ip, port) = match parse_sockaddr_in(ctx.current, addr_ptr, addr_len) {
                Ok(v) => v,
                Err(e) => return e,
            };
            match socket::socket_bind(sock_id, &ip, port) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        AF_UNIX => {
            let (path_buf, path_len) = match parse_sockaddr_un(ctx.current, addr_ptr, addr_len) {
                Ok(v) => v,
                Err(e) => return e,
            };
            match socket::socket_bind_unix(sock_id, &path_buf[..path_len]) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        _ => -EAFNOSUPPORT,
    }
}

fn sys_listen(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let backlog = call.arg(1) as u32;

    let (_, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };

    match socket::socket_listen(sock_id, backlog) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

fn sys_connect(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let addr_ptr = call.arg(1);
    let addr_len = call.arg(2) as usize;

    let (idx, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let flags = unsafe { (*ctx.current).user_fd_flags[idx] };
    let nb = (fd_open_flags(flags) & O_NONBLOCK) != 0;
    let domain = match socket::socket_domain(sock_id) {
        Ok(v) => v,
        Err(e) => return e,
    };

    match domain {
        AF_INET => {
            let (ip, port) = match parse_sockaddr_in(ctx.current, addr_ptr, addr_len) {
                Ok(v) => v,
                Err(e) => return e,
            };
            match socket::socket_connect(sock_id, &ip, port, nb) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        AF_UNIX => {
            let (path_buf, path_len) = match parse_sockaddr_un(ctx.current, addr_ptr, addr_len) {
                Ok(v) => v,
                Err(e) => return e,
            };
            match socket::socket_connect_unix(sock_id, &path_buf[..path_len], nb) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        _ => -EAFNOSUPPORT,
    }
}

fn sys_accept(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    // accept() is accept4() with flags=0
    let mut call4 = *call;
    // accept(fd, addr, addrlen) → accept4(fd, addr, addrlen, 0)
    call4.args[3] = 0;
    sys_accept4(ctx, &call4)
}

fn sys_accept4(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let addr_ptr = call.arg(1);
    let addrlen_ptr = call.arg(2);
    let flags = call.arg(3) as u32;
    let open_flags = match accept4_open_flags(flags) {
        Ok(v) => v,
        Err(err) => return err,
    };

    let (idx, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let listen_flags = unsafe { (*ctx.current).user_fd_flags[idx] };
    let nb = (fd_open_flags(listen_flags) & O_NONBLOCK) != 0;

    let domain = match socket::socket_domain(sock_id) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let new_sock_id = match domain {
        AF_INET => {
            let (new_sock_id, remote_ip, remote_port) = match socket::socket_accept(sock_id, nb) {
                Ok(v) => v,
                Err(e) => return e,
            };
            if addr_ptr != 0 {
                if let Err(e) =
                    write_sockaddr_in(ctx.current, addr_ptr, addrlen_ptr, &remote_ip, remote_port)
                {
                    let _ = socket::socket_close(new_sock_id);
                    return e;
                }
            }
            new_sock_id
        }
        AF_UNIX => {
            let new_sock_id = match socket::socket_accept_unix(sock_id, nb) {
                Ok(v) => v,
                Err(e) => return e,
            };
            if addr_ptr != 0 {
                if let Err(e) = write_sockaddr_un(ctx.current, addr_ptr, addrlen_ptr, &[]) {
                    let _ = socket::socket_close(new_sock_id);
                    return e;
                }
            }
            new_sock_id
        }
        _ => return -EAFNOSUPPORT,
    };

    // Allocate an FD for the new socket
    let new_fd_flags = fd_flags_from_open_flags(open_flags);

    match fd_alloc(
        ctx.current,
        task::USER_FD_KIND_SOCKET,
        new_sock_id,
        new_fd_flags,
    ) {
        Ok(fd) => fd,
        Err(e) => {
            let _ = socket::socket_close(new_sock_id);
            e
        }
    }
}

fn sys_sendto(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let buf_ptr = call.arg(1);
    let len = call.arg(2) as usize;
    let _flags = call.arg(3) as u32;
    let addr_ptr = call.arg(4);
    let addr_len = call.arg(5) as usize;

    let (idx, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let fd_flags = unsafe { (*ctx.current).user_fd_flags[idx] };
    let nb = (fd_open_flags(fd_flags) & O_NONBLOCK) != 0;

    // Copy data from user
    const SEND_MAX: usize = 4096;
    let copy_len = len.min(SEND_MAX);
    let mut kbuf = [0u8; SEND_MAX];
    if let Err(e) = copy_from_user_task(ctx.current, buf_ptr, &mut kbuf[..copy_len]) {
        return -e;
    }

    if addr_ptr != 0 && addr_len >= SOCKADDR_IN_SIZE {
        // sendto with explicit destination
        let (ip, port) = match parse_sockaddr_in(ctx.current, addr_ptr, addr_len) {
            Ok(v) => v,
            Err(e) => return e,
        };
        match socket::socket_sendto(sock_id, &kbuf[..copy_len], &ip, port) {
            Ok(n) => n as i64,
            Err(e) => e,
        }
    } else {
        // No destination — use connected address (like send())
        match socket::socket_send(sock_id, &kbuf[..copy_len], nb) {
            Ok(n) => n as i64,
            Err(e) => e,
        }
    }
}

fn sys_recvfrom(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let buf_ptr = call.arg(1);
    let len = call.arg(2) as usize;
    let _flags = call.arg(3) as u32;
    let addr_ptr = call.arg(4);
    let addrlen_ptr = call.arg(5);

    let (idx, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let fd_flags = unsafe { (*ctx.current).user_fd_flags[idx] };
    let nb = (fd_open_flags(fd_flags) & O_NONBLOCK) != 0;

    const RECV_MAX: usize = 4096;
    let recv_len = len.min(RECV_MAX);
    let mut kbuf = [0u8; RECV_MAX];

    let (n, src_ip, src_port) = match socket::socket_recvfrom(sock_id, &mut kbuf[..recv_len], nb) {
        Ok(v) => v,
        Err(e) => return e,
    };

    if n > 0 {
        if let Err(e) = copy_to_user_task(ctx.current, buf_ptr, &kbuf[..n]) {
            return -e;
        }
    }

    // Write source address to user if requested
    if addr_ptr != 0 {
        let _ = write_sockaddr_in(ctx.current, addr_ptr, addrlen_ptr, &src_ip, src_port);
    }

    n as i64
}

// ---------------------------------------------------------------------------
// poll / ppoll implementation
// ---------------------------------------------------------------------------

const POLL_MAX_FDS: usize = 32;
const SELECT_MAX_FDS: usize = 1024;
const SELECT_FDSET_WORD_BITS: usize = size_of::<u64>() * 8;
const SELECT_FDSET_WORDS: usize = SELECT_MAX_FDS / SELECT_FDSET_WORD_BITS;

type SelectFdSet = [u64; SELECT_FDSET_WORDS];

#[inline]
fn select_fdset_words(nfds: usize) -> usize {
    if nfds == 0 {
        0
    } else {
        (nfds + SELECT_FDSET_WORD_BITS - 1) / SELECT_FDSET_WORD_BITS
    }
}

#[inline]
fn select_fdset_bytes(nfds: usize) -> usize {
    select_fdset_words(nfds) * size_of::<u64>()
}

#[inline]
fn select_fdset_is_set(set: &SelectFdSet, fd: usize) -> bool {
    let word = fd / SELECT_FDSET_WORD_BITS;
    let bit = fd % SELECT_FDSET_WORD_BITS;
    (set[word] & (1u64 << bit)) != 0
}

#[inline]
fn select_fdset_set(set: &mut SelectFdSet, fd: usize) {
    let word = fd / SELECT_FDSET_WORD_BITS;
    let bit = fd % SELECT_FDSET_WORD_BITS;
    set[word] |= 1u64 << bit;
}

fn read_select_fdset(
    task_ptr: *mut task::Task,
    user_ptr: u64,
    nfds: usize,
) -> Result<SelectFdSet, i64> {
    let mut bits = [0u64; SELECT_FDSET_WORDS];
    if user_ptr == 0 || nfds == 0 {
        return Ok(bits);
    }

    if nfds > SELECT_MAX_FDS {
        return Err(-EINVAL);
    }

    let byte_len = select_fdset_bytes(nfds);
    let raw = unsafe { core::slice::from_raw_parts_mut(bits.as_mut_ptr() as *mut u8, byte_len) };
    copy_from_user_task(task_ptr, user_ptr, raw).map_err(|e| -e)?;
    Ok(bits)
}

fn write_select_fdset(
    task_ptr: *mut task::Task,
    user_ptr: u64,
    nfds: usize,
    bits: &SelectFdSet,
) -> Result<(), i64> {
    if user_ptr == 0 || nfds == 0 {
        return Ok(());
    }

    if nfds > SELECT_MAX_FDS {
        return Err(-EINVAL);
    }

    let byte_len = select_fdset_bytes(nfds);
    let raw = unsafe { core::slice::from_raw_parts(bits.as_ptr() as *const u8, byte_len) };
    copy_to_user_task(task_ptr, user_ptr, raw).map_err(|e| -e)
}

fn validate_select_fds(
    task_ptr: *mut task::Task,
    nfds: usize,
    read_bits: &SelectFdSet,
    write_bits: &SelectFdSet,
    except_bits: &SelectFdSet,
) -> i64 {
    for fd in 0..nfds {
        let requested = select_fdset_is_set(read_bits, fd)
            || select_fdset_is_set(write_bits, fd)
            || select_fdset_is_set(except_bits, fd);
        if !requested {
            continue;
        }
        if fd >= task::MAX_TASK_FDS || !fd_slot_is_open(task_ptr, fd) {
            return -EBADF;
        }
    }
    0
}

fn do_select(
    ctx: &SyscallContext,
    nfds: usize,
    read_ptr: u64,
    write_ptr: u64,
    except_ptr: u64,
    deadline: Option<u64>,
) -> i64 {
    if nfds > SELECT_MAX_FDS {
        return -EINVAL;
    }

    let read_req = match read_select_fdset(ctx.current, read_ptr, nfds) {
        Ok(bits) => bits,
        Err(e) => return e,
    };
    let write_req = match read_select_fdset(ctx.current, write_ptr, nfds) {
        Ok(bits) => bits,
        Err(e) => return e,
    };
    let except_req = match read_select_fdset(ctx.current, except_ptr, nfds) {
        Ok(bits) => bits,
        Err(e) => return e,
    };

    let validate = validate_select_fds(ctx.current, nfds, &read_req, &write_req, &except_req);
    if validate != 0 {
        return validate;
    }

    loop {
        let mut out_read = [0u64; SELECT_FDSET_WORDS];
        let mut out_write = [0u64; SELECT_FDSET_WORDS];
        let mut out_except = [0u64; SELECT_FDSET_WORDS];
        let mut ready_count: i64 = 0;

        let eval_nfds = cmp::min(nfds, task::MAX_TASK_FDS);
        for fd in 0..eval_nfds {
            let fd_i32 = fd as i32;

            if select_fdset_is_set(&read_req, fd) {
                let revents = poll_fd_revents(ctx.current, fd_i32, POLLIN | POLLPRI);
                if (revents & (POLLIN | POLLERR | POLLHUP)) != 0 {
                    select_fdset_set(&mut out_read, fd);
                    ready_count += 1;
                }
            }

            if select_fdset_is_set(&write_req, fd) {
                let revents = poll_fd_revents(ctx.current, fd_i32, POLLOUT);
                if (revents & (POLLOUT | POLLERR)) != 0 {
                    select_fdset_set(&mut out_write, fd);
                    ready_count += 1;
                }
            }

            if select_fdset_is_set(&except_req, fd) {
                let revents = poll_fd_revents(ctx.current, fd_i32, POLLPRI);
                if (revents & (POLLPRI | POLLERR)) != 0 {
                    select_fdset_set(&mut out_except, fd);
                    ready_count += 1;
                }
            }
        }

        if ready_count > 0 || deadline == Some(0) {
            if let Err(e) = write_select_fdset(ctx.current, read_ptr, nfds, &out_read) {
                return e;
            }
            if let Err(e) = write_select_fdset(ctx.current, write_ptr, nfds, &out_write) {
                return e;
            }
            if let Err(e) = write_select_fdset(ctx.current, except_ptr, nfds, &out_except) {
                return e;
            }
            return ready_count;
        }

        if let Some(dl) = deadline {
            if pit::uptime_ms() >= dl {
                let zero = [0u64; SELECT_FDSET_WORDS];
                let _ = write_select_fdset(ctx.current, read_ptr, nfds, &zero);
                let _ = write_select_fdset(ctx.current, write_ptr, nfds, &zero);
                let _ = write_select_fdset(ctx.current, except_ptr, nfds, &zero);
                return 0;
            }
        }

        #[cfg(not(test))]
        {
            crate::net::poll();
            crate::sched::yield_now();
        }
    }
}

/// Compute revents for a single FD.
fn poll_fd_revents(task_ptr: *mut task::Task, fd: i32, requested: i16) -> i16 {
    let idx = match fd_index(fd) {
        Ok(i) => i,
        Err(_) => return POLLNVAL,
    };
    if !fd_slot_is_open(task_ptr, idx) {
        return POLLNVAL;
    }

    let (kind, inode, flags) = unsafe {
        (
            (*task_ptr).user_fd_kind[idx],
            (*task_ptr).user_fd_inode[idx],
            (*task_ptr).user_fd_flags[idx],
        )
    };

    let mut revents: i16 = 0;

    if kind == task::USER_FD_KIND_CONSOLE {
        if (requested & POLLIN) != 0 && tty::has_ready_data() {
            revents |= POLLIN;
        }
        if (requested & POLLOUT) != 0 {
            revents |= POLLOUT; // console always writable
        }
    } else if kind == task::USER_FD_KIND_VFS {
        if let Some(device_id) = fs::inode_input_device_id(inode) {
            if (requested & POLLIN) != 0 && crate::input::device_has_ready_data(device_id) {
                revents |= POLLIN;
            }
        } else {
            // Regular files are always ready
            if (requested & POLLIN) != 0 {
                revents |= POLLIN;
            }
            if (requested & POLLOUT) != 0 {
                revents |= POLLOUT;
            }
        }
    } else if kind == task::USER_FD_KIND_PIPE {
        if let Ok((has_data, has_space, read_eof, write_broken)) = ipc::pipe_poll_status(inode) {
            let can_read = fd_can_read(flags);
            let can_write = fd_can_write(flags);
            if can_read {
                if has_data {
                    revents |= POLLIN;
                }
                if read_eof {
                    revents |= POLLHUP;
                }
            }
            if can_write {
                if has_space {
                    revents |= POLLOUT;
                }
                if write_broken {
                    revents |= POLLERR;
                }
            }
        } else {
            revents |= POLLNVAL;
        }
    } else if kind == task::USER_FD_KIND_SOCKET {
        let sock_events = socket::socket_poll_events(inode) as i16;
        // Always report POLLERR/POLLHUP/POLLNVAL regardless of requested
        revents = sock_events & (requested | POLLERR | POLLHUP | POLLNVAL);
    } else if kind == task::USER_FD_KIND_PTY_MASTER {
        let (has_data, can_write) = crate::pty::master_poll(inode);
        if (requested & POLLIN) != 0 && has_data {
            revents |= POLLIN;
        }
        if (requested & POLLOUT) != 0 && can_write {
            revents |= POLLOUT;
        }
    } else if kind == task::USER_FD_KIND_PTY_SLAVE {
        let (has_data, can_write) = crate::pty::slave_poll(inode);
        if (requested & POLLIN) != 0 && has_data {
            revents |= POLLIN;
        }
        if (requested & POLLOUT) != 0 && can_write {
            revents |= POLLOUT;
        }
    }

    revents
}

#[inline]
fn task_has_unblocked_signal(task_ptr: *mut task::Task) -> bool {
    if task_ptr.is_null() {
        return false;
    }
    unsafe { ((*task_ptr).sig_pending & !(*task_ptr).signal_blocked) != 0 }
}

fn epoll_instance_get_mut(id: u16) -> Result<&'static mut EpollInstance, i64> {
    let idx = id as usize;
    if idx >= EPOLL_MAX_INSTANCES {
        return Err(-EBADF);
    }
    unsafe {
        let inst = &mut EPOLL_INSTANCES[idx];
        if !inst.used {
            return Err(-EBADF);
        }
        Ok(inst)
    }
}

fn epoll_alloc_instance() -> Result<u16, i64> {
    unsafe {
        for idx in 0..EPOLL_MAX_INSTANCES {
            if !EPOLL_INSTANCES[idx].used {
                let inst = &mut EPOLL_INSTANCES[idx];
                inst.reset();
                inst.used = true;
                inst.refs = 1;
                return Ok(idx as u16);
            }
        }
    }
    Err(-ENFILE)
}

fn epoll_retain(id: u16) -> Result<(), i64> {
    let inst = epoll_instance_get_mut(id)?;
    if inst.refs == 0 {
        return Err(-EBADF);
    }
    inst.refs = inst.refs.saturating_add(1);
    Ok(())
}

fn epoll_close(id: u16) -> Result<(), i64> {
    let inst = epoll_instance_get_mut(id)?;
    if inst.refs > 1 {
        inst.refs -= 1;
        return Ok(());
    }
    inst.reset();
    Ok(())
}

#[inline]
fn epoll_watch_find(inst: &EpollInstance, owner_pid: i32, fd: i32) -> Option<usize> {
    let mut idx = 0usize;
    while idx < EPOLL_MAX_WATCHES {
        let watch = inst.watches[idx];
        if watch.used && watch.owner_pid == owner_pid && watch.fd == fd {
            return Some(idx);
        }
        idx += 1;
    }
    None
}

#[inline]
fn epoll_watch_remove(inst: &mut EpollInstance, idx: usize) {
    if idx >= EPOLL_MAX_WATCHES {
        return;
    }
    if inst.watches[idx].used {
        inst.watches[idx] = EpollWatch::EMPTY;
        inst.watch_count = inst.watch_count.saturating_sub(1);
    }
}

fn epoll_drop_watches_for_fd(owner_pid: i32, fd: i32, kind: u8, inode: u16) {
    unsafe {
        let mut inst_idx = 0usize;
        while inst_idx < EPOLL_MAX_INSTANCES {
            let inst = &mut EPOLL_INSTANCES[inst_idx];
            if !inst.used || inst.watch_count == 0 {
                inst_idx += 1;
                continue;
            }

            let mut watch_idx = 0usize;
            while watch_idx < EPOLL_MAX_WATCHES {
                let watch = inst.watches[watch_idx];
                if watch.used
                    && watch.owner_pid == owner_pid
                    && watch.fd == fd
                    && watch.kind == kind
                    && watch.inode == inode
                {
                    epoll_watch_remove(inst, watch_idx);
                }
                watch_idx += 1;
            }
            inst_idx += 1;
        }
    }
}

fn epoll_watch_count_for_owner(owner_pid: i32) -> usize {
    if owner_pid <= 0 {
        return 0;
    }
    let mut count = 0usize;
    unsafe {
        let mut inst_idx = 0usize;
        while inst_idx < EPOLL_MAX_INSTANCES {
            let inst = &EPOLL_INSTANCES[inst_idx];
            if !inst.used || inst.watch_count == 0 {
                inst_idx += 1;
                continue;
            }
            let mut watch_idx = 0usize;
            while watch_idx < EPOLL_MAX_WATCHES {
                let watch = inst.watches[watch_idx];
                if watch.used && watch.owner_pid == owner_pid {
                    count = count.saturating_add(1);
                }
                watch_idx += 1;
            }
            inst_idx += 1;
        }
    }
    count
}

fn epoll_drop_watches_for_owner(owner_pid: i32) {
    if owner_pid <= 0 {
        return;
    }
    unsafe {
        let mut inst_idx = 0usize;
        while inst_idx < EPOLL_MAX_INSTANCES {
            let inst = &mut EPOLL_INSTANCES[inst_idx];
            if !inst.used || inst.watch_count == 0 {
                inst_idx += 1;
                continue;
            }
            let mut watch_idx = 0usize;
            while watch_idx < EPOLL_MAX_WATCHES {
                let watch = inst.watches[watch_idx];
                if watch.used && watch.owner_pid == owner_pid {
                    epoll_watch_remove(inst, watch_idx);
                }
                watch_idx += 1;
            }
            inst_idx += 1;
        }
    }
}

#[inline]
fn epoll_watch_add(inst: &mut EpollInstance, watch: EpollWatch) -> Result<(), i64> {
    let mut idx = 0usize;
    while idx < EPOLL_MAX_WATCHES {
        if !inst.watches[idx].used {
            inst.watches[idx] = watch;
            inst.watch_count = inst.watch_count.saturating_add(1);
            return Ok(());
        }
        idx += 1;
    }
    Err(-ENOSPC)
}

#[inline]
fn epoll_events_to_poll_mask(events: u32) -> i16 {
    let mut requested: i16 = 0;
    if (events & EPOLLIN) != 0 {
        requested |= POLLIN;
    }
    if (events & EPOLLPRI) != 0 {
        requested |= POLLPRI;
    }
    if (events & EPOLLOUT) != 0 {
        requested |= POLLOUT;
    }
    requested
}

#[inline]
fn poll_revents_to_epoll_events(requested: u32, revents: i16) -> u32 {
    let mut out = 0u32;
    if (revents & POLLIN) != 0 && (requested & EPOLLIN) != 0 {
        out |= EPOLLIN;
    }
    if (revents & POLLPRI) != 0 && (requested & EPOLLPRI) != 0 {
        out |= EPOLLPRI;
    }
    if (revents & POLLOUT) != 0 && (requested & EPOLLOUT) != 0 {
        out |= EPOLLOUT;
    }
    if (revents & POLLERR) != 0 || (revents & POLLNVAL) != 0 {
        out |= EPOLLERR;
    }
    if (revents & POLLHUP) != 0 || (revents & POLLNVAL) != 0 {
        out |= EPOLLHUP;
    }
    out
}

#[inline]
fn epoll_target_kind_supported(kind: u8) -> bool {
    kind == task::USER_FD_KIND_CONSOLE
        || kind == task::USER_FD_KIND_VFS
        || kind == task::USER_FD_KIND_PIPE
        || kind == task::USER_FD_KIND_SOCKET
        || kind == task::USER_FD_KIND_PTY_MASTER
        || kind == task::USER_FD_KIND_PTY_SLAVE
}

fn epoll_read_event_from_user(task_ptr: *mut task::Task, ptr: u64) -> Result<(u32, u64), i64> {
    let mut raw = [0u8; EPOLL_EVENT_SIZE];
    copy_from_user_task(task_ptr, ptr, &mut raw).map_err(|e| -e)?;
    let events = u32::from_ne_bytes([raw[0], raw[1], raw[2], raw[3]]);
    let data = u64::from_ne_bytes([
        raw[4], raw[5], raw[6], raw[7], raw[8], raw[9], raw[10], raw[11],
    ]);
    Ok((events, data))
}

fn epoll_write_event_to_user(
    task_ptr: *mut task::Task,
    events_ptr: u64,
    idx: usize,
    events: u32,
    data: u64,
) -> Result<(), i64> {
    let base = events_ptr
        .checked_add((idx * EPOLL_EVENT_SIZE) as u64)
        .ok_or(-EFAULT)?;
    let mut raw = [0u8; EPOLL_EVENT_SIZE];
    raw[0..4].copy_from_slice(&events.to_ne_bytes());
    raw[4..12].copy_from_slice(&data.to_ne_bytes());
    copy_to_user_task(task_ptr, base, &raw).map_err(|e| -e)
}

fn epoll_validate_fd_target(task_ptr: *mut task::Task, fd: i32) -> Result<(u8, u16), i64> {
    let idx = fd_index(fd)?;
    if !fd_slot_is_open(task_ptr, idx) {
        return Err(-EBADF);
    }
    let (kind, inode) = unsafe {
        (
            (*task_ptr).user_fd_kind[idx],
            (*task_ptr).user_fd_inode[idx],
        )
    };
    if kind == task::USER_FD_KIND_EPOLL {
        return Err(-EINVAL);
    }
    if !epoll_target_kind_supported(kind) {
        return Err(-EPERM);
    }
    if kind != task::USER_FD_KIND_CONSOLE && inode == task::USER_FD_NONE {
        return Err(-EBADF);
    }
    Ok((kind, inode))
}

fn epoll_fd_to_id(task_ptr: *mut task::Task, epfd: i32) -> Result<u16, i64> {
    let idx = fd_index(epfd)?;
    if !fd_slot_is_open(task_ptr, idx) {
        return Err(-EBADF);
    }
    let (kind, inode) = unsafe {
        (
            (*task_ptr).user_fd_kind[idx],
            (*task_ptr).user_fd_inode[idx],
        )
    };
    if kind != task::USER_FD_KIND_EPOLL {
        return Err(-EINVAL);
    }
    if inode == task::USER_FD_NONE {
        return Err(-EBADF);
    }
    Ok(inode)
}

fn epoll_ctl_add(
    inst: &mut EpollInstance,
    owner_pid: i32,
    owner_task: *mut task::Task,
    fd: i32,
    kind: u8,
    inode: u16,
    events: u32,
    data: u64,
) -> i64 {
    #[cfg(not(test))]
    let _ = owner_task;
    if owner_pid <= 0 {
        return -EINVAL;
    }
    if epoll_watch_find(inst, owner_pid, fd).is_some() {
        return -EEXIST;
    }
    if epoll_watch_count_for_owner(owner_pid) >= EPOLL_MAX_WATCHES_PER_TASK {
        return -ENOSPC;
    }
    if inst.watch_count as usize >= EPOLL_MAX_WATCHES {
        return -ENOSPC;
    }
    match epoll_watch_add(
        inst,
        EpollWatch {
            used: true,
            owner_pid,
            #[cfg(test)]
            owner_task,
            fd,
            kind,
            inode,
            events,
            data,
        },
    ) {
        Ok(()) => 0,
        Err(err) => err,
    }
}

fn epoll_ctl_mod(
    inst: &mut EpollInstance,
    owner_pid: i32,
    fd: i32,
    kind: u8,
    inode: u16,
    events: u32,
    data: u64,
) -> i64 {
    let idx = match epoll_watch_find(inst, owner_pid, fd) {
        Some(v) => v,
        None => return -ENOENT,
    };
    inst.watches[idx].kind = kind;
    inst.watches[idx].inode = inode;
    inst.watches[idx].events = events;
    inst.watches[idx].data = data;
    0
}

fn epoll_ctl_del(inst: &mut EpollInstance, owner_pid: i32, fd: i32) -> i64 {
    let idx = match epoll_watch_find(inst, owner_pid, fd) {
        Some(v) => v,
        None => return -ENOENT,
    };
    epoll_watch_remove(inst, idx);
    0
}

#[inline]
fn epoll_watch_owner_task(watch: &EpollWatch) -> *mut task::Task {
    let owner = task::get_by_pid(watch.owner_pid);
    if !owner.is_null() {
        return owner;
    }
    #[cfg(test)]
    {
        return watch.owner_task;
    }
    #[cfg(not(test))]
    {
        ptr::null_mut()
    }
}

#[inline]
fn epoll_watch_stale(watch: &EpollWatch) -> bool {
    let owner = epoll_watch_owner_task(watch);
    if owner.is_null() {
        return true;
    }
    let fd_idx = match fd_index(watch.fd) {
        Ok(v) => v,
        Err(_) => return true,
    };
    if !fd_slot_is_open(owner, fd_idx) {
        return true;
    }
    let (kind, inode) = unsafe {
        (
            (*owner).user_fd_kind[fd_idx],
            (*owner).user_fd_inode[fd_idx],
        )
    };
    kind != watch.kind || inode != watch.inode
}

fn epoll_collect_ready(inst: &mut EpollInstance, out: &mut [(u32, u64)]) -> usize {
    let mut ready = 0usize;
    let maxevents = out.len();
    let mut idx = 0usize;
    while idx < EPOLL_MAX_WATCHES && ready < maxevents {
        let watch = inst.watches[idx];
        if !watch.used {
            idx += 1;
            continue;
        }

        if epoll_watch_stale(&watch) {
            epoll_watch_remove(inst, idx);
            idx += 1;
            continue;
        }

        let owner = epoll_watch_owner_task(&watch);
        if owner.is_null() {
            epoll_watch_remove(inst, idx);
            idx += 1;
            continue;
        }

        let poll_requested = epoll_events_to_poll_mask(watch.events);
        let revents = poll_fd_revents(owner, watch.fd, poll_requested);
        if (revents & POLLNVAL) != 0 {
            epoll_watch_remove(inst, idx);
            idx += 1;
            continue;
        }

        let ep_events = poll_revents_to_epoll_events(watch.events, revents);
        if ep_events != 0 {
            out[ready] = (ep_events, watch.data);
            ready += 1;
        }
        idx += 1;
    }
    ready
}

fn do_poll(ctx: &SyscallContext, fds_ptr: u64, nfds: usize, deadline: Option<u64>) -> i64 {
    if nfds > POLL_MAX_FDS {
        return -EINVAL;
    }
    if fds_ptr == 0 && nfds > 0 {
        return -EFAULT;
    }

    // Read PollFd array from user
    let mut fds = [PollFd {
        fd: -1,
        events: 0,
        revents: 0,
    }; POLL_MAX_FDS];
    if nfds > 0 {
        let byte_len = nfds * size_of::<PollFd>();
        let bytes =
            unsafe { core::slice::from_raw_parts_mut(fds.as_mut_ptr() as *mut u8, byte_len) };
        if let Err(e) = copy_from_user_task(ctx.current, fds_ptr, bytes) {
            return -(e);
        }
    }

    loop {
        let mut ready_count: i64 = 0;
        for i in 0..nfds {
            fds[i].revents = 0;
            if fds[i].fd < 0 {
                continue;
            }
            fds[i].revents = poll_fd_revents(ctx.current, fds[i].fd, fds[i].events);
            if fds[i].revents != 0 {
                ready_count += 1;
            }
        }

        if ready_count > 0 || deadline == Some(0) {
            // Write back
            if nfds > 0 {
                let byte_len = nfds * size_of::<PollFd>();
                let bytes =
                    unsafe { core::slice::from_raw_parts(fds.as_ptr() as *const u8, byte_len) };
                if let Err(e) = copy_to_user_task(ctx.current, fds_ptr, bytes) {
                    return -(e);
                }
            }
            return ready_count;
        }

        // Check deadline
        if let Some(dl) = deadline {
            if pit::uptime_ms() >= dl {
                // Timeout — write back zero revents
                if nfds > 0 {
                    let byte_len = nfds * size_of::<PollFd>();
                    let bytes =
                        unsafe { core::slice::from_raw_parts(fds.as_ptr() as *const u8, byte_len) };
                    let _ = copy_to_user_task(ctx.current, fds_ptr, bytes);
                }
                return 0;
            }
        }

        // Drive NIC + TCP tick, then yield
        #[cfg(not(test))]
        {
            crate::net::poll();
            crate::sched::yield_now();
        }
    }
}

fn sys_ppoll(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fds_ptr = call.arg(0);
    let nfds = call.arg(1) as usize;
    let tspec_ptr = call.arg(2);
    // arg3 = sigmask (ignored)

    let deadline = if tspec_ptr == 0 {
        None // block indefinitely
    } else {
        match read_struct_from_user::<LinuxTimeSpec>(ctx.current, tspec_ptr) {
            Ok(ts) => {
                if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
                    return -EINVAL;
                }
                let timeout_ms = (ts.tv_sec as u64)
                    .saturating_mul(1000)
                    .saturating_add((ts.tv_nsec as u64) / 1_000_000);
                Some(pit::uptime_ms().saturating_add(timeout_ms))
            }
            Err(e) => return -(e),
        }
    };

    do_poll(ctx, fds_ptr, nfds, deadline)
}

fn sys_select(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let nfds_raw = call.arg(0) as i64;
    if nfds_raw < 0 {
        return -EINVAL;
    }
    let nfds = nfds_raw as usize;
    let read_ptr = call.arg(1);
    let write_ptr = call.arg(2);
    let except_ptr = call.arg(3);
    let timeout_ptr = call.arg(4);

    let deadline = if timeout_ptr == 0 {
        None
    } else {
        match read_struct_from_user::<LinuxTimeVal>(ctx.current, timeout_ptr) {
            Ok(tv) => {
                if tv.tv_sec < 0 || tv.tv_usec < 0 || tv.tv_usec >= 1_000_000 {
                    return -EINVAL;
                }
                let sec_ms = (tv.tv_sec as u64).saturating_mul(1000);
                let usec_ms = (tv.tv_usec as u64).saturating_add(999) / 1000;
                Some(pit::uptime_ms().saturating_add(sec_ms.saturating_add(usec_ms)))
            }
            Err(e) => return -e,
        }
    };

    do_select(ctx, nfds, read_ptr, write_ptr, except_ptr, deadline)
}

fn sys_poll(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fds_ptr = call.arg(0);
    let nfds = call.arg(1) as usize;
    let timeout_ms = call.arg(2) as i32;

    let deadline = if timeout_ms < 0 {
        None // block indefinitely
    } else if timeout_ms == 0 {
        Some(0u64) // non-blocking
    } else {
        Some(pit::uptime_ms().saturating_add(timeout_ms as u64))
    };

    do_poll(ctx, fds_ptr, nfds, deadline)
}

fn sys_epoll_create1(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let flags = call.arg(0) as u32;
    if (flags & !EPOLL_CLOEXEC) != 0 {
        return -EINVAL;
    }

    let epoll_id = match epoll_alloc_instance() {
        Ok(v) => v,
        Err(err) => return err,
    };

    let mut open_flags = fs::O_RDWR;
    if (flags & EPOLL_CLOEXEC) != 0 {
        open_flags |= O_CLOEXEC;
    }
    let fd_flags = fd_flags_from_open_flags(open_flags);

    match fd_alloc(ctx.current, task::USER_FD_KIND_EPOLL, epoll_id, fd_flags) {
        Ok(fd) => fd,
        Err(err) => {
            let _ = epoll_close(epoll_id);
            err
        }
    }
}

fn sys_epoll_ctl(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let epfd = call.arg(0) as i32;
    let op = call.arg(1) as i32;
    let fd = call.arg(2) as i32;
    let event_ptr = call.arg(3);

    if fd == epfd {
        return -EINVAL;
    }

    let epoll_id = match epoll_fd_to_id(ctx.current, epfd) {
        Ok(v) => v,
        Err(err) => return err,
    };

    let owner_pid = unsafe { (*ctx.current).pid };

    match op {
        EPOLL_CTL_ADD | EPOLL_CTL_MOD => {
            if event_ptr == 0 {
                return -EFAULT;
            }
            let (events, data) = match epoll_read_event_from_user(ctx.current, event_ptr) {
                Ok(v) => v,
                Err(err) => return err,
            };
            if (events & !EPOLL_SUPPORTED_EVENTS_MASK) != 0 {
                return -EINVAL;
            }
            let (kind, inode) = match epoll_validate_fd_target(ctx.current, fd) {
                Ok(v) => v,
                Err(err) => return err,
            };
            let inst = match epoll_instance_get_mut(epoll_id) {
                Ok(v) => v,
                Err(err) => return err,
            };
            if op == EPOLL_CTL_ADD {
                epoll_ctl_add(inst, owner_pid, ctx.current, fd, kind, inode, events, data)
            } else {
                epoll_ctl_mod(inst, owner_pid, fd, kind, inode, events, data)
            }
        }
        EPOLL_CTL_DEL => {
            let inst = match epoll_instance_get_mut(epoll_id) {
                Ok(v) => v,
                Err(err) => return err,
            };
            epoll_ctl_del(inst, owner_pid, fd)
        }
        _ => -EINVAL,
    }
}

fn sys_epoll_wait(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let epfd = call.arg(0) as i32;
    let events_ptr = call.arg(1);
    let maxevents = call.arg(2) as i32;
    let timeout = call.arg(3) as i32;

    if events_ptr == 0 {
        return -EFAULT;
    }
    if maxevents <= 0 {
        return -EINVAL;
    }
    if timeout < -1 {
        return -EINVAL;
    }

    let epoll_id = match epoll_fd_to_id(ctx.current, epfd) {
        Ok(v) => v,
        Err(err) => return err,
    };

    let maxevents = maxevents as usize;
    let deadline = if timeout < 0 {
        None
    } else if timeout == 0 {
        Some(0u64)
    } else {
        Some(pit::uptime_ms().saturating_add(timeout as u64))
    };
    let mut ready_events = [(0u32, 0u64); EPOLL_MAX_WATCHES];
    let event_cap = cmp::min(maxevents, ready_events.len());

    loop {
        let ready = {
            let inst = match epoll_instance_get_mut(epoll_id) {
                Ok(v) => v,
                Err(err) => return err,
            };
            epoll_collect_ready(inst, &mut ready_events[..event_cap])
        };

        if ready > 0 {
            let mut idx = 0usize;
            while idx < ready {
                let (events, data) = ready_events[idx];
                if let Err(err) =
                    epoll_write_event_to_user(ctx.current, events_ptr, idx, events, data)
                {
                    return err;
                }
                idx += 1;
            }
            return ready as i64;
        }

        if deadline == Some(0) {
            return 0;
        }
        if task_has_unblocked_signal(ctx.current) {
            return -EINTR;
        }
        if let Some(dl) = deadline {
            if pit::uptime_ms() >= dl {
                return 0;
            }
        }

        #[cfg(not(test))]
        {
            crate::net::poll();
            crate::sched::yield_now();
        }
    }
}

fn sys_epoll_pwait(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let sigmask = call.arg(4);
    if sigmask != 0 {
        return -ENOSYS;
    }
    sys_epoll_wait(ctx, call)
}

// ---------------------------------------------------------------------------
// sendmsg / recvmsg
// ---------------------------------------------------------------------------

#[inline]
fn cmsg_align(len: usize) -> usize {
    let align = size_of::<usize>();
    (len + align - 1) & !(align - 1)
}

fn unix_fd_transfer_from_task(
    task_ptr: *mut task::Task,
    fd: i32,
) -> Result<socket::UnixRightsFd, i64> {
    let idx = fd_index(fd)?;
    if !fd_slot_is_open(task_ptr, idx) {
        return Err(-EBADF);
    }
    let (kind, inode, offset, flags) = unsafe {
        (
            (*task_ptr).user_fd_kind[idx],
            (*task_ptr).user_fd_inode[idx],
            (*task_ptr).user_fd_offset[idx],
            (*task_ptr).user_fd_flags[idx],
        )
    };
    match kind {
        task::USER_FD_KIND_VFS
        | task::USER_FD_KIND_PIPE
        | task::USER_FD_KIND_SOCKET
        | task::USER_FD_KIND_PTY_MASTER
        | task::USER_FD_KIND_PTY_SLAVE => {}
        _ => return Err(-EOPNOTSUPP),
    }
    if inode == task::USER_FD_NONE {
        return Err(-EBADF);
    }
    Ok(socket::UnixRightsFd {
        kind,
        inode,
        flags,
        offset,
    })
}

fn parse_sendmsg_rights(
    task_ptr: *mut task::Task,
    hdr: &LinuxMsgHdr,
) -> Result<([socket::UnixRightsFd; socket::UNIX_RIGHTS_MAX_FDS], usize), i64> {
    let mut rights = [socket::UnixRightsFd::EMPTY; socket::UNIX_RIGHTS_MAX_FDS];
    if hdr.msg_controllen == 0 {
        return Ok((rights, 0));
    }
    if hdr.msg_control == 0 {
        return Err(-EFAULT);
    }

    let ctrl_len = hdr.msg_controllen as usize;
    let cmsg_hdr_len = size_of::<LinuxCmsgHdr>();
    if ctrl_len < cmsg_hdr_len {
        return Err(-EINVAL);
    }

    let mut count = 0usize;
    let mut off = 0usize;
    while off + cmsg_hdr_len <= ctrl_len {
        let cmsg =
            match read_struct_from_user::<LinuxCmsgHdr>(task_ptr, hdr.msg_control + off as u64) {
                Ok(v) => v,
                Err(e) => return Err(-e),
            };
        let msg_len = cmsg.cmsg_len as usize;
        if msg_len < cmsg_hdr_len || off + msg_len > ctrl_len {
            return Err(-EINVAL);
        }
        if cmsg.cmsg_level as u32 != SOL_SOCKET || cmsg.cmsg_type as u32 != SCM_RIGHTS {
            return Err(-EOPNOTSUPP);
        }

        let payload_len = msg_len - cmsg_hdr_len;
        if (payload_len % size_of::<i32>()) != 0 {
            return Err(-EINVAL);
        }
        let fd_count = payload_len / size_of::<i32>();
        if count + fd_count > socket::UNIX_RIGHTS_MAX_FDS {
            return Err(-EINVAL);
        }

        let mut idx = 0usize;
        while idx < fd_count {
            let fd = match read_i32_from_user(
                task_ptr,
                hdr.msg_control + (off + cmsg_hdr_len + idx * size_of::<i32>()) as u64,
            ) {
                Ok(v) => v,
                Err(e) => return Err(-e),
            };
            rights[count] = unix_fd_transfer_from_task(task_ptr, fd)?;
            count += 1;
            idx += 1;
        }

        let adv = cmsg_align(msg_len);
        if adv == 0 {
            break;
        }
        off += adv;
    }

    Ok((rights, count))
}

fn install_received_right(
    task_ptr: *mut task::Task,
    right: socket::UnixRightsFd,
    cloexec: bool,
) -> Result<i32, i64> {
    let preserved_internal = fd_internal_flags(right.flags) & !FD_INTERNAL_CLOEXEC;
    if (preserved_internal & FD_INTERNAL_MEMFD) != 0 {
        enforce_memfd_object_limit_for_inode(task_ptr, right.inode)?;
    }
    let idx = match fd_find_free_from(task_ptr, 0) {
        Some(v) => v,
        None => return Err(-EMFILE),
    };
    let mut open_flags = fd_open_flags(right.flags);
    if cloexec {
        open_flags |= O_CLOEXEC;
    }
    let fd_flags = fd_flags_from_open_flags(open_flags) | preserved_internal;
    fd_write_slot(
        task_ptr,
        idx,
        right.kind,
        right.inode,
        right.offset,
        fd_flags,
    );
    Ok(idx as i32)
}

fn sys_sendmsg(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let msghdr_ptr = call.arg(1);
    let msg_flags_arg = call.arg(2) as u32;
    if msghdr_ptr == 0 {
        return -EFAULT;
    }

    let (idx, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let fd_flags = unsafe { (*ctx.current).user_fd_flags[idx] };
    let nb = (fd_open_flags(fd_flags) & O_NONBLOCK) != 0 || (msg_flags_arg & MSG_DONTWAIT) != 0;

    let hdr = match read_struct_from_user::<LinuxMsgHdr>(ctx.current, msghdr_ptr) {
        Ok(h) => h,
        Err(e) => return -e,
    };
    let domain = match socket::socket_domain(sock_id) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let iov_count = hdr.msg_iovlen as usize;
    if iov_count > 64 {
        return -EINVAL;
    }

    let mut total_len: usize = 0;
    let mut iovecs = [LinuxIovec {
        iov_base: 0,
        iov_len: 0,
    }; 64];
    for (i, slot) in iovecs.iter_mut().enumerate().take(iov_count) {
        let entry_ptr = hdr.msg_iov + (i * size_of::<LinuxIovec>()) as u64;
        let iov = match read_struct_from_user::<LinuxIovec>(ctx.current, entry_ptr) {
            Ok(v) => v,
            Err(e) => return -e,
        };
        *slot = iov;
        total_len = total_len.saturating_add(iov.iov_len as usize);
    }

    const SENDMSG_BUF_SIZE: usize = 4096;
    let copy_len = total_len.min(SENDMSG_BUF_SIZE);
    let mut kbuf = [0u8; SENDMSG_BUF_SIZE];
    let mut remaining = copy_len;
    let mut dst_off: usize = 0;
    for iov in iovecs.iter().take(iov_count) {
        if remaining == 0 {
            break;
        }
        let chunk = remaining.min(iov.iov_len as usize);
        if chunk > 0 {
            if let Err(e) = copy_from_user_task(
                ctx.current,
                iov.iov_base,
                &mut kbuf[dst_off..dst_off + chunk],
            ) {
                return -e;
            }
            dst_off += chunk;
            remaining -= chunk;
        }
    }

    let (rights, rights_count) = match parse_sendmsg_rights(ctx.current, &hdr) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if rights_count > 0 {
        if domain != AF_UNIX || hdr.msg_name != 0 {
            return -EOPNOTSUPP;
        }
        if let Err(e) = socket::socket_send_rights(sock_id, &rights[..rights_count], nb) {
            return e;
        }
    }

    if hdr.msg_name != 0 {
        match domain {
            AF_INET => {
                let (ip, port) =
                    match parse_sockaddr_in(ctx.current, hdr.msg_name, hdr.msg_namelen as usize) {
                        Ok(v) => v,
                        Err(e) => return e,
                    };
                match socket::socket_sendto(sock_id, &kbuf[..copy_len], &ip, port) {
                    Ok(n) => n as i64,
                    Err(e) => e,
                }
            }
            AF_UNIX => -EOPNOTSUPP,
            _ => -EAFNOSUPPORT,
        }
    } else {
        match socket::socket_send(sock_id, &kbuf[..copy_len], nb) {
            Ok(n) => n as i64,
            Err(e) => e,
        }
    }
}

fn sys_recvmsg(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let msghdr_ptr = call.arg(1);
    let msg_flags_arg = call.arg(2) as u32;

    if msghdr_ptr == 0 {
        return -EFAULT;
    }

    let (idx, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let fd_flags = unsafe { (*ctx.current).user_fd_flags[idx] };
    let nb = (fd_open_flags(fd_flags) & O_NONBLOCK) != 0 || (msg_flags_arg & MSG_DONTWAIT) != 0;

    let hdr = match read_struct_from_user::<LinuxMsgHdr>(ctx.current, msghdr_ptr) {
        Ok(h) => h,
        Err(e) => return -e,
    };

    let iov_count = hdr.msg_iovlen as usize;
    if iov_count > 64 {
        return -EINVAL;
    }

    let mut total_len: usize = 0;
    let mut iovecs = [LinuxIovec {
        iov_base: 0,
        iov_len: 0,
    }; 64];
    for i in 0..iov_count {
        let entry_ptr = hdr.msg_iov + (i * size_of::<LinuxIovec>()) as u64;
        match read_struct_from_user::<LinuxIovec>(ctx.current, entry_ptr) {
            Ok(iov) => {
                iovecs[i] = iov;
                total_len = total_len.saturating_add(iov.iov_len as usize);
            }
            Err(e) => return -e,
        }
    }

    const RECVMSG_BUF_SIZE: usize = 4096;
    let recv_len = total_len.min(RECVMSG_BUF_SIZE);
    let mut kbuf = [0u8; RECVMSG_BUF_SIZE];

    let (n, src_ip, src_port) = match socket::socket_recvfrom(sock_id, &mut kbuf[..recv_len], nb) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let mut remaining = n;
    let mut src_off: usize = 0;
    for i in 0..iov_count {
        if remaining == 0 {
            break;
        }
        let chunk = remaining.min(iovecs[i].iov_len as usize);
        if chunk > 0 && iovecs[i].iov_base != 0 {
            if let Err(e) = copy_to_user_task(
                ctx.current,
                iovecs[i].iov_base,
                &kbuf[src_off..src_off + chunk],
            ) {
                return -e;
            }
        }
        src_off += chunk;
        remaining -= chunk;
    }

    let domain = match socket::socket_domain(sock_id) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let mut out_hdr = hdr;
    out_hdr.msg_namelen = 0;
    if hdr.msg_name != 0 {
        match domain {
            AF_INET => {
                out_hdr.msg_namelen = SOCKADDR_IN_SIZE as u32;
                if hdr.msg_namelen >= SOCKADDR_IN_SIZE as u32 {
                    let _ = write_sockaddr_in(ctx.current, hdr.msg_name, 0, &src_ip, src_port);
                }
            }
            AF_UNIX => {
                out_hdr.msg_namelen = SOCKADDR_UN_FAMILY_SIZE as u32;
                if hdr.msg_namelen >= SOCKADDR_UN_FAMILY_SIZE as u32 {
                    let _ = write_sockaddr_un(ctx.current, hdr.msg_name, 0, &[]);
                }
            }
            _ => return -EAFNOSUPPORT,
        }
    }

    out_hdr.msg_controllen = 0;
    out_hdr.msg_flags = 0;

    if domain == AF_UNIX {
        let rights_msg = match socket::socket_recv_rights(sock_id) {
            Ok(v) => v,
            Err(e) => return e,
        };

        if let Some(msg) = rights_msg {
            let cloexec = (msg_flags_arg & MSG_CMSG_CLOEXEC) != 0;
            let cmsg_hdr_len = size_of::<LinuxCmsgHdr>();
            let data_item_len = size_of::<i32>();

            if hdr.msg_control == 0 || hdr.msg_controllen < cmsg_hdr_len as u64 {
                out_hdr.msg_flags |= MSG_CTRUNC;
                socket::socket_release_rights(&msg.fds[..msg.count]);
            } else {
                let ctrl_cap = hdr.msg_controllen as usize;
                let max_fds_fit = (ctrl_cap - cmsg_hdr_len) / data_item_len;
                let fd_count = msg.count.min(max_fds_fit);

                if fd_count == 0 {
                    out_hdr.msg_flags |= MSG_CTRUNC;
                    socket::socket_release_rights(&msg.fds[..msg.count]);
                } else {
                    let mut installed = [0i32; socket::UNIX_RIGHTS_MAX_FDS];
                    let mut installed_count = 0usize;
                    while installed_count < fd_count {
                        match install_received_right(ctx.current, msg.fds[installed_count], cloexec)
                        {
                            Ok(new_fd) => {
                                installed[installed_count] = new_fd;
                                installed_count += 1;
                            }
                            Err(e) => {
                                let mut i = 0usize;
                                while i < installed_count {
                                    let _ = fd_close(ctx.current, installed[i]);
                                    i += 1;
                                }
                                socket::socket_release_rights(&msg.fds[installed_count..msg.count]);
                                return e;
                            }
                        }
                    }

                    let cmsg_len = cmsg_hdr_len + installed_count * data_item_len;
                    let cmsg = LinuxCmsgHdr {
                        cmsg_len: cmsg_len as u64,
                        cmsg_level: SOL_SOCKET as i32,
                        cmsg_type: SCM_RIGHTS as i32,
                    };
                    if let Err(e) = write_struct_to_user(ctx.current, hdr.msg_control, &cmsg) {
                        let mut i = 0usize;
                        while i < installed_count {
                            let _ = fd_close(ctx.current, installed[i]);
                            i += 1;
                        }
                        socket::socket_release_rights(&msg.fds[installed_count..msg.count]);
                        return -e;
                    }
                    let mut i = 0usize;
                    while i < installed_count {
                        if let Err(e) = write_i32_to_user(
                            ctx.current,
                            hdr.msg_control + (cmsg_hdr_len + i * data_item_len) as u64,
                            installed[i],
                        ) {
                            let mut j = 0usize;
                            while j < installed_count {
                                let _ = fd_close(ctx.current, installed[j]);
                                j += 1;
                            }
                            socket::socket_release_rights(&msg.fds[installed_count..msg.count]);
                            return -e;
                        }
                        i += 1;
                    }

                    out_hdr.msg_controllen = cmsg_align(cmsg_len) as u64;
                    if installed_count < msg.count {
                        out_hdr.msg_flags |= MSG_CTRUNC;
                        socket::socket_release_rights(&msg.fds[installed_count..msg.count]);
                    }
                }
            }
        }
    }

    if let Err(e) = write_struct_to_user(ctx.current, msghdr_ptr, &out_hdr) {
        return -e;
    }

    n as i64
}

fn sys_shutdown(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let how = call.arg(1) as i32;

    if how < 0 || how > 2 {
        return -EINVAL;
    }

    let (_, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };

    match socket::socket_shutdown(sock_id, how) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

fn sys_getsockname(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let addr_ptr = call.arg(1);
    let addrlen_ptr = call.arg(2);

    let (_, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let (ip, port) = match socket::socket_getsockname(sock_id) {
        Ok(v) => v,
        Err(e) => return e,
    };

    match write_sockaddr_in(ctx.current, addr_ptr, addrlen_ptr, &ip, port) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

fn sys_getpeername(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let addr_ptr = call.arg(1);
    let addrlen_ptr = call.arg(2);

    let (_, sock_id) = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let (ip, port) = match socket::socket_getpeername(sock_id) {
        Ok(v) => v,
        Err(e) => return e,
    };

    match write_sockaddr_in(ctx.current, addr_ptr, addrlen_ptr, &ip, port) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

fn sys_socketpair(_ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    let ctx = _ctx;
    let call = _call;

    let domain = call.arg(0) as u32;
    let raw_type = call.arg(1) as u32;
    let protocol = call.arg(2) as u32;
    let sv_ptr = call.arg(3);

    if sv_ptr == 0 {
        return -EFAULT;
    }
    if domain != AF_UNIX {
        return -EAFNOSUPPORT;
    }

    let (base_type, open_flags) = match socket_type_base_and_open_flags(raw_type) {
        Ok(v) => v,
        Err(err) => return err,
    };
    if base_type != SOCK_STREAM || protocol != 0 {
        return -EOPNOTSUPP;
    }

    let (sock0, sock1) = match socket::socket_create_unix_pair() {
        Ok(pair) => pair,
        Err(e) => return e,
    };

    let fd_flags = fd_flags_from_open_flags(open_flags);

    let fd0 = match fd_alloc(ctx.current, task::USER_FD_KIND_SOCKET, sock0, fd_flags) {
        Ok(fd) => fd as i32,
        Err(e) => {
            let _ = socket::socket_close(sock0);
            let _ = socket::socket_close(sock1);
            return e;
        }
    };

    let fd1 = match fd_alloc(ctx.current, task::USER_FD_KIND_SOCKET, sock1, fd_flags) {
        Ok(fd) => fd as i32,
        Err(e) => {
            let _ = fd_close(ctx.current, fd0);
            let _ = socket::socket_close(sock1);
            return e;
        }
    };

    if let Err(err) = write_i32_to_user(ctx.current, sv_ptr, fd0) {
        let _ = fd_close(ctx.current, fd0);
        let _ = fd_close(ctx.current, fd1);
        return -err;
    }
    if let Err(err) = write_i32_to_user(ctx.current, sv_ptr + size_of::<i32>() as u64, fd1) {
        let _ = fd_close(ctx.current, fd0);
        let _ = fd_close(ctx.current, fd1);
        return -err;
    }

    0
}

fn sys_setsockopt(_ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    // Stub: silently accept common options
    0
}

fn sys_getsockopt(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    let level = call.arg(1) as u32;
    let optname = call.arg(2) as u32;
    let optval_ptr = call.arg(3);
    let optlen_ptr = call.arg(4);

    // Validate this is a socket
    let _ = match fd_to_socket_id(ctx.current, fd) {
        Ok(v) => v,
        Err(e) => return e,
    };

    // Return 0 for SO_ERROR and other common queries
    if level == SOL_SOCKET && optname == SO_ERROR {
        let zero: u32 = 0;
        let bytes = zero.to_le_bytes();
        if let Err(e) = copy_to_user_task(ctx.current, optval_ptr, &bytes) {
            return -e;
        }
        if optlen_ptr != 0 {
            let len: u32 = 4;
            let len_bytes = len.to_le_bytes();
            if let Err(e) = copy_to_user_task(ctx.current, optlen_ptr, &len_bytes) {
                return -e;
            }
        }
        return 0;
    }

    // For other options, return 0 (success) with a zero value
    if optval_ptr != 0 {
        let zero: u32 = 0;
        let bytes = zero.to_le_bytes();
        let _ = copy_to_user_task(ctx.current, optval_ptr, &bytes);
    }
    if optlen_ptr != 0 {
        let len: u32 = 4;
        let len_bytes = len.to_le_bytes();
        let _ = copy_to_user_task(ctx.current, optlen_ptr, &len_bytes);
    }
    0
}

fn install_user_signal_frame_syscall(
    task_ptr: *mut task::Task,
    frame: &mut SyscallFrame,
    delivery: signal::UserDelivery,
) -> Result<(), i64> {
    let frame_size = size_of::<UserSignalFrameSyscall>() as u64;
    let frame_rsp = frame.rsp.checked_sub(frame_size).ok_or(EFAULT)?;
    let frame_rsp = align_down(frame_rsp, 16);
    let return_rsp = frame_rsp
        .checked_sub(size_of::<u64>() as u64)
        .ok_or(EFAULT)?;

    let signal_frame = UserSignalFrameSyscall {
        magic: SIGNAL_FRAME_MAGIC_SYSCALL,
        saved_mask: delivery.saved_mask,
        signum: delivery.signum as u64,
        r15: frame.r15,
        r14: frame.r14,
        r13: frame.r13,
        r12: frame.r12,
        rbp: frame.rbp,
        rbx: frame.rbx,
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
    let return_rsp = frame_rsp
        .checked_sub(size_of::<u64>() as u64)
        .ok_or(EFAULT)?;

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

    frame.r15 = signal_frame.r15;
    frame.r14 = signal_frame.r14;
    frame.r13 = signal_frame.r13;
    frame.r12 = signal_frame.r12;
    frame.rbp = signal_frame.rbp;
    frame.rbx = signal_frame.rbx;
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

#[inline]
fn log_execve_result(result: &Result<user::ExecImageStart, i64>) {
    if !cfg!(any(
        feature = "musl-ext2-exec",
        feature = "musl-ext2-cmd-smoke",
        feature = "musl-ext2-quickjs-smoke",
        feature = "musl-ext2-micropython-smoke",
        feature = "musl-ext2-dropbear-smoke"
    )) {
        return;
    }
    match result {
        Ok(start) => {
            sys_log!(
                "[SYSCALL] execve ok entry=0x{:x} rsp=0x{:x}",
                start.entry_rip,
                start.stack_ptr
            );
        }
        Err(err) => {
            sys_log!("[SYSCALL] execve err {}", err);
        }
    }
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
        for idx in 0..task::MAX_TASKS {
            TASK_VM_OWNER_PID[idx] = task::INVALID_PID;
            TASK_VM_MAPPINGS[idx] = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
        }
        for idx in 0..EPOLL_MAX_INSTANCES {
            EPOLL_INSTANCES[idx] = EpollInstance::EMPTY;
        }
    }
    vm_page_cache_reset();
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
    if frame.rax == SYS_EXECVE {
        if !ctx.is_user_task() {
            return to_sysret(-EPERM);
        }
        let exec_result = execve_common(&ctx, frame.rdi, frame.rsi, frame.rdx);
        log_execve_result(&exec_result);
        match exec_result {
            Ok(exec_start) => {
                frame.rax = 0;
                frame.rcx = exec_start.entry_rip;
                frame.rsp = exec_start.stack_ptr;
                return frame.rax;
            }
            Err(err) => {
                frame.rax = to_sysret(err);
                if let Some(ret) = apply_pending_signals_syscall(frame) {
                    frame.rax = to_sysret(ret);
                }
                return frame.rax;
            }
        }
    }
    if frame.rax == SYS_FORK || frame.rax == SYS_VFORK || frame.rax == SYS_CLONE {
        if !ctx.is_user_task() {
            return to_sysret(-EPERM);
        }
        let result = if frame.rax == SYS_CLONE {
            let call = call_from_syscall_frame(frame);
            let child_stack = match validate_clone_args(&call) {
                Ok(stack) => stack,
                Err(err) => {
                    frame.rax = to_sysret(err);
                    if let Some(ret) = apply_pending_signals_syscall(frame) {
                        frame.rax = to_sysret(ret);
                    }
                    return frame.rax;
                }
            };
            let child_frame = match build_clone_child_frame_from_syscall(frame, child_stack) {
                Ok(child_frame) => child_frame,
                Err(err) => {
                    frame.rax = to_sysret(err);
                    if let Some(ret) = apply_pending_signals_syscall(frame) {
                        frame.rax = to_sysret(ret);
                    }
                    return frame.rax;
                }
            };
            create_fork_child(&ctx, &child_frame)
        } else {
            let child_frame = build_child_frame_from_syscall_frame(frame);
            create_fork_child(&ctx, &child_frame)
        };
        frame.rax = to_sysret(match result {
            Ok(pid) => pid,
            Err(err) => err,
        });
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
    if frame.rax == SYS_EXECVE {
        frame.rax = if !ctx.is_user_task() {
            to_sysret(-EPERM)
        } else {
            let exec_result = execve_common(&ctx, frame.rdi, frame.rsi, frame.rdx);
            log_execve_result(&exec_result);
            match exec_result {
                Ok(exec_start) => {
                    frame.rip = exec_start.entry_rip;
                    frame.rsp = exec_start.stack_ptr;
                    0
                }
                Err(err) => to_sysret(err),
            }
        };
        if let Some(ret) = apply_pending_signals_int80(frame) {
            frame.rax = to_sysret(ret);
        }
        return;
    }
    if frame.rax == SYS_FORK || frame.rax == SYS_VFORK || frame.rax == SYS_CLONE {
        frame.rax = if !ctx.is_user_task() {
            to_sysret(-EPERM)
        } else if frame.rax == SYS_CLONE {
            let call = call_from_int80_frame(frame);
            let child_stack = match validate_clone_args(&call) {
                Ok(stack) => stack,
                Err(err) => {
                    frame.rax = to_sysret(err);
                    if let Some(ret) = apply_pending_signals_int80(frame) {
                        frame.rax = to_sysret(ret);
                    }
                    return;
                }
            };
            let child_frame = match build_clone_child_frame_from_int80(frame, child_stack) {
                Ok(child_frame) => child_frame,
                Err(err) => {
                    frame.rax = to_sysret(err);
                    if let Some(ret) = apply_pending_signals_int80(frame) {
                        frame.rax = to_sysret(ret);
                    }
                    return;
                }
            };
            let result = create_fork_child(&ctx, &child_frame);
            to_sysret(match result {
                Ok(pid) => pid,
                Err(err) => err,
            })
        } else {
            let child_frame = build_child_frame_from_int80_frame(frame);
            let result = create_fork_child(&ctx, &child_frame);
            to_sysret(match result {
                Ok(pid) => pid,
                Err(err) => err,
            })
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
    use std::sync::Mutex;

    static EPOLL_TEST_LOCK: Mutex<()> = Mutex::new(());
    static INPUT_TEST_LOCK: Mutex<()> = Mutex::new(());
    static MMAP_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn reset_epoll_for_test() {
        unsafe {
            for idx in 0..EPOLL_MAX_INSTANCES {
                EPOLL_INSTANCES[idx] = EpollInstance::EMPTY;
            }
        }
    }

    fn reset_vm_for_test() {
        fs::init();
        crate::input::init();
        let _ = fs::mkdir("/temp");
        task::Task::reset_registry_for_test();
        init();
    }

    #[test]
    fn user_range_validation() {
        assert!(user_range_ok(0x1000, 4));
        assert!(user_range_ok(USER_CANON_MAX, 1));
        assert!(!user_range_ok(USER_CANON_MAX + 1, 1));
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
        assert_eq!(lookup_syscall(SYS_SELECT).map(|d| d.name), Some("select"));
        assert_eq!(lookup_syscall(SYS_STAT).map(|d| d.name), Some("stat"));
        assert_eq!(lookup_syscall(SYS_LSTAT).map(|d| d.name), Some("lstat"));
        assert_eq!(lookup_syscall(SYS_KILL).map(|d| d.name), Some("kill"));
        assert_eq!(lookup_syscall(SYS_UNAME).map(|d| d.name), Some("uname"));
        assert_eq!(lookup_syscall(SYS_MMAP).map(|d| d.name), Some("mmap"));
        assert_eq!(
            lookup_syscall(SYS_MPROTECT).map(|d| d.name),
            Some("mprotect")
        );
        assert_eq!(lookup_syscall(SYS_MUNMAP).map(|d| d.name), Some("munmap"));
        assert_eq!(lookup_syscall(SYS_MSYNC).map(|d| d.name), Some("msync"));
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
        assert_eq!(lookup_syscall(SYS_WRITEV).map(|d| d.name), Some("writev"));
        assert_eq!(lookup_syscall(SYS_DUP).map(|d| d.name), Some("dup"));
        assert_eq!(lookup_syscall(SYS_DUP2).map(|d| d.name), Some("dup2"));
        assert_eq!(lookup_syscall(SYS_FCNTL).map(|d| d.name), Some("fcntl"));
        assert_eq!(lookup_syscall(SYS_FSYNC).map(|d| d.name), Some("fsync"));
        assert_eq!(lookup_syscall(SYS_SHMGET).map(|d| d.name), Some("shmget"));
        assert_eq!(lookup_syscall(SYS_SHMAT).map(|d| d.name), Some("shmat"));
        assert_eq!(lookup_syscall(SYS_SHMDT).map(|d| d.name), Some("shmdt"));
        assert_eq!(lookup_syscall(SYS_SHMCTL).map(|d| d.name), Some("shmctl"));
        assert_eq!(lookup_syscall(SYS_UMASK).map(|d| d.name), Some("umask"));
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
        assert_eq!(lookup_syscall(SYS_DUP3).map(|d| d.name), Some("dup3"));
        assert_eq!(lookup_syscall(SYS_PIPE2).map(|d| d.name), Some("pipe2"));
        assert_eq!(lookup_syscall(SYS_EXECVE).map(|d| d.name), Some("execve"));
        assert_eq!(
            lookup_syscall(SYS_ARCH_PRCTL).map(|d| d.name),
            Some("arch_prctl")
        );
        assert_eq!(lookup_syscall(SYS_CLONE).map(|d| d.name), Some("clone"));
        assert_eq!(lookup_syscall(SYS_FORK).map(|d| d.name), Some("fork"));
        assert_eq!(lookup_syscall(SYS_VFORK).map(|d| d.name), Some("vfork"));
        assert_eq!(lookup_syscall(SYS_WAIT4).map(|d| d.name), Some("wait4"));
        assert_eq!(lookup_syscall(SYS_IOCTL).map(|d| d.name), Some("ioctl"));
        assert_eq!(lookup_syscall(SYS_ACCESS).map(|d| d.name), Some("access"));
        assert_eq!(
            lookup_syscall(SYS_FACCESSAT).map(|d| d.name),
            Some("faccessat")
        );
        assert_eq!(
            lookup_syscall(SYS_FACCESSAT2).map(|d| d.name),
            Some("faccessat2")
        );
        assert_eq!(lookup_syscall(SYS_GETCWD).map(|d| d.name), Some("getcwd"));
        assert_eq!(lookup_syscall(SYS_CHDIR).map(|d| d.name), Some("chdir"));
        assert_eq!(lookup_syscall(SYS_FCHDIR).map(|d| d.name), Some("fchdir"));
        assert_eq!(lookup_syscall(SYS_RENAME).map(|d| d.name), Some("rename"));
        assert_eq!(lookup_syscall(SYS_LINK).map(|d| d.name), Some("link"));
        assert_eq!(lookup_syscall(SYS_UNLINK).map(|d| d.name), Some("unlink"));
        assert_eq!(
            lookup_syscall(SYS_READLINK).map(|d| d.name),
            Some("readlink")
        );
        assert_eq!(lookup_syscall(SYS_LINKAT).map(|d| d.name), Some("linkat"));
        assert_eq!(
            lookup_syscall(SYS_SYMLINKAT).map(|d| d.name),
            Some("symlinkat")
        );
        assert_eq!(
            lookup_syscall(SYS_READLINKAT).map(|d| d.name),
            Some("readlinkat")
        );
        assert_eq!(
            lookup_syscall(SYS_UTIMENSAT).map(|d| d.name),
            Some("utimensat")
        );
        assert_eq!(
            lookup_syscall(SYS_GETTIMEOFDAY).map(|d| d.name),
            Some("gettimeofday")
        );
        assert_eq!(
            lookup_syscall(SYS_CLOCK_GETTIME).map(|d| d.name),
            Some("clock_gettime")
        );
        assert_eq!(
            lookup_syscall(SYS_NANOSLEEP).map(|d| d.name),
            Some("nanosleep")
        );
        assert_eq!(lookup_syscall(SYS_GETUID).map(|d| d.name), Some("getuid"));
        assert_eq!(lookup_syscall(SYS_GETEUID).map(|d| d.name), Some("geteuid"));
        assert_eq!(lookup_syscall(SYS_GETGID).map(|d| d.name), Some("getgid"));
        assert_eq!(lookup_syscall(SYS_GETEGID).map(|d| d.name), Some("getegid"));
        assert_eq!(lookup_syscall(SYS_SETPGID).map(|d| d.name), Some("setpgid"));
        assert_eq!(lookup_syscall(SYS_SETSID).map(|d| d.name), Some("setsid"));
        assert_eq!(lookup_syscall(SYS_GETSID).map(|d| d.name), Some("getsid"));
        assert_eq!(lookup_syscall(SYS_UTIMES).map(|d| d.name), Some("utimes"));
        assert_eq!(
            lookup_syscall(SYS_FUTIMESAT).map(|d| d.name),
            Some("futimesat")
        );
        assert_eq!(
            lookup_syscall(SYS_COPY_FILE_RANGE).map(|d| d.name),
            Some("copy_file_range")
        );
        assert_eq!(
            lookup_syscall(SYS_MEMFD_CREATE).map(|d| d.name),
            Some("memfd_create")
        );
        assert_eq!(
            lookup_syscall(SYS_EPOLL_CREATE1).map(|d| d.name),
            Some("epoll_create1")
        );
        assert_eq!(
            lookup_syscall(SYS_EPOLL_CTL).map(|d| d.name),
            Some("epoll_ctl")
        );
        assert_eq!(
            lookup_syscall(SYS_EPOLL_WAIT).map(|d| d.name),
            Some("epoll_wait")
        );
        assert_eq!(
            lookup_syscall(SYS_EPOLL_PWAIT).map(|d| d.name),
            Some("epoll_pwait")
        );
    }

    #[test]
    fn mmap_prot_translation_sets_expected_bits() {
        let rw = mmap_prot_to_pte_flags(PROT_READ | PROT_WRITE).expect("rw flags");
        assert_ne!(rw & vmm::PTE_USER, 0);
        assert_ne!(rw & vmm::PTE_WRITE, 0);
        assert_ne!(rw & vmm::PTE_NO_EXECUTE, 0);

        let rx = mmap_prot_to_pte_flags(PROT_READ | PROT_EXEC).expect("rx flags");
        assert_ne!(rx & vmm::PTE_USER, 0);
        assert_eq!(rx & vmm::PTE_WRITE, 0);
        assert_eq!(rx & vmm::PTE_NO_EXECUTE, 0);

        let none = mmap_prot_to_pte_flags(PROT_NONE).expect("none flags");
        assert_eq!(none & vmm::PTE_USER, 0);
    }

    #[test]
    fn mmap_flag_validation_rejects_unknown_bits() {
        assert!(mmap_flags_are_supported(MAP_PRIVATE | MAP_ANONYMOUS));
        assert!(!mmap_flags_are_supported(MAP_PRIVATE | 0x8000_0000));
    }

    #[test]
    fn mmap_parse_sharing_mode_requires_exactly_one_mode() {
        assert_eq!(
            mmap_parse_sharing_mode(MAP_PRIVATE).expect("private mode"),
            (true, false)
        );
        assert_eq!(
            mmap_parse_sharing_mode(MAP_SHARED).expect("shared mode"),
            (false, true)
        );
        assert_eq!(mmap_parse_sharing_mode(0), Err(-EINVAL));
        assert_eq!(
            mmap_parse_sharing_mode(MAP_PRIVATE | MAP_SHARED),
            Err(-EINVAL)
        );
    }

    #[test]
    fn mmap_regions_unmap_range_splits_middle_mapping() {
        let mut regions = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
        regions[0] = VmMapping {
            start: 0x1000,
            len: 0x4000,
            file_offset: 0,
            file_inode: task::USER_FD_NONE,
            prot: PROT_READ,
            map_shared: false,
            map_shared_can_write: false,
        };

        mmap_regions_unmap_range(&mut regions, 0x2000, 0x3000).expect("unmap split should work");

        let left = mmap_regions_find_mapping(&regions, 0x1000).expect("left segment");
        let right = mmap_regions_find_mapping(&regions, 0x3000).expect("right segment");
        assert_eq!(left.start, 0x1000);
        assert_eq!(left.len, 0x1000);
        assert_eq!(right.start, 0x3000);
        assert_eq!(right.len, 0x2000);
        assert!(mmap_regions_find_mapping(&regions, 0x2000).is_none());
    }

    #[test]
    fn mmap_regions_unmap_range_adjusts_file_offsets() {
        let mut front_trim = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
        front_trim[0] = VmMapping {
            start: 0x1000,
            len: 0x4000,
            file_offset: 0x8000,
            file_inode: 12,
            prot: PROT_READ,
            map_shared: true,
            map_shared_can_write: true,
        };
        mmap_regions_unmap_range(&mut front_trim, 0x1000, 0x2000).expect("front trim");
        let trimmed = mmap_regions_find_mapping(&front_trim, 0x2000).expect("trimmed mapping");
        assert_eq!(trimmed.start, 0x2000);
        assert_eq!(trimmed.len, 0x3000);
        assert_eq!(trimmed.file_offset, 0x9000);

        let mut middle_split = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
        middle_split[0] = VmMapping {
            start: 0x1000,
            len: 0x4000,
            file_offset: 0xA000,
            file_inode: 15,
            prot: PROT_READ,
            map_shared: true,
            map_shared_can_write: true,
        };
        mmap_regions_unmap_range(&mut middle_split, 0x2000, 0x3000).expect("middle split");
        let left = mmap_regions_find_mapping(&middle_split, 0x1000).expect("left split");
        let right = mmap_regions_find_mapping(&middle_split, 0x3000).expect("right split");
        assert_eq!(left.file_offset, 0xA000);
        assert_eq!(right.file_offset, 0xC000);
    }

    #[test]
    fn mmap_regions_unmap_range_rolls_back_when_split_slot_unavailable() {
        let mut regions = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
        regions[0] = VmMapping {
            start: 0x1000,
            len: 0x3000,
            file_offset: 0,
            file_inode: task::USER_FD_NONE,
            prot: PROT_READ,
            map_shared: false,
            map_shared_can_write: false,
        };
        for (idx, region) in regions.iter_mut().enumerate().skip(1) {
            *region = VmMapping {
                start: 0x5000 + ((idx as u64 - 1) * 0x2000),
                len: 0x1000,
                file_offset: 0,
                file_inode: task::USER_FD_NONE,
                prot: PROT_READ,
                map_shared: false,
                map_shared_can_write: false,
            };
        }
        let original = regions;

        assert_eq!(
            mmap_regions_unmap_range(&mut regions, 0x2000, 0x3000),
            Err(-ENOMEM)
        );
        assert_eq!(regions, original);
    }

    #[test]
    fn mmap_regions_protect_range_splits_and_updates_target() {
        let mut regions = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
        regions[0] = VmMapping {
            start: 0x4000,
            len: 0x4000,
            file_offset: 0,
            file_inode: task::USER_FD_NONE,
            prot: PROT_READ,
            map_shared: false,
            map_shared_can_write: false,
        };

        mmap_regions_protect_range(&mut regions, 0x5000, 0x6000, PROT_READ | PROT_WRITE)
            .expect("mprotect split should work");

        let left = mmap_regions_find_mapping(&regions, 0x4000).expect("left segment");
        let middle = mmap_regions_find_mapping(&regions, 0x5000).expect("middle segment");
        let right = mmap_regions_find_mapping(&regions, 0x6000).expect("right segment");
        assert_eq!(left.prot, PROT_READ);
        assert_eq!(middle.prot, PROT_READ | PROT_WRITE);
        assert_eq!(right.prot, PROT_READ);
    }

    #[test]
    fn mmap_regions_protect_range_rejects_shared_write_upgrade_without_mutation() {
        let mut regions = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
        regions[0] = VmMapping {
            start: 0x7000,
            len: 0x3000,
            file_offset: 0,
            file_inode: 5,
            prot: PROT_READ,
            map_shared: true,
            map_shared_can_write: false,
        };
        let original = regions;

        assert_eq!(
            mmap_regions_protect_range(&mut regions, 0x8000, 0x9000, PROT_READ | PROT_WRITE),
            Err(-EINVAL)
        );
        assert_eq!(regions, original);
    }

    #[test]
    fn mmap_regions_protect_range_rolls_back_when_split_slot_unavailable() {
        let mut regions = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
        regions[0] = VmMapping {
            start: 0x1000,
            len: 0x3000,
            file_offset: 0,
            file_inode: task::USER_FD_NONE,
            prot: PROT_READ,
            map_shared: false,
            map_shared_can_write: false,
        };
        for (idx, region) in regions.iter_mut().enumerate().skip(1) {
            *region = VmMapping {
                start: 0x5000 + ((idx as u64 - 1) * 0x2000),
                len: 0x1000,
                file_offset: 0,
                file_inode: task::USER_FD_NONE,
                prot: PROT_READ,
                map_shared: false,
                map_shared_can_write: false,
            };
        }
        let original = regions;

        assert_eq!(
            mmap_regions_protect_range(&mut regions, 0x2000, 0x3000, PROT_READ | PROT_WRITE),
            Err(-ENOMEM)
        );
        assert_eq!(regions, original);
    }

    #[test]
    fn mmap_region_allows_prot_accepts_shared_write_when_capable() {
        let region = VmMapping {
            start: 0x9000,
            len: 0x1000,
            file_offset: 0,
            file_inode: task::USER_FD_NONE,
            prot: PROT_READ,
            map_shared: true,
            map_shared_can_write: true,
        };
        assert!(mmap_region_allows_prot(&region, PROT_READ | PROT_WRITE));
    }

    #[test]
    fn mmap_region_reclaimable_only_for_safe_file_backed_cases() {
        let shared_file = VmMapping {
            start: 0x1000,
            len: PAGE_SIZE,
            file_offset: 0,
            file_inode: 7,
            prot: PROT_READ | PROT_WRITE,
            map_shared: true,
            map_shared_can_write: true,
        };
        assert!(mmap_region_is_reclaimable(&shared_file));

        let private_ro_file = VmMapping {
            start: 0x2000,
            len: PAGE_SIZE,
            file_offset: 0,
            file_inode: 8,
            prot: PROT_READ,
            map_shared: false,
            map_shared_can_write: false,
        };
        assert!(mmap_region_is_reclaimable(&private_ro_file));
    }

    #[test]
    fn mmap_region_reclaimable_rejects_unsafe_cases() {
        let anon_shared = VmMapping {
            start: 0x3000,
            len: PAGE_SIZE,
            file_offset: 0,
            file_inode: task::USER_FD_NONE,
            prot: PROT_READ | PROT_WRITE,
            map_shared: true,
            map_shared_can_write: true,
        };
        assert!(!mmap_region_is_reclaimable(&anon_shared));

        let private_writable_file = VmMapping {
            start: 0x4000,
            len: PAGE_SIZE,
            file_offset: 0,
            file_inode: 9,
            prot: PROT_READ | PROT_WRITE,
            map_shared: false,
            map_shared_can_write: false,
        };
        assert!(!mmap_region_is_reclaimable(&private_writable_file));
    }

    #[test]
    fn mmap_region_reclaimable_rejects_framebuffer_device_mapping() {
        let _guard = MMAP_TEST_LOCK.lock().expect("mmap test lock");
        let fb = fs::open("/dev/fb0", fs::O_RDWR, 0).expect("open /dev/fb0");
        let region = VmMapping {
            start: 0x5000,
            len: PAGE_SIZE,
            file_offset: 0,
            file_inode: fb.inode,
            prot: PROT_READ | PROT_WRITE,
            map_shared: true,
            map_shared_can_write: true,
        };
        assert!(!mmap_region_is_reclaimable(&region));
    }

    #[test]
    fn release_unmapped_user_page_treats_framebuffer_as_device_mapping() {
        let _guard = MMAP_TEST_LOCK.lock().expect("mmap test lock");
        let fb = fs::open("/dev/fb0", fs::O_RDWR, 0).expect("open /dev/fb0");
        let region = VmMapping {
            start: 0x6000,
            len: PAGE_SIZE,
            file_offset: 0,
            file_inode: fb.inode,
            prot: PROT_READ | PROT_WRITE,
            map_shared: true,
            map_shared_can_write: true,
        };
        assert_eq!(
            release_unmapped_user_page(Some(region), PhysAddr::new(0x2000)),
            UnmappedPageRelease::Device
        );
    }

    #[test]
    fn msync_flag_validation_rejects_unknown_or_conflicting_bits() {
        assert!(msync_flags_are_valid(0));
        assert!(msync_flags_are_valid(MS_SYNC));
        assert!(msync_flags_are_valid(MS_ASYNC));
        assert!(msync_flags_are_valid(MS_SYNC | MS_INVALIDATE));
        assert!(!msync_flags_are_valid(MS_SYNC | MS_ASYNC));
        assert!(!msync_flags_are_valid(0x8000_0000));
    }

    #[test]
    fn mmap_prot_fault_access_checks_expected_modes() {
        assert!(mmap_prot_allows_fault_access(PROT_READ, 0));
        assert!(mmap_prot_allows_fault_access(
            PROT_WRITE,
            PAGE_FAULT_ERROR_WRITE
        ));
        assert!(!mmap_prot_allows_fault_access(
            PROT_READ,
            PAGE_FAULT_ERROR_WRITE
        ));
        assert!(mmap_prot_allows_fault_access(
            PROT_EXEC,
            PAGE_FAULT_ERROR_INSTR
        ));
        assert!(!mmap_prot_allows_fault_access(
            PROT_READ,
            PAGE_FAULT_ERROR_INSTR
        ));
        assert!(!mmap_prot_allows_fault_access(PROT_NONE, 0));
    }

    #[test]
    fn vm_page_cache_release_unknown_phys_returns_false() {
        assert!(!vm_page_cache_release_mapped_phys(PhysAddr::new(
            0x1234_5000
        )));
    }

    #[test]
    fn arch_prctl_set_fs_rejects_kernel_addresses() {
        assert!(arch_prctl_set_fs_addr_ok(0));
        assert!(arch_prctl_set_fs_addr_ok(USER_CANON_MAX));
        assert!(!arch_prctl_set_fs_addr_ok(USER_CANON_MAX + 1));
        assert!(!arch_prctl_set_fs_addr_ok(KERNEL_VIRT_BASE));
    }

    #[test]
    fn fd_cloexec_roundtrip_helpers() {
        let fd_flags = fd_flags_from_open_flags(fs::O_RDWR | fs::O_APPEND | O_CLOEXEC);
        assert!(fd_has_cloexec(fd_flags));
        assert_eq!(fd_open_flags(fd_flags), fs::O_RDWR | fs::O_APPEND);

        let cleared = fd_set_cloexec(fd_flags, false);
        assert!(!fd_has_cloexec(cleared));
        assert_eq!(fd_open_flags(cleared), fs::O_RDWR | fs::O_APPEND);
    }

    #[test]
    fn fcntl_setfl_applies_nonblock_without_touching_cloexec_or_access_mode() {
        let initial = fd_flags_from_open_flags(fs::O_RDONLY | O_CLOEXEC);
        let updated = fd_apply_fcntl_setfl(initial, task::USER_FD_KIND_PIPE, O_NONBLOCK);
        assert!(fd_has_cloexec(updated));
        assert_eq!(fd_open_flags(updated) & fs::O_ACCMODE, fs::O_RDONLY);
        assert!((fd_open_flags(updated) & O_NONBLOCK) != 0);
    }

    #[test]
    fn fcntl_setfl_ignores_unsettable_bits_for_descriptor_kind() {
        let initial = fd_flags_from_open_flags(fs::O_RDONLY);
        let updated = fd_apply_fcntl_setfl(
            initial,
            task::USER_FD_KIND_SOCKET,
            fs::O_APPEND | O_NONBLOCK,
        );
        assert_eq!(fd_open_flags(updated) & fs::O_ACCMODE, fs::O_RDONLY);
        assert!((fd_open_flags(updated) & O_NONBLOCK) != 0);
        assert_eq!(fd_open_flags(updated) & fs::O_APPEND, 0);
    }

    #[test]
    fn fcntl_setfl_preserves_internal_memfd_tag() {
        let initial = fd_flags_from_open_flags(fs::O_RDWR) | FD_INTERNAL_MEMFD;
        let updated = fd_apply_fcntl_setfl(initial, task::USER_FD_KIND_VFS, fs::O_APPEND);
        assert!(fd_is_memfd(updated));
        assert_eq!(fd_open_flags(updated) & fs::O_APPEND, fs::O_APPEND);
    }

    #[test]
    fn socket_raw_type_parsing_rejects_unknown_bits() {
        assert_eq!(
            socket_type_base_and_open_flags(SOCK_STREAM | SOCK_NONBLOCK),
            Ok((SOCK_STREAM, fs::O_RDWR | O_NONBLOCK))
        );
        assert_eq!(
            socket_type_base_and_open_flags(SOCK_DGRAM),
            Ok((SOCK_DGRAM, fs::O_RDWR))
        );
        assert_eq!(
            socket_type_base_and_open_flags(SOCK_STREAM | (1 << 30)),
            Err(-EINVAL)
        );
    }

    #[test]
    fn accept4_flag_parsing_rejects_unsupported_bits() {
        assert_eq!(
            accept4_open_flags(SOCK_CLOEXEC | SOCK_NONBLOCK),
            Ok(fs::O_RDWR | O_CLOEXEC | O_NONBLOCK)
        );
        assert_eq!(accept4_open_flags(0x40), Err(-EINVAL));
    }

    #[test]
    fn memfd_flag_validation_accepts_cloexec_and_rejects_unsupported() {
        assert_eq!(memfd_validate_flags(0), Ok(false));
        assert_eq!(memfd_validate_flags(MFD_CLOEXEC), Ok(true));
        assert_eq!(memfd_validate_flags(MFD_ALLOW_SEALING), Err(-ENOSYS));
        assert_eq!(memfd_validate_flags(0x1000), Err(-EINVAL));
    }

    #[test]
    fn framebuffer_ioctl_validates_arg_pointer_and_availability() {
        let ctx = SyscallContext::new(core::ptr::null_mut());
        assert_eq!(ioctl_framebuffer(&ctx, FBIOGETINFO, 0), -EFAULT);
        assert_eq!(ioctl_framebuffer(&ctx, FBIOGETINFO, 0x1000), -ENXIO);
        assert_eq!(ioctl_framebuffer(&ctx, 0xDEAD, 0x1000), -ENOTTY);
    }

    #[test]
    fn memfd_name_sanitize_rewrites_unsupported_bytes() {
        let mut out = [0u8; MEMFD_NAME_MAX];
        let len = memfd_name_sanitize(b"gui/buffer:\xff", &mut out);
        assert_eq!(&out[..len], b"gui_buffer__");

        let empty_len = memfd_name_sanitize(b"", &mut out);
        assert_eq!(&out[..empty_len], b"anon");
    }

    #[test]
    fn memfd_build_path_includes_pid_sequence_and_name() {
        let mut buf = [0u8; fs::VFS_MAX_PATH];
        let path = memfd_build_path(42, 7, b"front/buf#1", &mut buf).expect("path");
        assert_eq!(path, "/temp/.memfd/mfd-42-7-front_buf_1");
    }

    #[test]
    fn memfd_storage_dir_ready_creates_internal_dir_and_is_idempotent() {
        let _guard = MMAP_TEST_LOCK.lock().expect("mmap test lock");
        reset_vm_for_test();

        assert_eq!(fs::lookup("/temp/.memfd"), Err(fs::ENOENT));
        assert_eq!(memfd_storage_dir_ready(), Ok(()));

        let inode = fs::lookup("/temp/.memfd").expect("memfd storage dir");
        assert_eq!(fs::inode_is_dir(inode), Ok(true));
        assert_eq!(memfd_storage_dir_ready(), Ok(()));
    }

    #[test]
    fn mmap_validate_shared_write_window_tracks_resize_and_bounds() {
        let _guard = MMAP_TEST_LOCK.lock().expect("mmap test lock");
        reset_vm_for_test();

        let opened = fs::open("/temp/shared-buffer", fs::O_CREAT | fs::O_RDWR, 0o600)
            .expect("shared buffer create");
        let inode = opened.inode;

        assert_eq!(
            mmap_validate_shared_write_window(inode, 0, PAGE_SIZE),
            Err(-EINVAL)
        );

        fs::truncate_to(inode, PAGE_SIZE * 2).expect("extend backing file");
        assert_eq!(
            mmap_validate_shared_write_window(inode, 0, PAGE_SIZE * 2),
            Ok(())
        );
        assert_eq!(
            mmap_validate_shared_write_window(inode, PAGE_SIZE, PAGE_SIZE),
            Ok(())
        );

        fs::truncate_to(inode, PAGE_SIZE).expect("shrink backing file");
        assert_eq!(
            mmap_validate_shared_write_window(inode, 0, PAGE_SIZE),
            Ok(())
        );
        assert_eq!(
            mmap_validate_shared_write_window(inode, 0, PAGE_SIZE * 2),
            Err(-EINVAL)
        );
        assert_eq!(
            mmap_validate_shared_write_window(inode, PAGE_SIZE, PAGE_SIZE),
            Err(-EINVAL)
        );
    }

    #[test]
    fn memfd_object_accounting_deduplicates_by_inode() {
        let mut task = task::Task::for_test(63);
        let task_ptr = &mut task as *mut task::Task;
        let memfd_flags = fd_flags_from_open_flags(fs::O_RDWR) | FD_INTERNAL_MEMFD;

        fd_write_slot(task_ptr, 3, task::USER_FD_KIND_VFS, 40, 0, memfd_flags);
        fd_write_slot(task_ptr, 4, task::USER_FD_KIND_VFS, 40, 0, memfd_flags);
        fd_write_slot(task_ptr, 5, task::USER_FD_KIND_VFS, 41, 0, memfd_flags);
        fd_write_slot(
            task_ptr,
            6,
            task::USER_FD_KIND_VFS,
            80,
            0,
            fd_flags_from_open_flags(fs::O_RDWR),
        );

        assert_eq!(task_memfd_object_count(task_ptr), 2);
        assert!(task_memfd_contains_inode(task_ptr, 40));
        assert!(!task_memfd_contains_inode(task_ptr, 80));
    }

    #[test]
    fn memfd_object_limit_blocks_new_inode_only() {
        let mut task = task::Task::for_test(64);
        let task_ptr = &mut task as *mut task::Task;
        let memfd_flags = fd_flags_from_open_flags(fs::O_RDWR) | FD_INTERNAL_MEMFD;
        let mut fd = 3usize;
        while fd < 3 + MEMFD_MAX_BUFFERS_PER_TASK {
            fd_write_slot(
                task_ptr,
                fd,
                task::USER_FD_KIND_VFS,
                (200 + fd) as u16,
                0,
                memfd_flags,
            );
            fd += 1;
        }

        assert_eq!(
            task_memfd_object_count(task_ptr),
            MEMFD_MAX_BUFFERS_PER_TASK
        );
        assert_eq!(enforce_memfd_create_limit(task_ptr), Err(-EMFILE));
        assert_eq!(enforce_memfd_object_limit_for_inode(task_ptr, 205), Ok(()));
        assert_eq!(
            enforce_memfd_object_limit_for_inode(task_ptr, 999),
            Err(-EMFILE)
        );
    }

    #[test]
    fn mmap_clone_task_regions_copies_shared_writable_mapping_metadata_per_task() {
        let _guard = MMAP_TEST_LOCK.lock().expect("mmap test lock");
        reset_vm_for_test();

        let mut parent = task::Task::for_test(201);
        let mut child = task::Task::for_test(202);
        let parent_ptr = &mut parent as *mut task::Task;
        let child_ptr = &mut child as *mut task::Task;

        task::Task::install_for_test(0, parent_ptr);
        task::Task::install_for_test(1, child_ptr);

        let parent_slot = mmap_prepare_task_slot(parent_ptr).expect("parent slot");
        let mapping = VmMapping {
            start: 0x4000_0000,
            len: PAGE_SIZE * 2,
            file_offset: PAGE_SIZE,
            file_inode: 77,
            prot: PROT_READ | PROT_WRITE,
            map_shared: true,
            map_shared_can_write: true,
        };
        unsafe {
            TASK_VM_MAPPINGS[parent_slot][0] = mapping;
        }

        mmap_clone_task_regions(parent_ptr, child_ptr).expect("clone shared mappings");
        let child_slot = mmap_prepare_task_slot(child_ptr).expect("child slot");
        assert_eq!(unsafe { TASK_VM_MAPPINGS[child_slot][0] }, mapping);

        unsafe {
            TASK_VM_MAPPINGS[parent_slot][0].len = PAGE_SIZE;
        }
        assert_eq!(unsafe { TASK_VM_MAPPINGS[child_slot][0] }, mapping);
    }

    #[test]
    fn mmap_clear_task_regions_only_clears_target_task_slot() {
        let _guard = MMAP_TEST_LOCK.lock().expect("mmap test lock");
        reset_vm_for_test();

        let mut parent = task::Task::for_test(211);
        let mut child = task::Task::for_test(212);
        let parent_ptr = &mut parent as *mut task::Task;
        let child_ptr = &mut child as *mut task::Task;

        task::Task::install_for_test(0, parent_ptr);
        task::Task::install_for_test(1, child_ptr);

        let parent_slot = mmap_prepare_task_slot(parent_ptr).expect("parent slot");
        let child_slot = mmap_prepare_task_slot(child_ptr).expect("child slot");
        let parent_mapping = VmMapping {
            start: 0x5000_0000,
            len: PAGE_SIZE,
            file_offset: 0,
            file_inode: 91,
            prot: PROT_READ | PROT_WRITE,
            map_shared: true,
            map_shared_can_write: true,
        };
        let child_mapping = VmMapping {
            start: 0x6000_0000,
            len: PAGE_SIZE,
            file_offset: PAGE_SIZE,
            file_inode: 91,
            prot: PROT_READ | PROT_WRITE,
            map_shared: true,
            map_shared_can_write: true,
        };
        unsafe {
            TASK_VM_MAPPINGS[parent_slot][0] = parent_mapping;
            TASK_VM_MAPPINGS[child_slot][0] = child_mapping;
        }

        mmap_clear_task_regions(parent_ptr);

        assert!(unsafe { TASK_VM_MAPPINGS[parent_slot][0].is_empty() });
        assert_eq!(unsafe { TASK_VM_MAPPINGS[child_slot][0] }, child_mapping);
    }

    #[test]
    fn fd_dup_helpers_apply_requested_cloexec() {
        let mut task = task::Task::for_test(7);
        let task_ptr = &mut task as *mut task::Task;
        fd_write_slot(
            task_ptr,
            5,
            task::USER_FD_KIND_VFS,
            42,
            11,
            fd_set_cloexec(fs::O_RDONLY, true),
        );

        let plain = fd_dup_from(task_ptr, 5, 0, false).expect("dup without cloexec");
        let plain_idx = plain as usize;
        assert_eq!(plain_idx, 0);
        assert!(!fd_has_cloexec(unsafe {
            (*task_ptr).user_fd_flags[plain_idx]
        }));
        assert_eq!(unsafe { (*task_ptr).user_fd_offset[plain_idx] }, 11);

        let clo = fd_dup_from(task_ptr, 5, 1, true).expect("dup with cloexec");
        let clo_idx = clo as usize;
        assert_eq!(clo_idx, 1);
        assert!(fd_has_cloexec(unsafe {
            (*task_ptr).user_fd_flags[clo_idx]
        }));
        assert_eq!(unsafe { (*task_ptr).user_fd_inode[clo_idx] }, 42);
    }

    #[test]
    fn epoll_create1_validates_flags_and_installs_fd() {
        let _guard = EPOLL_TEST_LOCK.lock().expect("epoll test lock");
        reset_epoll_for_test();

        let mut task = task::Task::for_test(31);
        let ctx = SyscallContext::new(&mut task as *mut task::Task);

        let bad = SyscallArgs {
            num: SYS_EPOLL_CREATE1,
            args: [0x1000, 0, 0, 0, 0, 0],
        };
        assert_eq!(sys_epoll_create1(&ctx, &bad), -EINVAL);

        let good = SyscallArgs {
            num: SYS_EPOLL_CREATE1,
            args: [EPOLL_CLOEXEC as u64, 0, 0, 0, 0, 0],
        };
        let epfd = sys_epoll_create1(&ctx, &good);
        assert!(epfd >= 0);
        let idx = epfd as usize;
        assert_eq!(task.user_fd_kind[idx], task::USER_FD_KIND_EPOLL);
        assert!(fd_has_cloexec(task.user_fd_flags[idx]));
        let _ = fd_close(ctx.current, epfd as i32);
    }

    #[test]
    fn epoll_pwait_supports_null_sigmask_compat_only() {
        let _guard = EPOLL_TEST_LOCK.lock().expect("epoll test lock");
        reset_epoll_for_test();

        let mut task = task::Task::for_test(32);
        let ctx = SyscallContext::new(&mut task as *mut task::Task);

        let null_sigmask = SyscallArgs {
            num: SYS_EPOLL_PWAIT,
            args: [0, 1, 0, 0, 0, 8],
        };
        assert_eq!(sys_epoll_pwait(&ctx, &null_sigmask), -EINVAL);

        let masked = SyscallArgs {
            num: SYS_EPOLL_PWAIT,
            args: [0, 1, 1, 0, 0x1000, 8],
        };
        assert_eq!(sys_epoll_pwait(&ctx, &masked), -ENOSYS);
    }

    #[test]
    fn epoll_ctl_add_mod_del_enforces_duplicate_and_missing_semantics() {
        let _guard = EPOLL_TEST_LOCK.lock().expect("epoll test lock");
        reset_epoll_for_test();

        let mut owner = task::Task::for_test(41);
        let owner_ptr = &mut owner as *mut task::Task;
        let epoll_id = epoll_alloc_instance().expect("epoll alloc");
        let inst = epoll_instance_get_mut(epoll_id).expect("epoll instance");

        assert_eq!(
            epoll_ctl_add(
                inst,
                41,
                owner_ptr,
                5,
                task::USER_FD_KIND_VFS,
                9,
                EPOLLIN,
                0xAA
            ),
            0
        );
        assert_eq!(
            epoll_ctl_add(
                inst,
                41,
                owner_ptr,
                5,
                task::USER_FD_KIND_VFS,
                9,
                EPOLLIN,
                0xAA
            ),
            -EEXIST
        );
        assert_eq!(
            epoll_ctl_mod(inst, 41, 7, task::USER_FD_KIND_VFS, 9, EPOLLOUT, 0xBB),
            -ENOENT
        );
        assert_eq!(
            epoll_ctl_mod(inst, 41, 5, task::USER_FD_KIND_VFS, 9, EPOLLOUT, 0xCC),
            0
        );
        let slot = epoll_watch_find(inst, 41, 5).expect("watch slot");
        assert_eq!(inst.watches[slot].events, EPOLLOUT);
        assert_eq!(inst.watches[slot].data, 0xCC);
        assert_eq!(epoll_ctl_del(inst, 41, 6), -ENOENT);
        assert_eq!(epoll_ctl_del(inst, 41, 5), 0);
        assert_eq!(inst.watch_count, 0);
    }

    #[test]
    fn epoll_owner_watch_limit_and_owner_cleanup_work_across_instances() {
        let _guard = EPOLL_TEST_LOCK.lock().expect("epoll test lock");
        reset_epoll_for_test();

        let mut owner = task::Task::for_test(44);
        let owner_ptr = &mut owner as *mut task::Task;
        let epoll_a = epoll_alloc_instance().expect("epoll a");
        let epoll_b = epoll_alloc_instance().expect("epoll b");
        let inst_a = epoll_instance_get_mut(epoll_a).expect("epoll instance a");

        for fd in 0..40 {
            assert_eq!(
                epoll_ctl_add(
                    inst_a,
                    44,
                    owner_ptr,
                    fd,
                    task::USER_FD_KIND_VFS,
                    (1000 + fd) as u16,
                    EPOLLIN,
                    fd as u64,
                ),
                0
            );
        }

        let inst_b = epoll_instance_get_mut(epoll_b).expect("epoll instance b");
        for fd in 40..EPOLL_MAX_WATCHES_PER_TASK as i32 {
            assert_eq!(
                epoll_ctl_add(
                    inst_b,
                    44,
                    owner_ptr,
                    fd,
                    task::USER_FD_KIND_VFS,
                    (1000 + fd) as u16,
                    EPOLLIN,
                    fd as u64,
                ),
                0
            );
        }
        assert_eq!(
            epoll_ctl_add(
                inst_b,
                44,
                owner_ptr,
                999,
                task::USER_FD_KIND_VFS,
                1999,
                EPOLLIN,
                0,
            ),
            -ENOSPC
        );
        assert_eq!(epoll_watch_count_for_owner(44), EPOLL_MAX_WATCHES_PER_TASK);

        epoll_drop_watches_for_owner(44);
        assert_eq!(epoll_watch_count_for_owner(44), 0);
        assert_eq!(inst_a.watch_count, 0);
        assert_eq!(inst_b.watch_count, 0);
    }

    #[test]
    fn epoll_collect_ready_is_level_triggered_for_vfs_fds() {
        let _guard = EPOLL_TEST_LOCK.lock().expect("epoll test lock");
        reset_epoll_for_test();

        let mut owner = task::Task::for_test(51);
        let owner_ptr = &mut owner as *mut task::Task;
        fd_write_slot(owner_ptr, 3, task::USER_FD_KIND_VFS, 12, 0, fs::O_RDONLY);

        let epoll_id = epoll_alloc_instance().expect("epoll alloc");
        let inst = epoll_instance_get_mut(epoll_id).expect("epoll instance");
        assert_eq!(
            epoll_ctl_add(
                inst,
                51,
                owner_ptr,
                3,
                task::USER_FD_KIND_VFS,
                12,
                EPOLLIN,
                0x1234
            ),
            0
        );

        let mut out = [(0u32, 0u64); 2];
        let ready1 = epoll_collect_ready(inst, &mut out);
        assert_eq!(ready1, 1);
        assert_eq!(out[0], (EPOLLIN, 0x1234));

        out[0] = (0, 0);
        let ready2 = epoll_collect_ready(inst, &mut out);
        assert_eq!(ready2, 1);
        assert_eq!(out[0], (EPOLLIN, 0x1234));
    }

    #[test]
    fn epoll_collect_ready_drops_stale_watch_after_fd_close() {
        let _guard = EPOLL_TEST_LOCK.lock().expect("epoll test lock");
        reset_epoll_for_test();

        let mut owner = task::Task::for_test(61);
        let owner_ptr = &mut owner as *mut task::Task;
        fd_write_slot(owner_ptr, 4, task::USER_FD_KIND_VFS, 77, 0, fs::O_RDONLY);

        let epoll_id = epoll_alloc_instance().expect("epoll alloc");
        let inst = epoll_instance_get_mut(epoll_id).expect("epoll instance");
        assert_eq!(
            epoll_ctl_add(
                inst,
                61,
                owner_ptr,
                4,
                task::USER_FD_KIND_VFS,
                77,
                EPOLLIN,
                0x55
            ),
            0
        );

        let _ = fd_close(owner_ptr, 4);
        let mut out = [(0u32, 0u64); 1];
        let ready = epoll_collect_ready(inst, &mut out);
        assert_eq!(ready, 0);
        assert_eq!(inst.watch_count, 0);
        assert!(epoll_watch_find(inst, 61, 4).is_none());
    }

    #[test]
    fn epoll_revents_always_include_hup_and_err_bits() {
        let events = poll_revents_to_epoll_events(0, POLLHUP | POLLERR);
        assert_eq!(events & EPOLLHUP, EPOLLHUP);
        assert_eq!(events & EPOLLERR, EPOLLERR);
    }

    #[test]
    fn mmap_regions_total_pages_sums_non_empty_regions() {
        let mut regions = [VmMapping::EMPTY; MMAP_MAX_REGIONS_PER_TASK];
        regions[0] = VmMapping {
            start: 0x1000,
            len: PAGE_SIZE,
            file_offset: 0,
            file_inode: task::USER_FD_NONE,
            prot: PROT_READ,
            map_shared: false,
            map_shared_can_write: false,
        };
        regions[1] = VmMapping {
            start: 0x3000,
            len: PAGE_SIZE * 2,
            file_offset: 0,
            file_inode: task::USER_FD_NONE,
            prot: PROT_READ,
            map_shared: false,
            map_shared_can_write: false,
        };
        assert_eq!(mmap_regions_total_pages(&regions).expect("page count"), 3);
        assert!(mmap_regions_total_pages(&regions).expect("count") < MMAP_MAX_PAGES_PER_TASK);
    }

    #[test]
    fn epoll_pipe_read_hup_matches_poll_core_revents() {
        let _guard = EPOLL_TEST_LOCK.lock().expect("epoll test lock");
        reset_epoll_for_test();
        ipc::init();

        let mut owner = task::Task::for_test(71);
        let owner_ptr = &mut owner as *mut task::Task;
        let pipe = ipc::pipe_open().expect("pipe open");

        fd_write_slot(owner_ptr, 3, task::USER_FD_KIND_PIPE, pipe, 0, fs::O_RDONLY);
        fd_write_slot(owner_ptr, 4, task::USER_FD_KIND_PIPE, pipe, 0, fs::O_WRONLY);

        ipc::pipe_close(pipe, false, true).expect("close writer end");

        let requested = EPOLLIN;
        let poll_requested = epoll_events_to_poll_mask(requested);
        let revents = poll_fd_revents(owner_ptr, 3, poll_requested);
        let expected_events = poll_revents_to_epoll_events(requested, revents);
        assert_eq!(expected_events & EPOLLHUP, EPOLLHUP);

        let epoll_id = epoll_alloc_instance().expect("epoll alloc");
        let inst = epoll_instance_get_mut(epoll_id).expect("epoll instance");
        assert_eq!(
            epoll_ctl_add(
                inst,
                71,
                owner_ptr,
                3,
                task::USER_FD_KIND_PIPE,
                pipe,
                requested,
                0xA1
            ),
            0
        );

        let mut out = [(0u32, 0u64); 1];
        let ready = epoll_collect_ready(inst, &mut out);
        assert_eq!(ready, 1);
        assert_eq!(out[0], (expected_events, 0xA1));
    }

    #[test]
    fn epoll_pipe_write_err_matches_poll_core_revents() {
        let _guard = EPOLL_TEST_LOCK.lock().expect("epoll test lock");
        reset_epoll_for_test();
        ipc::init();

        let mut owner = task::Task::for_test(72);
        let owner_ptr = &mut owner as *mut task::Task;
        let pipe = ipc::pipe_open().expect("pipe open");

        fd_write_slot(owner_ptr, 3, task::USER_FD_KIND_PIPE, pipe, 0, fs::O_RDONLY);
        fd_write_slot(owner_ptr, 4, task::USER_FD_KIND_PIPE, pipe, 0, fs::O_WRONLY);

        ipc::pipe_close(pipe, true, false).expect("close reader end");

        let requested = EPOLLOUT;
        let poll_requested = epoll_events_to_poll_mask(requested);
        let revents = poll_fd_revents(owner_ptr, 4, poll_requested);
        let expected_events = poll_revents_to_epoll_events(requested, revents);
        assert_eq!(expected_events & EPOLLERR, EPOLLERR);

        let epoll_id = epoll_alloc_instance().expect("epoll alloc");
        let inst = epoll_instance_get_mut(epoll_id).expect("epoll instance");
        assert_eq!(
            epoll_ctl_add(
                inst,
                72,
                owner_ptr,
                4,
                task::USER_FD_KIND_PIPE,
                pipe,
                requested,
                0xB2
            ),
            0
        );

        let mut out = [(0u32, 0u64); 1];
        let ready = epoll_collect_ready(inst, &mut out);
        assert_eq!(ready, 1);
        assert_eq!(out[0], (expected_events, 0xB2));
    }

    #[test]
    fn poll_reports_input_devices_only_when_events_are_queued() {
        let _guard = INPUT_TEST_LOCK.lock().expect("input test lock");
        crate::input::init();
        fs::init();

        let mut owner = task::Task::for_test(73);
        let owner_ptr = &mut owner as *mut task::Task;
        let opened = fs::open("/dev/input/event0", fs::O_RDONLY, 0).expect("open event0");
        fd_write_slot(
            owner_ptr,
            3,
            task::USER_FD_KIND_VFS,
            opened.inode,
            0,
            fs::O_RDONLY,
        );

        assert_eq!(poll_fd_revents(owner_ptr, 3, POLLIN | POLLOUT), 0);

        crate::input::push_keyboard_key(crate::input::KEY_A, true);
        assert_eq!(poll_fd_revents(owner_ptr, 3, POLLIN | POLLOUT), POLLIN);
    }

    #[test]
    fn cmsg_align_rounds_up_to_word_boundary() {
        assert_eq!(cmsg_align(1), size_of::<usize>());
        assert_eq!(cmsg_align(size_of::<usize>()), size_of::<usize>());
        assert_eq!(cmsg_align(size_of::<usize>() + 1), size_of::<usize>() * 2);
    }

    #[test]
    fn unix_fd_transfer_rejects_non_transferable_kind() {
        let mut task = task::Task::for_test(21);
        let task_ptr = &mut task as *mut task::Task;
        fd_write_slot(
            task_ptr,
            6,
            task::USER_FD_KIND_CONSOLE,
            task::USER_FD_NONE,
            0,
            fs::O_RDWR,
        );
        assert_eq!(unix_fd_transfer_from_task(task_ptr, 6), Err(-EOPNOTSUPP));
    }

    #[test]
    fn install_received_right_sets_cloexec_when_requested() {
        let mut task = task::Task::for_test(22);
        let task_ptr = &mut task as *mut task::Task;

        let right = socket::UnixRightsFd {
            kind: task::USER_FD_KIND_VFS,
            inode: 77,
            flags: fs::O_RDWR,
            offset: 123,
        };
        let fd = install_received_right(task_ptr, right, true).expect("install right");
        let idx = fd as usize;
        assert!(fd_has_cloexec(unsafe { (*task_ptr).user_fd_flags[idx] }));
        assert_eq!(unsafe { (*task_ptr).user_fd_inode[idx] }, 77);
        assert_eq!(unsafe { (*task_ptr).user_fd_offset[idx] }, 123);
    }

    #[test]
    fn close_cloexec_fds_keeps_non_cloexec_entries() {
        let mut task = task::Task::for_test(8);
        let task_ptr = &mut task as *mut task::Task;
        fd_write_slot(
            task_ptr,
            3,
            task::USER_FD_KIND_VFS,
            10,
            0,
            fd_set_cloexec(fs::O_RDONLY, true),
        );
        fd_write_slot(task_ptr, 4, task::USER_FD_KIND_VFS, 11, 3, fs::O_RDWR);

        close_cloexec_fds(task_ptr);

        assert!(!fd_slot_is_open(task_ptr, 3));
        assert!(fd_slot_is_open(task_ptr, 4));
        assert_eq!(unsafe { (*task_ptr).user_fd_inode[4] }, 11);
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

    #[test]
    fn access_mode_validation_rejects_unknown_bits() {
        assert!(access_mode_is_valid(F_OK));
        assert!(access_mode_is_valid(R_OK | W_OK | X_OK));
        assert!(!access_mode_is_valid(0x80));
    }

    #[test]
    fn access_allowed_uses_owner_bits_for_current_uid() {
        let stat = fs::FileStat {
            ino: 1,
            mode: STAT_S_IRUSR | STAT_S_IWUSR,
            uid: 1000,
            gid: 1000,
            nlink: 1,
            size: 0,
            blksize: 0,
            blocks: 0,
            atime: 0,
            mtime: 0,
            ctime: 0,
            file_type: fs::FileType::Regular,
        };
        assert!(access_allowed(&stat, R_OK, 1000, 1000));
        assert!(!access_allowed(&stat, X_OK, 1000, 1000));
    }

    #[test]
    fn ticks_to_seconds_nanos_matches_pit_frequency() {
        assert_eq!(ticks_to_seconds_nanos(100), (1, 0));
        assert_eq!(ticks_to_seconds_nanos(123), (1, 230_000_000));
    }

    #[test]
    fn timespec_to_sleep_ticks_rounds_up_partial_tick() {
        let one_ns = LinuxTimeSpec {
            tv_sec: 0,
            tv_nsec: 1,
        };
        assert_eq!(timespec_to_sleep_ticks(&one_ns), Ok(1));

        let ten_ms = LinuxTimeSpec {
            tv_sec: 0,
            tv_nsec: 10_000_000,
        };
        assert_eq!(timespec_to_sleep_ticks(&ten_ms), Ok(1));

        let invalid = LinuxTimeSpec {
            tv_sec: 0,
            tv_nsec: 1_000_000_000,
        };
        assert_eq!(timespec_to_sleep_ticks(&invalid), Err(-EINVAL));
    }

    #[test]
    fn utimens_timespec_special_values_are_handled() {
        assert_eq!(
            timespec_to_unix_seconds(
                LinuxTimeSpec {
                    tv_sec: -1,
                    tv_nsec: UTIME_NOW,
                },
                77,
                11
            ),
            Ok(77)
        );
        assert_eq!(
            timespec_to_unix_seconds(
                LinuxTimeSpec {
                    tv_sec: -1,
                    tv_nsec: UTIME_OMIT,
                },
                77,
                11
            ),
            Ok(11)
        );
    }

    #[test]
    fn utimens_timespec_rejects_invalid_nsec_or_negative_sec() {
        assert_eq!(
            timespec_to_unix_seconds(
                LinuxTimeSpec {
                    tv_sec: -1,
                    tv_nsec: 0,
                },
                1,
                1
            ),
            Err(-EINVAL)
        );
        assert_eq!(
            timespec_to_unix_seconds(
                LinuxTimeSpec {
                    tv_sec: 0,
                    tv_nsec: 1_000_000_000,
                },
                1,
                1
            ),
            Err(-EINVAL)
        );
    }

    #[test]
    fn wait_status_encodes_exit_code_for_waitpid_macros() {
        assert_eq!(wait_status_from_exit_code(0), 0);
        assert_eq!(wait_status_from_exit_code(1), 0x0100);
        assert_eq!(wait_status_from_exit_code(255), 0xFF00_u32 as i32);
    }

    #[test]
    fn pid_arg_zero_maps_to_current_pid() {
        assert_eq!(pid_from_syscall_arg_or_current(0, 42), Ok(42));
        assert_eq!(pid_from_syscall_arg_or_current(7, 42), Ok(7));
        assert_eq!(
            pid_from_syscall_arg_or_current((-1_i64) as u64, 42),
            Err(-EINVAL)
        );
    }

    #[test]
    fn pgid_arg_zero_maps_to_target_pid() {
        assert_eq!(pgid_from_syscall_arg_or_target(0, 55), Ok(55));
        assert_eq!(pgid_from_syscall_arg_or_target(9, 55), Ok(9));
        assert_eq!(
            pgid_from_syscall_arg_or_target((-1_i64) as u64, 55),
            Err(-EINVAL)
        );
    }

    #[test]
    fn setsid_rejects_process_group_leader() {
        let mut task = task::Task::for_test(99);
        let ctx = SyscallContext::new(&mut task as *mut task::Task);
        let call = SyscallArgs {
            num: 0,
            args: [0; 6],
        };
        assert_eq!(sys_setsid(&ctx, &call), -EPERM);
    }

    #[test]
    fn setsid_promotes_non_leader_to_session_leader() {
        let mut task = task::Task::for_test(100);
        task.pgid = 77;
        task.sid = 77;
        let ctx = SyscallContext::new(&mut task as *mut task::Task);
        let call = SyscallArgs {
            num: 0,
            args: [0; 6],
        };

        assert_eq!(sys_setsid(&ctx, &call), 100);
        assert_eq!(task.pgid, 100);
        assert_eq!(task.sid, 100);
    }

    #[test]
    fn getsid_rejects_negative_pid_argument() {
        let mut task = task::Task::for_test(10);
        let ctx = SyscallContext::new(&mut task as *mut task::Task);
        let call = SyscallArgs {
            num: 0,
            args: [(-1_i64) as u64, 0, 0, 0, 0, 0],
        };
        assert_eq!(sys_getsid(&ctx, &call), -EINVAL);
    }

    #[test]
    fn clone_args_accept_fork_compatible_flags() {
        let call = SyscallArgs {
            num: SYS_CLONE,
            args: [SIGCHLD_EXIT_SIGNAL, 0, 0, 0, 0, 0],
        };
        assert_eq!(validate_clone_args(&call), Ok(0));
    }

    #[test]
    fn clone_args_accept_vfork_compat_flags() {
        let call = SyscallArgs {
            num: SYS_CLONE,
            args: [CLONE_VM | CLONE_VFORK | SIGCHLD_EXIT_SIGNAL, 0, 0, 0, 0, 0],
        };
        assert_eq!(validate_clone_args(&call), Ok(0));
    }

    #[test]
    fn clone_args_reject_partial_vm_vfork_combo() {
        let call = SyscallArgs {
            num: SYS_CLONE,
            args: [CLONE_VM | SIGCHLD_EXIT_SIGNAL, 0, 0, 0, 0, 0],
        };
        assert_eq!(validate_clone_args(&call), Err(-EINVAL));
    }

    #[test]
    fn clone_args_reject_nonzero_tid_tls_pointers() {
        let call = SyscallArgs {
            num: SYS_CLONE,
            args: [SIGCHLD_EXIT_SIGNAL, 0, 1, 0, 0, 0],
        };
        assert_eq!(validate_clone_args(&call), Err(-EINVAL));
    }
}
