use arch_x86_64::port::outl;

pub const QEMU_EXIT_PORT: u16 = 0xF4;

pub fn exit_success() {
    exit_with_code(0x10);
}

fn exit_with_code(code: u32) {
    unsafe {
        outl(QEMU_EXIT_PORT, code);
    }
}
