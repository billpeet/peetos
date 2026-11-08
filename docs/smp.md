# SMP and APIC

This document covers ACPI MADT discovery, the Local APIC / I/O APIC interrupt
path, and application-processor (AP) bring-up. This is SMP phase 1: every CPU
is brought online and can take interrupts, but the scheduler still runs only
on the bootstrap processor (BSP).

**Where It Lives**
- `crates/arch_x86_64/src/acpi.rs`: RSDP/RSDT/XSDT/MADT parsing, ISA IRQ override resolution (pure, host-tested)
- `crates/arch_x86_64/src/lapic.rs`: xAPIC MMIO driver (enable, EOI, IPIs)
- `crates/arch_x86_64/src/ioapic.rs`: redirection-table programming
- `crates/arch_x86_64/src/irq.rs`: controller-neutral ISA IRQ layer (`unmask`/`mask`/`eoi`)
- `crates/arch_x86_64/src/pit.rs`: `busy_wait_us()` for startup delays with interrupts off
- `kernel/src/smp.rs`: discovery, APIC switch-over, AP startup, IPI self-test
- `kernel/arch/x86_64/entry.asm`: real-mode AP trampoline and `ap_high_entry`

**Boot Sequence**
`kmain` calls these right after the heap is up (interrupts still disabled):

1. `smp::init()`
   - Finds the RSDP (EBDA first KiB, then `0xE0000-0xFFFFF`), follows the
     RSDT/XSDT to the MADT, and validates every table checksum.
   - Maps each IOAPIC and the LAPIC uncached (`PCD|PWT`) via MMIO windows,
     and registers the IOAPICs (all pins masked).
   - Enables the BSP LAPIC: spurious vector `0xFF`, TPR 0, timer, LINT0
     and error LVTs masked, LINT1 set to NMI.
   - Resolves ISA IRQ 0-15 through MADT interrupt source overrides (QEMU maps
     IRQ0 to GSI2; IRQ 5/9/10/11 are level-triggered) and calls
     `irq::switch_to_apic()`. IRQs that drivers had already unmasked on the PIC
     stay unmasked on the IOAPIC. The 8259 is then fully masked.
   - If there is no MADT, or no usable IOAPIC, the kernel logs it and stays on
     the legacy PIC. Nothing else changes.
2. `smp::start_aps()`
   - Copies the trampoline to physical `0x8000` and builds transitional page
     tables at `0x9000-0xBFFF`. They identity-map the low 2 MiB and share the
     kernel's `PML4[511]`.
   - Per AP: allocates a 16 KiB stack, writes the trampoline data block
     (`0x8F00`), and sends INIT, waits 10 ms, then sends up to two STARTUP
     IPIs with vector `0x08`. It then waits up to 200 ms for check-in.
   - The AP goes from real mode to protected mode to long mode, using the
     BSP's CR0/CR4/EFER bits. It jumps to `ap_high_entry`, loads the kernel
     CR3, and calls `ap_rust_entry`, which loads the shared GDT and IDT,
     enables its LAPIC, marks itself online, and parks in `sti; hlt`.
3. `smp::ipi_self_test()` sends one fixed IPI (vector `0xF0`) to each online
   AP and checks that every AP's handler ran.

Expected serial output with `-smp 4`:
```
[APIC] IOAPIC id=0 addr=0xfec00000 gsi=0..23
[APIC] LAPIC id=0 at 0xfee00000; 15 ISA IRQ(s) routed via IOAPIC, PIC disabled
[APIC] Timer IRQ0 -> GSI 2
[SMP] MADT lists 4 CPU(s)
[SMP] CPU 1 online (APIC id 1)
...
[SMP] 4 of 4 CPU(s) online
[SMP] IPI self-test: 3/3 AP(s) responded
```

**Interrupt Vectors**
- `0x20-0x2F`: ISA IRQ 0-15. The vector is the same under the PIC and the IOAPIC, so drivers register with `irq::irq_to_vector(irq)` either way.
- `0x80`: `int 0x80` syscall gate.
- `0xF0-0xF3`: IPI vectors with installed stubs. `0xF0` is the self-test IPI.
- `0xFF`: LAPIC spurious vector. It is never acknowledged.

The IDT dispatcher (`idt::isr_handler`) sends the EOI for every IRQ and IPI
vector after the handler runs. Drivers must not send their own EOI. An
unacknowledged LAPIC vector blocks every vector in its priority class or
below, including the timer. Before this change the e1000 handler never sent
an EOI at all.

**Scheduler Interaction**
- Only the BSP schedules. The PIT (IRQ0) and all device IRQs are routed to the BSP.
- `sched::isr_hook` returns early on an AP (`smp::is_bsp()` compares LAPIC ids), so an IPI taken on an AP never touches the run queue or `CURRENT_TASK`.
- APs do not allocate, print, or take locks after check-in.

**Running**
- `make test` runs `tests/smoke.sh`, which boots the `test-exit` kernel twice:
  - With `-smp 1` it checks the uniprocessor APIC path.
  - With `-smp 4` it checks that all CPUs come online and the IPI self-test passes.
  - Both runs check that the PIT ticks through the IOAPIC.
- `make run QEMU_SMP=4` (or any `run*` target) boots with several CPUs.

Limitations / Simplifications
- **The scheduler is BSP-only.** APs park in `hlt` after check-in. Running tasks on them needs:
  - per-CPU state (GS base, `current` task, idle task, TSS/RSP0)
  - a per-CPU or locked run queue
  - TLB shootdown IPIs
  - an audit of the many `static mut` globals and the code that relies on "interrupts off means exclusive". Today those include the PMM, heap and VMM; see their docs.
- **No per-CPU GDT/TSS.** APs load the shared GDT without `ltr`, so they cannot enter ring 3 yet.
- **xAPIC MMIO only.** There is no x2APIC mode. CPUs with APIC ids above 255 are skipped, and at most `MAX_CPUS` (16) CPUs are tracked.
- **ISA IRQs only.** PCI devices work only through their legacy ISA IRQ line plus MADT overrides. There is no `_PRT`/AML evaluation and no MSI/MSI-X. All routes target the BSP; there is no IRQ balancing.
- **IMCR not programmed.** The IMCR (ports `0x22/0x23`) is not programmed for old chipsets that boot in PIC mode. This doesn't matter on QEMU or modern hardware.
- **PIT is still the system timer.** The LAPIC timer is masked and not calibrated.
- **Fixed low-memory pages.** The trampoline uses fixed pages `0x8000-0xBFFF`. This relies on the PMM reserving the low 1 MiB and on nothing else using that range after boot. AP stacks come from the kernel heap and are never freed, even if an AP fails to start.
- **Time-based startup.** AP startup waits are delays measured with the PIT, not acknowledgements. A CPU that misses the 200 ms window is reported and skipped.
- **Direct-map limit.** ACPI tables above the 128 MiB direct map are reached through an MMIO window. Each such lookup uses up one of the 16 windows.
