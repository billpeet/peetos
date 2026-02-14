; ===========================================================================
; PeetOS Boot Constants - constants.inc
; ===========================================================================
; This file contains all the memory addresses and constants used during
; the boot process. Having them in one place makes it easy to understand
; the memory layout and modify addresses if needed.
;
; MEMORY MAP DURING BOOT:
; =======================
; 0x00000000 - 0x000003FF : Real Mode Interrupt Vector Table (IVT)
; 0x00000400 - 0x000004FF : BIOS Data Area
; 0x00000500 - 0x00007BFF : Free memory (we use for stack)
; 0x00007C00 - 0x00007DFF : Stage 1 Bootloader (MBR, loaded by BIOS)
; 0x00007E00 - 0x0000FFFF : Stage 2 Bootloader area
; 0x00010000 - 0x00013FFF : Page Tables (16 KB for 4-level paging)
; 0x00018000 - 0x000B7FFF : Kernel staging area (up to 640 KB)
; 0x000A0000 - 0x000BFFFF : VGA Video Memory
; 0x000B8000 - 0x000B8F9F : VGA Text Mode Buffer (80x25 characters)
; 0x000C0000 - 0x000FFFFF : BIOS ROM
; 0x00100000+             : Extended Memory (kernel final destination)
; ===========================================================================

; ---------------------------------------------------------------------------
; Memory Map (E820) Constants
; ---------------------------------------------------------------------------
; The BIOS E820 memory map is stored here for the kernel to read.
; Each entry is 24 bytes. We support up to 32 entries (768 bytes = 0x300).
; We use the free memory area at 0x500-0x7BFF (before Stage 1).
MMAP_ADDR           equ 0x500       ; Memory map entries start here
MMAP_COUNT_ADDR     equ 0x7F0       ; Number of entries stored here (16-bit)
MMAP_MAX_ENTRIES    equ 32          ; Maximum entries we'll store

; E820 memory types
MMAP_TYPE_USABLE    equ 1           ; Usable RAM
MMAP_TYPE_RESERVED  equ 2           ; Reserved by BIOS
MMAP_TYPE_ACPI      equ 3           ; ACPI reclaimable
MMAP_TYPE_NVS       equ 4           ; ACPI NVS
MMAP_TYPE_BAD       equ 5           ; Bad memory

; ---------------------------------------------------------------------------
; Stage 1 (MBR) Constants
; ---------------------------------------------------------------------------
; The BIOS loads the first sector (512 bytes) of the boot drive to this
; address and jumps to it. This is where our Stage 1 bootloader lives.
STAGE1_LOAD_ADDR    equ 0x7C00

; Boot signature that must be at bytes 510-511 of the MBR.
; The BIOS checks for this signature to identify a bootable disk.
BOOT_SIGNATURE      equ 0xAA55

; ---------------------------------------------------------------------------
; Stage 2 Constants
; ---------------------------------------------------------------------------
; Stage 2 is loaded immediately after the MBR.
; We load it at 0x7E00 which is right after the 512-byte MBR.
STAGE2_LOAD_ADDR    equ 0x7E00

; Segment address for Stage 2 (0x7E00 / 16 = 0x07E0)
; In Real Mode, addresses are calculated as: segment * 16 + offset
STAGE2_SEGMENT      equ 0x07E0

; First disk sector to read for Stage 2.
; Sector 1 is the MBR (0-indexed: sector 0), so Stage 2 starts at sector 2.
; Note: BIOS uses 1-indexed sectors, so sector 2 in BIOS terms.
STAGE2_START_SECTOR equ 2

; Number of sectors to read for Stage 2.
; We'll allocate 16 sectors (8 KB) for Stage 2, which is plenty for
; all the mode transition code.
STAGE2_SECTORS      equ 16

; ---------------------------------------------------------------------------
; Kernel Constants
; ---------------------------------------------------------------------------
; Temporary location where we load the kernel from disk.
; This stays in conventional memory and below VGA memory.
KERNEL_LOAD_ADDR    equ 0x18000

; Segment for kernel loading (0x18000 / 16 = 0x1800)
KERNEL_SEGMENT      equ 0x1800

; Disk sector where kernel starts.
; MBR = sector 1, Stage 2 = sectors 2-17, Kernel = sector 18+
KERNEL_START_SECTOR equ 18

; Number of sectors to read for the kernel.
; 1280 sectors = 640 KB.
; This keeps the staging buffer below 0xB8000 (VGA text buffer), leaving
; room for kernel growth while still staying below the relocation gap to
; KERNEL_START.
; Note: Must be large enough to include .data section (check build/kernel.bin).
KERNEL_SECTORS      equ 1280

; Final kernel address after relocation (1 MB mark).
; This is in extended memory, beyond the legacy 640KB limit.
KERNEL_START        equ 0x100000

; ---------------------------------------------------------------------------
; Page Table Addresses
; ---------------------------------------------------------------------------
; Long Mode requires 4-level paging. Each table is 4KB (512 entries * 8 bytes).
; We place them consecutively starting at 0x10000.
;
; Paging hierarchy:
; PML4 -> PDPT -> PDT -> PT -> Physical Page
;
; Each entry at levels 1-3 points to the next level table.
; Each PT entry points to a 4KB physical page.

; Page Map Level 4 - Top of the hierarchy
PML4_ADDR           equ 0x10000

; Page Directory Pointer Table
PDPT_ADDR           equ 0x11000

; Page Directory Table
PDT_ADDR            equ 0x12000

; Page Table (maps 4KB pages) - for low memory identity mapping
PT_ADDR             equ 0x13000

; Higher-half kernel page tables
; The kernel will be mapped at 0xFFFFFFFF80000000 (canonical high address)
; This requires: PML4[511] -> PDPT_HIGH -> PD_HIGH -> physical kernel
PDPT_HIGH_ADDR      equ 0x14000     ; PDPT for higher-half (PML4 entry 511)
PD_HIGH_ADDR        equ 0x15000     ; PD for higher-half

; Higher-half kernel virtual address
; This is the canonical address 0xFFFFFFFF80000000
; In the linker script, the kernel will be linked at this address
KERNEL_VIRT_BASE    equ 0xFFFFFFFF80000000

; Page flags (present in all page table entries)
PAGE_PRESENT        equ (1 << 0)    ; Page is present in memory
PAGE_WRITE          equ (1 << 1)    ; Page is writable
PAGE_USER           equ (1 << 2)    ; Page is accessible from user mode

; ---------------------------------------------------------------------------
; Stack Configuration
; ---------------------------------------------------------------------------
; We set the stack pointer to 0x7C00, which means the stack grows downward
; from there. This gives us about 30KB of stack space (0x7C00 - 0x500).
STACK_TOP           equ 0x7C00

; 64-bit stack for Long Mode kernel (grows down from here)
STACK_TOP_64        equ 0x90000

; ---------------------------------------------------------------------------
; VGA Text Mode
; ---------------------------------------------------------------------------
; The VGA text buffer is a memory-mapped I/O region at 0xB8000.
; Each character cell is 2 bytes: 1 byte for the character, 1 for attributes.
; Attributes: low nibble = foreground color, high nibble = background color
VGA_TEXT_BUFFER     equ 0xB8000
VGA_WIDTH           equ 80          ; Characters per line
VGA_HEIGHT          equ 25          ; Lines on screen

; VGA colors (for attribute byte)
VGA_BLACK           equ 0x0
VGA_BLUE            equ 0x1
VGA_GREEN           equ 0x2
VGA_CYAN            equ 0x3
VGA_RED             equ 0x4
VGA_MAGENTA         equ 0x5
VGA_BROWN           equ 0x6
VGA_LIGHT_GREY      equ 0x7
VGA_DARK_GREY       equ 0x8
VGA_LIGHT_BLUE      equ 0x9
VGA_LIGHT_GREEN     equ 0xA
VGA_LIGHT_CYAN      equ 0xB
VGA_LIGHT_RED       equ 0xC
VGA_LIGHT_MAGENTA   equ 0xD
VGA_YELLOW          equ 0xE
VGA_WHITE           equ 0xF

; Default text attribute: white text on black background
VGA_DEFAULT_ATTR    equ ((VGA_BLACK << 4) | VGA_WHITE)

; ---------------------------------------------------------------------------
; CPU Control Registers
; ---------------------------------------------------------------------------
; CR0 bits
CR0_PE              equ (1 << 0)    ; Protection Enable (enter Protected Mode)
CR0_MP              equ (1 << 1)    ; Monitor coprocessor
CR0_EM              equ (1 << 2)    ; Emulation (0 = FPU present)
CR0_NE              equ (1 << 5)    ; Numeric Error (native FPU error reporting)
CR0_PG              equ (1 << 31)   ; Paging Enable

; CR4 bits
CR4_PAE             equ (1 << 5)    ; Physical Address Extension (required for Long Mode)
CR4_OSFXSR          equ (1 << 9)    ; Enable FXSAVE/FXRSTOR instructions
CR4_OSXMMEXCPT      equ (1 << 10)   ; Enable unmasked SSE exceptions

; EFER MSR (Extended Feature Enable Register)
EFER_MSR            equ 0xC0000080  ; MSR number for EFER
EFER_LME            equ (1 << 8)    ; Long Mode Enable bit
EFER_NXE            equ (1 << 11)   ; No Execute Enable bit (required for NX page table bit)

; ---------------------------------------------------------------------------
; GDT Selectors
; ---------------------------------------------------------------------------
; These are offsets into the GDT. Each GDT entry is 8 bytes.
; Selector = entry_number * 8
;
; For 32-bit GDT (gdt32):
GDT_NULL_SEL        equ 0x00        ; Null descriptor (required, never used)
GDT_CODE32_SEL      equ 0x08        ; 32-bit code segment
GDT_DATA32_SEL      equ 0x10        ; 32-bit data segment
;
; For 64-bit GDT (gdt64) - this is a SEPARATE GDT, so selectors restart!
; Entry 0: Null (0x00)
; Entry 1: Code (0x08)
; Entry 2: Data (0x10)
GDT_CODE64_SEL      equ 0x08        ; 64-bit code segment
GDT_DATA64_SEL      equ 0x10        ; 64-bit data segment

; ---------------------------------------------------------------------------
; VESA/VBE Graphics Mode Constants
; ---------------------------------------------------------------------------
; VESA BIOS Extensions (VBE) allow setting higher resolutions and accessing
; a linear framebuffer for graphics output.
;
; Memory Layout for VESA data:
; 0x00000800 - 0x000009FF : VBE Controller Info (512 bytes)
; 0x00000A00 - 0x00000AFF : VBE Mode Info (256 bytes)
; 0x00000B00 - 0x00000B07 : VESA Status (8 bytes)

; VBE information addresses (stored by bootloader for kernel)
VESA_INFO_ADDR          equ 0x800       ; VBE Controller Info (512 bytes)
VESA_MODE_INFO_ADDR     equ 0xA00       ; VBE Mode Info (256 bytes)
VESA_STATUS_ADDR        equ 0xB00       ; Status flags (8 bytes)

; VESA status structure at VESA_STATUS_ADDR:
;   Byte 0:     Available (1 = VESA mode set, 0 = VGA fallback)
;   Byte 1:     Reserved
;   Bytes 2-3:  VBE mode number (0 if VGA fallback)
;   Bytes 4-7:  Framebuffer physical address

; Desired VESA mode parameters
VESA_DESIRED_WIDTH      equ 1024        ; Target width in pixels
VESA_DESIRED_HEIGHT     equ 768         ; Target height in pixels
VESA_DESIRED_BPP        equ 32          ; Target bits per pixel (32-bit ARGB)

; VBE function codes (passed in AX to INT 0x10)
VBE_GET_CONTROLLER_INFO equ 0x4F00      ; Get VBE controller info
VBE_GET_MODE_INFO       equ 0x4F01      ; Get VBE mode info
VBE_SET_MODE            equ 0x4F02      ; Set VBE mode

; VBE mode flags (OR'd with mode number)
VBE_MODE_LFB_BIT        equ 0x4000      ; Use Linear Frame Buffer
VBE_MODE_CLEAR_BIT      equ 0x8000      ; Don't clear display memory

; VBE Mode Info Block offsets (structure at VESA_MODE_INFO_ADDR)
VBE_MODE_ATTRIBUTES     equ 0           ; Mode attributes (word)
VBE_MODE_PITCH          equ 16          ; Bytes per scanline (word)
VBE_MODE_WIDTH          equ 18          ; Width in pixels (word)
VBE_MODE_HEIGHT         equ 20          ; Height in pixels (word)
VBE_MODE_BPP            equ 25          ; Bits per pixel (byte)
VBE_MODE_PHYS_BASE      equ 40          ; Framebuffer physical address (dword)

; Additional page table for framebuffer mapping
; Used when framebuffer is at high physical address (e.g., 0xFD000000)
PDPT_FB_ADDR            equ 0x16000     ; PDPT for framebuffer (if needed)
PD_FB_ADDR              equ 0x17000     ; PD for framebuffer
