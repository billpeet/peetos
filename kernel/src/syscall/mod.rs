use core::cmp;
use core::mem::{offset_of, size_of, MaybeUninit};
use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering};

use arch_x86_64::addr::{PhysAddr, VirtAddr, KERNEL_VIRT_BASE, PAGE_SIZE, USER_SPACE_END};
use arch_x86_64::fpu::{self, FxSaveArea};
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
/// Kernel-internal: a wait was interrupted by a signal. Never returned to
/// userspace; turned into a restart or `EINTR` when signals are delivered.
const ERESTARTSYS: i64 = signal::ERESTARTSYS;
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
const ENODEV: i64 = 19;
const EFBIG: i64 = 27;
const ENOLCK: i64 = 37;
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
const SYS_GETITIMER: u64 = 36;
const SYS_ALARM: u64 = 37;
const SYS_SETITIMER: u64 = 38;
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
const SYS_MKDIR: u64 = 83;
const SYS_RMDIR: u64 = 84;
const SYS_LINK: u64 = 86;
const SYS_UNLINK: u64 = 87;
const SYS_READLINK: u64 = 89;
const SYS_CHMOD: u64 = 90;
const SYS_FADVISE64: u64 = 221;
const SYS_UMASK: u64 = 95;
const SYS_GETTIMEOFDAY: u64 = 96;
const SYS_GETUID: u64 = 102;
const SYS_GETGID: u64 = 104;
const SYS_GETEUID: u64 = 107;
const SYS_GETEGID: u64 = 108;
const SYS_GETRESUID: u64 = 118;
const SYS_GETRESGID: u64 = 120;
const SYS_SETPGID: u64 = 109;
const SYS_GETPID: u64 = 39;
const SYS_EXECVE: u64 = 59;
const SYS_EXIT: u64 = 60;
const SYS_KILL: u64 = 62;
const SYS_SHMDT: u64 = 67;
const SYS_GETPPID: u64 = 110;
const SYS_SETSID: u64 = 112;
const SYS_GETSID: u64 = 124;
const SYS_PRCTL: u64 = 157;
const SYS_ARCH_PRCTL: u64 = 158;
const SYS_MEMBARRIER: u64 = 324;
const SYS_STATX: u64 = 332;
const SYS_INOTIFY_INIT: u64 = 253;
const SYS_INOTIFY_ADD_WATCH: u64 = 254;
const SYS_INOTIFY_RM_WATCH: u64 = 255;
const SYS_INOTIFY_INIT1: u64 = 294;
const SYS_GETTID: u64 = 186;
const SYS_TKILL: u64 = 200;
const SYS_TGKILL: u64 = 234;
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
const SYS_MREMAP: u64 = 25;
const SYS_FLOCK: u64 = 73;
const SYS_SIGNALFD: u64 = 282;
const SYS_TIMERFD_CREATE: u64 = 283;
const SYS_EVENTFD: u64 = 284;
const SYS_FALLOCATE: u64 = 285;
const SYS_TIMERFD_SETTIME: u64 = 286;
const SYS_TIMERFD_GETTIME: u64 = 287;
const SYS_SIGNALFD4: u64 = 289;
const SYS_EVENTFD2: u64 = 290;
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
const CLONE_FS: u64 = 0x0000_0200;
const CLONE_FILES: u64 = 0x0000_0400;
const CLONE_SIGHAND: u64 = 0x0000_0800;
const CLONE_VFORK: u64 = 0x0000_4000;
const CLONE_THREAD: u64 = 0x0001_0000;
const CLONE_SYSVSEM: u64 = 0x0004_0000;
const CLONE_SETTLS: u64 = 0x0008_0000;
const CLONE_PARENT_SETTID: u64 = 0x0010_0000;
const CLONE_CHILD_CLEARTID: u64 = 0x0020_0000;
const CLONE_DETACHED: u64 = 0x0040_0000;
const CLONE_CHILD_SETTID: u64 = 0x0100_0000;
/// Everything a POSIX thread shares with its creator. PeetOS supports these
/// only as a set: partial sharing (e.g. CLONE_VM without CLONE_FILES) is
/// rejected.
const CLONE_THREAD_SHARE: u64 = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD;
const CLONE_TID_FLAGS: u64 =
    CLONE_SETTLS | CLONE_PARENT_SETTID | CLONE_CHILD_SETTID | CLONE_CHILD_CLEARTID;

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
const MAP_LOCKED: u32 = 0x2000;
const MAP_NORESERVE: u32 = 0x4000;
const MAP_POPULATE: u32 = 0x8000;
const MAP_NONBLOCK: u32 = 0x10000;
const MAP_STACK: u32 = 0x20000;
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
const SO_PEERCRED: u32 = 17;
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
/// Shared byte budget for execve argument and environment strings (see
/// `ExecStrings`): they must fit the 32 KiB eagerly mapped top of the new
/// user stack together with the pointer arrays and auxv.
const EXEC_STRINGS_MAX: usize = 28 * 1024;
const EXEC_MAX_IMAGE_SIZE: usize = 4 * 1024 * 1024;
const UTS_FIELD_LEN: usize = 65;
/// Region slots per task. Adjacent compatible regions are merged on insert
/// and after mprotect (see `mmap_regions_insert`/`mmap_regions_coalesce`), so
/// this bounds distinct mappings, not mmap calls. Each shared library takes
/// several (one per segment with distinct protection).
///
/// A table is 8 KiB (256 x 32 bytes), half the 16 KiB kernel stack, so it
/// must never be copied onto the stack: transactional updates use a heap
/// `RegionScratch`, and everything else works on `TASK_VM_MAPPINGS` in place
/// (borrow with `task_regions`, copy slots with `region_table_copy`).
const MMAP_MAX_REGIONS_PER_TASK: usize = 256;
/// Per-process mmap budget (4 GiB of address space). Mappings are populated
/// lazily, so this bounds virtual reservations (foot maps a sparse 512 MiB
/// pool, and the compositor maps it again through wl_shm), not physical
/// memory, which the PMM limits.
const MMAP_MAX_PAGES_PER_TASK: u64 = 1 << 20;
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
/// One task's VM region table.
type RegionTable = [VmMapping; MMAP_MAX_REGIONS_PER_TASK];

/// Zero-initialised so the 512 KiB array lands in .bss rather than in the
/// kernel image (`VmMapping::EMPTY` is not all-zero). An all-zero entry has
/// `len == 0`, so it is already empty; `init()` refills with `EMPTY` anyway.
static mut TASK_VM_MAPPINGS: [RegionTable; task::MAX_TASKS] = unsafe { core::mem::zeroed() };
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

    /// Process id: the thread group id, shared by every thread.
    #[inline]
    fn pid(&self) -> Result<i64, i64> {
        if !self.has_task() {
            return Err(-ESRCH);
        }
        Ok(unsafe { (*self.current).tgid as i64 })
    }

    /// Thread id: unique per task.
    #[inline]
    fn tid(&self) -> Result<i64, i64> {
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
        Ok(unsafe { (*crate::task::leader_of(self.current)).ppid as i64 })
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
    syscall_def(SYS_GETITIMER, "getitimer", SyscallMode::UserTask, sys_getitimer),
    syscall_def(SYS_ALARM, "alarm", SyscallMode::UserTask, sys_alarm),
    syscall_def(SYS_SETITIMER, "setitimer", SyscallMode::UserTask, sys_setitimer),
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
    syscall_def(SYS_MKDIR, "mkdir", SyscallMode::UserTask, sys_mkdir),
    syscall_def(SYS_RMDIR, "rmdir", SyscallMode::UserTask, sys_rmdir),
    syscall_def(SYS_LINK, "link", SyscallMode::UserTask, sys_link),
    syscall_def(
        SYS_READLINK,
        "readlink",
        SyscallMode::UserTask,
        sys_readlink,
    ),
    syscall_def(SYS_CHMOD, "chmod", SyscallMode::UserTask, sys_chmod),
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
    syscall_def(SYS_GETRESUID, "getresuid", SyscallMode::UserTask, sys_getresuid),
    syscall_def(SYS_GETRESGID, "getresgid", SyscallMode::UserTask, sys_getresgid),
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
    syscall_def(SYS_FADVISE64, "fadvise64", SyscallMode::UserTask, sys_fadvise64),
    syscall_def(SYS_PRCTL, "prctl", SyscallMode::UserTask, sys_prctl),
    syscall_def(SYS_MEMBARRIER, "membarrier", SyscallMode::UserTask, sys_membarrier),
    syscall_def(SYS_STATX, "statx", SyscallMode::UserTask, sys_statx),
    syscall_def(SYS_INOTIFY_INIT, "inotify_init", SyscallMode::UserTask, sys_inotify_unsupported),
    syscall_def(
        SYS_INOTIFY_ADD_WATCH,
        "inotify_add_watch",
        SyscallMode::UserTask,
        sys_inotify_unsupported,
    ),
    syscall_def(
        SYS_INOTIFY_RM_WATCH,
        "inotify_rm_watch",
        SyscallMode::UserTask,
        sys_inotify_unsupported,
    ),
    syscall_def(SYS_INOTIFY_INIT1, "inotify_init1", SyscallMode::UserTask, sys_inotify_unsupported),
    syscall_def(SYS_UNLINK, "unlink", SyscallMode::UserTask, sys_unlink),
    syscall_def(
        SYS_ARCH_PRCTL,
        "arch_prctl",
        SyscallMode::UserTask,
        sys_arch_prctl,
    ),
    syscall_def(SYS_GETTID, "gettid", SyscallMode::AnyTask, sys_gettid),
    syscall_def(SYS_TKILL, "tkill", SyscallMode::UserTask, sys_tkill),
    syscall_def(SYS_TGKILL, "tgkill", SyscallMode::UserTask, sys_tgkill),
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
    syscall_def(SYS_MREMAP, "mremap", SyscallMode::UserTask, sys_mremap),
    syscall_def(SYS_FLOCK, "flock", SyscallMode::UserTask, sys_flock),
    syscall_def(SYS_FALLOCATE, "fallocate", SyscallMode::UserTask, sys_fallocate),
    syscall_def(SYS_EVENTFD, "eventfd", SyscallMode::UserTask, sys_eventfd),
    syscall_def(SYS_EVENTFD2, "eventfd2", SyscallMode::UserTask, sys_eventfd2),
    syscall_def(
        SYS_TIMERFD_CREATE,
        "timerfd_create",
        SyscallMode::UserTask,
        sys_timerfd_create,
    ),
    syscall_def(
        SYS_TIMERFD_SETTIME,
        "timerfd_settime",
        SyscallMode::UserTask,
        sys_timerfd_settime,
    ),
    syscall_def(
        SYS_TIMERFD_GETTIME,
        "timerfd_gettime",
        SyscallMode::UserTask,
        sys_timerfd_gettime,
    ),
    syscall_def(SYS_SIGNALFD, "signalfd", SyscallMode::UserTask, sys_signalfd),
    syscall_def(SYS_SIGNALFD4, "signalfd4", SyscallMode::UserTask, sys_signalfd4),
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

fn ipc_to_errno(err: i64) -> i64 {
    match err {
        ipc::EINVAL => EINVAL,
        ipc::ENOSPC => ENOSPC,
        ipc::EPIPE => EPIPE,
        ipc::EAGAIN => EAGAIN,
        ipc::ENOENT => ENOENT,
        ipc::ETIMEDOUT => ETIMEDOUT,
        signal::ERESTARTSYS => ERESTARTSYS,
        signal::EINTR => EINTR,
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
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
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

// Linux x86-64 signal-frame ABI (`asm/sigcontext.h`, `asm/ucontext.h`,
// `asm-generic/siginfo.h`). Handlers and libc (e.g. musl's thread
// cancellation) read and edit these structures, so the layout must match.

/// Number of general registers in `mcontext_t.gregs`.
const NGREG: usize = 23;
const REG_R8: usize = 0;
const REG_R9: usize = 1;
const REG_R10: usize = 2;
const REG_R11: usize = 3;
const REG_R12: usize = 4;
const REG_R13: usize = 5;
const REG_R14: usize = 6;
const REG_R15: usize = 7;
const REG_RDI: usize = 8;
const REG_RSI: usize = 9;
const REG_RBP: usize = 10;
const REG_RBX: usize = 11;
const REG_RDX: usize = 12;
const REG_RAX: usize = 13;
const REG_RCX: usize = 14;
const REG_RSP: usize = 15;
const REG_RIP: usize = 16;
const REG_EFL: usize = 17;
const REG_CSGSFS: usize = 18;

const SS_DISABLE: i32 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxSigInfo {
    si_signo: i32,
    si_errno: i32,
    si_code: i32,
    _pad0: i32,
    si_pid: i32,
    si_uid: u32,
    _rest: [u64; 13],
}

impl LinuxSigInfo {
    fn new(signum: u32, code: i32, sender: i32) -> Self {
        Self {
            si_signo: signum as i32,
            si_errno: 0,
            si_code: code,
            _pad0: 0,
            si_pid: sender,
            si_uid: 0,
            _rest: [0; 13],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxStackT {
    ss_sp: u64,
    ss_flags: i32,
    _pad: i32,
    ss_size: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxMcontext {
    gregs: [u64; NGREG],
    /// User address of the FXSAVE image, or 0.
    fpregs: u64,
    reserved: [u64; 8],
}

/// Kernel `struct ucontext` (the kernel sigset is the 8-byte prefix of
/// libc's larger `uc_sigmask`).
#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxUcontext {
    uc_flags: u64,
    uc_link: u64,
    uc_stack: LinuxStackT,
    uc_mcontext: LinuxMcontext,
    uc_sigmask: u64,
}

/// `struct rt_sigframe`: what RSP points at when a handler starts.
#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxRtSigFrame {
    pretcode: u64,
    uc: LinuxUcontext,
    info: LinuxSigInfo,
}

const _: () = {
    assert!(size_of::<LinuxSigInfo>() == 128);
    assert!(size_of::<LinuxMcontext>() == 256);
    assert!(offset_of!(LinuxUcontext, uc_mcontext) == 40);
    assert!(offset_of!(LinuxUcontext, uc_sigmask) == 296);
    assert!(offset_of!(LinuxRtSigFrame, uc) == 8);
    assert!(offset_of!(LinuxRtSigFrame, info) == 312);
};

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

const TMPFS_MAGIC: i64 = 0x01021994;
const EXT2_SUPER_MAGIC_STATFS: i64 = 0xEF53;

// ---------------------------------------------------------------------------
// poll / ppoll implementation
// ---------------------------------------------------------------------------

const POLL_MAX_FDS: usize = 32;
const SELECT_MAX_FDS: usize = 1024;
const SELECT_FDSET_WORD_BITS: usize = size_of::<u64>() * 8;
const SELECT_FDSET_WORDS: usize = SELECT_MAX_FDS / SELECT_FDSET_WORD_BITS;

type SelectFdSet = [u64; SELECT_FDSET_WORDS];

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
    if !ctx.current.is_null() {
        unsafe {
            (*ctx.current).last_syscall = call.num;
            (*ctx.current).last_syscall_args = call.args;
        }
    }
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
            TASK_VM_MAPPINGS[idx].fill(VmMapping::EMPTY);
        }
        for idx in 0..EPOLL_MAX_INSTANCES {
            EPOLL_INSTANCES[idx] = EpollInstance::EMPTY;
        }
        VM_INODE_PINNED = [false; fs::VFS_MAX_INODES];
    }
    vm_page_cache_reset();
}

/// SYSCALL entry from `syscall_entry` (ring 3, interrupts disabled).
/// [`is_lock_free_syscall`] numbers run without the big kernel lock;
/// everything else holds it for the whole call and releases it just before
/// the SYSRET back to the same task.
#[no_mangle]
pub extern "C" fn syscall_handler(frame: &mut SyscallFrame) -> u64 {
    if let Some(ret) = lock_free_syscall(frame) {
        return ret;
    }
    crate::bkl::syscall_enter();
    let ret = syscall_dispatch(frame);
    crate::bkl::syscall_exit();
    ret
}

/// Syscalls served without the big kernel lock. Each touches only state
/// that is fixed for the calling task (its ids), the scheduler (own lock),
/// the PIT tick counter (atomic) and its own user memory (copied under
/// `VMM_LOCK`, see `uaccess::copy_user_bytes`).
pub(crate) const fn is_lock_free_syscall(nr: u64) -> bool {
    matches!(
        nr,
        SYS_YIELD
            | SYS_GETPID
            | SYS_GETTID
            | SYS_GETPPID
            | SYS_CLOCK_GETTIME
            | SYS_GETTIMEOFDAY
            | SYS_NANOSLEEP
    )
}

/// Whether returning from a syscall must go through the locked path anyway:
/// a deliverable signal is pending (delivery needs the big kernel lock) or
/// the task has been told to exit.
pub(crate) const fn needs_locked_return(sig_pending: u64, signal_blocked: u64, flags: u32) -> bool {
    (sig_pending & !signal_blocked) != 0 || (flags & task::TASK_FLAG_EXITING) != 0
}

/// Lock-free syscalls dispatched by [`syscall_handler`] (diagnostics).
pub(crate) static LOCK_FREE_SYSCALLS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

fn task_needs_locked_return(current: *mut task::Task) -> bool {
    let (sig_pending, signal_blocked, flags) =
        unsafe { ((*current).sig_pending, (*current).signal_blocked, (*current).flags) };
    needs_locked_return(sig_pending, signal_blocked, flags)
}

/// Fast path for [`is_lock_free_syscall`] numbers. Returns `None` to fall
/// back to the normal, locked dispatcher.
fn lock_free_syscall(frame: &mut SyscallFrame) -> Option<u64> {
    let nr = frame.rax;
    if !is_lock_free_syscall(nr) {
        return None;
    }
    let current = task::current();
    if current.is_null() || task_needs_locked_return(current) {
        return None;
    }
    let ctx = SyscallContext::new(current);
    let call = call_from_syscall_frame(frame);
    let result = match nr {
        SYS_GETPID => sys_getpid(&ctx, &call),
        SYS_GETTID => sys_gettid(&ctx, &call),
        SYS_GETPPID => sys_getppid(&ctx, &call),
        SYS_CLOCK_GETTIME => sys_clock_gettime(&ctx, &call),
        SYS_GETTIMEOFDAY => sys_gettimeofday(&ctx, &call),
        SYS_NANOSLEEP => sys_nanosleep(&ctx, &call),
        SYS_YIELD => {
            crate::sched::yield_now();
            0
        }
        _ => return None,
    };
    LOCK_FREE_SYSCALLS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    frame.rax = to_sysret(result);

    if task_needs_locked_return(current) {
        // A signal arrived meanwhile (e.g. it woke a nanosleep): deliver it
        // exactly as the locked dispatcher would.
        crate::bkl::syscall_enter();
        if let Some(ret) = apply_pending_signals_syscall(frame, Some(nr)) {
            frame.rax = to_sysret(ret);
        }
        let ret = frame.rax;
        crate::bkl::syscall_exit();
        return Some(ret);
    }

    // A yield or sleep resumes with interrupts enabled. The SYSRET epilogue
    // runs SWAPGS before SYSRETQ, and an interrupt between the two would
    // enter the kernel with the user GS base.
    arch_x86_64::interrupts::disable();
    Some(frame.rax)
}

fn syscall_dispatch(frame: &mut SyscallFrame) -> u64 {
    let ctx = SyscallContext::new(task::current());
    let nr = frame.rax;
    if frame.rax == SYS_RT_SIGRETURN {
        if !ctx.is_user_task() {
            return to_sysret(-EPERM);
        }
        let restore = restore_rt_signal_frame(ctx.current, frame);
        if restore < 0 {
            return to_sysret(restore);
        }
        if let Some(ret) = apply_pending_signals_syscall(frame, None) {
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
                if let Some(ret) = apply_pending_signals_syscall(frame, Some(nr)) {
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
            let req = match validate_clone_args(&call) {
                Ok(req) => req,
                Err(err) => {
                    frame.rax = to_sysret(err);
                    if let Some(ret) = apply_pending_signals_syscall(frame, Some(nr)) {
                        frame.rax = to_sysret(ret);
                    }
                    return frame.rax;
                }
            };
            let child_frame = match build_clone_child_frame_from_syscall(frame, req.child_stack) {
                Ok(child_frame) => child_frame,
                Err(err) => {
                    frame.rax = to_sysret(err);
                    if let Some(ret) = apply_pending_signals_syscall(frame, Some(nr)) {
                        frame.rax = to_sysret(ret);
                    }
                    return frame.rax;
                }
            };
            create_fork_child(&ctx, &child_frame, &req)
        } else {
            let child_frame = build_child_frame_from_syscall_frame(frame);
            create_fork_child(&ctx, &child_frame, &CloneRequest::FORK)
        };
        frame.rax = to_sysret(match result {
            Ok(pid) => pid,
            Err(err) => err,
        });
        if let Some(ret) = apply_pending_signals_syscall(frame, Some(nr)) {
            frame.rax = to_sysret(ret);
        }
        return frame.rax;
    }

    let result = dispatch_syscall(call_from_syscall_frame(frame));
    frame.rax = to_sysret(result);
    if let Some(ret) = apply_pending_signals_syscall(frame, Some(nr)) {
        frame.rax = to_sysret(ret);
    }
    frame.rax
}

pub extern "C" fn syscall_int80_handler(frame: &mut InterruptFrame) {
    let ctx = SyscallContext::new(task::current());
    let nr = frame.rax;
    if frame.rax == SYS_RT_SIGRETURN {
        frame.rax = if !ctx.is_user_task() {
            to_sysret(-EPERM)
        } else {
            let restore = restore_rt_signal_frame(ctx.current, frame);
            if restore < 0 {
                to_sysret(restore)
            } else {
                frame.rax
            }
        };
        if let Some(ret) = apply_pending_signals_int80(frame, None) {
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
        if let Some(ret) = apply_pending_signals_int80(frame, Some(nr)) {
            frame.rax = to_sysret(ret);
        }
        return;
    }
    if frame.rax == SYS_FORK || frame.rax == SYS_VFORK || frame.rax == SYS_CLONE {
        frame.rax = if !ctx.is_user_task() {
            to_sysret(-EPERM)
        } else if frame.rax == SYS_CLONE {
            let call = call_from_int80_frame(frame);
            let req = match validate_clone_args(&call) {
                Ok(req) => req,
                Err(err) => {
                    frame.rax = to_sysret(err);
                    if let Some(ret) = apply_pending_signals_int80(frame, Some(nr)) {
                        frame.rax = to_sysret(ret);
                    }
                    return;
                }
            };
            let child_frame = match build_clone_child_frame_from_int80(frame, req.child_stack) {
                Ok(child_frame) => child_frame,
                Err(err) => {
                    frame.rax = to_sysret(err);
                    if let Some(ret) = apply_pending_signals_int80(frame, Some(nr)) {
                        frame.rax = to_sysret(ret);
                    }
                    return;
                }
            };
            let result = create_fork_child(&ctx, &child_frame, &req);
            to_sysret(match result {
                Ok(pid) => pid,
                Err(err) => err,
            })
        } else {
            let child_frame = build_child_frame_from_int80_frame(frame);
            let result = create_fork_child(&ctx, &child_frame, &CloneRequest::FORK);
            to_sysret(match result {
                Ok(pid) => pid,
                Err(err) => err,
            })
        };
        if let Some(ret) = apply_pending_signals_int80(frame, Some(nr)) {
            frame.rax = to_sysret(ret);
        }
        return;
    }

    frame.rax = to_sysret(dispatch_syscall(call_from_int80_frame(frame)));
    if let Some(ret) = apply_pending_signals_int80(frame, Some(nr)) {
        frame.rax = to_sysret(ret);
    }
}

// Syscall implementations, grouped by area. Each submodule pulls in this
// module's shared types and helpers via `use super::*`.
mod drmdev;
mod eventfds;
mod fd;
mod file;
mod fileext;
mod io;
mod ioctl;
mod misc;
mod poll;
mod process;
mod signals;
mod sockets;
mod uaccess;
mod vm;

use drmdev::*;
use eventfds::*;
use fd::*;
use file::*;
use fileext::*;
use io::*;
use ioctl::*;
use misc::*;
use poll::*;
use process::*;
use signals::*;
use sockets::*;
use uaccess::*;
use vm::*;

pub(crate) use fd::{fd_object_release, fd_object_retain};
pub(crate) use vm::vm_page_cache_release_mapped_phys;
pub(crate) use vm::mmap_register_user_stack;
pub use vm::{terminate_current_user_fault, try_handle_page_fault};

#[cfg(test)]
mod tests;
