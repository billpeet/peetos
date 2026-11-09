#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

#[cfg(test)]
extern crate std;
extern crate alloc;

#[cfg(not(test))]
use core::panic::PanicInfo;

mod arp;
mod bkl;
mod block;
mod display_mode;
mod drm;
mod e1000;
mod elf;
mod evfd;
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
mod shmem;
mod signal;
#[cfg(target_os = "none")]
mod smp;
#[cfg(all(target_os = "none", feature = "test-exit"))]
mod smp_test;
#[cfg(not(target_os = "none"))]
mod smp {
    //! Host-test stand-in: the real module links against the AP trampoline
    //! in `entry.asm`, which only exists in kernel builds.
    pub fn init() {}
    pub fn start_aps() {}
    pub fn release_aps() {}
    pub fn ipi_self_test() -> (usize, usize) {
        (0, 0)
    }
    pub fn cpu_count() -> usize {
        1
    }
    pub fn online_count() -> usize {
        1
    }
    pub fn is_online(index: usize) -> bool {
        index == 0
    }
    pub fn send_resched(_index: usize) {}
    pub fn note_active_cr3(_cr3: u64) {}
    pub fn service_tlb_shootdown() {}
    pub fn tlb_shootdown(_pml4: u64) {}
}
mod socket;
mod storage;
mod sync;
mod syscall;
mod sysfs;
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
    pub fn activate_for_fatal_report() -> bool {
        true
    }
    pub fn lock_holder() -> (&'static str, Option<&'static core::panic::Location<'static>>, i32) {
        ("none", None, -1)
    }
}

#[cfg(not(feature = "test-exit"))]
mod font8x16_vga;

use arch_x86_64::{gdt, idt, interrupts, pic, pit, syscall as arch_syscall};
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

    // Per-CPU GDT/TSS and GS base for the BSP (CPU 0).
    arch_x86_64::percpu::init_cpu(0);
    serial_println!("[TSS] Loaded (per-CPU area for CPU 0)");

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
    // All RAM is in the direct map now; let the PMM hand out the rest.
    pmm::release_high_memory();

    heap::init();
    serial_println!("[KMALLOC] Initialized");

    // Needs the heap/VMM for MMIO mappings; must precede drivers that unmask
    // IRQs after this point (e1000) and the first interrupts::enable().
    smp::init();
    smp::start_aps();
    smp::ipi_self_test();

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

    arch_x86_64::fpu::init();
    serial_println!("[FPU] FXSAVE context switching enabled (MXCSR mask {:#x})", arch_x86_64::fpu::mxcsr_mask());

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
    idt::set_entry_hook(bkl::interrupt_enter, bkl::interrupt_release);
    // The PIT tick only touches the scheduler (its own lock).
    idt::set_no_entry_lock(arch_x86_64::irq::irq_to_vector(pic::IRQ_TIMER));
    idt::set_exit_hook(bkl::frame_exit_hook);
    idt::set_post_handler_hook(sched::isr_hook);
    smp::release_aps();

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
    {
        // Prove the timer IRQ is actually delivered (via the IOAPIC when APIC
        // mode is active) before reporting success.
        let start = pit::ticks();
        let mut spins: u64 = 0;
        while pit::ticks() < start + 5 && spins < 50_000_000 {
            core::hint::spin_loop();
            spins += 1;
        }
        let delta = pit::ticks() - start;
        serial_println!(
            "[TEST] Timer advanced {} tick(s) via {}",
            delta,
            if arch_x86_64::irq::is_apic_mode() { "IOAPIC" } else { "PIC" }
        );
        if delta < 5 {
            qemu::exit_failure();
        }
        let expected = core::cmp::min(smp::online_count() as u32, 2);
        if smp_test::run() < expected {
            qemu::exit_failure();
        }
        if smp::online_count() >= 2 && smp_test::run_bkl_free() == 0 {
            qemu::exit_failure();
        }
        qemu::exit_success();
    }

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
        // Idle until the next interrupt without holding the big kernel lock,
        // so other CPUs can run kernel code meanwhile.
        bkl::wait_for_interrupt();
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial::init();
    display_mode::activate_console();
    // Never wait for the console lock here: the panic may come from code
    // that holds it.
    fb_console::activate_for_fatal_report();
    mouse::sync_display_owner();
    serial_println!("KERNEL PANIC: {}", info);

    loop {
        unsafe {
            core::arch::asm!("cli; hlt");
        }
    }
}

/// Set once a fatal exception is being reported, so a fault inside the
/// reporting path (console redraw) does not recurse into it again.
static IN_FATAL_REPORT: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

fn report_current_task() {
    let current = task::current();
    if !current.is_null() {
        let name = unsafe { &(*current).name };
        let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        serial_println!(
            "[EXC] current task pid {} '{}', last syscall {} args {:x?}",
            unsafe { (*current).pid },
            core::str::from_utf8(&name[..len]).unwrap_or("?"),
            unsafe { (*current).last_syscall },
            unsafe { (*current).last_syscall_args }
        );
    }
}

extern "C" fn default_exception_handler(frame: &mut idt::InterruptFrame) {
    serial::init();
    if exception::handle(frame) {
        return;
    }
    let nested = IN_FATAL_REPORT.swap(true, core::sync::atomic::Ordering::SeqCst);

    // Report to serial first, before touching the console: redrawing it is
    // the riskiest step, and the original fault must not be lost if it fails.
    // While the compositor owns the display, console writes go to serial only.
    if nested {
        serial_println!("[EXC] fault while reporting an earlier fatal exception");
    }
    report_current_task();
    exception::report(frame);
    if frame.cs & 3 == 0 {
        // Kernel-mode fault: if the stack page was overwritten, say whether a
        // process still maps that frame.
        task::report_user_mappings_of_kernel_addr(frame.rsp);
        task::report_kernel_stack_usage();
    }

    if !nested {
        display_mode::activate_console();
        // Never wait for the console lock here: the faulting code may hold it
        // (interrupts are off, so it could never release it).
        if fb_console::activate_for_fatal_report() {
            // Repeat the report on the now-visible console.
            exception::report(frame);
        } else {
            let (site, caller, pid) = fb_console::lock_holder();
            match caller {
                Some(loc) => serial_println!(
                    "[EXC] console busy at the fault (held by {} from {}:{} in pid {})",
                    site,
                    loc.file(),
                    loc.line(),
                    pid
                ),
                None => serial_println!("[EXC] console busy at the fault (held by {} in pid {})", site, pid),
            }
        }
        mouse::sync_display_owner();
    }

    loop {
        unsafe {
            core::arch::asm!("cli; hlt");
        }
    }
}
