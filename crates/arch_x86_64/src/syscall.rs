#[cfg(test)]
use core::mem::size_of;

#[cfg(target_os = "none")]
use crate::gdt::{GDT_KERNEL_CS, GDT_KERNEL_DS};
#[cfg(target_os = "none")]
use crate::msr;

pub const EFER_SCE: u64 = 1 << 0;
pub const EFER_NXE: u64 = 1 << 11;

pub const RFLAGS_IF: u64 = 1 << 9;
pub const RFLAGS_TF: u64 = 1 << 8;
pub const RFLAGS_DF: u64 = 1 << 10;

#[repr(C)]
pub struct SyscallFrame {
    pub r9: u64,
    pub r8: u64,
    pub r10: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rax: u64,
    pub r11: u64,
    pub rcx: u64,
    pub rsp: u64,
}

#[cfg(target_os = "none")]
pub fn init() {
    unsafe {
        let mut efer = msr::read(msr::IA32_EFER);
        efer |= EFER_SCE | EFER_NXE;
        msr::write(msr::IA32_EFER, efer);

        let star = ((GDT_KERNEL_DS as u64) << 48) | ((GDT_KERNEL_CS as u64) << 32);
        msr::write(msr::IA32_STAR, star);
        msr::write(msr::IA32_LSTAR, syscall_entry as *const () as u64);
        msr::write(msr::IA32_FMASK, RFLAGS_IF | RFLAGS_TF | RFLAGS_DF);
    }
}

#[cfg(not(target_os = "none"))]
pub fn init() {}

#[cfg(target_os = "none")]
#[inline]
pub fn set_user_fs_base(base: u64) {
    unsafe {
        msr::write(msr::IA32_FS_BASE, base);
    }
}

#[cfg(not(target_os = "none"))]
#[inline]
pub fn set_user_fs_base(_base: u64) {}

#[cfg(target_os = "none")]
#[inline]
pub fn user_fs_base() -> u64 {
    unsafe { msr::read(msr::IA32_FS_BASE) }
}

#[cfg(not(target_os = "none"))]
#[inline]
pub fn user_fs_base() -> u64 {
    0
}

#[cfg(target_os = "none")]
extern "C" {
    fn syscall_entry();
}

#[cfg(target_os = "none")]
core::arch::global_asm!(
    r#"
.section .text
.extern syscall_handler

.global syscall_entry
syscall_entry:
    push rsp
    push rcx
    push r11

    push rax
    push rdi
    push rsi
    push rdx
    push r10
    push r8
    push r9

    mov rdi, rsp
    call syscall_handler

    mov [rsp + 48], rax
    mov r9, [rsp + 0]
    mov r8, [rsp + 8]
    mov r10, [rsp + 16]
    mov rdx, [rsp + 24]
    mov rsi, [rsp + 32]
    mov rdi, [rsp + 40]
    mov rax, [rsp + 48]
    mov r11, [rsp + 56]
    mov rcx, [rsp + 64]
    mov rsp, [rsp + 72]
    sysretq
"#
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syscall_frame_size() {
        assert_eq!(size_of::<SyscallFrame>(), 80);
    }
}
