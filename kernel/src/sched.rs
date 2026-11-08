//! Round-robin scheduler with one shared run queue and per-CPU state.
//!
//! Every CPU has its own current task, idle task and preempt flag.
//! `SCHED_LOCK`, an interrupt-safe spinlock, protects:
//! - the run queue,
//! - the per-CPU scheduler state,
//! - the task table,
//! - the scheduler-owned task state transitions (Ready/Running/Blocked/Sleeping).
//!
//! The scheduler does **not** need the big kernel lock. Timer ticks,
//! reschedule IPIs and context switches all run without it.
//!
//! Context switch protocol:
//! 1. `schedule_from` takes `SCHED_LOCK` (raw form) and records in the
//!    outgoing task whether it held the big kernel lock (`Task::bkl_resume`).
//! 2. The CPU switches to the incoming frame's stack.
//! 3. The frame-exit hook (`bkl::frame_exit_hook`) calls [`finish_switch`]
//!    to drop `SCHED_LOCK` on the new stack, then takes or drops the big
//!    kernel lock to match the incoming task's `bkl_resume`.
//!
//! Holding `SCHED_LOCK` until the old stack is no longer in use means no
//! other CPU can resume the outgoing task early. Lock order:
//! BKL -> SCHED_LOCK. Never acquire the BKL while holding SCHED_LOCK.
//!
//! The PIT tick on the BSP drives sleeper wake-ups and the BSP's time slice.
//! Each AP's LAPIC timer drives its own time slice (`tick_local`).

use core::ptr;

use arch_x86_64::acpi::MAX_CPUS;
use arch_x86_64::addr::PhysAddr;
use arch_x86_64::idt::InterruptFrame;
use arch_x86_64::interrupts;
use arch_x86_64::percpu;
use arch_x86_64::pit;
use arch_x86_64::tss;

use crate::memory::vmm;
use crate::serial_println;
use crate::sync::{IrqSpinLock, IrqSpinLockGuard};
use crate::task::{self, Task, TaskState, TASK_FLAG_IDLE};

pub const DEFAULT_TIME_SLICE: u64 = 5;

struct CpuSched {
    current: *mut Task,
    idle: *mut Task,
    preempt_pending: bool,
    /// `SCHED_LOCK` is held across an in-progress switch on this CPU.
    switch_locked: bool,
    /// State to give the current task when `schedule_from` runs next
    /// (set by `block`/`sleep` with interrupts disabled).
    block_as: Option<TaskState>,
}

impl CpuSched {
    const fn new() -> Self {
        Self {
            current: ptr::null_mut(),
            idle: ptr::null_mut(),
            preempt_pending: false,
            switch_locked: false,
            block_as: None,
        }
    }
}

static SCHED_LOCK: IrqSpinLock<()> = IrqSpinLock::new(());

/// Take the scheduler lock (interrupts disabled while held).
pub fn lock() -> IrqSpinLockGuard<'static, ()> {
    SCHED_LOCK.lock()
}

static mut CPUS: [CpuSched; MAX_CPUS] = [const { CpuSched::new() }; MAX_CPUS];
static mut RUN_QUEUE_HEAD: *mut Task = ptr::null_mut();
static mut RUN_QUEUE_TAIL: *mut Task = ptr::null_mut();
static mut RUN_QUEUE_COUNT: usize = 0;
static mut SCHED_RUNNING: bool = false;
static mut STAT_CONTEXT_SWITCHES: u64 = 0;
static mut STAT_TIMER_TICKS: u64 = 0;

#[inline]
fn cpu_at(index: usize) -> &'static mut CpuSched {
    unsafe { &mut *ptr::addr_of_mut!(CPUS[index]) }
}

#[inline]
fn this_cpu() -> &'static mut CpuSched {
    cpu_at(percpu::current_index())
}

fn run_queue_push(task: *mut Task) {
    unsafe {
        (*task).next = ptr::null_mut();
        (*task).prev = RUN_QUEUE_TAIL;
        if !RUN_QUEUE_TAIL.is_null() {
            (*RUN_QUEUE_TAIL).next = task;
        } else {
            RUN_QUEUE_HEAD = task;
        }
        RUN_QUEUE_TAIL = task;
        RUN_QUEUE_COUNT = RUN_QUEUE_COUNT.saturating_add(1);
    }
}

fn run_queue_pop() -> *mut Task {
    unsafe {
        if RUN_QUEUE_HEAD.is_null() {
            return ptr::null_mut();
        }

        let task = RUN_QUEUE_HEAD;
        RUN_QUEUE_HEAD = (*task).next;
        if !RUN_QUEUE_HEAD.is_null() {
            (*RUN_QUEUE_HEAD).prev = ptr::null_mut();
        } else {
            RUN_QUEUE_TAIL = ptr::null_mut();
        }
        (*task).next = ptr::null_mut();
        (*task).prev = ptr::null_mut();
        RUN_QUEUE_COUNT = RUN_QUEUE_COUNT.saturating_sub(1);
        task
    }
}

fn run_queue_remove(task: *mut Task) {
    unsafe {
        if task.is_null() {
            return;
        }

        if !(*task).prev.is_null() {
            (*(*task).prev).next = (*task).next;
        } else {
            RUN_QUEUE_HEAD = (*task).next;
        }

        if !(*task).next.is_null() {
            (*(*task).next).prev = (*task).prev;
        } else {
            RUN_QUEUE_TAIL = (*task).prev;
        }

        (*task).next = ptr::null_mut();
        (*task).prev = ptr::null_mut();
        RUN_QUEUE_COUNT = RUN_QUEUE_COUNT.saturating_sub(1);
    }
}

/// A task told to exit (exit_group/execve from a sibling on another CPU)
/// that was preempted in user mode. It must not run again; the killing
/// thread finishes it once it sees it parked.
fn exiting_in_user_mode(task: *mut Task) -> bool {
    unsafe { ((*task).flags & task::TASK_FLAG_EXITING) != 0 && parked_in_user_mode(task) }
}

/// Caller holds `SCHED_LOCK`.
fn pick_next_task() -> *mut Task {
    let candidates = unsafe { RUN_QUEUE_COUNT };
    for _ in 0..candidates {
        let next = run_queue_pop();
        if next.is_null() {
            break;
        }
        if exiting_in_user_mode(next) {
            run_queue_push(next);
            continue;
        }
        return next;
    }
    this_cpu().idle
}

extern "C" fn idle_task_func(_arg: *mut u8) {
    // Runs without the big kernel lock (it is dropped when the CPU switches
    // to its idle task), so it must not touch shared kernel state.
    loop {
        interrupts::enable();
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
}

fn halt_forever(msg: &str) -> ! {
    serial_println!("[SCHED] ERROR: {}", msg);
    loop {
        unsafe {
            core::arch::asm!("cli; hlt", options(nomem, nostack, preserves_flags));
        }
    }
}

/// Create the idle task for CPU `index` (kept off the run queue).
pub fn create_idle(index: usize) -> bool {
    let mut name = [0u8; 8];
    name[..4].copy_from_slice(b"idle");
    let mut len = 4;
    if index > 0 {
        if index >= 10 {
            name[len] = b'0' + (index / 10) as u8;
            len += 1;
        }
        name[len] = b'0' + (index % 10) as u8;
        len += 1;
    }
    let name = core::str::from_utf8(&name[..len]).unwrap_or("idle");
    let pid = task::kthread_create(name, idle_task_func, ptr::null_mut());
    if pid == task::INVALID_PID {
        return false;
    }
    let idle = task::get_by_pid(pid);
    if idle.is_null() {
        return false;
    }
    let _guard = lock();
    unsafe {
        (*idle).flags |= TASK_FLAG_IDLE;
        (*idle).bkl_resume = false;
        run_queue_remove(idle);
        (*idle).state = TaskState::Ready;
    }
    cpu_at(index).idle = idle;
    true
}

pub fn init() {
    serial_println!("[SCHED] Initializing scheduler...");

    let boot = task::boot_task();
    if boot.is_null() {
        halt_forever("boot task not found");
    }

    let cpu = this_cpu();
    cpu.current = boot;
    unsafe {
        (*boot).state = TaskState::Running;
        (*boot).time_slice = DEFAULT_TIME_SLICE;
    }
    task::apply_user_tls_base(boot);

    if !create_idle(0) {
        halt_forever("failed to create idle task");
    }
    unsafe {
        SCHED_RUNNING = true;
    }

    serial_println!("[SCHED] Scheduler initialized");
}

/// First scheduler entry on an application processor: become CPU `index`'s
/// idle task and never return. The big kernel lock is not needed.
pub fn ap_enter(index: usize) -> ! {
    let cpu = cpu_at(index);
    let idle;
    {
        let _guard = lock();
        idle = cpu.idle;
        if idle.is_null() {
            halt_forever("AP has no idle task");
        }
        cpu.current = idle;
        cpu.preempt_pending = false;
        unsafe {
            (*idle).state = TaskState::Running;
            (*idle).time_slice = DEFAULT_TIME_SLICE;
            tss::set_rsp0((*idle).kernel_stack_top);
        }
    }
    interrupts::disable();
    vmm::switch(PhysAddr::new(0));
    task::apply_user_tls_base(idle);
    #[cfg(target_os = "none")]
    unsafe {
        sched_enter_frame((*idle).saved_rsp);
    }
    #[cfg(not(target_os = "none"))]
    halt_forever("ap_enter on host");
}

pub fn current_task() -> *mut Task {
    this_cpu().current
}

/// True when this CPU is running its idle task.
pub fn current_is_idle() -> bool {
    let cpu = this_cpu();
    !cpu.current.is_null() && cpu.current == cpu.idle
}

/// Wake one idle CPU (other than this one) so a newly runnable task does not
/// wait for that CPU's next timer tick. Caller holds `SCHED_LOCK`.
fn kick_idle_cpu() {
    let me = percpu::current_index();
    for index in 0..crate::smp::cpu_count() {
        if index == me || !crate::smp::is_online(index) {
            continue;
        }
        let cpu = cpu_at(index);
        if !cpu.idle.is_null() && cpu.current == cpu.idle {
            crate::smp::send_resched(index);
            return;
        }
    }
}

/// Caller holds `SCHED_LOCK`.
fn make_ready(task: *mut Task) {
    run_queue_push(task);
    kick_idle_cpu();
}

/// Outcome of [`detach_for_kill`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KillDetach {
    /// Preempted in user mode: removed from the run queue and made a zombie.
    /// It will never run again.
    Detached,
    /// Executing on another CPU; that CPU has been asked to reschedule.
    RunningElsewhere,
    /// Inside the kernel (blocked, sleeping, or runnable with a kernel
    /// frame). It must unwind and exit on its own.
    InKernel,
}

/// Atomically classify a sibling thread for exit_group/execve and, when it
/// is parked in user mode, take it off the run queue as a zombie.
pub fn detach_for_kill(t: *mut Task) -> KillDetach {
    let _guard = lock();
    unsafe {
        if (*t).state == TaskState::Running && t != this_cpu().current {
            let me = percpu::current_index();
            for index in 0..crate::smp::cpu_count() {
                let cpu = cpu_at(index);
                if index != me && cpu.current == t {
                    cpu.preempt_pending = true;
                    crate::smp::send_resched(index);
                }
            }
            return KillDetach::RunningElsewhere;
        }
        if (*t).state == TaskState::Ready && parked_in_user_mode(t) {
            run_queue_remove(t);
            (*t).state = TaskState::Zombie;
            return KillDetach::Detached;
        }
    }
    KillDetach::InKernel
}

fn parked_in_user_mode(task: *mut Task) -> bool {
    unsafe {
        if (*task).saved_rsp == 0 {
            return false;
        }
        let frame = (*task).saved_rsp as *const InterruptFrame;
        (ptr::read_unaligned(ptr::addr_of!((*frame).cs)) & 3) == 3
    }
}

pub fn add_task(task: *mut Task) {
    if task.is_null() {
        return;
    }
    let _guard = lock();
    unsafe {
        if (*task).state != TaskState::Ready {
            return;
        }
    }
    make_ready(task);
}

pub fn remove_task(task: *mut Task) {
    if task.is_null() {
        return;
    }
    let _guard = lock();
    run_queue_remove(task);
}

/// PIT tick on the BSP: sleeper wake-ups plus the BSP's own time slice.
pub fn tick() {
    if !is_running() {
        return;
    }
    let guard = lock();
    unsafe {

        STAT_TIMER_TICKS = STAT_TIMER_TICKS.saturating_add(1);

        let now = pit::ticks();
        for idx in 0..task::MAX_TASKS {
            let t = task::get_by_index(idx);
            if !t.is_null() {
                crate::signal::itimer_real_tick(t, now);
            }
            if !t.is_null() && (*t).state == TaskState::Sleeping {
                if now >= (*t).sleep_until {
                    (*t).sleep_until = 0;
                    (*t).state = TaskState::Ready;
                    make_ready(t);
                }
            }
        }
    }
    drop(guard);
    tick_local();
}

/// Per-CPU time-slice accounting (PIT on the BSP, LAPIC timer on APs).
pub fn tick_local() {
    if !is_running() {
        return;
    }
    let _guard = lock();
    unsafe {
        let cpu = this_cpu();
        let current = cpu.current;
        if current.is_null() {
            return;
        }

        (*current).total_ticks = (*current).total_ticks.saturating_add(1);

        if current == cpu.idle {
            if RUN_QUEUE_COUNT > 0 {
                cpu.preempt_pending = true;
            }
            return;
        }

        if (*current).time_slice > 0 {
            (*current).time_slice -= 1;
        }

        if (*current).time_slice == 0 {
            if (*current).state == TaskState::Zombie || (*current).state == TaskState::Dead {
                cpu.preempt_pending = true;
            } else if RUN_QUEUE_COUNT > 0 {
                cpu.preempt_pending = true;
            } else {
                (*current).time_slice = DEFAULT_TIME_SLICE;
            }
        }
    }
}

pub fn request_preempt() {
    let _guard = lock();
    this_cpu().preempt_pending = true;
}

pub fn yield_now() {
    if !is_running() {
        return;
    }

    #[cfg(target_os = "none")]
    unsafe {
        sched_yield_asm();
    }

    // Busy-wait loops (wait4, poll, futex retries) yield while waiting for
    // other tasks. If nothing else was runnable here the switch came straight
    // back, and the task that must make progress may be on another CPU,
    // spinning for the big kernel lock. Let it in.
    crate::bkl::relax();
}

pub fn block(reason: &str) {
    if !is_running() {
        return;
    }

    let current = current_task();
    if current.is_null() {
        return;
    }
    // A signal that arrived before we got here must not be lost by
    // sleeping: return so the caller's loop notices it.
    if crate::signal::wait_interrupted(current) {
        return;
    }
    unsafe {
        serial_println!(
            "[SCHED] Blocking task '{}' ({})",
            task_name(&*current),
            reason
        );
    }

    // The state change is applied by schedule_from under SCHED_LOCK.
    // Interrupts stay off until then so a preemption cannot apply it early.
    interrupts::disable();
    this_cpu().block_as = Some(TaskState::Blocked);
    #[cfg(target_os = "none")]
    unsafe {
        sched_block_asm();
    }
    #[cfg(not(target_os = "none"))]
    {
        this_cpu().block_as = None;
        interrupts::enable();
    }
}

pub fn unblock(task: *mut Task) {
    if task.is_null() {
        return;
    }

    let _guard = lock();
    unsafe {
        if (*task).state != TaskState::Blocked && (*task).state != TaskState::Sleeping {
            return;
        }

        if (*task).state == TaskState::Sleeping {
            (*task).sleep_until = 0;
        }
        (*task).state = TaskState::Ready;
    }
    make_ready(task);
}

pub fn sleep(ticks: u64) {
    if ticks == 0 {
        return;
    }

    if !is_running() {
        pit::sleep_ticks(ticks);
        return;
    }

    let current = current_task();
    if current.is_null() {
        return;
    }
    if crate::signal::wait_interrupted(current) {
        return;
    }
    unsafe {
        let wake_time = pit::ticks().saturating_add(ticks);
        (*current).sleep_until = wake_time;
    }

    // Sleeping is applied by schedule_from under SCHED_LOCK, so the
    // lock-free PIT tick can never wake this task (and let another CPU
    // resume it) while it is still on its own stack.
    interrupts::disable();
    this_cpu().block_as = Some(TaskState::Sleeping);
    #[cfg(target_os = "none")]
    unsafe {
        sched_block_asm();
    }
    #[cfg(not(target_os = "none"))]
    {
        this_cpu().block_as = None;
        interrupts::enable();
    }
}

/// Block the current task until it is unblocked or the PIT tick count
/// reaches `deadline`, whichever comes first.
pub fn block_until(reason: &str, deadline: u64) {
    if !is_running() {
        return;
    }

    let current = current_task();
    if current.is_null() {
        return;
    }
    if crate::signal::wait_interrupted(current) || pit::ticks() >= deadline {
        return;
    }
    unsafe {
        (*current).sleep_until = deadline;
    }
    let _ = reason;

    // Same hand-off as `sleep`: Sleeping is applied by schedule_from under
    // SCHED_LOCK, so the lock-free PIT tick cannot wake this task (and let
    // another CPU resume it) while it is still on its own stack.
    interrupts::disable();
    this_cpu().block_as = Some(TaskState::Sleeping);
    #[cfg(target_os = "none")]
    unsafe {
        sched_block_asm();
    }
    #[cfg(not(target_os = "none"))]
    {
        this_cpu().block_as = None;
        interrupts::enable();
    }
}

pub fn is_running() -> bool {
    unsafe { SCHED_RUNNING }
}

/// Post-interrupt hook (interrupts disabled). `irq_acquired_bkl` says the
/// interrupt entry itself took the big kernel lock, so the interrupted
/// context did not hold it.
pub fn isr_hook(frame: &mut InterruptFrame, irq_acquired_bkl: bool) -> u64 {
    if !is_running() {
        return 0;
    }
    let cpu = this_cpu();
    if cpu.current.is_null() {
        return 0;
    }
    {
        let _guard = lock();
        let idle_with_work = cpu.current == cpu.idle && unsafe { RUN_QUEUE_COUNT } > 0;
        if !cpu.preempt_pending && !idle_with_work {
            return 0;
        }
        // A task that marked itself a zombie is still running its exit path
        // in the kernel (releasing resources, waking the parent) and leaves
        // the CPU voluntarily when done. The scheduler never resumes zombies,
        // so preempting it here would lose that work for good.
        let zombie_in_kernel = unsafe { (*cpu.current).state } == TaskState::Zombie
            && unsafe { ptr::read_unaligned(ptr::addr_of!(frame.cs)) } & 3 == 0;
        if zombie_in_kernel {
            return 0;
        }
        cpu.preempt_pending = false;
    }

    let current_rsp = frame as *mut InterruptFrame as u64;
    schedule_from(current_rsp, true, irq_acquired_bkl)
}

#[no_mangle]
pub extern "C" fn sched_yield_switch(current_rsp: u64) -> u64 {
    schedule_from(current_rsp, true, false)
}

#[no_mangle]
pub extern "C" fn sched_block_switch(current_rsp: u64) -> u64 {
    schedule_from(current_rsp, false, false)
}

/// Drop `SCHED_LOCK` if this CPU took it for a switch. Called by the
/// frame-exit hook once the CPU runs on the incoming frame's stack. Returns
/// true when a switch (or a switch attempt) just finished on this CPU.
pub fn finish_switch() -> bool {
    let cpu = this_cpu();
    if !cpu.switch_locked {
        return false;
    }
    cpu.switch_locked = false;
    SCHED_LOCK.unlock_raw();
    true
}

/// Whether the task now current on this CPU must hold the big kernel lock.
pub fn current_needs_bkl() -> bool {
    let cpu = this_cpu();
    let current = cpu.current;
    !current.is_null() && current != cpu.idle && unsafe { (*current).bkl_resume }
}

/// Pick the next task. Interrupts must be disabled. Returns with
/// `SCHED_LOCK` held (released by [`finish_switch`]) unless this CPU has no
/// current task yet.
fn schedule_from(current_rsp: u64, requeue_current: bool, irq_acquired_bkl: bool) -> u64 {
    let cpu = this_cpu();
    if cpu.current.is_null() {
        return current_rsp;
    }
    SCHED_LOCK.lock_raw();
    cpu.switch_locked = true;
    unsafe {
        let current = cpu.current;
        let idle = cpu.idle;
        (*current).saved_rsp = current_rsp;
        (*current).bkl_resume = crate::bkl::held_by_me() && !irq_acquired_bkl;
        if let Some(state) = cpu.block_as.take() {
            (*current).state = state;
        }

        let is_runnable = (*current).state == TaskState::Running;
        if requeue_current && is_runnable && current != idle {
            (*current).state = TaskState::Ready;
            run_queue_push(current);
        }

        let mut next = pick_next_task();
        if next.is_null() {
            next = current;
        }

        if next == current {
            if (*current).state != TaskState::Zombie && (*current).state != TaskState::Dead {
                (*current).state = TaskState::Running;
                (*current).time_slice = DEFAULT_TIME_SLICE;
                return current_rsp;
            }
            if !idle.is_null() {
                next = idle;
            }
        }

        // Kernel code is built without SSE, so the live x87/SSE registers
        // still hold the outgoing task's user state here.
        (*current).fpu.save();
        (*next).fpu.restore();

        (*next).state = TaskState::Running;
        (*next).time_slice = DEFAULT_TIME_SLICE;
        cpu.current = next;
        STAT_CONTEXT_SWITCHES = STAT_CONTEXT_SWITCHES.saturating_add(1);

        if (*next).kernel_stack_top != 0 {
            tss::set_rsp0((*next).kernel_stack_top);
        }

        if (*next).pml4_phys != 0 {
            vmm::switch(PhysAddr::new((*next).pml4_phys));
        } else {
            vmm::switch(PhysAddr::new(0));
        }
        task::apply_user_tls_base(next);

        (*next).saved_rsp
    }
}

fn task_name(task: &Task) -> &str {
    let name = &task.name;
    let end = name.iter().position(|b| *b == 0).unwrap_or(name.len());
    core::str::from_utf8(&name[..end]).unwrap_or("<invalid>")
}

#[cfg(target_os = "none")]
extern "C" {
    fn sched_yield_asm();
    fn sched_block_asm();
    fn sched_enter_frame(rsp: u64) -> !;
}

// Build an interrupt frame for the calling context, let the scheduler pick
// the next frame, then leave through the shared IDT exit path
// (`arch_x86_64_frame_exit`): frame-exit hook (BKL release), register
// restore, SWAPGS when returning to ring 3, IRETQ.
#[cfg(target_os = "none")]
core::arch::global_asm!(
    r#"
.section .text
.extern sched_yield_switch
.extern sched_block_switch
.extern arch_x86_64_frame_exit

.macro SCHED_SWITCH_STUB name, target
.global \name
\name:
    mov rax, [rsp]
    pushfq
    pop rcx
    or rcx, 0x200
    cli
    lea rdx, [rsp + 8]

    push 0x10
    push rdx
    push rcx
    push 0x08
    push rax

    push 0
    push 0

    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15

    mov rdi, rsp
    call \target
    mov rsp, rax
    jmp arch_x86_64_frame_exit
.endm

SCHED_SWITCH_STUB sched_yield_asm, sched_yield_switch
SCHED_SWITCH_STUB sched_block_asm, sched_block_switch

.global sched_enter_frame
sched_enter_frame:
    cli
    mov rsp, rdi
    jmp arch_x86_64_frame_exit
"#
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Task;

    fn reset_queue() {
        unsafe {
            RUN_QUEUE_HEAD = ptr::null_mut();
            RUN_QUEUE_TAIL = ptr::null_mut();
            RUN_QUEUE_COUNT = 0;
        }
    }

    #[test]
    fn run_queue_push_pop_order() {
        reset_queue();
        let mut t1 = Task::for_test(1);
        let mut t2 = Task::for_test(2);

        run_queue_push(&mut t1 as *mut Task);
        run_queue_push(&mut t2 as *mut Task);

        let first = run_queue_pop();
        assert!(!first.is_null());
        unsafe {
            assert_eq!((*first).pid, 1);
        }

        let second = run_queue_pop();
        assert!(!second.is_null());
        unsafe {
            assert_eq!((*second).pid, 2);
        }

        let empty = run_queue_pop();
        assert!(empty.is_null());
    }

    #[test]
    fn run_queue_remove_updates_links() {
        reset_queue();
        let mut t1 = Task::for_test(1);
        let mut t2 = Task::for_test(2);
        let mut t3 = Task::for_test(3);

        run_queue_push(&mut t1 as *mut Task);
        run_queue_push(&mut t2 as *mut Task);
        run_queue_push(&mut t3 as *mut Task);

        run_queue_remove(&mut t2 as *mut Task);

        let first = run_queue_pop();
        let second = run_queue_pop();

        unsafe {
            assert_eq!((*first).pid, 1);
            assert_eq!((*second).pid, 3);
        }
    }
}
