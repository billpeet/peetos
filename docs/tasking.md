# Tasking and Scheduler

This document describes the baseline tasking and scheduler implementation introduced in Phase 4. It currently supports kernel threads and preemptive round-robin scheduling driven by the PIT.

**Where It Lives**
- `kernel/src/task.rs`
- `kernel/src/sched.rs`
- `crates/arch_x86_64/src/idt.rs` (interrupt hook for preemption)
- `crates/arch_x86_64/src/pit.rs` (tick hook)
- `kernel/src/main.rs`

**Task Structure**
`kernel/src/task.rs` defines the `Task` struct and task table. Each task stores:
- IDs (PID/PPID)
- Name
- State and flags
- Saved interrupt-frame stack pointer (`saved_rsp`)
- Kernel stack base/top
- Scheduling metadata (time slice, tick counters, sleep deadline)

The boot task represents the current execution context and is created in `task::init()`.

**Kernel Threads**
`task::kthread_create()` allocates a kernel stack and builds an `InterruptFrame` at the top of the stack. When scheduled, the ISR return path restores this frame and begins execution at `kthread_entry_wrapper`, which calls the thread entry function and then `kthread_exit()`.

**Scheduler**
`kernel/src/sched.rs` provides a simple round-robin scheduler:
- Run queue is a doubly-linked list of READY tasks.
- Time slices default to 5 ticks (50ms at 100Hz).
- The idle task runs when no other tasks are ready.

**Preemption and Yield**
- The PIT timer interrupt increments ticks and calls `sched::tick()` via a hook.
- When a time slice expires, `sched::tick()` sets a preemption flag.
- After the interrupt handler returns, the IDT post-handler hook calls `sched::isr_hook()` and performs a stack switch if needed.
- Voluntary yield and block/sleep paths build a synthetic interrupt frame in assembly (`sched_yield_asm`, `sched_block_asm`) to reuse the same return path.

**Initialization Order**
1. `task::init()`
2. `sched::init()`
3. Register PIT tick hook and IDT post-handler hook

Limitations / Simplifications
- Kernel threads only; there is no user-mode task support yet.
- No context switching of address spaces (CR3) or TLS (FS/GS) yet.
- No FPU/SSE save/restore per task.
- Task cleanup is minimal; zombie tasks are not reaped automatically.
- Run queue operations are protected only by interrupt masking (single-core assumption).
