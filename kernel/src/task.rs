use core::mem::{size_of, MaybeUninit};
use core::ptr;

use arch_x86_64::addr::VirtAddr;
use arch_x86_64::idt::InterruptFrame;

use crate::memory::pmm;
use crate::serial_println;

pub type Pid = i32;

pub const INVALID_PID: Pid = -1;

pub const TASK_NAME_MAX: usize = 32;
pub const KERNEL_STACK_SIZE: u64 = 16 * 1024;
pub const KERNEL_STACK_PAGES: usize = (KERNEL_STACK_SIZE / 4096) as usize;
pub const MAX_TASKS: usize = 64;
pub const MAX_TASK_FDS: usize = 32;

pub const TASK_FLAG_KERNEL: u32 = 0x01;
pub const TASK_FLAG_USER: u32 = 0x02;
pub const TASK_FLAG_IDLE: u32 = 0x04;

pub const USER_FD_KIND_EMPTY: u8 = 0;
pub const USER_FD_KIND_CONSOLE: u8 = 1;
pub const USER_FD_KIND_VFS: u8 = 2;
pub const USER_FD_KIND_PIPE: u8 = 3;
pub const USER_FD_NONE: u16 = u16::MAX;

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
    pub(crate) pid: Pid,
    pub(crate) ppid: Pid,
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
    pub(crate) time_slice: u64,
    pub(crate) total_ticks: u64,
    pub(crate) sleep_until: u64,
    pub(crate) next: *mut Task,
    pub(crate) prev: *mut Task,
}

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
            ppid: 0,
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
            time_slice: 0,
            total_ticks: 0,
            sleep_until: 0,
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
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
        task.ppid = current().as_ref().map(|t| t.pid).unwrap_or(0);
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
        task.time_slice = 0;
        task.total_ticks = 0;
        task.sleep_until = 0;
        task.next = ptr::null_mut();
        task.prev = ptr::null_mut();
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

#[cfg(test)]
impl Task {
    pub(crate) fn for_test(pid: Pid) -> Self {
        Task {
            pid,
            ppid: 0,
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
            time_slice: 0,
            total_ticks: 0,
            sleep_until: 0,
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
        }
    }
}
