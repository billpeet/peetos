//! Legacy ISA IRQ routing that works the same whether the 8259 PIC or the
//! LAPIC/IOAPIC pair is delivering interrupts.
//!
//! ISA IRQ `n` is always delivered on vector `IRQ_VECTOR_BASE + n`, so driver
//! handler registration does not depend on the active controller. Interrupt
//! acknowledgement (EOI) is done centrally by the IDT dispatcher; drivers
//! must not send their own EOI.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::acpi::IsaRoute;
use crate::{ioapic, lapic, pic};

pub const IRQ_VECTOR_BASE: u8 = 32;
pub const ISA_IRQ_COUNT: usize = 16;

static APIC_MODE: AtomicBool = AtomicBool::new(false);
static DEST_APIC_ID: AtomicU32 = AtomicU32::new(0);
/// Encoded `IsaRoute` per ISA IRQ; `NO_ROUTE` when the IRQ is not wired.
static ROUTES: [AtomicU64; ISA_IRQ_COUNT] = [const { AtomicU64::new(NO_ROUTE) }; ISA_IRQ_COUNT];

const NO_ROUTE: u64 = u64::MAX;
const ROUTE_ACTIVE_LOW: u64 = 1 << 32;
const ROUTE_LEVEL: u64 = 1 << 33;

#[inline]
pub const fn irq_to_vector(irq: u8) -> u8 {
    IRQ_VECTOR_BASE + irq
}

/// ISA IRQ number for an IRQ vector, if it is one.
#[inline]
pub const fn vector_to_irq(vector: u64) -> Option<u8> {
    if vector >= IRQ_VECTOR_BASE as u64 && vector < (IRQ_VECTOR_BASE as u64 + ISA_IRQ_COUNT as u64)
    {
        Some((vector - IRQ_VECTOR_BASE as u64) as u8)
    } else {
        None
    }
}

const fn encode_route(route: IsaRoute) -> u64 {
    let mut raw = route.gsi as u64;
    if route.active_low {
        raw |= ROUTE_ACTIVE_LOW;
    }
    if route.level_triggered {
        raw |= ROUTE_LEVEL;
    }
    raw
}

const fn decode_route(raw: u64) -> Option<IsaRoute> {
    if raw == NO_ROUTE {
        return None;
    }
    Some(IsaRoute {
        gsi: raw as u32,
        active_low: raw & ROUTE_ACTIVE_LOW != 0,
        level_triggered: raw & ROUTE_LEVEL != 0,
    })
}

/// Choose which ISA IRQs get an IOAPIC route. IRQ2 (PIC cascade) is never
/// routed, and when two IRQs resolve to the same GSI (for example IRQ0 is
/// overridden onto GSI2) the lower IRQ wins.
pub fn plan_routes(routes: &[IsaRoute; ISA_IRQ_COUNT]) -> [Option<IsaRoute>; ISA_IRQ_COUNT] {
    let mut plan = [None; ISA_IRQ_COUNT];
    for irq in 0..ISA_IRQ_COUNT {
        if irq == pic::IRQ_CASCADE as usize {
            continue;
        }
        let route = routes[irq];
        let taken = plan[..irq]
            .iter()
            .any(|r: &Option<IsaRoute>| r.map(|r| r.gsi) == Some(route.gsi));
        if !taken {
            plan[irq] = Some(route);
        }
    }
    plan
}

pub fn is_apic_mode() -> bool {
    APIC_MODE.load(Ordering::Acquire)
}

/// Move ISA IRQ delivery from the 8259 PIC to the IOAPIC(s).
///
/// IRQs that are currently unmasked on the PIC stay unmasked on the IOAPIC,
/// so drivers initialised earlier keep working. All entries target
/// `dest_apic_id` (the BSP). The LAPIC and IOAPICs must already be set up and
/// interrupts must be disabled. Returns the number of IRQs routed.
pub fn switch_to_apic(routes: &[IsaRoute; ISA_IRQ_COUNT], dest_apic_id: u32) -> usize {
    let pic_mask = pic::get_mask();
    let plan = plan_routes(routes);
    let mut routed = 0;
    for (irq, route) in plan.iter().enumerate() {
        let Some(route) = route else {
            continue;
        };
        let masked = pic_mask & (1 << irq) != 0;
        let entry = ioapic::redirection_entry(
            irq_to_vector(irq as u8),
            dest_apic_id,
            route.active_low,
            route.level_triggered,
            masked,
        );
        if ioapic::set_route(route.gsi, entry) {
            ROUTES[irq].store(encode_route(*route), Ordering::Relaxed);
            routed += 1;
        }
    }
    DEST_APIC_ID.store(dest_apic_id, Ordering::Relaxed);
    pic::disable();
    APIC_MODE.store(true, Ordering::Release);
    routed
}

/// Route for an ISA IRQ in APIC mode.
pub fn route(irq: u8) -> Option<IsaRoute> {
    ROUTES.get(irq as usize).and_then(|r| decode_route(r.load(Ordering::Relaxed)))
}

pub fn unmask(irq: u8) {
    if is_apic_mode() {
        if let Some(route) = route(irq) {
            ioapic::set_masked(route.gsi, false);
        }
    } else {
        pic::unmask_irq(irq);
    }
}

pub fn mask(irq: u8) {
    if is_apic_mode() {
        if let Some(route) = route(irq) {
            ioapic::set_masked(route.gsi, true);
        }
    } else {
        pic::mask_irq(irq);
    }
}

/// Spurious IRQ check for the legacy PIC (IRQ7/IRQ15). The IOAPIC path uses
/// a dedicated LAPIC spurious vector instead, so this is always false there.
pub fn is_spurious(irq: u8) -> bool {
    if is_apic_mode() {
        false
    } else {
        pic::is_spurious(irq)
    }
}

/// Acknowledge an ISA IRQ.
pub fn eoi(irq: u8) {
    if is_apic_mode() {
        lapic::eoi();
    } else {
        pic::send_eoi(irq);
    }
}

pub fn dest_apic_id() -> u32 {
    DEST_APIC_ID.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acpi::{isa_route, InterruptOverride};

    fn qemu_routes() -> [IsaRoute; ISA_IRQ_COUNT] {
        let overrides = [
            InterruptOverride {
                source_irq: 0,
                gsi: 2,
                flags: 0,
            },
            InterruptOverride {
                source_irq: 11,
                gsi: 11,
                flags: 0xD,
            },
        ];
        core::array::from_fn(|irq| isa_route(&overrides, irq as u8))
    }

    #[test]
    fn vector_mapping_round_trips() {
        assert_eq!(irq_to_vector(0), 32);
        assert_eq!(irq_to_vector(15), 47);
        assert_eq!(vector_to_irq(32), Some(0));
        assert_eq!(vector_to_irq(47), Some(15));
        assert_eq!(vector_to_irq(31), None);
        assert_eq!(vector_to_irq(48), None);
        assert_eq!(vector_to_irq(0xFF), None);
    }

    #[test]
    fn route_encoding_round_trips() {
        let route = IsaRoute {
            gsi: 11,
            active_low: true,
            level_triggered: true,
        };
        assert_eq!(decode_route(encode_route(route)), Some(route));
        assert_eq!(decode_route(NO_ROUTE), None);
    }

    #[test]
    fn plan_skips_cascade_and_gsi_collisions() {
        let plan = plan_routes(&qemu_routes());
        // IRQ0 owns GSI2; IRQ2 is the cascade and never routed.
        assert_eq!(plan[0].map(|r| r.gsi), Some(2));
        assert_eq!(plan[2], None);
        assert_eq!(plan[1].map(|r| r.gsi), Some(1));
        assert_eq!(plan[11].map(|r| r.level_triggered), Some(true));
        assert_eq!(plan.iter().filter(|r| r.is_some()).count(), 15);
    }

    #[test]
    fn plan_lower_irq_wins_shared_gsi() {
        let mut routes = qemu_routes();
        routes[9].gsi = 5;
        let plan = plan_routes(&routes);
        assert_eq!(plan[5].map(|r| r.gsi), Some(5));
        assert_eq!(plan[9], None);
    }
}
