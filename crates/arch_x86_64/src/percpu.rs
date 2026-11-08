//! Per-CPU area reached through the GS segment base.
//!
//! While a CPU executes kernel code, `IA32_GS_BASE` points at its `PerCpu`
//! slot and `IA32_KERNEL_GS_BASE` holds the user GS base (always 0 today).
//! Every user→kernel transition (SYSCALL, interrupts/exceptions from ring 3)
//! executes `swapgs` on entry and again before returning to ring 3.
//!
//! Each CPU also owns its GDT and TSS, so `RSP0` (the kernel stack used for
//! ring-3 interrupts) is per CPU.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::acpi::MAX_CPUS;
use crate::gdt;
use crate::tss::Tss;

/// Byte offsets used by assembly (`gs:[OFFSET]`). Keep in sync with `PerCpu`.
pub const OFFSET_SELF: usize = 0;
pub const OFFSET_USER_RSP: usize = 8;
pub const OFFSET_KERNEL_RSP0: usize = 16;
pub const OFFSET_INDEX: usize = 24;

#[repr(C, align(64))]
pub struct PerCpu {
    self_ptr: u64,
    /// SYSCALL entry scratch: the user RSP while switching stacks.
    user_rsp: u64,
    /// Top of the current task's kernel stack (mirrors `tss.rsp0`).
    kernel_rsp0: u64,
    index: u64,
    gdt: [u64; gdt::GDT_ENTRIES],
    tss: Tss,
}

impl PerCpu {
    const fn new() -> Self {
        Self {
            self_ptr: 0,
            user_rsp: 0,
            kernel_rsp0: 0,
            index: 0,
            gdt: [0; gdt::GDT_ENTRIES],
            tss: Tss::zero(),
        }
    }
}

static mut PERCPU: [PerCpu; MAX_CPUS] = [const { PerCpu::new() }; MAX_CPUS];
/// Set once the BSP has loaded its per-CPU area; before that every query
/// reports CPU 0 without touching GS.
static READY: AtomicBool = AtomicBool::new(false);

/// Encode a 64-bit available-TSS system descriptor (two GDT slots).
pub const fn tss_descriptor(base: u64, limit: u32) -> (u64, u64) {
    let low = (limit as u64 & 0xFFFF)
        | ((base & 0xFF_FFFF) << 16)
        | ((gdt::GDT_ACCESS_TSS as u64) << 40)
        | (((limit as u64 >> 16) & 0xF) << 48)
        | (((base >> 24) & 0xFF) << 56);
    let high = base >> 32;
    (low, high)
}

/// Build and load the per-CPU GDT/TSS for `index`, then point GS at the
/// per-CPU area. Must run on the CPU it describes, after [`gdt::init`] built
/// the template GDT. Interrupts must be disabled.
#[cfg(target_os = "none")]
pub fn init_cpu(index: usize) {
    assert!(index < MAX_CPUS);
    unsafe {
        let slot = &mut *core::ptr::addr_of_mut!(PERCPU[index]);
        slot.self_ptr = slot as *mut PerCpu as u64;
        slot.index = index as u64;
        slot.tss = Tss::zero();
        slot.gdt = gdt::template();
        let tss_base = core::ptr::addr_of!(slot.tss) as u64;
        let (low, high) = tss_descriptor(tss_base, (core::mem::size_of::<Tss>() - 1) as u32);
        slot.gdt[(gdt::GDT_TSS / 8) as usize] = low;
        slot.gdt[(gdt::GDT_TSS / 8) as usize + 1] = high;

        gdt::load_table(slot.gdt.as_ptr() as u64, gdt::GDT_ENTRIES);
        gdt::load_tss();

        crate::msr::write(crate::msr::IA32_GS_BASE, slot.self_ptr);
        crate::msr::write(crate::msr::IA32_KERNEL_GS_BASE, 0);
    }
    if index == 0 {
        READY.store(true, Ordering::Release);
    }
}

#[cfg(not(target_os = "none"))]
pub fn init_cpu(_index: usize) {}

pub fn is_ready() -> bool {
    READY.load(Ordering::Acquire)
}

/// Logical index of the executing CPU (0 = BSP).
#[cfg(target_os = "none")]
#[inline]
pub fn current_index() -> usize {
    if !is_ready() {
        return 0;
    }
    let index: u64;
    unsafe {
        core::arch::asm!(
            "mov {}, qword ptr gs:[{off}]",
            out(reg) index,
            off = const OFFSET_INDEX,
            options(nostack, readonly, preserves_flags)
        );
    }
    index as usize
}

#[cfg(not(target_os = "none"))]
pub fn current_index() -> usize {
    0
}

/// Set the kernel stack used when the executing CPU takes an interrupt or
/// SYSCALL from ring 3.
pub fn set_rsp0(rsp0: u64) {
    unsafe {
        let slot = &mut *core::ptr::addr_of_mut!(PERCPU[current_index()]);
        slot.tss.set_rsp0(rsp0);
        slot.kernel_rsp0 = rsp0;
    }
}

/// User RSP saved by the most recent SYSCALL entry on this CPU.
pub fn user_rsp() -> u64 {
    unsafe { (*core::ptr::addr_of!(PERCPU[current_index()])).user_rsp }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::offset_of;

    #[test]
    fn asm_offsets_match_layout() {
        assert_eq!(offset_of!(PerCpu, self_ptr), OFFSET_SELF);
        assert_eq!(offset_of!(PerCpu, user_rsp), OFFSET_USER_RSP);
        assert_eq!(offset_of!(PerCpu, kernel_rsp0), OFFSET_KERNEL_RSP0);
        assert_eq!(offset_of!(PerCpu, index), OFFSET_INDEX);
    }

    #[test]
    fn tss_descriptor_encoding() {
        // Base 0xFFFF_FFFF_8012_3456, limit 0x67 (104-byte TSS).
        let (low, high) = tss_descriptor(0xFFFF_FFFF_8012_3456, 0x67);
        assert_eq!(low & 0xFFFF, 0x67);
        assert_eq!((low >> 16) & 0xFF_FFFF, 0x12_3456);
        assert_eq!((low >> 40) & 0xFF, 0x89); // present, DPL0, type 9
        assert_eq!((low >> 48) & 0xF, 0);
        assert_eq!((low >> 56) & 0xFF, 0x80);
        assert_eq!(high, 0xFFFF_FFFF);
    }
}
