; ===========================================================================
; PeetOS Stage 2 Bootloader - stage2.asm
; ===========================================================================
;
; WHAT IS THIS FILE?
; ==================
; This is Stage 2 of our bootloader. Stage 1 (the MBR) loaded this code
; into memory at 0x7E00 and jumped here.
;
; Stage 2 is responsible for the critical task of transitioning the CPU
; from 16-bit Real Mode all the way to 64-bit Long Mode.
;
; THE JOURNEY TO LONG MODE:
; =========================
;
;   ┌─────────────────────────────────────────────────────────────┐
;   │                    16-bit Real Mode                         │
;   │  • Enable A20 line (access memory > 1 MB)                   │
;   │  • Load kernel from disk                                    │
;   │  • Load 32-bit GDT                                          │
;   │  • Set CR0.PE = 1 (Protection Enable)                       │
;   │  • Far jump to 32-bit code segment                          │
;   └─────────────────────────────────────────────────────────────┘
;                              │
;                              ▼
;   ┌─────────────────────────────────────────────────────────────┐
;   │                   32-bit Protected Mode                      │
;   │  • Set up 32-bit segment registers                          │
;   │  • Set up page tables (identity map first 128 MB)            │
;   │  • Enable PAE (CR4.PAE = 1)                                 │
;   │  • Load CR3 with PML4 address                               │
;   │  • Enable Long Mode (EFER.LME = 1)                          │
;   │  • Enable Paging (CR0.PG = 1) - this activates Long Mode    │
;   │  • Load 64-bit GDT                                          │
;   │  • Far jump to 64-bit code segment                          │
;   └─────────────────────────────────────────────────────────────┘
;                              │
;                              ▼
;   ┌─────────────────────────────────────────────────────────────┐
;   │                    64-bit Long Mode                          │
;   │  • Set up 64-bit segment registers                          │
;   │  • Set up 64-bit stack                                      │
;   │  • Jump to kernel                                           │
;   └─────────────────────────────────────────────────────────────┘
;
; ===========================================================================

; ---------------------------------------------------------------------------
; 16-BIT REAL MODE SECTION
; ---------------------------------------------------------------------------
; We start here, fresh from Stage 1's jump instruction.
; ---------------------------------------------------------------------------

[BITS 16]                       ; Still in 16-bit Real Mode
[ORG 0x7E00]                    ; Stage 2 is loaded at 0x7E00

; Include our constants
%include "boot/include/constants.inc"

; ===========================================================================
; STAGE 2 ENTRY POINT
; ===========================================================================

stage2_start:
    ; -----------------------------------------------------------------------
    ; STEP 1: Set up segment registers
    ; -----------------------------------------------------------------------
    ; Stage 1 jumped here, but segment registers might not be what we expect.
    ; Let's set them explicitly.

    xor ax, ax                  ; AX = 0
    mov ds, ax                  ; DS = 0 (Data Segment)
    mov es, ax                  ; ES = 0 (Extra Segment)
    mov ss, ax                  ; SS = 0 (Stack Segment)
    mov sp, STACK_TOP           ; SP = 0x7C00 (Stack grows down from here)

    ; -----------------------------------------------------------------------
    ; STEP 1.5: Initialize serial port for debugging
    ; -----------------------------------------------------------------------
    call serial_init_16

    ; -----------------------------------------------------------------------
    ; STEP 2: Print welcome message (to both VGA/BIOS and serial)
    ; -----------------------------------------------------------------------

    mov si, msg_stage2
    call print_string_16
    call serial_print_16

    ; -----------------------------------------------------------------------
    ; STEP 3: Enable A20 line
    ; -----------------------------------------------------------------------
    ; This allows us to access memory above 1 MB.
    ; Without this, our kernel at 0x100000 would be inaccessible!

    mov si, msg_a20
    call print_string_16
    call serial_print_16

    call enable_a20             ; Returns CF=1 on failure
    jc a20_failed               ; Jump if A20 enable failed

    mov si, msg_done
    call print_string_16
    call serial_print_16

    ; -----------------------------------------------------------------------
    ; STEP 4: Detect available memory (E820)
    ; -----------------------------------------------------------------------
    ; We need to detect memory using BIOS before entering Protected Mode,
    ; since BIOS calls only work in Real Mode.

    mov si, msg_detect_mem
    call print_string_16
    call serial_print_16

    call detect_memory          ; Detect memory using INT 15h, E820h
    jc memory_detect_failed     ; Jump if detection failed

    mov si, msg_done
    call print_string_16
    call serial_print_16

    ; Print memory map to serial for debugging
    call print_memory_map

    ; -----------------------------------------------------------------------
    ; STEP 4.5: Initialize VESA/VBE Graphics
    ; -----------------------------------------------------------------------
    ; Attempt to set 1024x768x32 graphics mode using VBE.
    ; Falls back to VGA text mode if VESA is not available.
    ; MUST be done before Protected Mode (BIOS INT 0x10 required).

    mov si, msg_vesa_init
    call print_string_16
    call serial_print_16

    call vesa_init                  ; Detect and set VESA mode
    ; vesa_init never fails fatally - it falls back to VGA

    mov si, msg_done
    call print_string_16
    call serial_print_16

    ; -----------------------------------------------------------------------
    ; STEP 5: Check for Long Mode support (renumbered from STEP 4)
    ; -----------------------------------------------------------------------
    ; Not all x86 CPUs support Long Mode (64-bit).
    ; We need to check before trying to enable it.

    mov si, msg_check_lm
    call print_string_16
    call serial_print_16

    call check_long_mode_support
    jc no_long_mode             ; Jump if Long Mode not supported

    mov si, msg_done
    call print_string_16
    call serial_print_16

    ; -----------------------------------------------------------------------
    ; STEP 6: Load kernel from disk
    ; -----------------------------------------------------------------------
    ; The kernel is stored on disk after Stage 2.
    ; We load it into memory at KERNEL_LOAD_ADDR (0x18000) as a staging buffer.
    ; It will be relocated to KERNEL_START (0x100000) in 32-bit mode.

    mov si, msg_load_kernel
    call print_string_16
    call serial_print_16

    call load_kernel
    jc kernel_load_failed

    mov si, msg_done
    call print_string_16
    call serial_print_16

    ; -----------------------------------------------------------------------
    ; STEP 7: Enter Protected Mode
    ; -----------------------------------------------------------------------
    ; Now we're ready to leave Real Mode behind!

    mov si, msg_protected
    call print_string_16
    call serial_print_16

    ; Disable interrupts - we're about to change CPU modes!
    cli

    ; Load the 32-bit Global Descriptor Table (GDT)
    lgdt [gdt32_descriptor]

    ; Enable Protected Mode by setting bit 0 of CR0
    mov eax, cr0
    or eax, CR0_PE              ; Set Protection Enable bit
    mov cr0, eax

    ; Far jump to 32-bit code!
    ; This flushes the instruction pipeline and loads CS with the 32-bit
    ; code segment selector (0x08 = second GDT entry).
    jmp GDT_CODE32_SEL:protected_mode_start

; ===========================================================================
; ERROR HANDLERS (16-bit)
; ===========================================================================

a20_failed:
    mov si, msg_a20_failed
    call print_string_16
    jmp halt_16

no_long_mode:
    mov si, msg_no_long_mode
    call print_string_16
    jmp halt_16

kernel_load_failed:
    mov si, msg_kernel_failed
    call print_string_16
    call serial_print_16
    jmp halt_16

memory_detect_failed:
    mov si, msg_mem_failed
    call print_string_16
    jmp halt_16

halt_16:
    cli
    hlt
    jmp halt_16

; ===========================================================================
; SUBROUTINE: check_long_mode_support
; ===========================================================================
; Checks if the CPU supports Long Mode (64-bit).
;
; Uses CPUID instruction to query CPU capabilities.
; CPUID with EAX=0x80000000 returns the highest extended function.
; CPUID with EAX=0x80000001 returns extended features (including LM bit).
;
; Output:
;   CF = 0 if Long Mode supported
;   CF = 1 if Long Mode NOT supported
; ===========================================================================

check_long_mode_support:
    ; First, check if CPUID is supported
    ; We can detect CPUID by trying to flip the ID bit (bit 21) in EFLAGS.
    ; If we can flip it, CPUID is supported.

    pushfd                      ; Push EFLAGS
    pop eax                     ; EAX = EFLAGS
    mov ecx, eax                ; Save original EFLAGS

    xor eax, 1 << 21            ; Flip ID bit
    push eax
    popfd                       ; Load modified EFLAGS

    pushfd
    pop eax                     ; EAX = new EFLAGS
    push ecx
    popfd                       ; Restore original EFLAGS

    cmp eax, ecx                ; Did the bit flip stick?
    je .no_cpuid                ; If equal, bit didn't flip, no CPUID

    ; CPUID is supported. Check for extended functions.
    mov eax, 0x80000000         ; Query highest extended function
    cpuid
    cmp eax, 0x80000001         ; Must support at least 0x80000001
    jb .no_long_mode            ; If not, no Long Mode

    ; Check for Long Mode bit
    mov eax, 0x80000001         ; Query extended features
    cpuid
    test edx, 1 << 29           ; Test bit 29 (Long Mode)
    jz .no_long_mode            ; If zero, no Long Mode

    ; Long Mode is supported!
    clc                         ; Clear carry (success)
    ret

.no_cpuid:
.no_long_mode:
    stc                         ; Set carry (failure)
    ret

; ===========================================================================
; SUBROUTINE: load_kernel
; ===========================================================================
; Loads the kernel from disk using BIOS INT 13h.
;
; Output:
;   CF = 0 on success
;   CF = 1 on failure
; ===========================================================================

load_kernel:
    ; We need to read multiple sectors. BIOS INT 13h AH=02h can read up to
    ; 127 sectors, but we read in smaller chunks to avoid track boundary issues.
    ;
    ; We use INT 13h AH=02h with CHS addressing, reading 32 sectors at a time.
    ; The drive geometry is assumed to be 63 sectors/track, 16 heads (standard HD).
    ;
    ; LBA to CHS conversion:
    ;   Sector = (LBA mod 63) + 1
    ;   Head = (LBA / 63) mod 16
    ;   Cylinder = LBA / (63 * 16) = LBA / 1008

    ; Initialize tracking variables
    mov word [.sectors_remaining], KERNEL_SECTORS
    mov word [.current_lba], KERNEL_START_SECTOR - 1   ; LBA 17 (0-indexed)
    mov word [.dest_offset], 0
    mov word [.dest_segment], KERNEL_SEGMENT

.read_loop:
    ; Check if we're done
    mov ax, [.sectors_remaining]
    cmp ax, 0
    je .success

    ; Calculate sectors to read this iteration
    ; Read up to 32 sectors at a time, but don't cross track boundary
    mov bx, [.current_lba]

    ; Calculate sectors remaining in current track
    mov ax, bx                      ; AX = current LBA
    xor dx, dx
    mov cx, 63                      ; SPT = 63
    div cx                          ; AX = LBA / 63, DX = LBA mod 63
    ; DX = sector - 1 (0-indexed within track)
    ; Sectors remaining in track = 63 - DX
    mov ax, 63
    sub ax, dx                      ; AX = sectors remaining in track

    ; Take minimum of: sectors remaining in track, 32, total remaining
    cmp ax, 32
    jle .check_total
    mov ax, 32                      ; Limit to 32 sectors per read
.check_total:
    cmp ax, [.sectors_remaining]
    jle .use_count
    mov ax, [.sectors_remaining]    ; Don't read more than remaining
.use_count:
    mov [.sectors_this_read], ax    ; Save count for this read

    ; Convert current LBA to CHS
    ; LBA is in BX (we saved it above)
    mov ax, bx                      ; AX = LBA
    xor dx, dx
    mov cx, 63                      ; SPT = 63
    div cx                          ; AX = LBA / 63, DX = LBA mod 63
    inc dx                          ; DX = sector number (1-indexed)
    mov [.sector], dl               ; Save sector

    ; AX = LBA / 63 (track number across all heads)
    xor dx, dx
    mov cx, 16                      ; 16 heads
    div cx                          ; AX = cylinder, DX = head
    mov [.head], dl                 ; Save head
    mov [.cylinder], al             ; Save cylinder (low 8 bits)

    ; Set up for INT 13h AH=02h
    mov ax, [.dest_segment]
    mov es, ax
    mov bx, [.dest_offset]          ; ES:BX = destination

    mov al, [.sectors_this_read]    ; AL = number of sectors
    mov ah, 0x02                    ; AH = read sectors function
    mov ch, [.cylinder]             ; CH = cylinder (low 8 bits)
    mov cl, [.sector]               ; CL = sector (bits 0-5)
    mov dh, [.head]                 ; DH = head
    mov dl, 0x80                    ; DL = first hard disk

    int 0x13
    jc .error

    ; Update tracking for next iteration
    mov ax, [.sectors_this_read]
    sub [.sectors_remaining], ax
    add [.current_lba], ax

    ; Advance destination pointer
    mov ax, [.sectors_this_read]
    shl ax, 9                       ; AX = sectors * 512
    add [.dest_offset], ax
    jnc .read_loop                  ; If no overflow, continue

    ; Handle offset overflow: advance segment by 0x1000 (64KB)
    add word [.dest_segment], 0x1000
    jmp .read_loop

.success:
    ; Restore ES to 0
    xor ax, ax
    mov es, ax
    clc                             ; Success
    ret

.error:
    ; Restore ES
    xor ax, ax
    mov es, ax
    stc                             ; Failure
    ret

; Local variables for load_kernel
.sectors_remaining: dw 0
.current_lba:       dw 0
.dest_offset:       dw 0
.dest_segment:      dw 0
.sectors_this_read: dw 0
.sector:            db 0
.head:              db 0
.cylinder:          db 0

; ===========================================================================
; INCLUDE 16-BIT HELPER FILES
; ===========================================================================

%include "boot/stage2/print16.asm"
%include "boot/stage2/a20.asm"
%include "boot/stage2/serial.asm"
%include "boot/stage2/memory.asm"
%include "boot/stage2/vesa.asm"

; ===========================================================================
; 16-BIT DATA
; ===========================================================================

msg_stage2:         db "Stage 2: Entering Long Mode journey...", 13, 10, 0
msg_a20:            db "  Enabling A20 line... ", 0
msg_detect_mem:     db "  Detecting memory (E820)... ", 0
msg_vesa_init:      db "  Initializing VESA graphics... ", 0
msg_check_lm:       db "  Checking Long Mode support... ", 0
msg_load_kernel:    db "  Loading kernel from disk... ", 0
msg_protected:      db "  Switching to Protected Mode...", 13, 10, 0
msg_done:           db "OK", 13, 10, 0
msg_a20_failed:     db "FAILED! A20 could not be enabled.", 13, 10, 0
msg_no_long_mode:   db "FAILED! CPU does not support Long Mode.", 13, 10, 0
msg_kernel_failed:  db "FAILED! Could not load kernel.", 13, 10, 0
msg_mem_failed:     db "FAILED! Memory detection failed.", 13, 10, 0

; ===========================================================================
; 32-BIT GLOBAL DESCRIPTOR TABLE (GDT)
; ===========================================================================
; The GDT defines memory segments for Protected Mode.
;
; Each GDT entry is 8 bytes and has this format:
;
;  ┌───────────────────────────────────────────────────────────────────┐
;  │ Byte 7  │ Byte 6  │ Byte 5  │ Byte 4  │ Bytes 3-2 │ Bytes 1-0  │
;  ├─────────┼─────────┼─────────┼─────────┼───────────┼────────────┤
;  │ Base    │ Flags + │ Access  │ Base    │ Base      │ Limit      │
;  │ [31:24] │ Limit   │ Byte    │ [23:16] │ [15:0]    │ [15:0]     │
;  │         │ [19:16] │         │         │           │            │
;  └───────────────────────────────────────────────────────────────────┘
;
; Access byte bits:
;   Bit 7: Present (P) - must be 1 for valid segment
;   Bits 6-5: DPL (Privilege Level) - 0 = kernel, 3 = user
;   Bit 4: Descriptor type - 1 = code/data, 0 = system
;   Bit 3: Executable - 1 = code segment, 0 = data segment
;   Bit 2: Direction/Conforming
;   Bit 1: Readable (code) / Writable (data)
;   Bit 0: Accessed - CPU sets this
;
; Flags nibble:
;   Bit 3: Granularity - 1 = limit is in 4KB units
;   Bit 2: Size - 1 = 32-bit, 0 = 16-bit
;   Bit 1: Long mode - 1 = 64-bit code (for 64-bit GDT)
;   Bit 0: Reserved
;
; ===========================================================================

align 16                        ; Align GDT for performance
gdt32_start:

; Null descriptor (required - entry 0 cannot be used)
gdt32_null:
    dq 0                        ; 8 bytes of zeros

; Code segment descriptor (selector 0x08)
; Base = 0, Limit = 0xFFFFF (4 GB with granularity)
; Access: Present, Ring 0, Code, Executable, Readable
; Flags: 4KB granularity, 32-bit
gdt32_code:
    dw 0xFFFF                   ; Limit [15:0]
    dw 0x0000                   ; Base [15:0]
    db 0x00                     ; Base [23:16]
    db 10011010b                ; Access byte: P=1, DPL=00, S=1, E=1, DC=0, RW=1, A=0
    db 11001111b                ; Flags (4 bits) + Limit [19:16]: G=1, D=1, L=0, Limit=0xF
    db 0x00                     ; Base [31:24]

; Data segment descriptor (selector 0x10)
; Base = 0, Limit = 0xFFFFF (4 GB with granularity)
; Access: Present, Ring 0, Data, Writable
; Flags: 4KB granularity, 32-bit
gdt32_data:
    dw 0xFFFF                   ; Limit [15:0]
    dw 0x0000                   ; Base [15:0]
    db 0x00                     ; Base [23:16]
    db 10010010b                ; Access byte: P=1, DPL=00, S=1, E=0, DC=0, RW=1, A=0
    db 11001111b                ; Flags + Limit [19:16]
    db 0x00                     ; Base [31:24]

gdt32_end:

; GDT descriptor (loaded with LGDT instruction)
gdt32_descriptor:
    dw gdt32_end - gdt32_start - 1  ; Size of GDT minus 1
    dd gdt32_start                   ; Linear address of GDT

; ---------------------------------------------------------------------------
; 32-BIT PROTECTED MODE SECTION
; ---------------------------------------------------------------------------
; We land here after the far jump from Real Mode.
; ---------------------------------------------------------------------------

[BITS 32]                       ; Now we're in 32-bit mode!

protected_mode_start:
    ; -----------------------------------------------------------------------
    ; STEP 1: Set up 32-bit segment registers
    ; -----------------------------------------------------------------------
    ; After the far jump, CS contains the code segment selector (0x08).
    ; We need to load the data segment selector (0x10) into the other
    ; segment registers.

    mov ax, GDT_DATA32_SEL      ; Data segment selector (0x10)
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax
    mov esp, STACK_TOP          ; Set up stack

    ; -----------------------------------------------------------------------
    ; STEP 1.5: Relocate kernel to 1 MB (extended memory)
    ; -----------------------------------------------------------------------
    ; The kernel is loaded in low memory for BIOS disk I/O, but its linked
    ; physical address is 0x100000. Move the image before enabling paging.

    cld
    mov esi, KERNEL_LOAD_ADDR
    mov edi, KERNEL_START
    mov ecx, KERNEL_SECTORS * 128   ; 512 bytes/sector / 4 bytes = 128 dwords
    rep movsd

    ; -----------------------------------------------------------------------
    ; DEBUG: Print to serial that we made it to 32-bit mode
    ; -----------------------------------------------------------------------
    mov esi, msg_pm_serial
    call serial_print_32

    ; -----------------------------------------------------------------------
    ; STEP 2: Print to VGA (we can't use BIOS anymore!)
    ; -----------------------------------------------------------------------
    ; Now that we're in Protected Mode, BIOS interrupts don't work.
    ; We write directly to VGA text buffer at 0xB8000.

    mov edi, VGA_TEXT_BUFFER    ; VGA buffer address
    mov esi, msg_protected_32
    call print_string_32

    ; Debug: print paging setup message
    mov esi, msg_paging_serial
    call serial_print_32

    ; -----------------------------------------------------------------------
    ; STEP 3: Set up paging
    ; -----------------------------------------------------------------------

    call setup_paging           ; Set up 4-level page tables

    mov esi, msg_paging_done_serial
    call serial_print_32

    ; -----------------------------------------------------------------------
    ; STEP 4: Enable PAE (Physical Address Extension)
    ; -----------------------------------------------------------------------
    ; PAE is required for Long Mode.

    call enable_pae

    mov esi, msg_pae_serial
    call serial_print_32

    ; -----------------------------------------------------------------------
    ; STEP 5: Load CR3 with PML4 address
    ; -----------------------------------------------------------------------

    mov eax, PML4_ADDR
    mov cr3, eax

    mov esi, msg_cr3_serial
    call serial_print_32

    ; -----------------------------------------------------------------------
    ; STEP 6: Enable Long Mode in EFER MSR
    ; -----------------------------------------------------------------------

    call enable_long_mode

    mov esi, msg_lm_serial
    call serial_print_32

    ; -----------------------------------------------------------------------
    ; STEP 7: Enable Paging (this activates Long Mode!)
    ; -----------------------------------------------------------------------

    mov esi, msg_enabling_paging_serial
    call serial_print_32

    mov eax, cr0
    or eax, CR0_PG              ; Set Paging bit
    mov cr0, eax                ; Long Mode is now ACTIVE!

    ; -----------------------------------------------------------------------
    ; STEP 8: Load 64-bit GDT
    ; -----------------------------------------------------------------------

    lgdt [gdt64_descriptor]

    ; -----------------------------------------------------------------------
    ; STEP 9: Far jump to 64-bit code!
    ; -----------------------------------------------------------------------
    ; This loads the 64-bit code segment selector and jumps to 64-bit code.

    jmp GDT_CODE64_SEL:long_mode_start

; ===========================================================================
; 32-BIT SUBROUTINES
; ===========================================================================

; Print string to VGA in 32-bit mode
; Input: ESI = string address, EDI = VGA buffer position
; Output: EDI = updated position
print_string_32:
    push eax
.loop:
    lodsb                       ; Load byte from [ESI] into AL
    test al, al                 ; Check for null terminator
    jz .done
    mov ah, VGA_DEFAULT_ATTR    ; White on black
    stosw                       ; Store character + attribute to [EDI]
    jmp .loop
.done:
    pop eax
    ret

; Include paging setup
%include "boot/stage2/paging.asm"

; ===========================================================================
; 32-BIT DATA
; ===========================================================================

msg_protected_32: db "32-bit Protected Mode active. Setting up Long Mode...", 0

; Serial debug messages
msg_pm_serial: db "[SERIAL] 32-bit Protected Mode entered", 13, 10, 0
msg_paging_serial: db "[SERIAL] Setting up paging...", 13, 10, 0
msg_paging_done_serial: db "[SERIAL] Paging tables set up", 13, 10, 0
msg_pae_serial: db "[SERIAL] PAE enabled", 13, 10, 0
msg_cr3_serial: db "[SERIAL] CR3 loaded with PML4", 13, 10, 0
msg_lm_serial: db "[SERIAL] Long Mode enabled in EFER", 13, 10, 0
msg_enabling_paging_serial: db "[SERIAL] Enabling paging (activating Long Mode)...", 13, 10, 0

; ===========================================================================
; 64-BIT GLOBAL DESCRIPTOR TABLE
; ===========================================================================
; In Long Mode, most segment fields are ignored. Only the following matter:
;   - Present bit (must be 1)
;   - DPL (for privilege checks)
;   - L bit (set to 1 for 64-bit code segment)
;   - D bit (must be 0 when L=1)
;
; ===========================================================================

align 16
gdt64_start:

; Null descriptor
gdt64_null:
    dq 0

; Code segment (selector 0x08)
gdt64_code:
    dw 0x0000                   ; Limit (ignored in Long Mode)
    dw 0x0000                   ; Base (ignored)
    db 0x00                     ; Base (ignored)
    db 10011010b                ; Access: P=1, DPL=00, S=1, E=1, DC=0, RW=1, A=0
    db 00100000b                ; Flags: G=0, D=0, L=1 (64-bit!), Reserved=0
    db 0x00                     ; Base (ignored)

; Data segment (selector 0x10)
gdt64_data:
    dw 0x0000                   ; Limit (ignored)
    dw 0x0000                   ; Base (ignored)
    db 0x00                     ; Base (ignored)
    db 10010010b                ; Access: P=1, DPL=00, S=1, E=0, DC=0, RW=1, A=0
    db 00000000b                ; Flags: All zeros for data segment
    db 0x00                     ; Base (ignored)

gdt64_end:

gdt64_descriptor:
    dw gdt64_end - gdt64_start - 1
    dq gdt64_start              ; Note: 64-bit address!

; ---------------------------------------------------------------------------
; 64-BIT LONG MODE SECTION
; ---------------------------------------------------------------------------
; We've made it! This is 64-bit code.
; ---------------------------------------------------------------------------

[BITS 64]                       ; Now we're in 64-bit mode!

long_mode_start:
    ; -----------------------------------------------------------------------
    ; STEP 1: Set up 64-bit segment registers
    ; -----------------------------------------------------------------------
    ; In Long Mode, segment registers are mostly ignored for addressing,
    ; but we still need to set them to valid selectors.

    mov ax, GDT_DATA64_SEL      ; Data segment selector (0x10)
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax

    ; -----------------------------------------------------------------------
    ; STEP 2: Set up 64-bit stack
    ; -----------------------------------------------------------------------

    mov rsp, STACK_TOP_64       ; Stack pointer

    ; -----------------------------------------------------------------------
    ; STEP 2.5: Enable FPU and SSE
    ; -----------------------------------------------------------------------
    ; Configure CR0 for FPU:
    ;   - Clear EM (bit 2) to indicate FPU is present
    ;   - Set MP (bit 1) for WAIT/FWAIT instructions
    ;   - Set NE (bit 5) for native FPU error handling

    mov rax, cr0
    and rax, ~CR0_EM            ; Clear EM (emulation) bit
    or rax, CR0_MP              ; Set MP (monitor coprocessor) bit
    or rax, CR0_NE              ; Set NE (numeric error) bit
    mov cr0, rax

    ; Configure CR4 for SSE:
    ;   - Set OSFXSR (bit 9) to enable FXSAVE/FXRSTOR
    ;   - Set OSXMMEXCPT (bit 10) to enable SSE exceptions

    mov rax, cr4
    or rax, CR4_OSFXSR          ; Enable FXSAVE/FXRSTOR
    or rax, CR4_OSXMMEXCPT      ; Enable SSE exceptions
    mov cr4, rax

    ; Initialize FPU state
    fninit                      ; Initialize FPU (no exception check)

    ; -----------------------------------------------------------------------
    ; DEBUG: Print to serial that we made it to 64-bit mode
    ; -----------------------------------------------------------------------
    mov rsi, msg_lm64_serial
    call serial_print_64

    ; -----------------------------------------------------------------------
    ; STEP 3: Clear the screen
    ; -----------------------------------------------------------------------

    mov rdi, VGA_TEXT_BUFFER
    mov rcx, VGA_WIDTH * VGA_HEIGHT  ; 80 * 25 = 2000 characters
    mov rax, 0x0F200F20         ; Two space characters with white-on-black
    rep stosq                   ; Clear 8 bytes at a time

    ; -----------------------------------------------------------------------
    ; STEP 4: Print 64-bit mode message
    ; -----------------------------------------------------------------------

    mov rdi, VGA_TEXT_BUFFER
    mov rsi, msg_long_mode_64
    call print_string_64

    ; -----------------------------------------------------------------------
    ; STEP 5: Jump to kernel!
    ; -----------------------------------------------------------------------
    ; The kernel was loaded at KERNEL_LOAD_ADDR and relocated to KERNEL_START.
    ; Jump to the relocated image (identity-mapped).

    mov rsi, msg_jumping_serial
    call serial_print_64

    ; Give a brief pause for the message to be visible
    mov rdi, VGA_TEXT_BUFFER + 160  ; Second line
    mov rsi, msg_jumping_kernel
    call print_string_64

    ; Jump to kernel entry point
    mov rax, KERNEL_START
    jmp rax

; ===========================================================================
; 64-BIT SUBROUTINES
; ===========================================================================

; Print string to VGA in 64-bit mode
; Input: RDI = VGA buffer position, RSI = string address
; Output: RDI = updated position
print_string_64:
    push rax
.loop:
    lodsb                       ; Load byte from [RSI] into AL
    test al, al                 ; Check for null
    jz .done
    mov ah, VGA_DEFAULT_ATTR    ; White on black
    stosw                       ; Store character + attribute
    jmp .loop
.done:
    pop rax
    ret

; ===========================================================================
; 64-BIT DATA
; ===========================================================================

msg_long_mode_64:    db "PeetOS: 64-bit Long Mode active!", 0
msg_jumping_kernel:  db "Jumping to kernel...", 0
msg_lm64_serial:     db "[SERIAL] 64-bit Long Mode active!", 13, 10, 0
msg_jumping_serial:  db "[SERIAL] Jumping to kernel...", 13, 10, 0

; ===========================================================================
; END OF STAGE 2
; ===========================================================================
; At this point we've:
;   1. Enabled A20 line
;   2. Verified Long Mode support
;   3. Loaded the kernel from disk
;   4. Transitioned: Real Mode -> Protected Mode -> Long Mode
;   5. Set up paging with identity mapping
;   6. Jumped to the kernel
;
; The kernel is now in control!
; ===========================================================================
