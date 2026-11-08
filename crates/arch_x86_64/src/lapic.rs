//! Local APIC (xAPIC, MMIO mode) driver.
//!
//! The kernel maps the LAPIC register page and hands the virtual address to
//! [`set_base`]. Every CPU sees its own LAPIC at the same physical address, so
//! one mapping serves all cores.

use core::sync::atomic::{AtomicU64, Ordering};

pub const IA32_APIC_BASE_MSR: u32 = 0x1B;
pub const APIC_BASE_BSP: u64 = 1 << 8;
pub const APIC_BASE_ENABLE: u64 = 1 << 11;
pub const APIC_BASE_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

pub const REG_ID: usize = 0x020;
pub const REG_VERSION: usize = 0x030;
pub const REG_TPR: usize = 0x080;
pub const REG_EOI: usize = 0x0B0;
pub const REG_SVR: usize = 0x0F0;
pub const REG_ESR: usize = 0x280;
pub const REG_ICR_LOW: usize = 0x300;
pub const REG_ICR_HIGH: usize = 0x310;
pub const REG_LVT_TIMER: usize = 0x320;
pub const REG_LVT_LINT0: usize = 0x350;
pub const REG_LVT_LINT1: usize = 0x360;
pub const REG_LVT_ERROR: usize = 0x370;

pub const SVR_APIC_ENABLE: u32 = 1 << 8;
pub const LVT_MASKED: u32 = 1 << 16;
pub const LVT_DELIVERY_NMI: u32 = 0b100 << 8;

/// Vector used for LAPIC spurious interrupts. These must not be EOI'd.
pub const SPURIOUS_VECTOR: u8 = 0xFF;

pub const ICR_DELIVERY_FIXED: u32 = 0b000 << 8;
pub const ICR_DELIVERY_INIT: u32 = 0b101 << 8;
pub const ICR_DELIVERY_STARTUP: u32 = 0b110 << 8;
pub const ICR_DELIVERY_PENDING: u32 = 1 << 12;
pub const ICR_LEVEL_ASSERT: u32 = 1 << 14;
pub const ICR_TRIGGER_LEVEL: u32 = 1 << 15;

static BASE: AtomicU64 = AtomicU64::new(0);

/// Low dword of the ICR for a fixed-delivery IPI to a single destination.
pub const fn icr_fixed(vector: u8) -> u32 {
    ICR_DELIVERY_FIXED | ICR_LEVEL_ASSERT | vector as u32
}

/// Low dword of the ICR for an INIT IPI (level-assert form).
pub const fn icr_init() -> u32 {
    ICR_DELIVERY_INIT | ICR_LEVEL_ASSERT | ICR_TRIGGER_LEVEL
}

/// Low dword of the ICR for a STARTUP IPI. `trampoline_phys` must be a
/// page-aligned address below 1 MiB.
pub const fn icr_startup(trampoline_phys: u64) -> u32 {
    ICR_DELIVERY_STARTUP | ICR_LEVEL_ASSERT | ((trampoline_phys >> 12) as u32 & 0xFF)
}

/// High dword of the ICR selecting a physical destination APIC id.
pub const fn icr_destination(apic_id: u32) -> u32 {
    (apic_id & 0xFF) << 24
}

pub fn set_base(virt: u64) {
    BASE.store(virt, Ordering::Release);
}

pub fn is_initialized() -> bool {
    BASE.load(Ordering::Acquire) != 0
}

#[cfg(target_os = "none")]
#[inline]
fn reg(offset: usize) -> *mut u32 {
    (BASE.load(Ordering::Relaxed) as usize + offset) as *mut u32
}

#[cfg(target_os = "none")]
#[inline]
pub fn read(offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile(reg(offset)) }
}

#[cfg(target_os = "none")]
#[inline]
pub fn write(offset: usize, value: u32) {
    unsafe { core::ptr::write_volatile(reg(offset), value) }
}

#[cfg(not(target_os = "none"))]
pub fn read(_offset: usize) -> u32 {
    0
}

#[cfg(not(target_os = "none"))]
pub fn write(_offset: usize, _value: u32) {}

/// Physical LAPIC base reported by IA32_APIC_BASE.
#[cfg(target_os = "none")]
pub fn msr_base_phys() -> u64 {
    unsafe { crate::msr::read(IA32_APIC_BASE_MSR) & APIC_BASE_ADDR_MASK }
}

#[cfg(not(target_os = "none"))]
pub fn msr_base_phys() -> u64 {
    0xFEE0_0000
}

/// Whether the executing CPU is the bootstrap processor.
#[cfg(target_os = "none")]
pub fn is_bsp() -> bool {
    unsafe { crate::msr::read(IA32_APIC_BASE_MSR) & APIC_BASE_BSP != 0 }
}

#[cfg(not(target_os = "none"))]
pub fn is_bsp() -> bool {
    true
}

/// APIC id of the executing CPU. Returns 0 before [`set_base`].
pub fn id() -> u32 {
    if !is_initialized() {
        return 0;
    }
    read(REG_ID) >> 24
}

/// Software-enable the executing CPU's LAPIC and put it in a known state:
/// spurious vector installed, TPR 0, timer/LINT0/error masked, LINT1 = NMI.
#[cfg(target_os = "none")]
pub fn init_current_cpu() {
    unsafe {
        let base = crate::msr::read(IA32_APIC_BASE_MSR);
        crate::msr::write(IA32_APIC_BASE_MSR, base | APIC_BASE_ENABLE);
    }
    write(REG_TPR, 0);
    write(REG_LVT_TIMER, LVT_MASKED);
    write(REG_LVT_LINT0, LVT_MASKED);
    write(REG_LVT_LINT1, LVT_DELIVERY_NMI);
    write(REG_LVT_ERROR, LVT_MASKED);
    // Clear any latched errors (write then read per the SDM).
    write(REG_ESR, 0);
    let _ = read(REG_ESR);
    write(REG_SVR, SVR_APIC_ENABLE | SPURIOUS_VECTOR as u32);
    eoi();
}

#[cfg(not(target_os = "none"))]
pub fn init_current_cpu() {}

#[inline]
pub fn eoi() {
    write(REG_EOI, 0);
}

/// Spin until the previous IPI has been accepted. Returns false on timeout.
pub fn wait_for_delivery() -> bool {
    for _ in 0..1_000_000 {
        if read(REG_ICR_LOW) & ICR_DELIVERY_PENDING == 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// Send an IPI. Writing the low dword triggers delivery, so the destination
/// must be written first. Callers must keep interrupts disabled so the
/// two-register write sequence is not interleaved on this CPU.
pub fn send_ipi(dest_apic_id: u32, icr_low: u32) -> bool {
    write(REG_ICR_HIGH, icr_destination(dest_apic_id));
    write(REG_ICR_LOW, icr_low);
    wait_for_delivery()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icr_encodings_match_sdm() {
        // INIT, level assert, level trigger.
        assert_eq!(icr_init(), 0x0000_C500);
        // STARTUP for trampoline at 0x8000 -> vector 0x08.
        assert_eq!(icr_startup(0x8000), 0x0000_4608);
        assert_eq!(icr_fixed(0xF0), 0x0000_40F0);
        assert_eq!(icr_destination(3), 0x0300_0000);
        assert_eq!(icr_destination(0x1FF), 0xFF00_0000);
    }
}
