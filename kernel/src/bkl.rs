//! Big kernel lock (BKL).
//!
//! SMP phase 2 serialises all kernel execution behind one lock owned by a
//! CPU, not a task. The kernel was written for one core, where "interrupts
//! off" meant exclusive access. The BKL keeps that assumption true while
//! every CPU runs tasks.
//!
//! Rules:
//! - **Acquire** on every kernel entry where this CPU does not already own
//!   it: the SYSCALL path ([`syscall_enter`]), any non-lock-free interrupt or
//!   exception (the IDT entry hook), and explicit waits ([`relax`],
//!   [`wait_for_interrupt`]). Re-acquiring on the owning CPU is a no-op, so
//!   nested interrupts in kernel mode just proceed.
//! - **Keep** the lock across context switches between kernel contexts.
//!   The scheduler requires it.
//! - **Release** only after the CPU has switched to the frame it is about to
//!   restore ([`frame_exit_hook`], called from assembly on the new stack),
//!   and only when that frame returns to ring 3 or into an idle task. Doing
//!   it later than the run-queue update means no other CPU can resume the
//!   previous task while its kernel stack is still in use here.
//! - The SYSCALL return path releases in [`syscall_exit`] (same task, same
//!   stack, so there is no hand-off hazard).
//!
//! Lock-free paths (they never take the BKL): the PIT tick counter, the
//! TLB-shootdown and test IPIs, and the idle loop's `hlt`.

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use arch_x86_64::idt::InterruptFrame;
use arch_x86_64::percpu;

const NO_OWNER: usize = usize::MAX;

// FIFO ticket lock. A plain test-and-set lock starves waiters when the owner
// releases and immediately re-acquires (busy-wait loops that `relax()`),
// which in practice happens constantly under QEMU's vCPU scheduling.
//
// The BSP owns the lock from boot (ticket 0): kernel initialisation runs
// before anything else can contend.
static NEXT_TICKET: AtomicU32 = AtomicU32::new(1);
static NOW_SERVING: AtomicU32 = AtomicU32::new(0);
/// CPU index that owns the lock, or `NO_OWNER`.
static OWNER: AtomicUsize = AtomicUsize::new(0);

#[inline]
pub fn held_by_me() -> bool {
    OWNER.load(Ordering::Relaxed) == percpu::current_index()
}

/// Take the lock if this CPU does not own it.
///
/// Waits with interrupts disabled: an interrupt taken while this CPU holds
/// an unserved ticket would re-enter `acquire`, draw a second ticket and
/// deadlock behind its own first one. While spinning it services
/// TLB-shootdown requests, so the owner can never deadlock waiting for this
/// CPU. The caller's interrupt state is restored once the lock is held.
pub fn acquire() {
    let me = percpu::current_index();
    if OWNER.load(Ordering::Relaxed) == me {
        return;
    }
    let was_enabled = arch_x86_64::interrupts::are_enabled();
    arch_x86_64::interrupts::disable();
    let ticket = NEXT_TICKET.fetch_add(1, Ordering::Relaxed);
    while NOW_SERVING.load(Ordering::Acquire) != ticket {
        crate::smp::service_tlb_shootdown();
        core::hint::spin_loop();
    }
    OWNER.store(me, Ordering::Relaxed);
    if was_enabled {
        arch_x86_64::interrupts::enable();
    }
}

/// Drop the lock if this CPU owns it.
pub fn release() {
    if !held_by_me() {
        return;
    }
    OWNER.store(NO_OWNER, Ordering::Relaxed);
    NOW_SERVING.fetch_add(1, Ordering::Release);
}

/// Let other CPUs run kernel code while this CPU waits for something they
/// must do. Because the lock is FIFO, every CPU already waiting gets a turn
/// before this one re-acquires. The caller must not hold references into
/// shared kernel state across the call.
pub fn relax() {
    if !held_by_me() {
        acquire();
        return;
    }
    release();
    core::hint::spin_loop();
    acquire();
}

/// `sti; hlt` without holding the lock, for kernel loops that idle (the
/// boot task's main loop). Returns with the lock held and interrupts enabled.
pub fn wait_for_interrupt() {
    arch_x86_64::interrupts::disable();
    release();
    // `sti` takes effect after the next instruction, so no interrupt can slip
    // in between enabling and halting.
    unsafe {
        core::arch::asm!("sti; hlt", options(nomem, nostack));
    }
    arch_x86_64::interrupts::disable();
    acquire();
    arch_x86_64::interrupts::enable();
}

/// IDT entry hook: every interrupt that is not lock-free.
pub fn interrupt_enter() {
    acquire();
}

/// SYSCALL entry: user mode never holds the lock.
#[inline]
pub fn syscall_enter() {
    acquire();
}

/// SYSCALL return to ring 3. Interrupts are disabled first: a task resumed
/// after blocking runs with IF=1, and an interrupt between the release and
/// SYSRET could otherwise preempt it in kernel mode and resume it later on a
/// CPU that holds the lock, carrying the lock into user mode. SYSRET restores
/// the user's RFLAGS (including IF) from R11.
#[inline]
pub fn syscall_exit() {
    arch_x86_64::interrupts::disable();
    release();
}

/// Called from `arch_x86_64_frame_exit` with the frame about to be
/// restored, already running on that frame's stack.
pub extern "C" fn frame_exit_hook(frame: *const InterruptFrame) {
    if !held_by_me() {
        return;
    }
    let cs = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*frame).cs)) };
    if cs & 3 == 3 || crate::sched::current_is_idle() {
        release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_release_round_trip() {
        // Host builds always report CPU 0, which owns the lock at boot.
        assert!(held_by_me());
        release();
        assert!(!held_by_me());
        acquire();
        assert!(held_by_me());
        // Re-acquire on the owner is a no-op.
        acquire();
        assert!(held_by_me());
    }
}
