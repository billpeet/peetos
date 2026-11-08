; ===========================================================================
; PeetOS Kernel Entry Point - entry.asm
; ===========================================================================
;
; Tell the linker we don't need an executable stack
; This suppresses the linker warning about missing .note.GNU-stack
section .note.GNU-stack noalloc noexec nowrite progbits

; ===========================================================================
;
; WHAT IS THIS FILE?
; ==================
; This is the entry point for the kernel. When the bootloader finishes
; transitioning to 64-bit Long Mode, it jumps here.
;
; This file is written in assembly because we need precise control over
; the early kernel environment before we can call Rust code.
;
; WHAT DOES IT DO?
; ================
; 1. Sets up a proper stack for 64-bit operation
; 2. Clears the BSS section (uninitialized data)
; 3. Calls the Rust kernel main function (kmain)
; 4. If kmain returns (it shouldn't), halts the CPU
;
; THE BSS SECTION:
; ================
; In C, global and static variables that are not explicitly initialized
; are placed in the BSS section. The C standard requires these to be
; zero-initialized, but the linker/loader doesn't actually store zeros
; in the binary - that would waste space. Instead, the BSS section is
; simply a reserved area that must be zeroed at runtime.
;
; It's our responsibility to zero it before running Rust code!
;
; ===========================================================================

[BITS 64]                       ; We're in 64-bit Long Mode

; ---------------------------------------------------------------------------
; EXTERNAL SYMBOLS
; ---------------------------------------------------------------------------
; These symbols are defined in the linker script or C code.

extern kmain                    ; The C kernel main function

; These are defined in the linker script (kernel.ld)
extern __bss_start              ; Start address of BSS section
extern __bss_end                ; End address of BSS section

; ---------------------------------------------------------------------------
; EXPORTED SYMBOLS
; ---------------------------------------------------------------------------

global _start                   ; Export our entry point

; ===========================================================================
; CODE SECTION
; ===========================================================================

section .text

; ===========================================================================
; _start - Kernel Entry Point
; ===========================================================================
; This is the first kernel code executed.
; The bootloader has set up:
;   - 64-bit Long Mode
;   - Identity mapping of first 2 MB
;   - Basic stack (but we'll set up our own)
; ===========================================================================

_start:
    ; -----------------------------------------------------------------------
    ; STEP 1: Enter with interrupts disabled and clear direction flag
    ; -----------------------------------------------------------------------
    ; Keep IRQs disabled during early init. We only enable them in Rust after
    ; IDT/PIC/PIT/tasking are fully initialized.
    cli                         ; Disable maskable interrupts

    ; The direction flag (DF) controls the direction of string operations.
    ; DF=0: String operations increment (forward)
    ; DF=1: String operations decrement (backward)
    ;
    ; The System V AMD64 ABI expects DF=0, so we ensure that.

    cld                         ; Clear Direction Flag (forward string ops)

    ; -----------------------------------------------------------------------
    ; STEP 1.5: Jump to higher-half and ensure cache coherency
    ; -----------------------------------------------------------------------
    ; CRITICAL FIX: The bootloader jumped to us at the identity-mapped
    ; address (0x100000). We need to immediately jump to our higher-half
    ; virtual address to fix cache aliasing.
    ;
    ; Note: We keep the identity mapping because the framebuffer uses it,
    ; but jumping to higher-half ensures all kernel code/data access uses
    ; the higher-half mapping consistently.

    mov rax, .higher_half       ; Load absolute address (sign-extended imm64)
    jmp rax                     ; Jump to higher-half

.higher_half:
    ; Now executing at higher-half virtual address!

    ; -----------------------------------------------------------------------
    ; STEP 2: Clear the BSS section
    ; -----------------------------------------------------------------------
    ; The BSS section contains uninitialized global/static variables that
    ; must be zeroed by the C standard. The linker provides __bss_start and
    ; __bss_end symbols marking the range to clear.
    ;
    ; IMPORTANT: We do NOT clear the .data section! Variables initialized
    ; to non-zero values (like pml4_addr = 0xDEADBEEFDEADBEEF in vmm.c)
    ; are in .data and are loaded from disk with their initialized values.
    ;
    ; Using linker symbols instead of hardcoded addresses ensures this code
    ; remains correct even when the kernel size changes.

    ; Calculate BSS size and clear it. We clear both full qwords and
    ; the trailing 0..7 bytes so every .bss byte is initialized.
    mov rdi, __bss_start        ; Start address (from linker script)
    mov rcx, __bss_end          ; End address (from linker script)
    sub rcx, rdi                ; Total size in bytes
    mov rdx, rcx                ; Save byte count for tail clear
    xor rax, rax                ; Value to write (0)
    shr rcx, 3                  ; Convert to qwords (divide by 8)
    rep stosq                   ; Store RAX to [RDI], repeat RCX times
    mov rcx, rdx
    and rcx, 7                  ; Remaining bytes
    rep stosb

    ; Memory barrier to ensure all writes complete before proceeding
    mfence

    ; -----------------------------------------------------------------------
    ; STEP 3: Unmap identity mapping
    ; -----------------------------------------------------------------------
    ; Unmap the identity mapping (PML4[0]) now that we're running from higher-half.
    ; This prevents accidental use of low addresses and frees up the lower half
    ; of the address space for future user processes.

    mov rax, 0xFFFFFFFF80010000  ; PML4 in higher-half (physical 0x10000)
    mov qword [rax], 0           ; Clear PML4[0] (unmap identity mapping)

    ; Flush TLB to remove stale entries
    mov rax, cr3
    mov cr3, rax

    ; -----------------------------------------------------------------------
    ; STEP 4: Set up the kernel stack
    ; -----------------------------------------------------------------------
    ; We need a proper stack for C function calls.
    ; The stack grows downward, so RSP points to the top (highest address).
    ; We reserve 64 KB for the stack to cover early Rust formatting/logging
    ; and exception-report paths before task stacks are online.
    ;
    ; Note: The x86_64 ABI requires the stack to be 16-byte aligned
    ; before a CALL instruction. Our stack_top is aligned in the BSS.

    mov rsp, stack_top          ; Set stack pointer

    ; -----------------------------------------------------------------------
    ; STEP 4: Enable SSE/SSE2 for user-space programs
    ; -----------------------------------------------------------------------
    ; musl libc and many user programs use SSE instructions by default.
    ; We need to enable SSE support in the CPU:
    ;   - CR0.EM (bit 2) = 0: Disable x87 emulation mode
    ;   - CR0.MP (bit 1) = 1: Monitor coprocessor (enables WAIT/FWAIT)
    ;   - CR4.OSFXSR (bit 9) = 1: Enable FXSAVE/FXRSTOR for SSE state
    ;   - CR4.OSXMMEXCPT (bit 10) = 1: Enable unmasked SSE exceptions

    ; Modify CR0: clear EM (bit 2), set MP (bit 1)
    mov rax, cr0
    and ax, 0xFFFB              ; Clear bit 2 (EM)
    or ax, 0x0002               ; Set bit 1 (MP)
    mov cr0, rax

    ; Modify CR4: set OSFXSR (bit 9) and OSXMMEXCPT (bit 10)
    mov rax, cr4
    or ax, (1 << 9) | (1 << 10) ; Set OSFXSR and OSXMMEXCPT
    mov cr4, rax

    ; -----------------------------------------------------------------------
    ; STEP 5: Call the Rust kernel main function
    ; -----------------------------------------------------------------------
    ; Now that we have a stack and BSS is cleared, we can call Rust code!
    ;
    ; Note: We don't pass any arguments to kmain(). If we wanted to pass
    ; boot information, we'd put it in RDI, RSI, etc. (System V AMD64 ABI).

    call kmain                  ; Call the Rust kernel

    ; -----------------------------------------------------------------------
    ; STEP 6: Halt if kmain returns
    ; -----------------------------------------------------------------------
    ; kmain() should never return - it should run an infinite loop.
    ; But if it does, we halt the CPU.

.hang:
    cli                         ; Disable interrupts
    hlt                         ; Halt CPU
    jmp .hang                   ; Loop forever (in case of NMI)

; ===========================================================================
; DATA SECTION - Read-Only Data
; ===========================================================================

section .rodata

; No read-only data for now

; ===========================================================================
; BSS SECTION - Uninitialized Data
; ===========================================================================
; This section is for uninitialized global variables.
; It's zeroed by our code above before kmain() is called.
; ===========================================================================

section .bss

; Align the stack to 16 bytes (required by x86_64 ABI)
align 16

; Reserve space for the stack.
; Remember: stack grows DOWN, so stack_top is at the HIGH address.

stack_bottom:                   ; Lowest address of stack (never used directly)
    resb 65536                  ; Reserve 64 KB
stack_top:                      ; Highest address of stack (initial RSP value)

; ===========================================================================
; END OF ENTRY.ASM
; ===========================================================================
; At this point, we've:
;   1. Set up a 64 KB stack
;   2. Cleared the BSS section
;   3. Called kmain()
;
; The kernel is now running in Rust!
; See kernel/src/main.rs for what happens next.
; ===========================================================================

; ===========================================================================
; APPLICATION PROCESSOR (AP) TRAMPOLINE
; ===========================================================================
;
; Application processors wake up in 16-bit real mode at the page named by the
; STARTUP IPI vector. The kernel copies the bytes between ap_trampoline_start
; and ap_trampoline_end to physical AP_TRAMPOLINE_BASE and fills the data
; block at AP_DATA before each STARTUP IPI (see kernel/src/smp.rs).
;
; Steps: real mode -> 32-bit protected mode (temporary GDT) -> enable PAE,
; load transitional page tables (identity-maps the low 2 MB and shares the
; kernel's PML4[511]), set EFER.LME/NXE, enable paging -> 64-bit mode ->
; jump to ap_high_entry in the higher half, which switches to the real
; kernel CR3 and calls the Rust ap_rust_entry(cpu_index).
;
; The code is position dependent: every address is computed relative to
; AP_TRAMPOLINE_BASE with the TRAMP() macro. It lives in .rodata because it
; only ever executes from the low-memory copy.
; ===========================================================================

AP_TRAMPOLINE_BASE  equ 0x8000
AP_DATA             equ AP_TRAMPOLINE_BASE + 0xF00
AP_DATA_PML4        equ AP_DATA + 0x00  ; u32: transitional PML4 (phys, < 4 GB)
AP_DATA_KERNEL_CR3  equ AP_DATA + 0x08  ; u64: kernel CR3
AP_DATA_STACK_TOP   equ AP_DATA + 0x10  ; u64: higher-half stack top
AP_DATA_CPU_INDEX   equ AP_DATA + 0x18  ; u64: logical CPU index
AP_DATA_CR0         equ AP_DATA + 0x20  ; u32: BSP CR0 (includes PE|PG)
AP_DATA_CR4         equ AP_DATA + 0x28  ; u32: BSP CR4 (includes PAE)
AP_DATA_EFER        equ AP_DATA + 0x30  ; u32: EFER bits to set

%define TRAMP(label) ((label) - ap_trampoline_start + AP_TRAMPOLINE_BASE)

extern ap_rust_entry
global ap_trampoline_start
global ap_trampoline_end

section .rodata.ap_trampoline progbits alloc noexec nowrite align=16
align 16

[BITS 16]
ap_trampoline_start:
    cli
    cld
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax

    lgdt [TRAMP(ap_gdtr)]

    mov eax, cr0
    or eax, 1                       ; CR0.PE
    mov cr0, eax
    jmp dword 0x08:TRAMP(ap_pm32)

[BITS 32]
ap_pm32:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax

    mov eax, [AP_DATA_CR4]          ; PAE, OSFXSR, OSXMMEXCPT, ... as on BSP
    mov cr4, eax

    mov eax, [AP_DATA_PML4]
    mov cr3, eax

    mov ecx, 0xC0000080             ; IA32_EFER
    rdmsr
    or eax, [AP_DATA_EFER]          ; LME (+ NXE, SCE)
    wrmsr

    mov eax, [AP_DATA_CR0]          ; PG|PE plus the BSP's FPU bits
    mov cr0, eax

    jmp 0x18:TRAMP(ap_lm64)

[BITS 64]
ap_lm64:
    xor eax, eax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax

    fninit

    mov rdi, [AP_DATA_CPU_INDEX]
    mov rsi, [AP_DATA_KERNEL_CR3]
    mov rsp, [AP_DATA_STACK_TOP]
    mov rax, ap_high_entry          ; absolute higher-half address
    jmp rax

align 16
ap_gdt:
    dq 0                            ; 0x00 null
    dq 0x00CF9A000000FFFF           ; 0x08 32-bit code
    dq 0x00CF92000000FFFF           ; 0x10 32-bit data
    dq 0x00AF9A000000FFFF           ; 0x18 64-bit code
ap_gdt_end:

ap_gdtr:
    dw ap_gdt_end - ap_gdt - 1
    dd TRAMP(ap_gdt)

ap_trampoline_end:

; ---------------------------------------------------------------------------
; ap_high_entry - first higher-half code on an AP
; rdi = cpu index, rsi = kernel CR3, rsp = per-AP kernel stack (higher half)
; ---------------------------------------------------------------------------
section .text
[BITS 64]
ap_high_entry:
    mov cr3, rsi                    ; drop the transitional identity map
    xor ebp, ebp
    call ap_rust_entry              ; never returns
.ap_hang:
    cli
    hlt
    jmp .ap_hang
