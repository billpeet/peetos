#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

#[cfg(test)]
extern crate std;

#[cfg(not(test))]
use core::panic::PanicInfo;

mod elf;
mod exception;
mod ipc;
mod memory;
mod qemu;
mod sched;
mod serial;
mod shm;
mod sync;
mod syscall;
mod task;
mod user;
mod vfs;

use arch_x86_64::{gdt, idt, interrupts, pic, pit, syscall as arch_syscall, tss};
use memory::{heap, pmm, vmm};

#[no_mangle]
pub extern "C" fn kmain() -> ! {
    serial::init();
    serial_println!("PeetOS Rust kernel: serial online");

    gdt::init();
    serial_println!("[GDT] Loaded");

    tss::init();
    serial_println!("[TSS] Loaded");

    idt::init();
    for vector in 0u8..32u8 {
        idt::register_handler(vector, default_exception_handler);
    }
    idt::install_syscall_gate();
    idt::register_handler(idt::SYSCALL_VECTOR, syscall::syscall_int80_handler);
    serial_println!("[IDT] Loaded");

    pic::init();
    serial_println!("[PIC] Remapped");

    pit::init();
    serial_println!("[PIT] Initialized");

    pmm::init();
    serial_println!("[PMM] Initialized");

    vmm::init();
    serial_println!("[VMM] Initialized");

    heap::init();
    serial_println!("[KMALLOC] Initialized");

    vfs::init();

    task::init();
    serial_println!("[TASK] Initialized");

    sched::init();
    serial_println!("[SCHED] Initialized");

    ipc::init();
    serial_println!("[IPC] Initialized");

    shm::init();
    serial_println!("[SHM] Initialized");

    pit::set_tick_hook(sched::tick);
    idt::set_post_handler_hook(sched::isr_hook);

    #[cfg(not(feature = "test-exit"))]
    user::spawn_init();

    arch_syscall::init();
    syscall::init();
    serial_println!("[SYSCALL] MSRs configured");

    interrupts::enable();

    #[cfg(feature = "test-exit")]
    qemu::exit_success();

    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial::init();
    serial_println!("KERNEL PANIC: {}", info);

    loop {
        unsafe {
            core::arch::asm!("cli; hlt");
        }
    }
}

extern "C" fn default_exception_handler(frame: &mut idt::InterruptFrame) {
    serial::init();
    exception::report(frame);

    loop {
        unsafe {
            core::arch::asm!("cli; hlt");
        }
    }
}
