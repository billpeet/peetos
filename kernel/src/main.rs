#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

#[cfg(test)]
extern crate std;

#[cfg(not(test))]
use core::panic::PanicInfo;

mod serial;
mod qemu;

use arch_x86_64::{gdt, idt};

#[no_mangle]
pub extern "C" fn kmain() -> ! {
    serial::init();
    serial_println!("PeetOS Rust kernel: serial online");

    gdt::init();
    serial_println!("[GDT] Loaded");

    idt::init();
    for vector in 0u8..32u8 {
        idt::register_handler(vector, default_exception_handler);
    }
    serial_println!("[IDT] Loaded");

    #[cfg(feature = "test-exit")]
    qemu::exit_success();

    loop {
        unsafe { core::arch::asm!("hlt"); }
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial::init();
    serial_println!("KERNEL PANIC: {}", info);

    loop {
        unsafe { core::arch::asm!("cli; hlt"); }
    }
}

extern "C" fn default_exception_handler(frame: &mut idt::InterruptFrame) {
    serial::init();
    let int_no = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(frame.int_no)) };
    let error_code = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(frame.error_code)) };
    serial_println!("CPU EXCEPTION: vector={} error=0x{:x}", int_no, error_code);

    loop {
        unsafe { core::arch::asm!("cli; hlt"); }
    }
}
