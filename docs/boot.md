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
1. Disable maskable interrupts (`cli`) and clear DF to satisfy SysV ABI expectations.
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
- The early stack is a fixed 64 KB region with no guard pages.
- Stage2 uses a fixed low-memory kernel staging window (`0x18000..0x9FFFF`) before relocating to `0x100000`; oversized kernels must be rejected at build time.

**Kernel Initialization Sequence (`kmain`)**
1. Initialize serial output and print the boot banner.
2. Load the GDT/TSS/IDT, then register default exception + syscall handlers.
3. Initialize framebuffer state from bootloader VESA handoff, then initialize the PIC, PIT, TTY state, keyboard IRQ handling, and PS/2 mouse IRQ/cursor state.
4. Initialize PMM, VMM, and the kernel heap.
5. Run baseline PCI enumeration and print a bounded serial inventory.
6. Run storage-controller bring-up (IDE/AHCI classification, AHCI ABAR capture, AHCI command-bit setup, AHCI MMIO capability/port probing, AHCI DMA `IDENTIFY`, AHCI LBA48 DMA read/write command path wiring, ATA PIO `IDENTIFY`), register discovered block devices, then print storage/block summaries.
7. Initialize networking (E1000 path), then attempt USB UHCI HID-tablet bring-up for absolute pointer input.
8. Initialize VFS (attempt block-backed ext2 mount and enable block sync bridge for mutating VFS syscalls), then tasking/scheduler, IPC, and shared-memory subsystems.
9. Configure syscall MSRs and dispatch state.
10. Enable interrupts.
11. Optionally exit QEMU when `test-exit` is enabled.
12. Halt in an infinite loop.

Limitations / Simplifications
- Single-core bring-up only; there is no per-CPU initialization yet.
- The default exception handler reports to serial and halts; no recovery is attempted.
