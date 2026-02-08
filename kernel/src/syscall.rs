use arch_x86_64::idt::InterruptFrame;
use arch_x86_64::syscall::SyscallFrame;

const ENOSYS: i64 = 38;
const SYS_EXIT: u64 = 1;
const SYS_YIELD: u64 = 24;

#[no_mangle]
pub extern "C" fn syscall_handler(frame: &mut SyscallFrame) -> u64 {
    let syscall_num = frame.rax;
    let ret = dispatch_syscall(syscall_num, frame.rdi, frame.rsi, frame.rdx, frame.r10, frame.r8, frame.r9);
    ret
}

pub extern "C" fn syscall_int80_handler(frame: &mut InterruptFrame) {
    let syscall_num = frame.rax;
    let ret = dispatch_syscall(syscall_num, frame.rdi, frame.rsi, frame.rdx, frame.r10, frame.r8, frame.r9);
    frame.rax = ret;
}

fn dispatch_syscall(num: u64, arg0: u64, _arg1: u64, _arg2: u64, _arg3: u64, _arg4: u64, _arg5: u64) -> u64 {
    match num {
        SYS_EXIT => {
            sys_exit(arg0 as i32);
            0
        }
        SYS_YIELD => {
            crate::sched::request_preempt();
            0
        }
        _ => {
            crate::serial_println!("[SYSCALL] Unhandled syscall {}", num);
            (-ENOSYS) as u64
        }
    }
}

fn sys_exit(code: i32) {
    let task = crate::task::current();
    if task.is_null() {
        crate::serial_println!("[SYSCALL] sys_exit: no current task");
        return;
    }

    unsafe {
        (*task).exit_code = code;
        (*task).state = crate::task::TaskState::Zombie;
    }

    crate::sched::request_preempt();
}
