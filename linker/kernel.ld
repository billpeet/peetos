/**
 * ===========================================================================
 * PeetOS Kernel Linker Script - kernel.ld
 * ===========================================================================
 *
 * WHAT IS A LINKER SCRIPT?
 * ========================
 * A linker script tells the linker how to combine object files (.o) into
 * the final executable. It specifies:
 *   - The memory layout (where each section goes)
 *   - The entry point (first code to execute)
 *   - The output format
 *   - Symbol definitions (like __bss_start, __bss_end)
 *
 * WHY DO WE NEED ONE?
 * ===================
 * By default, the linker assumes we're making a user-space program that
 * will be loaded by an operating system. But WE are the operating system!
 *
 * We need to:
 *   1. Specify where our code will be loaded in memory
 *   2. Control the order of sections
 *   3. Define symbols that our assembly code needs
 *   4. Remove unnecessary sections
 *
 * HIGHER-HALF KERNEL:
 * ===================
 * This is a "higher-half" kernel. We use two addresses:
 *
 *   VMA (Virtual Memory Address): 0xFFFFFFFF80100000
 *     - This is where the kernel thinks it's running
 *     - All symbols and addresses in the code use this
 *     - The kernel accesses memory through this virtual address
 *
 *   LMA (Load Memory Address): 0x100000
 *     - This is where the bootloader actually loads the kernel
 *     - The bootloader loads to physical memory at this address
 *     - Before paging, the kernel runs briefly at this address
 *
 * The bootloader sets up page tables that map:
 *   Virtual 0xFFFFFFFF80000000-0xFFFFFFFF801FFFFF -> Physical 0x0-0x1FFFFF
 *
 * So the kernel at physical 0x100000 is also accessible at virtual
 * 0xFFFFFFFF80100000.
 *
 * Section order:
 *   .text   - Executable code (read-only, executable)
 *   .rodata - Read-only data (constants, strings)
 *   .data   - Initialized read-write data
 *   .bss    - Uninitialized read-write data (zeroed by startup code)
 *
 * ALIGNMENT:
 * ==========
 * We align sections to 4 KB (4096 bytes) boundaries. This is the page size
 * on x86_64 and makes it easy to set different permissions for different
 * sections using paging.
 *
 * ===========================================================================
 */

/*
 * OUTPUT FORMAT AND ARCHITECTURE
 * ==============================
 * Specify that we're creating a 64-bit ELF binary for x86_64.
 * We'll later extract the raw binary from this ELF.
 */
OUTPUT_FORMAT("elf64-x86-64")
OUTPUT_ARCH(i386:x86-64)

/*
 * ENTRY POINT
 * ===========
 * Specify the first function to execute.
 * This must match the symbol exported from entry.asm.
 */
ENTRY(_start)

/*
 * KERNEL ADDRESSES
 * ================
 * We have two addresses for the higher-half kernel:
 *
 * KERNEL_PHYS_ADDR: Physical address where bootloader loads the kernel
 * KERNEL_VIRT_ADDR: Virtual address where kernel code is linked to run
 *
 * The difference (KERNEL_VIRT_ADDR - KERNEL_PHYS_ADDR) is the offset
 * between physical and virtual addresses.
 */
KERNEL_PHYS_ADDR = 0x100000;
KERNEL_VIRT_ADDR = 0xFFFFFFFF80100000;

/*
 * SECTIONS
 * ========
 * Define how sections from object files are combined and placed in memory.
 */
SECTIONS
{
    /*
     * Start at the kernel VIRTUAL address.
     * The '.' is the "location counter" - it tracks the current address.
     *
     * All symbols will be assigned virtual addresses starting here.
     */
    . = KERNEL_VIRT_ADDR;

    /*
     * Record the start of the kernel (virtual address).
     * This symbol can be used by the kernel to know where it starts.
     */
    __kernel_start = .;

    /*
     * Also record physical start for boot code that needs it.
     */
    __kernel_phys_start = KERNEL_PHYS_ADDR;

    /*
     * TEXT SECTION
     * ============
     * Contains executable code (.text from all object files).
     *
     * ALIGN(4K) ensures the section starts on a 4 KB boundary.
     * This is important for memory protection - we can mark this
     * section as read-only and executable.
     *
     * The AT(KERNEL_PHYS_ADDR) directive tells the linker to place
     * this section at the PHYSICAL address in the output file,
     * while keeping the VIRTUAL address for symbol resolution.
     *
     * The *(.text) and *(.text.*) patterns collect:
     *   - .text sections from all object files
     *   - Sections like .text.startup, .text.exit, etc.
     */
    .text ALIGN(4K) : AT(KERNEL_PHYS_ADDR) {
        /* Ensure entry.o comes first - it contains _start */
        *entry.o(.text)
        *(.text)
        *(.text.*)
    }

    /*
     * READ-ONLY DATA SECTION
     * ======================
     * Contains constant data (strings, lookup tables, etc.).
     *
     * This should be read-only but NOT executable.
     * Common contents:
     *   - String literals ("Hello, World!")
     *   - const variables
     *   - Switch statement jump tables
     */
    .rodata ALIGN(4K) : AT(ADDR(.rodata) - KERNEL_VIRT_ADDR + KERNEL_PHYS_ADDR) {
        *(.rodata)
        *(.rodata.*)
    }

    /*
     * INITIALIZED DATA SECTION
     * ========================
     * Contains global/static variables with initial values.
     *
     * Example: int foo = 42;
     * This variable has an initial value that must be stored in the binary.
     * The .data section is read-write.
     */
    .data ALIGN(4K) : AT(ADDR(.data) - KERNEL_VIRT_ADDR + KERNEL_PHYS_ADDR) {
        *(.data)
        *(.data.*)
    }

    /*
     * BSS SECTION (Block Started by Symbol)
     * =====================================
     * Contains uninitialized global/static variables.
     *
     * Example: int bar;
     * This variable should be zero-initialized at runtime.
     *
     * The BSS section doesn't take space in the binary file!
     * The linker just records its size, and the loader (our entry.asm)
     * zeros it at runtime.
     *
     * We export __bss_start and __bss_end so entry.asm knows what to zero.
     */
    .bss ALIGN(4K) : AT(ADDR(.bss) - KERNEL_VIRT_ADDR + KERNEL_PHYS_ADDR) {
        __bss_start = .;        /* Symbol: start of BSS */
        *(.bss)
        *(.bss.*)
        *(COMMON)               /* Common symbols (uninitialized global variables) */
        __bss_end = .;          /* Symbol: end of BSS */
    }

    /*
     * Record the end of the kernel.
     * This is useful for the memory allocator to know where free memory starts.
     */
    __kernel_end = .;

    /*
     * DISCARDED SECTIONS
     * ==================
     * These sections are not needed in the final binary.
     *
     * .comment     - Compiler version information
     * .note*       - ELF notes (build ID, etc.)
     * .eh_frame*   - Exception handling frames (for C++ exceptions)
     */
    /DISCARD/ : {
        *(.comment)
        *(.note*)
        *(.eh_frame*)
    }
}
