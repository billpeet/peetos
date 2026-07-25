#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

#[cfg(test)]
extern crate std;

#[cfg(not(test))]
use core::panic::PanicInfo;

mod arp;
mod block;
mod display_mode;
mod e1000;
mod elf;
mod ethernet;
mod exception;
mod framebuffer;
#[cfg(test)]
mod gui_protocol;
#[cfg(any(feature = "musl-ext2-gui-smoke", feature = "musl-ext2-gui-demo"))]
mod gui_smoke;
mod icmp;
mod input;
mod ipc;
mod ipv4;
mod keyboard;
mod memory;
mod mmio;
mod mouse;
mod net;
mod pty;
mod qemu;
mod sched;
mod serial;
mod shm;
mod signal;
mod socket;
mod storage;
mod sync;
mod syscall;
mod task;
mod tcp;
mod tty;
mod udp;
#[cfg(feature = "usb-tablet")]
mod usb;
mod user;
mod vfs;

#[cfg(not(feature = "test-exit"))]
mod fb_console;
#[cfg(feature = "test-exit")]
mod fb_console {
    pub fn init() {}
    pub fn set_active(_active: bool, _redraw: bool) {}
    pub fn tick() {}
    pub fn write_bytes(_bytes: &[u8]) {}
}

#[cfg(not(feature = "test-exit"))]
mod font8x16_vga;

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
    let display_owner = display_mode::init(framebuffer::is_available());
    fb_console::init();
    if framebuffer::is_available() {
        serial_println!("[FB] Active");
    } else {
        serial_println!("[FB] Unavailable (VGA fallback)");
    }
    match display_owner {
        display_mode::DisplayOwner::Console => {
            serial_println!("[GUI] Display owner: console");
        }
        display_mode::DisplayOwner::Compositor => {
            serial_println!("[GUI] Display owner: compositor-first");
        }
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

    input::init();
    serial_println!("[INPUT] Initialized");

    tty::init();
    serial_println!("[TTY] Initialized");

    keyboard::init();
    serial_println!("[KBD] Initialized");

    mouse::init();

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

    net::init(&pci_devices[..pci_summary.stored_devices]);
    #[cfg(feature = "usb-tablet")]
    usb::init(&pci_devices[..pci_summary.stored_devices]);

    vfs::init();

    task::init();
    serial_println!("[TASK] Initialized");

    sched::init();
    serial_println!("[SCHED] Initialized");

    ipc::init();
    serial_println!("[IPC] Initialized");

    shm::init();
    serial_println!("[SHM] Initialized");

    pty::init();
    serial_println!("[PTY] Initialized");

    pit::set_tick_hook(sched::tick);
    idt::set_post_handler_hook(sched::isr_hook);

    #[cfg(not(feature = "test-exit"))]
    let init_started = user::spawn_init();
    #[cfg(feature = "test-exit")]
    let init_started = true;
    if !init_started && display_mode::compositor_active() {
        display_mode::activate_console();
        fb_console::set_active(true, true);
        mouse::sync_display_owner();
        serial_println!("[GUI] Init launch failed; framebuffer ownership returned to console");
    }

    arch_syscall::init();
    syscall::init();
    serial_println!("[SYSCALL] MSRs configured");

    interrupts::enable();

    #[cfg(feature = "test-exit")]
    qemu::exit_success();

    loop {
        net::poll();
        #[cfg(feature = "usb-tablet")]
        usb::poll();
        mouse::begin_framebuffer_update();
        fb_console::tick();
        mouse::end_framebuffer_update();
        mouse::tick();
        #[cfg(any(feature = "musl-ext2-gui-smoke", feature = "musl-ext2-gui-demo"))]
        gui_smoke::tick();
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial::init();
    display_mode::activate_console();
    fb_console::set_active(true, true);
    mouse::sync_display_owner();
    serial_println!("KERNEL PANIC: {}", info);

    loop {
        unsafe {
            core::arch::asm!("cli; hlt");
        }
    }
}

extern "C" fn default_exception_handler(frame: &mut idt::InterruptFrame) {
    serial::init();
    if exception::handle(frame) {
        return;
    }
    display_mode::activate_console();
    fb_console::set_active(true, true);
    mouse::sync_display_owner();
    exception::report(frame);

    loop {
        unsafe {
            core::arch::asm!("cli; hlt");
        }
    }
}
