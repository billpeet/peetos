use core::ptr;

use arch_x86_64::addr::PhysAddr;
use arch_x86_64::idt::InterruptFrame;
use arch_x86_64::interrupts;
use arch_x86_64::pit;
use arch_x86_64::tss;

use crate::memory::vmm;
use crate::serial_println;
use crate::task::{self, Task, TaskState, TASK_FLAG_IDLE};

pub const DEFAULT_TIME_SLICE: u64 = 5;

static mut CURRENT_TASK: *mut Task = ptr::null_mut();
static mut RUN_QUEUE_HEAD: *mut Task = ptr::null_mut();
static mut RUN_QUEUE_TAIL: *mut Task = ptr::null_mut();
static mut RUN_QUEUE_COUNT: usize = 0;
static mut IDLE_TASK: *mut Task = ptr::null_mut();
static mut SCHED_RUNNING: bool = false;
static mut PREEMPT_PENDING: bool = false;
static mut STAT_CONTEXT_SWITCHES: u64 = 0;
static mut STAT_TIMER_TICKS: u64 = 0;

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

fn pick_next_task() -> *mut Task {
    let next = run_queue_pop();
    if !next.is_null() {
        return next;
    }
    unsafe { IDLE_TASK }
}

extern "C" fn idle_task_func(_arg: *mut u8) {
    loop {
        interrupts::enable();
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
}

pub fn init() {
    serial_println!("[SCHED] Initializing scheduler...");

    let boot = task::boot_task();
    if boot.is_null() {
        serial_println!("[SCHED] ERROR: boot task not found");
        loop {
            unsafe {
                core::arch::asm!("cli; hlt", options(nomem, nostack, preserves_flags));
            }
        }
    }

    unsafe {
        CURRENT_TASK = boot;
        (*boot).state = TaskState::Running;
        (*boot).time_slice = DEFAULT_TIME_SLICE;
    }
    task::apply_user_tls_base(boot);

    let idle_pid = task::kthread_create("idle", idle_task_func, ptr::null_mut());
    if idle_pid == task::INVALID_PID {
        serial_println!("[SCHED] ERROR: failed to create idle task");
        loop {
            unsafe {
                core::arch::asm!("cli; hlt", options(nomem, nostack, preserves_flags));
            }
        }
    }

    let idle = task::get_by_pid(idle_pid);
    if idle.is_null() {
        serial_println!("[SCHED] ERROR: idle task not found");
        loop {
            unsafe {
                core::arch::asm!("cli; hlt", options(nomem, nostack, preserves_flags));
            }
        }
    }

    unsafe {
        (*idle).flags |= TASK_FLAG_IDLE;
        run_queue_remove(idle);
        (*idle).state = TaskState::Ready;
        IDLE_TASK = idle;
        SCHED_RUNNING = true;
    }

    serial_println!("[SCHED] Scheduler initialized");
}

pub fn current_task() -> *mut Task {
    unsafe { CURRENT_TASK }
}

pub fn add_task(task: *mut Task) {
    if task.is_null() {
        return;
    }
    unsafe {
        if (*task).state != TaskState::Ready {
            return;
        }
    }

    interrupts::without_interrupts(|| run_queue_push(task));
}

pub fn remove_task(task: *mut Task) {
    if task.is_null() {
        return;
    }

    interrupts::without_interrupts(|| run_queue_remove(task));
}

pub fn tick() {
    unsafe {
        if !SCHED_RUNNING || CURRENT_TASK.is_null() {
            return;
        }

        STAT_TIMER_TICKS = STAT_TIMER_TICKS.saturating_add(1);
        (*CURRENT_TASK).total_ticks = (*CURRENT_TASK).total_ticks.saturating_add(1);

        let now = pit::ticks();
        for idx in 0..task::MAX_TASKS {
            let t = task::get_by_index(idx);
            if !t.is_null() && (*t).state == TaskState::Sleeping {
                if now >= (*t).sleep_until {
                    (*t).sleep_until = 0;
                    (*t).state = TaskState::Ready;
                    run_queue_push(t);
                }
            }
        }

        if (*CURRENT_TASK).time_slice > 0 {
            (*CURRENT_TASK).time_slice -= 1;
        }

        if (*CURRENT_TASK).time_slice == 0 {
            if (*CURRENT_TASK).state == TaskState::Zombie
                || (*CURRENT_TASK).state == TaskState::Dead
            {
                PREEMPT_PENDING = true;
            } else if RUN_QUEUE_COUNT > 0 {
                PREEMPT_PENDING = true;
            } else {
                (*CURRENT_TASK).time_slice = DEFAULT_TIME_SLICE;
            }
        }
    }
}

pub fn request_preempt() {
    unsafe {
        PREEMPT_PENDING = true;
    }
}

pub fn yield_now() {
    if !is_running() {
        return;
    }

    #[cfg(target_os = "none")]
    unsafe {
        sched_yield_asm();
    }
}

pub fn block(reason: &str) {
    if !is_running() {
        return;
    }

    unsafe {
        if CURRENT_TASK.is_null() {
            return;
        }

        if (*CURRENT_TASK).state != TaskState::Blocked
            && (*CURRENT_TASK).state != TaskState::Sleeping
        {
            (*CURRENT_TASK).state = TaskState::Blocked;
        }

        serial_println!(
            "[SCHED] Blocking task '{}' ({})",
            task_name(&*CURRENT_TASK),
            reason
        );
    }

    #[cfg(target_os = "none")]
    unsafe {
        sched_block_asm();
    }
}

pub fn unblock(task: *mut Task) {
    if task.is_null() {
        return;
    }

    unsafe {
        if (*task).state != TaskState::Blocked && (*task).state != TaskState::Sleeping {
            return;
        }

        if (*task).state == TaskState::Sleeping {
            (*task).sleep_until = 0;
        }
        (*task).state = TaskState::Ready;
    }

    interrupts::without_interrupts(|| run_queue_push(task));
}

pub fn sleep(ticks: u64) {
    if ticks == 0 {
        return;
    }

    if !is_running() {
        pit::sleep_ticks(ticks);
        return;
    }

    unsafe {
        if CURRENT_TASK.is_null() {
            return;
        }

        let wake_time = pit::ticks().saturating_add(ticks);
        (*CURRENT_TASK).sleep_until = wake_time;
        (*CURRENT_TASK).state = TaskState::Sleeping;
        serial_println!(
            "[SCHED] Task '{}' sleeping for {} ticks",
            task_name(&*CURRENT_TASK),
            ticks
        );
    }

    #[cfg(target_os = "none")]
    unsafe {
        sched_block_asm();
    }
}

pub fn is_running() -> bool {
    unsafe { SCHED_RUNNING }
}

pub fn isr_hook(frame: &mut InterruptFrame) -> u64 {
    unsafe {
        if !SCHED_RUNNING || CURRENT_TASK.is_null() {
            return 0;
        }
        if !PREEMPT_PENDING {
            return 0;
        }
        PREEMPT_PENDING = false;
    }

    let current_rsp = frame as *mut InterruptFrame as u64;
    schedule_from(current_rsp, true)
}

#[no_mangle]
pub extern "C" fn sched_yield_switch(current_rsp: u64) -> u64 {
    schedule_from(current_rsp, true)
}

#[no_mangle]
pub extern "C" fn sched_block_switch(current_rsp: u64) -> u64 {
    schedule_from(current_rsp, false)
}

fn schedule_from(current_rsp: u64, requeue_current: bool) -> u64 {
    unsafe {
        if CURRENT_TASK.is_null() {
            return current_rsp;
        }

        let current = CURRENT_TASK;
        (*current).saved_rsp = current_rsp;

        let is_runnable = (*current).state == TaskState::Running;
        if requeue_current && is_runnable && current != IDLE_TASK {
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
            if !IDLE_TASK.is_null() {
                next = IDLE_TASK;
            }
        }

        // Kernel code is built without SSE, so the live x87/SSE registers
        // still hold the outgoing task's user state here.
        (*current).fpu.save();
        (*next).fpu.restore();

        (*next).state = TaskState::Running;
        (*next).time_slice = DEFAULT_TIME_SLICE;
        CURRENT_TASK = next;
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
}

#[cfg(target_os = "none")]
core::arch::global_asm!(
    r#"
.section .text
.extern sched_yield_switch
.extern sched_block_switch

.global sched_yield_asm
sched_yield_asm:
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
    call sched_yield_switch
    mov rsp, rax

    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax

    add rsp, 16
    iretq

.global sched_block_asm
sched_block_asm:
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
    call sched_block_switch
    mov rsp, rax

    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax

    add rsp, 16
    iretq
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
