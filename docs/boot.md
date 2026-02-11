# Boot and Kernel Entry

This document describes the early boot path into the Rust kernel and the high-level `kmain()` initialization sequence.

**Where It Lives**
- `kernel/arch/x86_64/entry.asm`
- `linker/kernel.ld`
- `kernel/src/main.rs`

**Bootloader Assumptions**
- The bootloader enters 64-bit long mode and jumps to `_start`.
- The first 2 MB are identity-mapped, and the kernel is also mapped in the higher-half.
- Page tables are located at fixed physical addresses reused from the C kernel.

**Entry Flow (`entry.asm`)**
1. Clear DF to satisfy the SysV ABI expectations.
2. Jump immediately to the higher-half address to avoid cache aliasing.
3. Zero the `.bss` section using `__bss_start`/`__bss_end` from the linker.
4. Unmap the identity-mapped lower half (PML4[0]) and flush the TLB.
5. Initialize the kernel stack (`stack_top`).
6. Enable SSE/SSE2 by configuring `CR0` and `CR4`.
7. Call `kmain()`.
8. If `kmain()` returns, halt in a loop.

Limitations / Simplifications
- Assumes a fixed page table layout and that PML4 is at physical `0x10000` (mapped at `0xFFFF_FFFF_8001_0000`).
- No boot-info structure is passed to `kmain()` yet; the kernel relies on fixed memory locations for early data.
- SSE enablement is unconditional and does not check CPUID feature bits.
- The early stack is a fixed 16 KB region with no guard pages.

**Kernel Initialization Sequence (`kmain`)**
1. Initialize serial output and print the boot banner.
2. Load the GDT/TSS/IDT, then register default exception + syscall handlers.
3. Initialize the PIC and PIT.
4. Initialize PMM, VMM, and the kernel heap.
5. Run baseline PCI enumeration and print a bounded serial inventory.
6. Run baseline storage-controller discovery (IDE/AHCI classification, AHCI ABAR capture, AHCI command-bit setup) and print a serial summary.
7. Initialize VFS, tasking/scheduler, IPC, and shared-memory subsystems.
8. Configure syscall MSRs and dispatch state.
9. Enable interrupts.
10. Optionally exit QEMU when `test-exit` is enabled.
11. Halt in an infinite loop.

Limitations / Simplifications
- Single-core bring-up only; there is no per-CPU initialization yet.
- The default exception handler reports to serial and halts; no recovery is attempted.
