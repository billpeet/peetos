//! APIC bring-up and application-processor (AP) startup.
//!
//! Boot flow (all on the BSP, interrupts disabled):
//! 1. [`init`] finds the ACPI MADT, maps the LAPIC and IOAPIC(s), enables the
//!    BSP's LAPIC and moves legacy ISA IRQ delivery from the 8259 PIC to the
//!    IOAPIC. Without a usable MADT the kernel stays on the PIC.
//! 2. [`start_aps`] copies the real-mode trampoline from `entry.asm` to
//!    `AP_TRAMPOLINE_PHYS`, then wakes each AP with INIT + STARTUP IPIs.
//!    APs load the shared GDT/IDT, enable their LAPIC and park in `hlt`
//!    with interrupts enabled so they can take IPIs.
//! 3. [`ipi_self_test`] sends a fixed IPI to every parked AP and waits for
//!    each one to acknowledge it, proving per-CPU interrupt delivery works.
//!
//! The scheduler still runs only on the BSP. APs never touch scheduler state;
//! `sched::isr_hook` ignores interrupts taken on an AP.

use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use arch_x86_64::acpi::{self, IsaRoute, MadtInfo, MAX_CPUS};
use arch_x86_64::addr::{KERNEL_VIRT_BASE, PAGE_SIZE};
use arch_x86_64::idt::{self, InterruptFrame};
use arch_x86_64::irq::{self, ISA_IRQ_COUNT};
use arch_x86_64::{gdt, interrupts, ioapic, lapic, pit};

use crate::memory::{heap, vmm};
use crate::mmio;
use crate::serial_println;

/// Physical memory reachable through the bootloader's higher-half direct map.
const DIRECT_MAP_LIMIT: u64 = 128 * 1024 * 1024;

/// Physical page the AP trampoline is copied to. Must be below 1 MiB and
/// page aligned; the STARTUP IPI vector is `AP_TRAMPOLINE_PHYS >> 12`.
/// The low 1 MiB is reserved by the PMM, and after boot nothing else uses
/// 0x8000-0xBFFF (stage2 lived there during boot only).
pub const AP_TRAMPOLINE_PHYS: u64 = 0x8000;
/// Must match `AP_DATA` in `entry.asm`.
const AP_DATA_PHYS: u64 = AP_TRAMPOLINE_PHYS + 0xF00;
const AP_DATA_PML4: u64 = 0x00;
const AP_DATA_KERNEL_CR3: u64 = 0x08;
const AP_DATA_STACK_TOP: u64 = 0x10;
const AP_DATA_CPU_INDEX: u64 = 0x18;
const AP_DATA_CR0: u64 = 0x20;
const AP_DATA_CR4: u64 = 0x28;
const AP_DATA_EFER: u64 = 0x30;

/// Transitional page tables used by APs while they enable paging.
const AP_PML4_PHYS: u64 = 0x9000;
const AP_PDPT_PHYS: u64 = 0xA000;
const AP_PD_PHYS: u64 = 0xB000;

const AP_STACK_SIZE: usize = 16 * 1024;
/// How long to wait for an AP to report in after its STARTUP IPIs.
const AP_START_TIMEOUT_US: u64 = 200_000;
const IPI_TEST_TIMEOUT_US: u64 = 100_000;

/// IPI vector used by [`ipi_self_test`].
pub const IPI_VECTOR_TEST: u8 = idt::IPI_VECTOR_FIRST;

const EFER_SCE: u64 = 1 << 0;
const EFER_LME: u64 = 1 << 8;
const EFER_NXE: u64 = 1 << 11;
const PTE_PRESENT: u64 = 1 << 0;
const PTE_WRITE: u64 = 1 << 1;
const PTE_LARGE: u64 = 1 << 7;

extern "C" {
    static ap_trampoline_start: u8;
    static ap_trampoline_end: u8;
}

struct CpuSlot {
    apic_id: AtomicU32,
    online: AtomicBool,
    ipi_count: AtomicU32,
}

impl CpuSlot {
    const fn new() -> Self {
        Self {
            apic_id: AtomicU32::new(0),
            online: AtomicBool::new(false),
            ipi_count: AtomicU32::new(0),
        }
    }
}

static CPUS: [CpuSlot; MAX_CPUS] = [const { CpuSlot::new() }; MAX_CPUS];
/// CPUs listed in the MADT (index 0 is always the BSP).
static CPU_COUNT: AtomicUsize = AtomicUsize::new(1);
static ONLINE_COUNT: AtomicUsize = AtomicUsize::new(1);
static BSP_APIC_ID: AtomicU32 = AtomicU32::new(0);
static APIC_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Set by an AP once it is running Rust code; cleared by the BSP before
/// each startup attempt.
static AP_CHECKIN: AtomicBool = AtomicBool::new(false);

pub fn apic_active() -> bool {
    APIC_ACTIVE.load(Ordering::Acquire)
}

pub fn cpu_count() -> usize {
    CPU_COUNT.load(Ordering::Acquire)
}

pub fn online_count() -> usize {
    ONLINE_COUNT.load(Ordering::Acquire)
}

/// True on the bootstrap processor (and always true before APIC init).
#[inline]
pub fn is_bsp() -> bool {
    !apic_active() || lapic::id() == BSP_APIC_ID.load(Ordering::Relaxed)
}

/// Logical index of the executing CPU (0 = BSP).
pub fn current_cpu_index() -> usize {
    if !apic_active() {
        return 0;
    }
    let id = lapic::id();
    let count = cpu_count();
    CPUS[..count]
        .iter()
        .position(|c| c.apic_id.load(Ordering::Relaxed) == id)
        .unwrap_or(0)
}

/// Borrow `len` bytes of physical memory. Low memory comes straight from the
/// direct map; anything above it goes through an MMIO window.
fn phys_bytes(addr: u64, len: usize) -> Option<&'static [u8]> {
    let end = addr.checked_add(len as u64)?;
    let ptr = if end <= DIRECT_MAP_LIMIT {
        (KERNEL_VIRT_BASE + addr) as *const u8
    } else {
        mmio::map_mmio_window(addr, len)? as *const u8
    };
    Some(unsafe { core::slice::from_raw_parts(ptr, len) })
}

fn phys_ptr<T>(addr: u64) -> *mut T {
    debug_assert!(addr < DIRECT_MAP_LIMIT);
    (KERNEL_VIRT_BASE + addr) as *mut T
}

fn find_rsdp() -> Option<acpi::Rsdp> {
    // EBDA segment pointer lives at 0x40E; scan its first KiB, then the BIOS
    // read-only area 0xE0000-0xFFFFF.
    let ebda_seg = unsafe { ptr::read_volatile(phys_ptr::<u16>(0x40E)) } as u64;
    let ebda = ebda_seg << 4;
    let mut regions: [(u64, usize); 2] = [(0xE0000, 0x20000), (0, 0)];
    if ebda >= 0x80000 && ebda < 0xA0000 {
        regions = [(ebda, 1024), (0xE0000, 0x20000)];
    }
    for (base, len) in regions {
        if len == 0 {
            continue;
        }
        let region = phys_bytes(base, len)?;
        if let Some(off) = acpi::find_rsdp(region) {
            return acpi::parse_rsdp(&region[off..]);
        }
    }
    None
}

fn map_table(addr: u64, signature: &[u8; 4]) -> Option<&'static [u8]> {
    let header = phys_bytes(addr, acpi::SDT_HEADER_LEN)?;
    let len = acpi::parse_sdt_header(header)?.length as usize;
    if len < acpi::SDT_HEADER_LEN || len > 0x10000 {
        return None;
    }
    acpi::validate_sdt(phys_bytes(addr, len)?, signature)
}

fn find_madt() -> Option<MadtInfo> {
    let rsdp = find_rsdp()?;
    let (root_addr, is_xsdt) = rsdp.root_table();
    let root_sig = if is_xsdt {
        acpi::XSDT_SIGNATURE
    } else {
        acpi::RSDT_SIGNATURE
    };
    let root = map_table(root_addr, root_sig)?;
    for i in 0..acpi::root_entry_count(root, is_xsdt) {
        let Some(addr) = acpi::root_entry(root, is_xsdt, i) else {
            continue;
        };
        let Some(header) = phys_bytes(addr, acpi::SDT_HEADER_LEN) else {
            continue;
        };
        if header.get(0..4) != Some(&acpi::MADT_SIGNATURE[..]) {
            continue;
        }
        if let Some(table) = map_table(addr, acpi::MADT_SIGNATURE) {
            return acpi::parse_madt(table);
        }
    }
    None
}

/// Discover the APIC topology and switch interrupt delivery to the
/// LAPIC/IOAPIC. Call after the heap is up and before interrupts are enabled.
pub fn init() {
    let Some(madt) = find_madt() else {
        serial_println!("[APIC] No ACPI MADT found; staying on legacy 8259 PIC");
        return;
    };
    if madt.ioapic_count == 0 {
        serial_println!("[APIC] MADT lists no IOAPIC; staying on legacy 8259 PIC");
        return;
    }

    let lapic_phys = if madt.local_apic_addr != 0 {
        madt.local_apic_addr
    } else {
        lapic::msr_base_phys()
    };
    let Some(lapic_virt) = mmio::map_mmio_window_uncached(lapic_phys, PAGE_SIZE as usize) else {
        serial_println!("[APIC] Failed to map LAPIC at {:#x}; staying on PIC", lapic_phys);
        return;
    };

    // Register IOAPICs before touching the LAPIC so a failure leaves the
    // machine exactly as it was (PIC in virtual-wire mode).
    for io in madt.ioapics() {
        let Some(virt) = mmio::map_mmio_window_uncached(io.address as u64, PAGE_SIZE as usize)
        else {
            serial_println!("[APIC] Failed to map IOAPIC {} at {:#x}", io.id, io.address);
            continue;
        };
        match ioapic::register(virt as u64, io.gsi_base) {
            Some(pins) => serial_println!(
                "[APIC] IOAPIC id={} addr={:#x} gsi={}..{}",
                io.id,
                io.address,
                io.gsi_base,
                io.gsi_base + pins - 1
            ),
            None => serial_println!("[APIC] Too many IOAPICs; ignoring id={}", io.id),
        }
    }
    if ioapic::count() == 0 {
        serial_println!("[APIC] No usable IOAPIC; staying on legacy 8259 PIC");
        return;
    }

    lapic::set_base(lapic_virt as u64);
    lapic::init_current_cpu();
    let bsp_id = lapic::id();
    BSP_APIC_ID.store(bsp_id, Ordering::Relaxed);

    // Record CPUs with the BSP first so logical index 0 is always the BSP.
    let mut count = 1;
    CPUS[0].apic_id.store(bsp_id, Ordering::Relaxed);
    CPUS[0].online.store(true, Ordering::Relaxed);
    for cpu in madt.cpus() {
        if cpu.apic_id == bsp_id || cpu.apic_id > 0xFF {
            continue; // xAPIC IPIs address 8-bit ids only
        }
        if count < MAX_CPUS {
            CPUS[count].apic_id.store(cpu.apic_id, Ordering::Relaxed);
            count += 1;
        }
    }
    CPU_COUNT.store(count, Ordering::Release);

    let routes: [IsaRoute; ISA_IRQ_COUNT] =
        core::array::from_fn(|irq| acpi::isa_route(madt.overrides(), irq as u8));
    let routed = irq::switch_to_apic(&routes, bsp_id);
    APIC_ACTIVE.store(true, Ordering::Release);

    serial_println!(
        "[APIC] LAPIC id={} at {:#x}; {} ISA IRQ(s) routed via IOAPIC, PIC disabled",
        bsp_id,
        lapic_phys,
        routed
    );
    let timer = irq::route(arch_x86_64::pic::IRQ_TIMER);
    if let Some(r) = timer {
        serial_println!("[APIC] Timer IRQ0 -> GSI {}", r.gsi);
    }
    serial_println!(
        "[SMP] MADT lists {} CPU(s){}",
        count,
        if madt.cpus_skipped > 0 { " (some skipped: MAX_CPUS)" } else { "" }
    );
}

fn read_cr0() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mov {}, cr0", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

fn read_cr4() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mov {}, cr4", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

fn write_ap_data(offset: u64, value: u64) {
    unsafe { ptr::write_volatile(phys_ptr::<u64>(AP_DATA_PHYS + offset), value) };
}

/// Copy the trampoline to low memory and build its transitional page tables.
fn install_trampoline(kernel_cr3: u64) -> bool {
    let start = ptr::addr_of!(ap_trampoline_start) as usize;
    let end = ptr::addr_of!(ap_trampoline_end) as usize;
    let len = end - start;
    if len == 0 || len as u64 > AP_DATA_PHYS - AP_TRAMPOLINE_PHYS {
        serial_println!("[SMP] Trampoline size {} does not fit", len);
        return false;
    }
    unsafe {
        ptr::copy_nonoverlapping(start as *const u8, phys_ptr::<u8>(AP_TRAMPOLINE_PHYS), len);

        let pml4 = phys_ptr::<u64>(AP_PML4_PHYS);
        let pdpt = phys_ptr::<u64>(AP_PDPT_PHYS);
        let pd = phys_ptr::<u64>(AP_PD_PHYS);
        ptr::write_bytes(pml4, 0, 512);
        ptr::write_bytes(pdpt, 0, 512);
        ptr::write_bytes(pd, 0, 512);
        // Identity-map the low 2 MiB so the trampoline survives enabling paging.
        ptr::write_volatile(pd, PTE_PRESENT | PTE_WRITE | PTE_LARGE);
        ptr::write_volatile(pdpt, AP_PD_PHYS | PTE_PRESENT | PTE_WRITE);
        ptr::write_volatile(pml4, AP_PDPT_PHYS | PTE_PRESENT | PTE_WRITE);
        // Share the kernel's higher half (code, heap, MMIO windows).
        let kernel_pml4 = phys_ptr::<u64>(kernel_cr3 & vmm::PTE_ADDR_MASK);
        ptr::write_volatile(pml4.add(511), ptr::read_volatile(kernel_pml4.add(511)));
    }

    let efer = unsafe { arch_x86_64::msr::read(arch_x86_64::msr::IA32_EFER) };
    write_ap_data(AP_DATA_PML4, AP_PML4_PHYS);
    write_ap_data(AP_DATA_KERNEL_CR3, kernel_cr3);
    write_ap_data(AP_DATA_CR0, read_cr0() & 0xFFFF_FFFF);
    write_ap_data(AP_DATA_CR4, read_cr4() & 0xFFFF_FFFF);
    write_ap_data(AP_DATA_EFER, (efer & (EFER_SCE | EFER_NXE)) | EFER_LME);
    true
}

fn wait_for_checkin(timeout_us: u64) -> bool {
    let step = 100;
    let mut waited = 0;
    while waited < timeout_us {
        if AP_CHECKIN.load(Ordering::Acquire) {
            return true;
        }
        pit::busy_wait_us(step);
        waited += step;
    }
    AP_CHECKIN.load(Ordering::Acquire)
}

fn start_one(index: usize) -> bool {
    let apic_id = CPUS[index].apic_id.load(Ordering::Relaxed);
    let stack = heap::kmalloc(AP_STACK_SIZE);
    if stack.is_null() {
        serial_println!("[SMP] No memory for CPU {} stack", index);
        return false;
    }
    let stack_top = (stack as u64 + AP_STACK_SIZE as u64) & !0xF;
    write_ap_data(AP_DATA_STACK_TOP, stack_top);
    write_ap_data(AP_DATA_CPU_INDEX, index as u64);
    AP_CHECKIN.store(false, Ordering::Release);

    // INIT, then two STARTUPs (Intel MP spec / SDM 8.4.4.1).
    lapic::write(lapic::REG_ESR, 0);
    lapic::send_ipi(apic_id, lapic::icr_init());
    pit::busy_wait_us(10_000);
    for _ in 0..2 {
        lapic::write(lapic::REG_ESR, 0);
        lapic::send_ipi(apic_id, lapic::icr_startup(AP_TRAMPOLINE_PHYS));
        if wait_for_checkin(200) {
            break;
        }
    }
    if wait_for_checkin(AP_START_TIMEOUT_US) {
        // Wait until the AP has finished its setup and marked itself online.
        for _ in 0..1000 {
            if CPUS[index].online.load(Ordering::Acquire) {
                return true;
            }
            pit::busy_wait_us(100);
        }
    }
    // Leave the stack allocated: a late AP could still be using it.
    serial_println!("[SMP] CPU {} (APIC id {}) did not start", index, apic_id);
    false
}

/// Wake every AP listed in the MADT. Requires [`init`] to have switched to
/// APIC mode; the PIT must be initialised (used for startup delays).
pub fn start_aps() {
    if !apic_active() {
        return;
    }
    let count = cpu_count();
    if count <= 1 {
        serial_println!("[SMP] 1 CPU online (uniprocessor)");
        return;
    }
    idt::register_handler(IPI_VECTOR_TEST, test_ipi_handler);

    let kernel_cr3 = vmm::get_kernel_pml4().as_u64();
    if !interrupts::without_interrupts(|| install_trampoline(kernel_cr3)) {
        return;
    }
    for index in 1..count {
        if start_one(index) {
            ONLINE_COUNT.fetch_add(1, Ordering::AcqRel);
            serial_println!(
                "[SMP] CPU {} online (APIC id {})",
                index,
                CPUS[index].apic_id.load(Ordering::Relaxed)
            );
        }
    }
    serial_println!("[SMP] {} of {} CPU(s) online", online_count(), count);
}

extern "C" fn test_ipi_handler(_frame: &mut InterruptFrame) {
    let index = current_cpu_index();
    CPUS[index].ipi_count.fetch_add(1, Ordering::AcqRel);
}

/// Send one test IPI to each online AP and wait for every handler to run.
/// Returns (responded, expected).
pub fn ipi_self_test() -> (usize, usize) {
    let count = cpu_count();
    let mut expected = 0;
    let mut before = [0u32; MAX_CPUS];
    interrupts::without_interrupts(|| {
        for index in 1..count {
            if !CPUS[index].online.load(Ordering::Acquire) {
                continue;
            }
            expected += 1;
            before[index] = CPUS[index].ipi_count.load(Ordering::Acquire);
            let apic_id = CPUS[index].apic_id.load(Ordering::Relaxed);
            lapic::send_ipi(apic_id, lapic::icr_fixed(IPI_VECTOR_TEST));
        }
    });

    if expected == 0 {
        return (0, 0);
    }

    let responded = |before: &[u32; MAX_CPUS]| {
        (1..count)
            .filter(|&i| {
                CPUS[i].online.load(Ordering::Acquire)
                    && CPUS[i].ipi_count.load(Ordering::Acquire) > before[i]
            })
            .count()
    };
    let mut waited = 0;
    while responded(&before) < expected && waited < IPI_TEST_TIMEOUT_US {
        pit::busy_wait_us(100);
        waited += 100;
    }
    let got = responded(&before);
    serial_println!("[SMP] IPI self-test: {}/{} AP(s) responded", got, expected);
    (got, expected)
}

/// First Rust code on an AP, called from `ap_high_entry` in `entry.asm`
/// with the kernel CR3 loaded and a private stack.
#[no_mangle]
pub extern "C" fn ap_rust_entry(cpu_index: u64) -> ! {
    AP_CHECKIN.store(true, Ordering::Release);

    gdt::load();
    idt::load();
    lapic::init_current_cpu();

    let index = cpu_index as usize;
    if index < MAX_CPUS {
        CPUS[index].online.store(true, Ordering::Release);
    }

    // Park. Interrupts are enabled so the AP can receive IPIs; no device IRQ
    // is routed here and the scheduler ignores APs (see sched::isr_hook).
    loop {
        unsafe {
            core::arch::asm!("sti; hlt", options(nomem, nostack));
        }
    }
}
