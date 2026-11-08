# SMP and APIC

This document covers ACPI MADT discovery, the Local APIC / I/O APIC interrupt
path, application-processor (AP) bring-up, per-CPU state, and the big kernel
lock (BKL) that lets every CPU run tasks.

Status:
- **Phase 1:** every CPU comes online, and IRQs go through the IOAPIC.
- **Phase 2:** the scheduler runs on every CPU. The kernel is serialised by one big kernel lock owned by a CPU.
- **Phase 3 (in progress):** the scheduler, PMM, heap and VMM have their own locks. Timer ticks, reschedule IPIs, context switches and a few syscalls no longer take the big lock, and big-lock ownership follows tasks across switches.

**Where It Lives**
- `crates/arch_x86_64/src/acpi.rs`: RSDP/RSDT/XSDT/MADT parsing and ISA IRQ override resolution (pure, host-tested)
- `crates/arch_x86_64/src/lapic.rs`: xAPIC MMIO driver (enable, EOI, IPIs, timer calibration, periodic timer)
- `crates/arch_x86_64/src/ioapic.rs`: redirection-table programming
- `crates/arch_x86_64/src/irq.rs`: controller-neutral ISA IRQ layer
- `crates/arch_x86_64/src/percpu.rs`: per-CPU area via GS, plus a GDT and TSS for each CPU
- `crates/arch_x86_64/src/idt.rs`: `swapgs`-aware entry/exit, fast (lock-free) handlers, entry/exit hooks
- `kernel/src/bkl.rs`: the big kernel lock
- `kernel/src/sched.rs`: shared run queue with per-CPU current/idle/preempt state
- `kernel/src/smp.rs`: discovery, APIC switch-over, AP startup, IPIs, TLB shootdown
- `kernel/src/smp_test.rs`: `test-exit` scheduler self-test
- `kernel/arch/x86_64/entry.asm`: real-mode AP trampoline and `ap_high_entry`

**Boot Sequence**
1. `percpu::init_cpu(0)` runs right after `gdt::init()`. The BSP gets its own GDT copy and TSS, and `GS_BASE` points at its per-CPU slot.
2. `smp::init()` runs after the heap is up, with interrupts still disabled.
   - Finds the MADT, maps the IOAPIC(s) and LAPIC uncached, and enables the BSP LAPIC (spurious vector `0xFF`).
   - Moves ISA IRQ delivery from the 8259 to the IOAPIC, honouring MADT interrupt source overrides. IRQs already unmasked on the PIC stay unmasked.
   - Without a MADT/IOAPIC the kernel stays on the PIC and on one CPU.
3. `smp::start_aps()`
   - Calibrates the LAPIC timer against the PIT (one 10 ms scheduler tick).
   - Installs the trampoline at physical `0x8000`, with transitional page tables at `0x9000-0xBFFF`.
   - Starts each AP with INIT, then STARTUP, then STARTUP.
   - Each AP runs `ap_rust_entry`: per-CPU GDT/TSS/GS, the shared IDT, its own SYSCALL MSRs, and a LAPIC enable. It then parks in `sti; hlt`.
4. `smp::ipi_self_test()` checks that every AP takes a lock-free test IPI.
5. After `sched::init()`, `smp::release_aps()` creates an idle task for each AP and releases the APs. Each AP then:
   - takes the BKL,
   - starts its periodic LAPIC timer (vector `0xF1`),
   - enters its idle task through `sched::ap_enter`.

   From then on every CPU picks tasks from the shared run queue.

**Per-CPU State**
- `PerCpu` (GS-relative) holds the self pointer, the SYSCALL user-RSP scratch, RSP0, the CPU index, the CPU's GDT and its TSS.
- Ring-3 transitions:
  - `syscall_entry` runs `swapgs` and switches to `gs:[16]` (RSP0) instead of using globals.
  - The ISR stubs run `swapgs` when the saved CS is ring 3, and again before an `iretq` back to ring 3.
- The scheduler keeps `current`, `idle` and `preempt_pending` for each CPU. `sched::current_task()` and `task::current()` therefore return the executing CPU's task.
- `smp` tracks each CPU's online flag, APIC id, active CR3, and pending TLB-flush flag.

**Locks**
The lock order is BKL, then `SCHED_LOCK`, then heap, then VMM, then PMM.

- **Big kernel lock** (`kernel/src/bkl.rs`): a FIFO ticket lock that still serialises most kernel code (syscalls, device IRQs, exceptions). It is owned by a CPU, but logically it follows the task:
  - A task switched out while holding it records that in `Task::bkl_resume` and gives it up.
  - The CPU that later resumes the task takes it again before the task runs. Kernel threads start with `bkl_resume = true` unless created with `kthread_create_with_lock(.., false)`.
- **`SCHED_LOCK`** (`kernel/src/sched.rs`): an interrupt-safe spinlock. It protects:
  - the run queue,
  - per-CPU current/idle/preempt state,
  - the task table,
  - the scheduler-owned task state transitions.

  The scheduler never needs the big lock.
- **PMM, heap and VMM locks**: interrupt-safe spinlocks taken at the public entry points (`alloc_page`, `free_page`, `kmalloc`, `kfree`, `map_page_in`, `unmap_page_in`, `clone_user_space`, ...).
  - Internal calls use unlocked `*_locked` variants.
  - `vmm::switch` and the read-only getters take no lock.
  - TLB shootdowns are sent only while holding `VMM_LOCK`, which keeps initiators to one at a time.

Big-lock rules:
- **Acquire** whenever this CPU enters the kernel through a path that needs it without owning it:
  - SYSCALL (`bkl::syscall_enter`),
  - interrupts and exceptions on vectors not marked lock-free (IDT entry hook, `bkl::interrupt_enter`),
  - explicit waits.

  Re-acquiring on the owning CPU is a no-op. The CPU waits for its ticket with interrupts disabled, so a nested interrupt cannot take a second ticket. While it waits it services TLB-shootdown requests.
- **Interrupt balance.** If the entry hook took the lock and the same frame resumes, the IDT dispatcher releases it (`bkl::interrupt_release`).
- **Context switch hand-off.**
  - `schedule_from` takes `SCHED_LOCK` (raw form) and records `bkl_resume` for the outgoing task.
  - On the incoming frame's stack, `arch_x86_64_frame_exit` calls `bkl::frame_exit_hook`. That hook drops `SCHED_LOCK` (`sched::finish_switch`), then acquires or releases the big lock to match the incoming task.
  - Until the CPU has left the old stack, no other CPU can resume the previous task.
- **SYSCALL return.** `bkl::syscall_exit` disables interrupts, then releases.
- **`block`/`sleep`.** These pass the new state to `schedule_from`, which applies it under `SCHED_LOCK`. The lock-free PIT tick can therefore never wake a sleeper, and let another CPU resume it, while the sleeper is still on its own stack.
- **`bkl::relax()`** releases and re-queues behind waiting CPUs. It is a no-op for lock-free callers. `sched::yield_now()` calls it after every yield, because busy-wait loops (`wait4`, `poll`, futex retries) otherwise starve the task they wait for.
- **`bkl::wait_for_interrupt()`** halts without the lock. The boot task's main loop uses it.

Paths that never take the big lock:
- The syscalls `sched_yield`, `getpid`, `gettid`, `getppid`, `clock_gettime`, `gettimeofday` and `nanosleep` (`syscall::lock_free_syscall`).
  - They fall back to the locked dispatcher when a deliverable signal is pending or the task is exiting on entry.
  - If a signal arrives during the call (for example, one that wakes a `nanosleep`), they take the lock only for the return-path signal delivery.
- User-memory copies (`uaccess::copy_user_bytes`). Each page goes through `vmm::with_user_page`, which does all of the following in a single `VMM_LOCK` hold:
  - reads the task's `pml4_phys`,
  - breaks COW sharing for writes,
  - translates the address,
  - performs the access.

  Every path that frees a user frame removes its mapping under `VMM_LOCK` first, so a concurrent `munmap` either completes before the copy or makes it fail with `EFAULT`. `execve` zeroes a sibling's `pml4_phys` before tearing the old space down, so a preempted copy sees either the intact old space or no address space.
- The PIT tick: the counter (a fast handler) and `sched::tick`, which runs sleeper wake-ups and the BSP time slice.
- The LAPIC timer tick and the reschedule IPI (`idt::set_no_entry_lock`).
- The test and TLB IPIs.
- Every context switch.
- The idle loop.

So timer preemption and task switching continue on every CPU even while another CPU sits in a long syscall holding the big lock.

**IPIs and Timers**
- `0xF0` test (lock-free).
- `0xF1` per-CPU LAPIC timer tick on APs, which calls `sched::tick_local()` (no big lock).
- `0xF2` TLB shootdown (lock-free).
- `0xF3` reschedule (no big lock).
- The PIT stays the global clock on the BSP. `sched::tick()` wakes sleepers and runs the BSP's time slice.
- When a task becomes ready, `sched` sends a reschedule IPI to one idle CPU.

**TLB Shootdown**
- Every VMM path that removes or downgrades a present mapping goes through `vmm::invalidate_in` or `flush_in`.
- These flush locally when the address space is live here (or the address is in the kernel half). They then call `smp::tlb_shootdown()`, which targets:
  - all online CPUs for kernel addresses,
  - only CPUs whose active CR3 matches, for user addresses.
- Targets flush their whole TLB (CR3 reload; no global pages are used) and acknowledge. The initiator holds the BKL and waits.
- Brand-new mappings over non-present entries need no shootdown.

**Threads on Other CPUs**
- `exit_group`/`execve` (`kill_other_threads`) flag each sibling `EXITING`, then classify it with `sched::detach_for_kill`, atomically under `SCHED_LOCK`:
  - parked in user mode: taken off the run queue as a zombie;
  - running on another CPU: that CPU is kicked with a reschedule IPI;
  - inside the kernel: woken, and it exits on its own.
- While any sibling is still running elsewhere, the caller `relax()`es and retries.
- The scheduler never picks an `EXITING` task parked in user mode, so a kicked sibling cannot be resumed while its killer waits.

**Running**
- `make test` runs `tests/smoke.sh`, which boots the `test-exit` kernel at `-smp 1` and at `-smp 4`. The 4-CPU run checks that:
  - all CPUs come online,
  - the IPI self-test passes,
  - the scheduler is running on 4 CPUs,
  - kernel worker threads were scheduled on at least two different CPUs,
  - scheduler-only threads kept yielding on other CPUs while a "hog" thread held the big lock with interrupts disabled (`smp_test::run_bkl_free`).
- `make run QEMU_SMP=4` (or any `run*` target) boots with several CPUs.

Limitations / Simplifications
- **Big kernel lock still covers most kernel code.** All other syscalls, device IRQs and exceptions are serialised by it. Kernel code that busy-waits without yielding still holds other syscalls out.
  - Next: give VFS, net and IPC their own locks, then move more syscalls to the lock-free path.
- **User copies hold the global `VMM_LOCK` for each page.** A per-address-space lock would scale better. Copies still do not fault in lazily mapped pages (an untouched anonymous page gives `EFAULT`, as before), and they do not check the PTE's user/write bits.
- **Zombies in kernel mode are never preempted.** A task that has marked itself a zombie is still running its exit path in the kernel and leaves the CPU voluntarily, so `sched::isr_hook` skips it.
- **One shared run queue.** No per-CPU queues, no load balancing, no CPU affinity. Tasks migrate freely.
- **Device IRQs and the PIT go to the BSP only.** There is no IRQ balancing, no MSI/MSI-X, and no `_PRT`/AML evaluation. PCI devices use their legacy ISA line plus MADT overrides.
- **Signals reach running tasks late.** A signal sent to a task running in user mode on another CPU is only noticed at that task's next syscall, which matches the single-core delivery point. Only `exit_group`/`execve` actively kick remote siblings.
- **Coarse TLB shootdowns.** Each one flushes the target's whole TLB and is sent once per invalidated page, with no batching or PCID.
- **No IST stacks.** The per-CPU TSS has only RSP0.
- **xAPIC MMIO only.** No x2APIC. APIC ids above 255 are skipped, and at most `MAX_CPUS` (16) CPUs are tracked.
- **LAPIC timer calibrated once.** It is measured against the PIT on the BSP and assumed identical on all CPUs.
- **Fixed low-memory pages.** The trampoline and its tables use fixed pages `0x8000-0xBFFF`. AP startup stacks come from the heap and are abandoned once the AP enters its idle task.
- **IMCR not programmed.** Not needed on QEMU or modern hardware.
