use arch_x86_64::port::outl;

pub const QEMU_EXIT_PORT: u16 = 0xF4;

pub fn exit_success() {
    exit_with_code(0x10);
}

/// Exit QEMU with a distinct status (`(0x11 << 1) | 1 = 35`).
#[cfg_attr(not(feature = "test-exit"), allow(dead_code))]
pub fn exit_failure() {
    exit_with_code(0x11);
}

fn exit_with_code(code: u32) {
    unsafe {
        outl(QEMU_EXIT_PORT, code);
    }
}
