# Interrupts and CPU Setup

This document summarizes the interrupt-related infrastructure in the Rust port, including GDT/IDT setup, PIC/PIT handling, and interrupt masking helpers.

**Where It Lives**
- `crates/arch_x86_64/src/port.rs`
- `crates/arch_x86_64/src/interrupts.rs`
- `crates/arch_x86_64/src/gdt.rs`
- `crates/arch_x86_64/src/idt.rs`
- `crates/arch_x86_64/src/pic.rs`
- `crates/arch_x86_64/src/pit.rs`
- `kernel/src/main.rs`
- `kernel/src/exception.rs`

**Initialization Order (Kernel)**
1. `gdt::init()`
2. `idt::init()`
3. Register default exception handlers for vectors 0..31
4. `pic::init()`
5. `pit::init()`
6. `interrupts::enable()`

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
- Interrupt masking is global and single-core oriented; there is no per-CPU tracking or nesting counter beyond the saved IF bit.
- Host-side implementations only simulate IF state for unit tests.

**GDT Layout**
`crates/arch_x86_64/src/gdt.rs` defines a 7-entry GDT:
- 0x00: Null
- 0x08: Kernel code (64-bit)
- 0x10: Kernel data
- 0x18: User data (must precede user code for SYSRET)
- 0x20: User code (64-bit)
- 0x28: TSS descriptor (spans two entries)

`gdt::init()` builds the table, loads it with `lgdt`, reloads segment registers, and uses `retfq` to reload CS. TSS helpers exist (`set_tss_base`, `load_tss`) but are not wired to a TSS yet.

Limitations / Simplifications
- There is no per-CPU GDT or TSS, and no Interrupt Stack Table (IST) support yet.
- The TSS descriptor is reserved but not populated with stack pointers.

**IDT and ISR Stubs**
`crates/arch_x86_64/src/idt.rs` builds a 256-entry IDT. It installs:
- Exception handlers for vectors 0..31
- IRQ stubs for vectors 32..47

The stubs are defined via `global_asm!` and push an interrupt number and error code (dummy for non-error-code exceptions). They save GP registers, call `isr_handler`, restore registers, and `iretq`.

`isr_handler` does the following:
1. Disables interrupts.
2. Checks for a registered handler in the vector table.
3. For IRQs 32..47, it sends EOI via the PIC (with spurious detection for IRQ7/IRQ15).
4. For exceptions, it invokes the default exception handler (halts in a loop).

The default exception path uses `kernel/src/exception.rs` to print a structured serial report (including CR2 decode for page faults).

Limitations / Simplifications
- ISR stubs do not switch stacks or use IST entries.
- There is no APIC/IOAPIC or MSI support; only legacy PIC IRQs are wired.
- Exception handling is fatal by default (reports and halts).

**PIC (8259) Remap**
`crates/arch_x86_64/src/pic.rs` remaps the PIC to:
- IRQ 0..7 -> vectors 32..39 (0x20..0x27)
- IRQ 8..15 -> vectors 40..47 (0x28..0x2F)

It masks all IRQs except cascade, provides `mask_irq`, `unmask_irq`, `send_eoi`, and spurious IRQ detection.

Limitations / Simplifications
- Legacy PIC only; no APIC, no advanced interrupt routing, and no IRQ balancing.
- IRQ priorities and edge/level configuration are not exposed beyond mask/unmask.

**PIT (8253/8254)**
`crates/arch_x86_64/src/pit.rs` configures channel 0 in mode 2 at 100 Hz (`TIMER_FREQUENCY`). It registers an IRQ0 handler that increments an atomic tick counter and sends EOI. Helpers provide tick-based sleep with `hlt` and uptime calculations.

Limitations / Simplifications
- Fixed 100 Hz rate with no calibration against TSC/HPET.
- Sleep is coarse (tick-based) and busy-waits with `hlt`.

**Status Notes**
- The PIC/PIT infrastructure is in place, but no device IRQ handlers are registered yet.
- Exception reporting is serial-only. VGA integration is not implemented.
- No APIC or SMP support yet.
