use core::mem::{size_of, MaybeUninit};
use core::ptr;

use arch_x86_64::addr::VirtAddr;
use arch_x86_64::fpu::FxSaveArea;
use arch_x86_64::idt::InterruptFrame;
use arch_x86_64::syscall as arch_syscall;

use crate::memory::pmm;
use crate::serial_println;

pub type Pid = i32;

pub const INVALID_PID: Pid = -1;

pub const TASK_NAME_MAX: usize = 32;
pub const KERNEL_STACK_SIZE: u64 = 16 * 1024;
pub const KERNEL_STACK_PAGES: usize = (KERNEL_STACK_SIZE / 4096) as usize;
pub const MAX_TASKS: usize = 64;
pub const MAX_TASK_FDS: usize = 32;
pub const MAX_TASK_SHM_ATTACH: usize = 8;
/// Signal slots per task: signals 1..=64 (index 0 unused).
pub const SIG_MAX_TASK_SIGNALS: usize = 65;

pub const TASK_FLAG_KERNEL: u32 = 0x01;
pub const TASK_FLAG_USER: u32 = 0x02;
pub const TASK_FLAG_IDLE: u32 = 0x04;
/// Non-leader member of a thread group (created by `clone(CLONE_THREAD)`).
/// Such tasks are never waited for; they are reaped automatically.
pub const TASK_FLAG_THREAD: u32 = 0x08;
/// Set on a thread told to exit because a sibling called exit_group or
/// execve. It leaves at its next return towards user mode.
pub const TASK_FLAG_EXITING: u32 = 0x10;
/// Set on a thread group leader once the process is exiting as a whole; its
/// `exit_code` then holds the process exit status.
pub const TASK_FLAG_GROUP_EXIT: u32 = 0x20;

pub const USER_FD_KIND_EMPTY: u8 = 0;
pub const USER_FD_KIND_CONSOLE: u8 = 1;
pub const USER_FD_KIND_VFS: u8 = 2;
pub const USER_FD_KIND_PIPE: u8 = 3;
pub const USER_FD_KIND_SOCKET: u8 = 4;
pub const USER_FD_KIND_PTY_MASTER: u8 = 5;
pub const USER_FD_KIND_PTY_SLAVE: u8 = 6;
pub const USER_FD_KIND_EPOLL: u8 = 7;
pub const USER_FD_NONE: u16 = u16::MAX;

pub const CTTY_NONE: u8 = 0;
pub const CTTY_CONSOLE: u8 = 1;
pub const CTTY_PTY: u8 = 2;

const KERNEL_CS: u64 = 0x08;
const KERNEL_DS: u64 = 0x10;
const RFLAGS_IF: u64 = 0x202;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TaskState {
    Created,
    Ready,
    Running,
    Blocked,
    Sleeping,
    Zombie,
    Dead,
}

#[repr(C)]
pub struct Task {
    /// Kernel task id. For a thread group leader this is also the process id.
    pub(crate) pid: Pid,
    /// Thread group (process) id: the leader's `pid`.
    pub(crate) tgid: Pid,
    /// Leader of this task's thread group, or null when this task is the
    /// leader. Process-wide state (fd table, cwd, brk, mmap regions, shm,
    /// signal actions, ids, exec metadata) lives on the leader; see
    /// `leader_of`.
    pub(crate) group_leader: *mut Task,
    pub(crate) ppid: Pid,
    pub(crate) pgid: Pid,
    pub(crate) sid: Pid,
    pub(crate) ctty_type: u8,
    pub(crate) ctty_id: u16,
    pub(crate) name: [u8; TASK_NAME_MAX],
    pub(crate) state: TaskState,
    pub(crate) flags: u32,
    pub(crate) exit_code: i32,
    pub(crate) saved_rsp: u64,
    pub(crate) kernel_stack_base: u64,
    pub(crate) kernel_stack_top: u64,
    pub(crate) pml4_phys: u64,
    pub(crate) user_entry: u64,
    pub(crate) user_stack_bottom: u64,
    pub(crate) user_stack_top: u64,
    pub(crate) user_stack_ptr: u64,
    pub(crate) user_argc: u64,
    pub(crate) user_argv: u64,
    pub(crate) user_envp: u64,
    pub(crate) user_auxv: u64,
    pub(crate) user_fs_base: u64,
    pub(crate) user_clear_tid: u64,
    pub(crate) user_brk_start: u64,
    pub(crate) user_brk: u64,
    pub(crate) user_phdr_addr: u64,
    pub(crate) user_phdr_count: u16,
    pub(crate) user_phdr_size: u16,
    pub(crate) user_base_addr: u64,
    pub(crate) user_interp_base: u64,
    pub(crate) user_is_dynamic: bool,
    pub(crate) user_uid: u32,
    pub(crate) user_gid: u32,
    pub(crate) user_cwd_inode: u16,
    pub(crate) user_fd_kind: [u8; MAX_TASK_FDS],
    pub(crate) user_fd_inode: [u16; MAX_TASK_FDS],
    pub(crate) user_fd_offset: [u64; MAX_TASK_FDS],
    pub(crate) user_fd_flags: [u32; MAX_TASK_FDS],
    pub(crate) user_shm_id: [i32; MAX_TASK_SHM_ATTACH],
    pub(crate) user_shm_addr: [u64; MAX_TASK_SHM_ATTACH],
    pub(crate) user_shm_size: [u64; MAX_TASK_SHM_ATTACH],
    pub(crate) user_shm_flags: [u32; MAX_TASK_SHM_ATTACH],
    pub(crate) sig_pending: u64,
    /// Per pending signal: packed `si_code` and sender pid (see
    /// `signal::send_with_info`).
    pub(crate) sig_info: [u64; SIG_MAX_TASK_SIGNALS],
    pub(crate) signal_blocked: u64,
    pub(crate) signal_handlers: [u64; SIG_MAX_TASK_SIGNALS],
    pub(crate) signal_flags: [u64; SIG_MAX_TASK_SIGNALS],
    pub(crate) signal_mask_saved: [u64; SIG_MAX_TASK_SIGNALS],
    pub(crate) signal_restorer: u64,
    pub(crate) time_slice: u64,
    pub(crate) total_ticks: u64,
    pub(crate) sleep_until: u64,
    pub(crate) next: *mut Task,
    pub(crate) prev: *mut Task,
    /// Saved user x87/SSE register state while the task is switched out.
    /// While the task is running, the hardware registers are authoritative.
    pub(crate) fpu: FxSaveArea,
}

// Tasks are allocated as a single physical page in `task_alloc`.
const _: () = assert!(size_of::<Task>() <= 4096);

static mut TASK_TABLE: [*mut Task; MAX_TASKS] = [ptr::null_mut(); MAX_TASKS];
static mut TASK_COUNT: usize = 0;
static mut NEXT_PID: Pid = 1;
static mut BOOT_TASK: MaybeUninit<Task> = MaybeUninit::uninit();
static mut TASK_INITIALIZED: bool = false;

fn write_name(dst: &mut [u8; TASK_NAME_MAX], name: &str) {
    let bytes = name.as_bytes();
    let mut i = 0;
    while i < TASK_NAME_MAX {
        dst[i] = 0;
        i += 1;
    }
    let mut i = 0;
    while i + 1 < TASK_NAME_MAX && i < bytes.len() {
        dst[i] = bytes[i];
        i += 1;
    }
}

fn find_free_slot() -> Option<usize> {
    unsafe {
        for (idx, slot) in TASK_TABLE.iter().enumerate() {
            if slot.is_null() {
                return Some(idx);
            }
        }
    }
    None
}

pub fn init() {
    serial_println!("[TASK] Initializing task subsystem...");

    unsafe {
        for slot in TASK_TABLE.iter_mut() {
            *slot = ptr::null_mut();
        }

        let mut boot = Task {
            pid: 0,
            tgid: 0,
            group_leader: ptr::null_mut(),
            ppid: 0,
            pgid: 0,
            sid: 0,
            ctty_type: CTTY_CONSOLE,
            ctty_id: 0,
            name: [0; TASK_NAME_MAX],
            state: TaskState::Running,
            flags: TASK_FLAG_KERNEL,
            exit_code: 0,
            saved_rsp: 0,
            kernel_stack_base: 0,
            kernel_stack_top: 0,
            pml4_phys: 0,
            user_entry: 0,
            user_stack_bottom: 0,
            user_stack_top: 0,
            user_stack_ptr: 0,
            user_argc: 0,
            user_argv: 0,
            user_envp: 0,
            user_auxv: 0,
            user_fs_base: 0,
            user_clear_tid: 0,
            user_brk_start: 0,
            user_brk: 0,
            user_phdr_addr: 0,
            user_phdr_count: 0,
            user_phdr_size: 0,
            user_base_addr: 0,
            user_interp_base: 0,
            user_is_dynamic: false,
            user_uid: 0,
            user_gid: 0,
            user_cwd_inode: fs::root_inode(),
            user_fd_kind: [USER_FD_KIND_EMPTY; MAX_TASK_FDS],
            user_fd_inode: [USER_FD_NONE; MAX_TASK_FDS],
            user_fd_offset: [0; MAX_TASK_FDS],
            user_fd_flags: [0; MAX_TASK_FDS],
            user_shm_id: [0; MAX_TASK_SHM_ATTACH],
            user_shm_addr: [0; MAX_TASK_SHM_ATTACH],
            user_shm_size: [0; MAX_TASK_SHM_ATTACH],
            user_shm_flags: [0; MAX_TASK_SHM_ATTACH],
            sig_pending: 0,
            sig_info: [0; SIG_MAX_TASK_SIGNALS],
            signal_blocked: 0,
            signal_handlers: [0; SIG_MAX_TASK_SIGNALS],
            signal_flags: [0; SIG_MAX_TASK_SIGNALS],
            signal_mask_saved: [0; SIG_MAX_TASK_SIGNALS],
            signal_restorer: 0,
            time_slice: 0,
            total_ticks: 0,
            sleep_until: 0,
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
            fpu: FxSaveArea::new_default(),
        };
        write_name(&mut boot.name, "kernel");

        BOOT_TASK.write(boot);
        let boot_ptr = BOOT_TASK.as_mut_ptr();
        TASK_TABLE[0] = boot_ptr;
        TASK_COUNT = 1;
        TASK_INITIALIZED = true;
    }

    serial_println!("[TASK] Task subsystem initialized");
}

pub fn task_alloc(name: &str, flags: u32) -> *mut Task {
    let slot = match find_free_slot() {
        Some(idx) => idx,
        None => {
            serial_println!("[TASK] ERROR: no free task slots");
            return ptr::null_mut();
        }
    };

    let task_phys = pmm::alloc_page();
    if task_phys.as_u64() == 0 {
        serial_println!("[TASK] ERROR: failed to allocate task struct");
        return ptr::null_mut();
    }
    let task_ptr = task_phys.to_virt().as_u64() as *mut Task;
    unsafe {
        ptr::write_bytes(task_ptr as *mut u8, 0, size_of::<Task>());
    }

    let stack_phys = pmm::alloc_pages(KERNEL_STACK_PAGES);
    if stack_phys.as_u64() == 0 {
        serial_println!("[TASK] ERROR: failed to allocate kernel stack");
        pmm::free_page(task_phys);
        return ptr::null_mut();
    }
    let stack_base = stack_phys.to_virt().as_u64();
    let stack_top = stack_base + KERNEL_STACK_SIZE;

    unsafe {
        let task = &mut *task_ptr;
        task.pid = NEXT_PID;
        NEXT_PID = NEXT_PID.saturating_add(1);
        task.tgid = task.pid;
        task.group_leader = ptr::null_mut();
        // A process's parent is the creating *process*, not a specific thread.
        task.ppid = current().as_ref().map(|t| t.tgid).unwrap_or(0);
        task.pgid = task.pid;
        task.sid = task.pid;
        task.ctty_type = CTTY_NONE;
        task.ctty_id = 0;
        task.state = TaskState::Created;
        task.flags = flags;
        task.exit_code = 0;
        task.kernel_stack_base = stack_base;
        task.kernel_stack_top = stack_top;
        task.pml4_phys = 0;
        task.user_entry = 0;
        task.user_stack_bottom = 0;
        task.user_stack_top = 0;
        task.user_stack_ptr = 0;
        task.user_argc = 0;
        task.user_argv = 0;
        task.user_envp = 0;
        task.user_auxv = 0;
        task.user_fs_base = 0;
        task.user_clear_tid = 0;
        task.user_brk_start = 0;
        task.user_brk = 0;
        task.user_phdr_addr = 0;
        task.user_phdr_count = 0;
        task.user_phdr_size = 0;
        task.user_base_addr = 0;
        task.user_interp_base = 0;
        task.user_is_dynamic = false;
        task.user_uid = 0;
        task.user_gid = 0;
        task.user_cwd_inode = fs::root_inode();
        task.user_fd_kind = [USER_FD_KIND_EMPTY; MAX_TASK_FDS];
        task.user_fd_inode = [USER_FD_NONE; MAX_TASK_FDS];
        task.user_fd_offset = [0; MAX_TASK_FDS];
        task.user_fd_flags = [0; MAX_TASK_FDS];
        task.user_shm_id = [0; MAX_TASK_SHM_ATTACH];
        task.user_shm_addr = [0; MAX_TASK_SHM_ATTACH];
        task.user_shm_size = [0; MAX_TASK_SHM_ATTACH];
        task.user_shm_flags = [0; MAX_TASK_SHM_ATTACH];
        task.sig_pending = 0;
        task.sig_info = [0; SIG_MAX_TASK_SIGNALS];
        task.signal_blocked = 0;
        task.signal_handlers = [0; SIG_MAX_TASK_SIGNALS];
        task.signal_flags = [0; SIG_MAX_TASK_SIGNALS];
        task.signal_mask_saved = [0; SIG_MAX_TASK_SIGNALS];
        task.signal_restorer = 0;
        task.time_slice = 0;
        task.total_ticks = 0;
        task.sleep_until = 0;
        task.next = ptr::null_mut();
        task.prev = ptr::null_mut();
        task.fpu = FxSaveArea::new_default();
        write_name(&mut task.name, name);

        TASK_TABLE[slot] = task_ptr;
        TASK_COUNT = TASK_COUNT.saturating_add(1);
    }

    serial_println!("[TASK] Created task '{}' PID={}", name, unsafe {
        (*task_ptr).pid
    });
    task_ptr
}

pub fn task_destroy(task: *mut Task) {
    if task.is_null() {
        return;
    }

    unsafe {
        if task == BOOT_TASK.as_mut_ptr() {
            return;
        }
    }

    unsafe {
        for slot in TASK_TABLE.iter_mut() {
            if *slot == task {
                *slot = ptr::null_mut();
                TASK_COUNT = TASK_COUNT.saturating_sub(1);
                break;
            }
        }

        let stack_base = (*task).kernel_stack_base;
        if stack_base != 0 {
            let stack_phys = VirtAddr::new(stack_base).to_phys();
            pmm::free_pages(stack_phys, KERNEL_STACK_PAGES);
        }

        let task_phys = VirtAddr::new(task as u64).to_phys();
        pmm::free_page(task_phys);
    }
}

/// The task that holds process-wide state for `task`: its thread group leader,
/// or `task` itself when it is a leader (or null).
#[inline]
pub fn leader_of(task: *mut Task) -> *mut Task {
    if task.is_null() {
        return task;
    }
    let leader = unsafe { (*task).group_leader };
    if leader.is_null() {
        task
    } else {
        leader
    }
}

#[inline]
pub fn is_live(task: *const Task) -> bool {
    unsafe { (*task).state != TaskState::Zombie && (*task).state != TaskState::Dead }
}

/// Number of live (not zombie/dead) tasks in thread group `tgid`.
pub fn live_threads_in_group(tgid: Pid) -> usize {
    let mut count = 0;
    unsafe {
        for t in TASK_TABLE.iter() {
            if !t.is_null() && (**t).tgid == tgid && is_live(*t) {
                count += 1;
            }
        }
    }
    count
}

/// Make `thread` a member of `leader`'s thread group.
pub fn join_thread_group(thread: *mut Task, leader: *mut Task) {
    if thread.is_null() || leader.is_null() {
        return;
    }
    let leader = leader_of(leader);
    unsafe {
        (*thread).tgid = (*leader).tgid;
        (*thread).group_leader = leader;
        (*thread).ppid = (*leader).ppid;
        (*thread).flags |= TASK_FLAG_THREAD;
    }
}

/// Make `thread` the leader of its thread group, as Linux does when a
/// non-leader thread calls `execve`: it takes over the process id and all
/// process-wide state, and the old leader becomes an ordinary member (with
/// the exec'ing thread's former id) so it can be terminated like any other
/// sibling.
pub fn take_over_group_leadership(thread: *mut Task) {
    let old = leader_of(thread);
    if thread.is_null() || old == thread {
        return;
    }
    unsafe {
        let new = &mut *thread;
        let prev = &mut *old;
        let tgid = prev.tgid;

        new.user_fd_kind = prev.user_fd_kind;
        new.user_fd_inode = prev.user_fd_inode;
        new.user_fd_offset = prev.user_fd_offset;
        new.user_fd_flags = prev.user_fd_flags;
        new.user_cwd_inode = prev.user_cwd_inode;
        new.user_brk_start = prev.user_brk_start;
        new.user_brk = prev.user_brk;
        new.user_shm_id = prev.user_shm_id;
        new.user_shm_addr = prev.user_shm_addr;
        new.user_shm_size = prev.user_shm_size;
        new.user_shm_flags = prev.user_shm_flags;
        new.signal_handlers = prev.signal_handlers;
        new.signal_flags = prev.signal_flags;
        new.signal_mask_saved = prev.signal_mask_saved;
        new.signal_restorer = prev.signal_restorer;
        new.user_uid = prev.user_uid;
        new.user_gid = prev.user_gid;
        new.pgid = prev.pgid;
        new.sid = prev.sid;
        new.ctty_type = prev.ctty_type;
        new.ctty_id = prev.ctty_id;
        new.ppid = prev.ppid;
        new.user_entry = prev.user_entry;
        new.user_stack_bottom = prev.user_stack_bottom;
        new.user_stack_top = prev.user_stack_top;
        new.user_stack_ptr = prev.user_stack_ptr;
        new.user_argc = prev.user_argc;
        new.user_argv = prev.user_argv;
        new.user_envp = prev.user_envp;
        new.user_auxv = prev.user_auxv;
        new.user_phdr_addr = prev.user_phdr_addr;
        new.user_phdr_count = prev.user_phdr_count;
        new.user_phdr_size = prev.user_phdr_size;
        new.user_base_addr = prev.user_base_addr;
        new.user_interp_base = prev.user_interp_base;
        new.user_is_dynamic = prev.user_is_dynamic;

        // The state now lives on `thread`; leave no second owner behind.
        prev.user_fd_kind = [USER_FD_KIND_EMPTY; MAX_TASK_FDS];
        prev.user_fd_inode = [USER_FD_NONE; MAX_TASK_FDS];
        prev.user_fd_offset = [0; MAX_TASK_FDS];
        prev.user_fd_flags = [0; MAX_TASK_FDS];
        prev.user_shm_id = [0; MAX_TASK_SHM_ATTACH];
        prev.user_shm_addr = [0; MAX_TASK_SHM_ATTACH];
        prev.user_shm_size = [0; MAX_TASK_SHM_ATTACH];
        prev.user_shm_flags = [0; MAX_TASK_SHM_ATTACH];

        let old_tid = new.pid;
        new.pid = tgid;
        prev.pid = old_tid;
        new.group_leader = ptr::null_mut();
        new.flags &= !TASK_FLAG_THREAD;
        prev.flags |= TASK_FLAG_THREAD;

        for slot in TASK_TABLE.iter() {
            let t = *slot;
            if !t.is_null() && t != thread && (*t).tgid == tgid {
                (*t).group_leader = thread;
            }
        }
    }
}

/// Index of `task` in the task table.
pub fn table_slot(task: *mut Task) -> Option<usize> {
    unsafe { TASK_TABLE.iter().position(|t| *t == task) }
}

pub fn current() -> *mut Task {
    if unsafe { !TASK_INITIALIZED } {
        return ptr::null_mut();
    }
    crate::sched::current_task()
}

pub fn get_by_pid(pid: Pid) -> *mut Task {
    unsafe {
        for task in TASK_TABLE.iter() {
            if !task.is_null() && (**task).pid == pid {
                return *task;
            }
        }
    }
    ptr::null_mut()
}

pub fn get_by_index(index: usize) -> *mut Task {
    if index >= MAX_TASKS {
        return ptr::null_mut();
    }
    unsafe { TASK_TABLE[index] }
}

pub fn boot_task() -> *mut Task {
    unsafe { BOOT_TASK.as_mut_ptr() }
}

pub fn state_name(state: TaskState) -> &'static str {
    match state {
        TaskState::Created => "CREATED",
        TaskState::Ready => "READY",
        TaskState::Running => "RUNNING",
        TaskState::Blocked => "BLOCKED",
        TaskState::Sleeping => "SLEEPING",
        TaskState::Zombie => "ZOMBIE",
        TaskState::Dead => "DEAD",
    }
}

pub type KThreadEntry = extern "C" fn(*mut u8);

extern "C" fn kthread_entry_wrapper(entry: KThreadEntry, arg: *mut u8) -> ! {
    entry(arg);
    kthread_exit(0);
}

pub fn kthread_create(name: &str, entry: KThreadEntry, arg: *mut u8) -> Pid {
    let task = task_alloc(name, TASK_FLAG_KERNEL);
    if task.is_null() {
        return INVALID_PID;
    }

    unsafe {
        let stack_top = (*task).kernel_stack_top;
        let mut frame_addr = stack_top - size_of::<InterruptFrame>() as u64;
        frame_addr &= !0xF;

        let frame = frame_addr as *mut InterruptFrame;
        ptr::write_bytes(frame as *mut u8, 0, size_of::<InterruptFrame>());

        (*frame).ss = KERNEL_DS;
        (*frame).rsp = stack_top;
        (*frame).rflags = RFLAGS_IF;
        (*frame).cs = KERNEL_CS;
        (*frame).rip = kthread_entry_wrapper as *const () as u64;
        (*frame).int_no = 0;
        (*frame).error_code = 0;
        (*frame).rdi = entry as u64;
        (*frame).rsi = arg as u64;

        (*task).saved_rsp = frame_addr;
        (*task).state = TaskState::Ready;
    }

    crate::sched::add_task(task);

    unsafe { (*task).pid }
}

pub fn kthread_exit(exit_code: i32) -> ! {
    let task = current();
    if task.is_null() {
        loop {
            unsafe {
                core::arch::asm!("cli; hlt", options(nomem, nostack, preserves_flags));
            }
        }
    }

    unsafe {
        (*task).exit_code = exit_code;
        (*task).state = TaskState::Zombie;
    }

    crate::sched::yield_now();

    loop {
        unsafe {
            core::arch::asm!("cli; hlt", options(nomem, nostack, preserves_flags));
        }
    }
}

pub fn user_stdio_init(task: *mut Task) {
    if task.is_null() {
        return;
    }

    unsafe {
        if ((*task).flags & TASK_FLAG_USER) == 0 {
            return;
        }

        (*task).user_fd_kind[0] = USER_FD_KIND_CONSOLE;
        (*task).user_fd_kind[1] = USER_FD_KIND_CONSOLE;
        (*task).user_fd_kind[2] = USER_FD_KIND_CONSOLE;

        (*task).user_fd_inode[0] = USER_FD_NONE;
        (*task).user_fd_inode[1] = USER_FD_NONE;
        (*task).user_fd_inode[2] = USER_FD_NONE;

        (*task).user_fd_offset[0] = 0;
        (*task).user_fd_offset[1] = 0;
        (*task).user_fd_offset[2] = 0;

        (*task).user_fd_flags[0] = fs::O_RDONLY;
        (*task).user_fd_flags[1] = fs::O_WRONLY;
        (*task).user_fd_flags[2] = fs::O_WRONLY;
    }
}

pub fn apply_user_tls_base(task: *mut Task) {
    if task.is_null() {
        arch_syscall::set_user_fs_base(0);
        return;
    }

    let fs_base = unsafe { (*task).user_fs_base };
    arch_syscall::set_user_fs_base(fs_base);
}

#[cfg(test)]
impl Task {
    pub(crate) fn for_test(pid: Pid) -> Self {
        Task {
            pid,
            tgid: pid,
            group_leader: ptr::null_mut(),
            ppid: 0,
            pgid: pid,
            sid: pid,
            ctty_type: CTTY_NONE,
            ctty_id: 0,
            name: [0; TASK_NAME_MAX],
            state: TaskState::Ready,
            flags: TASK_FLAG_KERNEL,
            exit_code: 0,
            saved_rsp: 0,
            kernel_stack_base: 0,
            kernel_stack_top: 0,
            pml4_phys: 0,
            user_entry: 0,
            user_stack_bottom: 0,
            user_stack_top: 0,
            user_stack_ptr: 0,
            user_argc: 0,
            user_argv: 0,
            user_envp: 0,
            user_auxv: 0,
            user_fs_base: 0,
            user_clear_tid: 0,
            user_brk_start: 0,
            user_brk: 0,
            user_phdr_addr: 0,
            user_phdr_count: 0,
            user_phdr_size: 0,
            user_base_addr: 0,
            user_interp_base: 0,
            user_is_dynamic: false,
            user_uid: 0,
            user_gid: 0,
            user_cwd_inode: fs::root_inode(),
            user_fd_kind: [USER_FD_KIND_EMPTY; MAX_TASK_FDS],
            user_fd_inode: [USER_FD_NONE; MAX_TASK_FDS],
            user_fd_offset: [0; MAX_TASK_FDS],
            user_fd_flags: [0; MAX_TASK_FDS],
            user_shm_id: [0; MAX_TASK_SHM_ATTACH],
            user_shm_addr: [0; MAX_TASK_SHM_ATTACH],
            user_shm_size: [0; MAX_TASK_SHM_ATTACH],
            user_shm_flags: [0; MAX_TASK_SHM_ATTACH],
            sig_pending: 0,
            sig_info: [0; SIG_MAX_TASK_SIGNALS],
            signal_blocked: 0,
            signal_handlers: [0; SIG_MAX_TASK_SIGNALS],
            signal_flags: [0; SIG_MAX_TASK_SIGNALS],
            signal_mask_saved: [0; SIG_MAX_TASK_SIGNALS],
            signal_restorer: 0,
            time_slice: 0,
            total_ticks: 0,
            sleep_until: 0,
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
            fpu: FxSaveArea::new_default(),
        }
    }

    pub(crate) fn install_for_test(slot: usize, task: *mut Task) {
        assert!(slot < MAX_TASKS);
        unsafe {
            let was_empty = TASK_TABLE[slot].is_null();
            TASK_TABLE[slot] = task;
            if was_empty && !task.is_null() {
                TASK_COUNT = TASK_COUNT.saturating_add(1);
            } else if !was_empty && task.is_null() {
                TASK_COUNT = TASK_COUNT.saturating_sub(1);
            }
            TASK_INITIALIZED = true;
        }
    }

    pub(crate) fn reset_registry_for_test() {
        unsafe {
            for slot in TASK_TABLE.iter_mut() {
                *slot = ptr::null_mut();
            }
            TASK_COUNT = 0;
            NEXT_PID = 1;
            TASK_INITIALIZED = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_leader_takes_over_process_identity() {
        Task::reset_registry_for_test();
        let mut leader = Task::for_test(10);
        leader.flags = TASK_FLAG_USER;
        leader.user_fd_kind[3] = USER_FD_KIND_PIPE;
        leader.user_fd_inode[3] = 7;
        leader.ppid = 2;
        let mut exec_thread = Task::for_test(11);
        let mut other = Task::for_test(12);
        let lp = &mut leader as *mut Task;
        let tp = &mut exec_thread as *mut Task;
        let op = &mut other as *mut Task;
        join_thread_group(tp, lp);
        join_thread_group(op, lp);
        Task::install_for_test(0, lp);
        Task::install_for_test(1, tp);
        Task::install_for_test(2, op);

        take_over_group_leadership(tp);

        unsafe {
            assert_eq!((*tp).pid, 10, "exec thread takes the process id");
            assert_eq!((*tp).tgid, 10);
            assert_eq!((*lp).pid, 11, "old leader takes the thread id");
            assert!(leader_of(tp) == tp);
            assert!(leader_of(lp) == tp);
            assert!(leader_of(op) == tp);
            assert_eq!((*tp).flags & TASK_FLAG_THREAD, 0);
            assert_ne!((*lp).flags & TASK_FLAG_THREAD, 0);
            assert_eq!((*tp).user_fd_kind[3], USER_FD_KIND_PIPE);
            assert_eq!((*tp).user_fd_inode[3], 7);
            assert_eq!((*tp).ppid, 2);
            assert_eq!((*lp).user_fd_kind[3], USER_FD_KIND_EMPTY);
        }
        assert!(get_by_pid(10) == tp);
        Task::reset_registry_for_test();
    }
}
