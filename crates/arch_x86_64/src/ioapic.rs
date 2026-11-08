//! I/O APIC driver: routes global system interrupts (GSIs) to LAPIC vectors.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::acpi::MAX_IOAPICS;

#[cfg(target_os = "none")]
const IOREGSEL: usize = 0x00;
#[cfg(target_os = "none")]
const IOWIN: usize = 0x10;

const REG_VERSION: u32 = 0x01;
const REG_REDTBL_BASE: u32 = 0x10;

pub const REDIR_POLARITY_LOW: u64 = 1 << 13;
pub const REDIR_TRIGGER_LEVEL: u64 = 1 << 15;
pub const REDIR_MASKED: u64 = 1 << 16;

/// Build a redirection table entry: fixed delivery, physical destination.
pub const fn redirection_entry(
    vector: u8,
    dest_apic_id: u32,
    active_low: bool,
    level_triggered: bool,
    masked: bool,
) -> u64 {
    let mut entry = vector as u64;
    if active_low {
        entry |= REDIR_POLARITY_LOW;
    }
    if level_triggered {
        entry |= REDIR_TRIGGER_LEVEL;
    }
    if masked {
        entry |= REDIR_MASKED;
    }
    entry | (((dest_apic_id & 0xFF) as u64) << 56)
}

/// Number of redirection entries encoded in the IOAPIC version register.
pub const fn max_redirections(version_reg: u32) -> u32 {
    ((version_reg >> 16) & 0xFF) + 1
}

struct Slot {
    base: AtomicU64,
    gsi_base: AtomicU32,
    count: AtomicU32,
}

impl Slot {
    const fn new() -> Self {
        Self {
            base: AtomicU64::new(0),
            gsi_base: AtomicU32::new(0),
            count: AtomicU32::new(0),
        }
    }
}

static SLOTS: [Slot; MAX_IOAPICS] = [Slot::new(), Slot::new(), Slot::new(), Slot::new()];
static SLOT_COUNT: AtomicUsize = AtomicUsize::new(0);

#[cfg(target_os = "none")]
fn read_reg(base: u64, reg: u32) -> u32 {
    unsafe {
        core::ptr::write_volatile((base as usize + IOREGSEL) as *mut u32, reg);
        core::ptr::read_volatile((base as usize + IOWIN) as *const u32)
    }
}

#[cfg(target_os = "none")]
fn write_reg(base: u64, reg: u32, value: u32) {
    unsafe {
        core::ptr::write_volatile((base as usize + IOREGSEL) as *mut u32, reg);
        core::ptr::write_volatile((base as usize + IOWIN) as *mut u32, value);
    }
}

#[cfg(not(target_os = "none"))]
fn read_reg(_base: u64, reg: u32) -> u32 {
    if reg == REG_VERSION {
        23 << 16
    } else {
        0
    }
}

#[cfg(not(target_os = "none"))]
fn write_reg(_base: u64, _reg: u32, _value: u32) {}

/// Register a mapped IOAPIC. Masks every redirection entry it owns.
/// Returns the number of GSIs it handles, or `None` if the table is full.
pub fn register(virt_base: u64, gsi_base: u32) -> Option<u32> {
    let index = SLOT_COUNT.load(Ordering::Acquire);
    if index >= MAX_IOAPICS {
        return None;
    }
    let count = max_redirections(read_reg(virt_base, REG_VERSION));
    let slot = &SLOTS[index];
    slot.base.store(virt_base, Ordering::Relaxed);
    slot.gsi_base.store(gsi_base, Ordering::Relaxed);
    slot.count.store(count, Ordering::Relaxed);
    SLOT_COUNT.store(index + 1, Ordering::Release);
    for pin in 0..count {
        write_entry(virt_base, pin, REDIR_MASKED);
    }
    Some(count)
}

fn lookup(gsi: u32) -> Option<(u64, u32)> {
    let count = SLOT_COUNT.load(Ordering::Acquire);
    SLOTS[..count].iter().find_map(|slot| {
        let base = slot.gsi_base.load(Ordering::Relaxed);
        let n = slot.count.load(Ordering::Relaxed);
        if gsi >= base && gsi < base + n {
            Some((slot.base.load(Ordering::Relaxed), gsi - base))
        } else {
            None
        }
    })
}

fn write_entry(base: u64, pin: u32, entry: u64) {
    let reg = REG_REDTBL_BASE + pin * 2;
    // Mask first, then program the high half, then the low half (with the
    // final mask state) so the entry is never live with a stale destination.
    write_reg(base, reg, REDIR_MASKED as u32);
    write_reg(base, reg + 1, (entry >> 32) as u32);
    write_reg(base, reg, entry as u32);
}

fn read_entry(base: u64, pin: u32) -> u64 {
    let reg = REG_REDTBL_BASE + pin * 2;
    let low = read_reg(base, reg) as u64;
    let high = read_reg(base, reg + 1) as u64;
    (high << 32) | low
}

/// Program the redirection entry for `gsi`. Returns false if no IOAPIC owns it.
pub fn set_route(gsi: u32, entry: u64) -> bool {
    match lookup(gsi) {
        Some((base, pin)) => {
            write_entry(base, pin, entry);
            true
        }
        None => false,
    }
}

pub fn set_masked(gsi: u32, masked: bool) -> bool {
    match lookup(gsi) {
        Some((base, pin)) => {
            let reg = REG_REDTBL_BASE + pin * 2;
            let mut low = read_entry(base, pin) as u32;
            if masked {
                low |= REDIR_MASKED as u32;
            } else {
                low &= !(REDIR_MASKED as u32);
            }
            write_reg(base, reg, low);
            true
        }
        None => false,
    }
}

pub fn count() -> usize {
    SLOT_COUNT.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirection_entry_encoding() {
        assert_eq!(redirection_entry(0x20, 0, false, false, false), 0x20);
        assert_eq!(
            redirection_entry(0x2B, 1, false, true, true),
            0x0100_0000_0001_802B
        );
        assert_eq!(
            redirection_entry(0x29, 0xFF, true, true, false),
            0xFF00_0000_0000_A029
        );
    }

    #[test]
    fn version_register_decodes_entry_count() {
        // QEMU's IOAPIC reports 24 redirection entries (max index 23).
        assert_eq!(max_redirections(0x0017_0020), 24);
        assert_eq!(max_redirections(0), 1);
    }
}
