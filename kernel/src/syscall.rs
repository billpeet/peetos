use core::cmp;
use core::ptr;

use arch_x86_64::addr::{PhysAddr, VirtAddr, KERNEL_VIRT_BASE, PAGE_SIZE};
use arch_x86_64::idt::InterruptFrame;
use arch_x86_64::syscall::SyscallFrame;

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
const ESRCH: i64 = 3;
const EBADF: i64 = 9;
const ENOMEM: i64 = 12;
const EFAULT: i64 = 14;
const EINVAL: i64 = 22;
const ENOSYS: i64 = 38;

const SYS_READ: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_BRK: u64 = 12;
const SYS_YIELD: u64 = 24;
const SYS_GETPID: u64 = 39;
const SYS_EXIT: u64 = 60;
const SYS_GETPPID: u64 = 110;
const SYS_GETTID: u64 = 186;
const SYS_SET_TID_ADDRESS: u64 = 218;
const SYS_EXIT_GROUP: u64 = 231;

const WRITE_CHUNK_SIZE: usize = 128;
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

fn sys_read(_ctx: &SyscallContext, _call: &SyscallArgs) -> i64 {
    -ENOSYS
}

fn sys_write(ctx: &SyscallContext, call: &SyscallArgs) -> i64 {
    let fd = call.arg(0) as i32;
    if fd != 1 && fd != 2 {
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

        crate::serial::write_bytes(&chunk[..take]);
        written += take;
    }

    written as i64
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

    // Full Linux semantics also require a futex wake at this address.
    unsafe {
        (*ctx.current).user_clear_tid = 0;
    }
}

fn sys_exit_common(ctx: &SyscallContext, code: i32) -> i64 {
    if !ctx.has_task() {
        return -ESRCH;
    }

    clear_child_tid(ctx);

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
