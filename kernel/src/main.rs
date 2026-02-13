#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

#[cfg(test)]
extern crate std;

#[cfg(not(test))]
use core::panic::PanicInfo;

mod block;
mod elf;
mod exception;
mod framebuffer;
mod ipc;
mod keyboard;
mod memory;
mod qemu;
mod sched;
mod serial;
mod shm;
mod signal;
mod storage;
mod sync;
mod syscall;
mod task;
mod tty;
mod user;
mod vfs;

#[cfg(not(feature = "test-exit"))]
mod fb_console;
#[cfg(feature = "test-exit")]
mod fb_console {
    pub fn init() {}
    pub fn write_bytes(_bytes: &[u8]) {}
}

#[cfg(not(feature = "test-exit"))]
mod font8x8_basic;

use arch_x86_64::{gdt, idt, interrupts, pic, pit, syscall as arch_syscall, tss};
use drivers::pci;
use memory::{heap, pmm, vmm};

const PCI_SCAN_LIMIT: usize = 64;
const PCI_LOG_LIMIT: usize = 16;

#[no_mangle]
pub extern "C" fn kmain() -> ! {
    serial::init();
    serial_println!("PeetOS Rust kernel: serial online");

    framebuffer::init();
    fb_console::init();
    if framebuffer::is_available() {
        serial_println!("[FB] Active");
    } else {
        serial_println!("[FB] Unavailable (VGA fallback)");
    }

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

    tty::init();
    serial_println!("[TTY] Initialized");

    keyboard::init();
    serial_println!("[KBD] Initialized");

    pmm::init();
    serial_println!("[PMM] Initialized");

    vmm::init();
    serial_println!("[VMM] Initialized");

    heap::init();
    serial_println!("[KMALLOC] Initialized");

    let mut pci_devices = [pci::DeviceInfo::EMPTY; PCI_SCAN_LIMIT];
    let pci_summary = pci::enumerate(&mut pci_devices);
    let pci_show = core::cmp::min(PCI_LOG_LIMIT, pci_summary.stored_devices);
    serial_println!(
        "[PCI] Enumerated {} function(s) (showing {})",
        pci_summary.total_devices,
        pci_show
    );
    for dev in pci_devices.iter().take(pci_show) {
        serial_println!(
            "[PCI] {:02X}:{:02X}.{} vendor={:04X} device={:04X} class={:02X}:{:02X} prog_if={:02X} irq={:02X}",
            dev.bus,
            dev.device,
            dev.function,
            dev.vendor_id,
            dev.device_id,
            dev.class_code,
            dev.subclass,
            dev.prog_if,
            dev.irq_line
        );
    }
    if pci_summary.overflow {
        serial_println!(
            "[PCI] {} additional function(s) not shown",
            pci_summary.total_devices - pci_show
        );
    }

    storage::init(&pci_devices[..pci_summary.stored_devices]);

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
