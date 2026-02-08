use arch_x86_64::syscall::SyscallFrame;

const ENOSYS: i64 = 38;

#[no_mangle]
pub extern "C" fn syscall_handler(frame: &mut SyscallFrame) -> u64 {
    let syscall_num = frame.rax;
    crate::serial_println!("[SYSCALL] Unhandled syscall {}", syscall_num);
    (-ENOSYS) as u64
}
