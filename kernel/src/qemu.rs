pub const QEMU_EXIT_PORT: u16 = 0xF4;

pub fn exit_success() {
    exit_with_code(0x10);
}

fn exit_with_code(code: u32) {
    unsafe {
        outl(QEMU_EXIT_PORT, code);
    }
}

unsafe fn outl(port: u16, val: u32) {
    core::arch::asm!(
        "out dx, eax",
        in("dx") port,
        in("eax") val,
        options(nomem, nostack, preserves_flags)
    );
}
