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
const SYS_EXIT_GROUP: u64 = 231;

const WRITE_CHUNK_SIZE: usize = 128;
const UNKNOWN_SYSCALL_LOG_LIMIT: u32 = 8;

static mut UNKNOWN_SYSCALL_LOG_COUNT: u32 = 0;

#[derive(Clone, Copy)]
struct SyscallArgs {
    num: u64,
    args: [u64; 6],
}

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

fn copy_from_user(task_ptr: *mut task::Task, user_src: u64, dst: &mut [u8]) -> Result<(), i64> {
    if dst.is_empty() {
        return Ok(());
    }

    if task_ptr.is_null() {
        return Err(ESRCH);
    }

    if !user_range_ok(user_src, dst.len()) {
        return Err(EFAULT);
    }

    let pml4 = unsafe { PhysAddr::new((*task_ptr).pml4_phys) };
    if pml4.as_u64() == 0 {
        return Err(EINVAL);
    }

    let mut remaining = dst.len();
    let mut copied = 0usize;

    while remaining > 0 {
        let src_vaddr = user_src + copied as u64;
        let page_vaddr = src_vaddr & !(PAGE_SIZE - 1);
        let page_off = (src_vaddr & (PAGE_SIZE - 1)) as usize;
        let bytes_in_page = cmp::min(remaining, PAGE_SIZE as usize - page_off);

        let page_phys = match vmm::get_physical_in(pml4, VirtAddr::new(page_vaddr)) {
            Some(phys) => phys,
            None => return Err(EFAULT),
        };

        let src_phys = page_phys.as_u64() + page_off as u64;
        let src_ptr = PhysAddr::new(src_phys).to_virt().as_u64() as *const u8;

        unsafe {
            ptr::copy_nonoverlapping(src_ptr, dst.as_mut_ptr().add(copied), bytes_in_page);
        }

        copied += bytes_in_page;
        remaining -= bytes_in_page;
    }

    Ok(())
}

fn map_user_pages(pml4: PhysAddr, start: u64, end: u64) -> Result<(), i64> {
    let mut addr = start;
    while addr < end {
        let virt = VirtAddr::new(addr);
        if vmm::get_physical_in(pml4, virt).is_some() {
            addr += PAGE_SIZE;
            continue;
        }

        let phys = pmm::alloc_page();
        if phys.as_u64() == 0 {
            return Err(ENOMEM);
        }

        unsafe {
            ptr::write_bytes(phys.to_virt().as_u64() as *mut u8, 0, PAGE_SIZE as usize);
        }

        let map_result = vmm::map_page_in(
            pml4,
            virt,
            phys,
            vmm::PTE_PRESENT | vmm::PTE_WRITE | vmm::PTE_USER | vmm::PTE_NO_EXECUTE,
        );
        if map_result != 0 {
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

fn set_program_break(task_ptr: *mut task::Task, requested: u64) -> i64 {
    if task_ptr.is_null() {
        return -ESRCH;
    }

    unsafe {
        if ((*task_ptr).flags & TASK_FLAG_USER) == 0 {
            return -EPERM;
        }
    }

    let (current, start, stack_bottom, pml4) = unsafe {
        (
            (*task_ptr).user_brk,
            (*task_ptr).user_brk_start,
            (*task_ptr).user_stack_bottom,
            PhysAddr::new((*task_ptr).pml4_phys),
        )
    };

    if current == 0 || start == 0 || stack_bottom == 0 || pml4.as_u64() == 0 {
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
        if map_user_pages(pml4, old_pages_end, new_pages_end).is_err() {
            unmap_user_pages(pml4, old_pages_end, new_pages_end);
            return current as i64;
        }
    } else if new_pages_end < old_pages_end {
        unmap_user_pages(pml4, new_pages_end, old_pages_end);
    }

    unsafe {
        (*task_ptr).user_brk = requested;
    }
    requested as i64
}

fn sys_exit(code: i32) {
    let task = task::current();
    if task.is_null() {
        sys_log!("[SYSCALL] sys_exit: no current task");
        return;
    }

    unsafe {
        (*task).exit_code = code;
        (*task).state = TaskState::Zombie;
    }

    crate::sched::request_preempt();
}

fn sys_yield() -> i64 {
    crate::sched::request_preempt();
    0
}

fn sys_getpid() -> i64 {
    let task = task::current();
    if task.is_null() {
        return -ESRCH;
    }
    unsafe { (*task).pid as i64 }
}

fn sys_getppid() -> i64 {
    let task = task::current();
    if task.is_null() {
        return -ESRCH;
    }
    unsafe { (*task).ppid as i64 }
}

fn sys_write(fd: i32, user_buf: u64, count: u64) -> i64 {
    if fd != 1 && fd != 2 {
        return -EBADF;
    }

    let len = match usize::try_from(count) {
        Ok(v) => v,
        Err(_) => return -EINVAL,
    };
    if len == 0 {
        return 0;
    }

    let task = task::current();
    if task.is_null() {
        return -ESRCH;
    }

    let is_user = unsafe { ((*task).flags & TASK_FLAG_USER) != 0 };
    if !is_user {
        return -EPERM;
    }

    let mut chunk = [0u8; WRITE_CHUNK_SIZE];
    let mut written = 0usize;

    while written < len {
        let take = cmp::min(chunk.len(), len - written);
        let src = user_buf + written as u64;
        let copy_res = copy_from_user(task, src, &mut chunk[..take]);
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
    match call.num {
        SYS_READ => -ENOSYS,
        SYS_WRITE => sys_write(call.args[0] as i32, call.args[1], call.args[2]),
        SYS_BRK => set_program_break(task::current(), call.args[0]),
        SYS_YIELD => sys_yield(),
        SYS_GETPID => sys_getpid(),
        SYS_GETPPID => sys_getppid(),
        SYS_EXIT | SYS_EXIT_GROUP => {
            sys_exit(call.args[0] as i32);
            0
        }
        _ => {
            log_unknown_syscall(call.num);
            -ENOSYS
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
    fn unknown_syscall_returns_enosys() {
        let ret = dispatch_syscall(SyscallArgs {
            num: 0xFFFF,
            args: [0; 6],
        });
        assert_eq!(ret, -ENOSYS);
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
