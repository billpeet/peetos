//! Big kernel lock (BKL).
//!
//! The kernel was written for one core, where "interrupts off" meant
//! exclusive access. The BKL keeps that assumption true for the code still
//! under it while several CPUs run tasks. The scheduler, PMM and heap have
//! their own locks and do not need it.
//!
//! The lock is a FIFO ticket lock owned by a CPU. Logically it belongs to
//! the task running there: a task switched out while holding it records
//! that in `Task::bkl_resume`, gives it up, and the CPU that later resumes
//! the task takes it again before the task runs.
//!
//! Rules:
//! - **Acquire** on every kernel entry that needs it where this CPU does not
//!   already own it:
//!   - the SYSCALL path ([`syscall_enter`]),
//!   - interrupts and exceptions whose vector is not marked lock-free (the
//!     IDT entry hook, [`interrupt_enter`]),
//!   - explicit waits ([`relax`], [`wait_for_interrupt`]).
//!
//!   Re-acquiring on the owning CPU is a no-op.
//! - **Interrupt balance.** If an interrupt's entry hook took the lock and
//!   the same frame resumes, the IDT dispatcher gives it back
//!   ([`interrupt_release`]).
//! - **Context switches.** [`frame_exit_hook`] runs on the incoming frame's
//!   stack. It first lets the scheduler drop its own lock, then acquires or
//!   releases the BKL to match the incoming task's `bkl_resume`. Doing it
//!   after the stack switch means no other CPU can resume the previous task
//!   while its kernel stack is still in use here.
//! - **SYSCALL return.** The return path releases in [`syscall_exit`] (same
//!   task, same stack, so there is no hand-off hazard).
//!
//! Lock-free paths (they never take the BKL):
//! - the PIT tick counter and scheduler tick,
//! - the LAPIC timer tick,
//! - the reschedule, TLB-shootdown and test IPIs,
//! - context switches,
//! - the idle loop's `hlt`.

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
        // Lock-free callers (scheduler-only kernel threads) have nothing to
        // give up.
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
/// Returns true when this call acquired the lock (the interrupted context
/// did not hold it).
pub fn interrupt_enter() -> bool {
    if held_by_me() {
        return false;
    }
    acquire();
    true
}

/// Give back a lock taken by [`interrupt_enter`] when no switch happened.
pub fn interrupt_release() {
    release();
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
    // Only a (possible) context switch changes who should hold the lock; a
    // plain interrupt return was balanced by the IDT dispatcher already.
    if !crate::sched::finish_switch() {
        return;
    }
    let cs = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*frame).cs)) };
    let want = cs & 3 == 0 && crate::sched::current_needs_bkl();
    if want {
        acquire();
    } else {
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
