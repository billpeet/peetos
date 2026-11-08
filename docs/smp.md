# SMP and APIC

This document covers ACPI MADT discovery, the Local APIC / I/O APIC interrupt
path, application-processor (AP) bring-up, per-CPU state, and the big kernel
lock (BKL) that lets every CPU run tasks.

Status:
- **Phase 1:** every CPU comes online, and IRQs go through the IOAPIC.
- **Phase 2:** the scheduler runs on every CPU. The kernel is serialised by one big kernel lock owned by a CPU.

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

**Big Kernel Lock**
The lock is owned by a CPU, not a task. It is a FIFO ticket lock: a plain test-and-set lock starved waiters whenever the owner released and immediately re-acquired.
- **Acquire** whenever this CPU enters the kernel without owning the lock:
  - SYSCALL (`bkl::syscall_enter`),
  - any interrupt or exception that is not lock-free (IDT entry hook),
  - explicit waits.

  Re-acquiring on the owning CPU is a no-op. The CPU waits for its ticket with interrupts disabled, so a nested interrupt cannot take a second ticket. While it waits it services TLB-shootdown requests.
- **Keep** the lock across kernel-to-kernel context switches.
- **Release** only in `arch_x86_64_frame_exit`, after `rsp` has moved to the frame being restored, and only when that frame returns to ring 3 or enters an idle task. Until the CPU has left the old stack, no other CPU can pick the previous task off the run queue. The SYSCALL return path releases in `bkl::syscall_exit`, with interrupts disabled first.
- **`bkl::relax()`** releases and re-queues. `sched::yield_now()` calls it after every yield, because busy-wait loops (`wait4`, `poll`, futex retries) otherwise spin forever while the task they wait for sits on another CPU, waiting for the lock.
- **`bkl::wait_for_interrupt()`** halts without the lock. The boot task's main loop uses it.
- **Lock-free paths:**
  - The PIT tick counter (a fast handler, so `ticks()` advances even while an AP holds the lock).
  - The test and TLB IPIs.
  - The idle loop.

**IPIs and Timers**
- `0xF0` test (lock-free).
- `0xF1` per-CPU LAPIC timer tick on APs, which calls `sched::tick_local()`.
- `0xF2` TLB shootdown (lock-free).
- `0xF3` reschedule.
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
- `exit_group`/`execve` (`kill_other_threads`) handle a sibling thread that is running on another CPU:
  - flag it `EXITING`,
  - kick its CPU with a reschedule IPI,
  - `relax()` until the sibling is parked or blocked, then finish it as before.
- The scheduler never picks an `EXITING` task parked in user mode, so a kicked sibling cannot be resumed while its killer waits.

**Running**
- `make test` runs `tests/smoke.sh`, which boots the `test-exit` kernel at `-smp 1` and at `-smp 4`. The 4-CPU run checks that:
  - all CPUs come online,
  - the IPI self-test passes,
  - the scheduler is running on 4 CPUs,
  - kernel worker threads were scheduled on at least two different CPUs.
- `make run QEMU_SMP=4` (or any `run*` target) boots with several CPUs.

Limitations / Simplifications
- **One big lock.** All kernel execution is serialised, so only user-mode work runs in parallel. Kernel code that busy-waits without yielding holds everyone else out, and a kernel thread that never blocks keeps its CPU's hold on the lock. The next step is per-subsystem locks.
- **One shared run queue.** No per-CPU queues, no load balancing, no CPU affinity. Tasks migrate freely.
- **Device IRQs and the PIT go to the BSP only.** There is no IRQ balancing, no MSI/MSI-X, and no `_PRT`/AML evaluation. PCI devices use their legacy ISA line plus MADT overrides.
- **Signals reach running tasks late.** A signal sent to a task running in user mode on another CPU is only noticed at that task's next syscall, which matches the single-core delivery point. Only `exit_group`/`execve` actively kick remote siblings.
- **Coarse TLB shootdowns.** Each one flushes the target's whole TLB and is sent once per invalidated page, with no batching or PCID.
- **No IST stacks.** The per-CPU TSS has only RSP0.
- **xAPIC MMIO only.** No x2APIC. APIC ids above 255 are skipped, and at most `MAX_CPUS` (16) CPUs are tracked.
- **LAPIC timer calibrated once.** It is measured against the PIT on the BSP and assumed identical on all CPUs.
- **Fixed low-memory pages.** The trampoline and its tables use fixed pages `0x8000-0xBFFF`. AP startup stacks come from the heap and are abandoned once the AP enters its idle task.
- **IMCR not programmed.** Not needed on QEMU or modern hardware.
