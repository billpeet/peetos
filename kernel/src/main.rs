#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

#[cfg(test)]
extern crate std;

#[cfg(not(test))]
use core::panic::PanicInfo;

mod serial;
mod qemu;

#[no_mangle]
pub extern "C" fn kmain() -> ! {
    serial::init();
    serial_println!("PeetOS Rust kernel: serial online");

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
