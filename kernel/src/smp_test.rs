//! `test-exit` self-test: prove the scheduler runs tasks on every CPU.
//!
//! Spawns kernel worker threads that repeatedly record the CPU they run on
//! and sleep for a tick. Sleepers are woken by the BSP's PIT tick and
//! handed to idle CPUs through reschedule IPIs, so with several CPUs the
//! workers spread out.

use core::ptr;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use arch_x86_64::{percpu, pit};

use crate::{bkl, sched, serial_println, task};

const WORKERS: usize = 8;
const ROUNDS: usize = 10;
const TIMEOUT_TICKS: u64 = 500;

static SEEN_CPUS: AtomicU32 = AtomicU32::new(0);
static DONE: AtomicUsize = AtomicUsize::new(0);

extern "C" fn worker(_arg: *mut u8) {
    for _ in 0..ROUNDS {
        SEEN_CPUS.fetch_or(1 << percpu::current_index(), Ordering::AcqRel);
        sched::sleep(1);
    }
    DONE.fetch_add(1, Ordering::AcqRel);
}

/// Returns the number of distinct CPUs that ran a worker.
pub fn run() -> u32 {
    for _ in 0..WORKERS {
        if task::kthread_create("smp-test", worker, ptr::null_mut()) == task::INVALID_PID {
            serial_println!("[TEST] SMP scheduler: failed to spawn worker");
            return 0;
        }
    }
    let start = pit::ticks();
    while DONE.load(Ordering::Acquire) < WORKERS && pit::ticks() < start + TIMEOUT_TICKS {
        bkl::wait_for_interrupt();
    }
    let cpus = SEEN_CPUS.load(Ordering::Acquire).count_ones();
    serial_println!(
        "[TEST] SMP scheduler: {}/{} workers finished on {} CPU(s)",
        DONE.load(Ordering::Acquire),
        WORKERS,
        cpus
    );
    cpus
}
