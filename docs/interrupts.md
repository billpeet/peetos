# Interrupts and CPU Setup

This document summarizes interrupt-related infrastructure in the current kernel, including GDT/IDT setup, PIC/PIT handling, and interrupt masking helpers. The LAPIC/IOAPIC path and multi-core bring-up are covered in `docs/smp.md`.

**Where It Lives**
- `crates/arch_x86_64/src/port.rs`
- `crates/arch_x86_64/src/interrupts.rs`
- `crates/arch_x86_64/src/gdt.rs`
- `crates/arch_x86_64/src/idt.rs`
- `crates/arch_x86_64/src/pic.rs`
- `crates/arch_x86_64/src/pit.rs`
- `crates/arch_x86_64/src/tss.rs`
- `crates/arch_x86_64/src/irq.rs` (PIC/IOAPIC-neutral ISA IRQ layer)
- `crates/arch_x86_64/src/lapic.rs`, `crates/arch_x86_64/src/ioapic.rs`
- `kernel/src/main.rs`
- `kernel/src/exception.rs`

**Initialization Order (Kernel)**
1. `gdt::init()`
2. `percpu::init_cpu(0)` (per-CPU GDT copy, TSS and GS base for the BSP)
3. `idt::init()`
4. Register default exception handlers for vectors 0..31
5. Install the `int 0x80` syscall gate and handler
6. `pic::init()`
7. `pit::init()`
8. Memory setup (`pmm`, `vmm`, `heap`)
9. `smp::init()`: switch ISA IRQ delivery to the IOAPIC when an ACPI MADT is present (otherwise stay on the PIC), then `smp::start_aps()` and `smp::ipi_self_test()`
10. `interrupts::enable()`

See `kernel/src/main.rs` for the current boot sequence.

**Port I/O Helpers**
`crates/arch_x86_64/src/port.rs` provides `inb`, `outb`, and `outl` for raw I/O port access. These are used by the PIC, PIT, serial, and QEMU exit helpers. Host builds stub nothing here, but these functions are only called on the bare-metal target.

Limitations / Simplifications
- Only 8-bit `inb`/`outb` and 32-bit `outl` are implemented; there are no 16-bit or string I/O helpers yet.
- No explicit I/O wait helper is provided; callers rely on device semantics and the inline asm barriers.

**Interrupt Masking Helpers**
`crates/arch_x86_64/src/interrupts.rs` provides:
- `enable()` and `disable()` for `sti`/`cli`.
- `are_enabled()` to read IF from RFLAGS (host builds use an `AtomicBool`).
- `without_interrupts(|| ...)` for scoped critical sections.
- `guard()` which returns an RAII `InterruptGuard` that restores the prior IF state on drop.

Limitations / Simplifications
- Interrupt masking only affects the executing CPU and is not a lock across CPUs. Cross-CPU exclusion comes from the big kernel lock (`kernel/src/bkl.rs`, see `docs/smp.md`), which every kernel entry takes. There is no nesting counter beyond the saved IF bit.
- Host-side implementations only simulate IF state for unit tests.

**GDT Layout**
`crates/arch_x86_64/src/gdt.rs` defines a 7-entry GDT:
- 0x00: Null
- 0x08: Kernel code (64-bit)
- 0x10: Kernel data
- 0x18: User data (must precede user code for SYSRET)
- 0x20: User code (64-bit)
- 0x28: TSS descriptor (spans two entries)

`gdt::init()` builds the table, loads it with `lgdt`, reloads segment registers, and uses `retfq` to reload CS. A minimal TSS is initialized after GDT load to support ring transitions.

Limitations / Simplifications
- `gdt::init()` builds a template GDT. `percpu::init_cpu()` gives each CPU a copy with its own TSS descriptor, loads it and runs `ltr`. There is no Interrupt Stack Table (IST) support yet.
- Only `RSP0` is used; no alternate stacks for faults/exceptions.

**TSS**
`crates/arch_x86_64/src/tss.rs` defines a minimal TSS with `RSP0` support. Each CPU owns one inside its per-CPU area (`percpu.rs`). The scheduler updates the executing CPU's `RSP0` on every task switch (`tss::set_rsp0`), so user-to-kernel transitions land on the correct kernel stack. The same value is mirrored at `gs:[16]` for `syscall_entry`.

Limitations / Simplifications
- Only `RSP0` is used; IST entries are not configured.
- The I/O permission bitmap is disabled (offset set to end of TSS).

**IDT and ISR Stubs**
`crates/arch_x86_64/src/idt.rs` builds a 256-entry IDT. It installs:
- Exception handlers for vectors 0..31
- IRQ stubs for vectors 32..47
- IPI stubs for vectors `0xF0..0xF3` and the LAPIC spurious vector `0xFF`
- A user-accessible `int 0x80` gate for syscalls

`idt::load()` loads the same IDT on an application processor.

Direction flag: the common stub starts with `cld`. Interrupts and exceptions inherit the interrupted RFLAGS (SYSCALL instead clears DF through `IA32_FMASK`), and user code can be inside `std; rep movs` (musl's backward `memmove`). Kernel string operations would then run backwards: a page-fault `memset` used to overwrite its own return address, an intermittent crash under multithreaded GTK. The interrupted DF comes back with `iretq`. `make run-fpu-smoke` covers it: with DF set, it takes page faults and timer interrupts and checks the pages and its DF.

Ring-3 transitions: the common stub runs `swapgs` on entry when the saved CS is ring 3. Every exit leaves through `arch_x86_64_frame_exit`, including the scheduler's yield/block/enter paths. That exit path:
- calls the kernel's frame-exit hook (the BKL release decision) with the frame about to be restored,
- restores registers,
- runs `swapgs` again when returning to ring 3, then `iretq`.

Handlers come in two kinds:
- **Fast handlers** (`register_fast_handler`) run first and must not need kernel locks: the PIT tick counter, the test IPI and the TLB-shootdown IPI. A vector with only a fast handler skips the entry hook, the normal handler and the post hook.
- **Normal handlers** run after the entry hook (BKL acquire), unless the vector is marked with `idt::set_no_entry_lock`. The PIT, the LAPIC timer and the reschedule IPI are marked that way; they take only the scheduler lock.

The entry hook reports whether it acquired the lock. If the same frame resumes, the dispatcher gives the lock back through the release hook. If the post hook switched tasks, the frame-exit hook settles ownership instead.

The stubs are defined via `global_asm!` and push an interrupt number and error code (dummy for non-error-code exceptions). They save GP registers, call `isr_handler`, restore registers, and `iretq`.

`isr_handler` does the following:
1. Disables interrupts.
2. Checks for a registered handler in the vector table.
3. Drops spurious interrupts without EOI: PIC IRQ7/IRQ15 in PIC mode, and LAPIC vector `0xFF`.
4. Runs the registered handler, or the default exception handler for unhandled vectors 0..31.
5. Sends the EOI centrally: `irq::eoi()` for vectors 32..47 (PIC or LAPIC depending on mode) and a LAPIC EOI for IPI vectors. Drivers never send their own EOI.
6. If configured, it invokes a post-handler hook for scheduling/preemption (ignored on APs).

The default exception path uses `kernel/src/exception.rs` to print a structured serial report (including CR2 decode for page faults).

Limitations / Simplifications
- ISR stubs do not switch stacks or use IST entries.
- Only legacy ISA IRQs are wired, through either the PIC or the IOAPIC. There is no MSI/MSI-X.
- Exception handling is fatal by default (reports and halts).

**ISA IRQ Layer**
`crates/arch_x86_64/src/irq.rs` hides which controller is active. ISA IRQ `n` is always vector `32 + n`:
- `irq::irq_to_vector()` and `irq::vector_to_irq()` convert between the two.
- `irq::unmask()` and `irq::mask()` go to the PIC, or to the IOAPIC entry for the IRQ's GSI after MADT override resolution.
- `irq::eoi()` and `irq::is_spurious()` are used by the IDT dispatcher.
- `irq::switch_to_apic()` programs the IOAPIC from the current PIC mask, then disables the PIC.

Limitations / Simplifications
- Only ISA IRQs 0-15 are routable. IRQ2 (cascade) is never routed, and when two IRQs resolve to the same GSI the lower one wins.

**PIC (8259) Remap**
`crates/arch_x86_64/src/pic.rs` remaps the PIC to:
- IRQ 0..7 -> vectors 32..39 (0x20..0x27)
- IRQ 8..15 -> vectors 40..47 (0x28..0x2F)

It masks all IRQs except cascade, provides `mask_irq`, `unmask_irq`, `send_eoi`, and spurious IRQ detection.

Limitations / Simplifications
- The PIC is used only when no ACPI MADT/IOAPIC is found. Otherwise it is fully masked after `irq::switch_to_apic()`.
- IRQ priorities and edge/level configuration are not exposed beyond mask/unmask.

**PIT (8253/8254)**
`crates/arch_x86_64/src/pit.rs` configures channel 0 in mode 2 at 100 Hz (`TIMER_FREQUENCY`). It registers an IRQ0 handler that increments an atomic tick counter (the dispatcher sends the EOI). `pit::busy_wait_us()` polls the channel-0 counter, so it gives microsecond delays even with interrupts disabled. AP startup uses it. Helpers provide tick-based sleep with `hlt` and uptime calculations.
If a tick hook is registered, the handler invokes it after incrementing the tick counter. The scheduler uses this hook to drive preemptive scheduling.

Limitations / Simplifications
- Fixed 100 Hz rate with no calibration against TSC/HPET.
- Sleep is coarse (tick-based) and busy-waits with `hlt`.

**Status Notes**
- The PIC/PIT infrastructure is in place and now has active device IRQ users (timer, keyboard, mouse).
- Exception reporting is serial-only. VGA integration is not implemented.
- With an ACPI MADT present (QEMU, real hardware), IRQs are delivered through the IOAPIC and the extra CPUs are brought online. See `docs/smp.md`.
