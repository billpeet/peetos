# Tasking and Scheduler

This document describes the baseline tasking and scheduler implementation introduced in Phase 4. It supports kernel threads and a minimal user-mode task, with preemptive round-robin scheduling driven by the PIT.

**Where It Lives**
- `kernel/src/task.rs`
- `kernel/src/sched.rs`
- `kernel/src/user.rs`
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

**User Task Bring-Up**
`kernel/src/user.rs` creates a minimal user task by:
- Creating a fresh address space with `vmm::create_address_space()`.
- Mapping a single code page at `0x0040_0000` and a single stack page at `0x0080_0000`.
- Building an `InterruptFrame` that returns to ring 3 via `iretq`.

The initial user program is a small in-memory blob that invokes `int 0x80` to call `SYS_EXIT`.

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
- The scheduler updates `TSS.rsp0` on every task switch so user->kernel transitions have a valid kernel stack.

**Initialization Order**
1. `task::init()`
2. `sched::init()`
3. Register PIT tick hook and IDT post-handler hook
4. `user::spawn_init()`

Limitations / Simplifications
- User mode is minimal: a single hardcoded user task with one code page and one stack page.
- No ELF loader or user binary loading yet.
- Address space switching and `TSS.rsp0` updates are minimal; no per-task TLS (FS/GS).
- No FPU/SSE save/restore per task.
- Task cleanup is minimal; zombie tasks are not reaped automatically.
- Run queue operations are protected only by interrupt masking (single-core assumption).
