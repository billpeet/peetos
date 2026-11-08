//! `test-exit` self-tests for SMP scheduling.
//!
//! 1. [`run`]: kernel worker threads repeatedly record the CPU they run on
//!    and sleep for a tick. Sleepers are woken by the BSP's PIT tick and
//!    handed to idle CPUs through reschedule IPIs, so with several CPUs the
//!    workers spread out.
//! 2. [`run_bkl_free`]: a "hog" thread holds the big kernel lock with
//!    interrupts disabled. Meanwhile, scheduler-only worker threads on other
//!    CPUs keep yielding. Every switch they make while the hog holds the lock
//!    proves the scheduler no longer depends on the big kernel lock.

use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use arch_x86_64::{interrupts, percpu, pit};

use crate::{bkl, sched, serial_println, smp, task};

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

const YIELDERS: usize = 6;
const HOG_SPINS: u64 = 2_000_000;

static STOP_YIELDERS: AtomicBool = AtomicBool::new(false);
static YIELDERS_DONE: AtomicUsize = AtomicUsize::new(0);
static YIELDS: AtomicU64 = AtomicU64::new(0);
static HOG_HELD_YIELDS: AtomicU64 = AtomicU64::new(0);
static HOG_DONE: AtomicBool = AtomicBool::new(false);
static HOG_CPU: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Scheduler-only thread: never takes the big kernel lock.
extern "C" fn yielder(_arg: *mut u8) {
    while !STOP_YIELDERS.load(Ordering::Acquire) {
        YIELDS.fetch_add(1, Ordering::AcqRel);
        sched::yield_now();
    }
    YIELDERS_DONE.fetch_add(1, Ordering::AcqRel);
}

/// Holds the big kernel lock (it is a normal kernel thread) with interrupts
/// disabled, so it cannot be preempted, and counts yields elsewhere meanwhile.
extern "C" fn hog(_arg: *mut u8) {
    interrupts::disable();
    let held = bkl::held_by_me();
    HOG_CPU.store(percpu::current_index(), Ordering::Release);
    let before = YIELDS.load(Ordering::Acquire);
    for _ in 0..HOG_SPINS {
        core::hint::spin_loop();
    }
    let after = YIELDS.load(Ordering::Acquire);
    HOG_HELD_YIELDS.store(if held { after - before } else { 0 }, Ordering::Release);
    interrupts::enable();
    HOG_DONE.store(true, Ordering::Release);
}

/// Returns the number of yields other CPUs completed while the hog held the
/// big kernel lock (0 on a uniprocessor, where the test is skipped).
pub fn run_bkl_free() -> u64 {
    if smp::online_count() < 2 {
        serial_println!("[TEST] BKL-free scheduling: skipped (1 CPU)");
        return 0;
    }
    for _ in 0..YIELDERS {
        if task::kthread_create_with_lock("yielder", yielder, ptr::null_mut(), false)
            == task::INVALID_PID
        {
            serial_println!("[TEST] BKL-free scheduling: failed to spawn yielder");
            return 0;
        }
    }
    // Let the yielders get going before the hog starts.
    let start = pit::ticks();
    while YIELDS.load(Ordering::Acquire) < 100 && pit::ticks() < start + TIMEOUT_TICKS {
        bkl::wait_for_interrupt();
    }
    if task::kthread_create("bkl-hog", hog, ptr::null_mut()) == task::INVALID_PID {
        serial_println!("[TEST] BKL-free scheduling: failed to spawn hog");
        return 0;
    }
    let start = pit::ticks();
    while !HOG_DONE.load(Ordering::Acquire) && pit::ticks() < start + TIMEOUT_TICKS {
        bkl::wait_for_interrupt();
    }
    STOP_YIELDERS.store(true, Ordering::Release);
    let start = pit::ticks();
    while YIELDERS_DONE.load(Ordering::Acquire) < YIELDERS && pit::ticks() < start + TIMEOUT_TICKS {
        bkl::wait_for_interrupt();
    }
    let held_yields = HOG_HELD_YIELDS.load(Ordering::Acquire);
    serial_println!(
        "[TEST] BKL-free scheduling: {} yield(s) on other CPUs while CPU {} held the BKL (hog done={}, yielders stopped={}/{})",
        held_yields,
        HOG_CPU.load(Ordering::Acquire),
        HOG_DONE.load(Ordering::Acquire),
        YIELDERS_DONE.load(Ordering::Acquire),
        YIELDERS
    );
    held_yields
}
