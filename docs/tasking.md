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
- User-process metadata (`pml4_phys`, user entry point, user stack bounds)
- Initial user bootstrap metadata (`user_stack_ptr`, `user_argc`, `user_argv`, `user_envp`, `user_auxv`, `user_clear_tid`)
- Per-task ELF/program metadata (`phdr_*`, `base_addr`, dynamic-linker flags)
- Per-task program break tracking (`user_brk_start`, `user_brk`)
- Per-task file descriptor table metadata (`kind`, inode handle, file offset, open flags)
- Scheduling metadata (time slice, tick counters, sleep deadline)

The boot task represents the current execution context and is created in `task::init()`.

**Kernel Threads**
`task::kthread_create()` allocates a kernel stack and builds an `InterruptFrame` at the top of the stack. When scheduled, the ISR return path restores this frame and begins execution at `kthread_entry_wrapper`, which calls the thread entry function and then `kthread_exit()`.

**User Task Bring-Up**
`kernel/src/user.rs` creates a minimal user task by:
- Creating a fresh address space with `vmm::create_address_space()`.
- Loading an embedded ELF64 image (`user/hello_dyn.elf`) into the new address space.
- If `PT_INTERP` is present, loading the embedded interpreter (`user/ld-peetos.so`) and entering user mode at the interpreter entry.
- Mapping a 32KB user stack at `USER_STACK_TOP = 0x7FFF_FF00_0000`.
- Writing a Linux-style initial stack (`argc`, `argv`, `envp`, `auxv`) before first ring-3 entry.
- Building an `InterruptFrame` that returns to ring 3 via `iretq`.

The initial user program is a small dynamic ELF that writes to stdout then exits via `int 0x80`.

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
- User mode is minimal: a single hardcoded user task with a fixed embedded ELF.
- No `exec`/`fork`; `argv`/`envp` are currently fixed at spawn time for the embedded init ELF.
- Dynamic-linking support is bootstrap-only (embedded `PT_INTERP` image, no reloc/symbol resolution).
- Per-task FD state exists and is used by baseline VFS syscalls, but advanced descriptor semantics (`dup*`, `fcntl`, CLOEXEC semantics on `exec`) are not implemented yet.
- No ASLR or stack guard pages; the stack is mapped at a fixed address.
- Address space switching and `TSS.rsp0` updates are minimal; no per-task TLS (FS/GS).
- Task metadata for user processes is tracked, but there is no `/proc` or userspace-visible tooling for inspecting it yet.
- `user_clear_tid` zeroing now performs a best-effort futex wake for one waiter at thread exit, but robust-list semantics and thread-group teardown are not implemented yet.
- No FPU/SSE save/restore per task.
- Task cleanup is minimal; zombie tasks are not reaped automatically.
- Run queue operations are protected only by interrupt masking (single-core assumption).
