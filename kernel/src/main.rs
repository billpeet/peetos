#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

#[cfg(test)]
extern crate std;

#[cfg(not(test))]
use core::panic::PanicInfo;

mod serial;
mod qemu;
mod exception;
mod syscall;
mod memory;

use arch_x86_64::{gdt, idt, interrupts, pic, pit, syscall as arch_syscall};
use memory::pmm;

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

    pic::init();
    serial_println!("[PIC] Remapped");

    pit::init();
    serial_println!("[PIT] Initialized");

    pmm::init();
    serial_println!("[PMM] Initialized");

    arch_syscall::init();
    serial_println!("[SYSCALL] MSRs configured");

    interrupts::enable();

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
    exception::report(frame);

    loop {
        unsafe { core::arch::asm!("cli; hlt"); }
    }
}
