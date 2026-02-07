; ===========================================================================
; PeetOS Stage 1 Bootloader (MBR) - boot.asm
; ===========================================================================
;
; WHAT IS THIS FILE?
; ==================
; This is the Master Boot Record (MBR) - the very first code that runs
; when your computer boots from this disk. It's loaded by the BIOS into
; memory at address 0x7C00 and must be exactly 512 bytes.
;
; The last 2 bytes MUST be 0x55 0xAA (the boot signature) or the BIOS
; will refuse to boot from this disk.
;
; WHAT DOES IT DO?
; ================
; 1. Sets up a minimal environment (segments, stack)
; 2. Prints a loading message using BIOS interrupts
; 3. Loads Stage 2 from disk (which is too big to fit in 512 bytes)
; 4. Jumps to Stage 2 to continue the boot process
;
; WHY TWO STAGES?
; ===============
; 512 bytes is tiny - we can barely fit the code to load more code!
; The real work (setting up protected mode, long mode, paging, loading
; the kernel) happens in Stage 2, which can be much larger.
;
; REAL MODE PRIMER:
; =================
; The CPU starts in "Real Mode" - a 16-bit mode dating back to the 8086.
; In Real Mode:
;   - Registers are 16-bit (AX, BX, CX, DX, SI, DI, SP, BP)
;   - Memory is accessed via segment:offset addressing
;   - Physical address = segment * 16 + offset
;   - Maximum addressable memory is 1 MB (0x00000 - 0xFFFFF)
;   - We have direct access to BIOS interrupts for I/O
;
; ===========================================================================

; ---------------------------------------------------------------------------
; ASSEMBLER DIRECTIVES
; ---------------------------------------------------------------------------

[BITS 16]                       ; Tell NASM we're writing 16-bit code.
                                ; This affects how instructions are encoded.

[ORG 0x7C00]                    ; Tell NASM our code will be loaded at 0x7C00.
                                ; This affects how labels are calculated.
                                ; Without this, 'jmp label' would calculate
                                ; wrong addresses!

; ---------------------------------------------------------------------------
; INCLUDES
; ---------------------------------------------------------------------------
; Note: We can't use %include here because Stage 1 is assembled as a flat
; binary, not linked. We'll define only what we need inline.

STAGE2_SEGMENT      equ 0x07E0  ; Segment for loading Stage 2 (0x7E00 / 16)
STAGE2_SECTORS      equ 16      ; Number of sectors to load (8 KB)

; ===========================================================================
; ENTRY POINT
; ===========================================================================
; This is where the BIOS jumps to after loading the MBR.
; The BIOS has already:
;   - Performed POST (Power-On Self Test)
;   - Initialized basic hardware
;   - Loaded this 512-byte sector to 0x7C00
;   - Set DL = boot drive number
; ===========================================================================

start:
    ; -----------------------------------------------------------------------
    ; STEP 1: Disable interrupts during setup
    ; -----------------------------------------------------------------------
    ; We don't want an interrupt occurring while we're setting up segments
    ; and stack - that could crash the system.
    cli                         ; Clear Interrupt Flag - disables interrupts

    ; -----------------------------------------------------------------------
    ; STEP 2: Set up segment registers
    ; -----------------------------------------------------------------------
    ; In Real Mode, memory addresses are calculated as: segment * 16 + offset
    ;
    ; For example, to access address 0x7C00:
    ;   - We could use segment 0x0000, offset 0x7C00 (0*16 + 0x7C00 = 0x7C00)
    ;   - Or segment 0x07C0, offset 0x0000 (0x7C0*16 + 0 = 0x7C00)
    ;
    ; We use the simple approach: segment = 0, offset = actual address.
    ; This makes our code easier to understand.
    ;
    ; There are 4 segment registers in Real Mode:
    ;   CS - Code Segment: Where instructions come from (set by JMP/CALL)
    ;   DS - Data Segment: Default for data access (MOV, etc.)
    ;   ES - Extra Segment: Used for string operations destination
    ;   SS - Stack Segment: Where the stack lives

    xor ax, ax                  ; AX = 0 (XOR is faster than MOV AX, 0)
    mov ds, ax                  ; DS = 0 (Data Segment)
    mov es, ax                  ; ES = 0 (Extra Segment)
    mov ss, ax                  ; SS = 0 (Stack Segment)

    ; Note: We can't set CS directly - it's set implicitly by JMP/CALL.
    ; The BIOS might have jumped to us with CS:IP = 0x0000:0x7C00 or
    ; CS:IP = 0x07C0:0x0000 - both point to the same physical address.
    ; By using ORG 0x7C00 and DS=0, we handle either case.

    ; -----------------------------------------------------------------------
    ; STEP 3: Set up the stack
    ; -----------------------------------------------------------------------
    ; The stack grows DOWNWARD in memory. SP (Stack Pointer) points to the
    ; top of the stack. When we PUSH, SP decreases. When we POP, SP increases.
    ;
    ; We set SP = 0x7C00, so the stack will grow downward from there.
    ; This gives us about 30 KB of stack space (0x7C00 - 0x0500).
    ; Memory below 0x0500 is used by the BIOS.
    ;
    ; STACK LAYOUT:
    ; 0x0500  ─┬─ (BIOS data area above)
    ;          │
    ;          │  ← Stack grows into this area
    ;          │
    ; 0x7BFF  ─┴─ Stack bottom (SP starts here, grows down)
    ; 0x7C00  ─── Our bootloader starts here

    mov sp, 0x7C00              ; Stack pointer = 0x7C00

    ; -----------------------------------------------------------------------
    ; STEP 4: Save the boot drive number
    ; -----------------------------------------------------------------------
    ; The BIOS passes the boot drive number in DL register:
    ;   0x00 = First floppy (A:)
    ;   0x80 = First hard disk (C:)
    ;   0x81 = Second hard disk
    ;   etc.
    ;
    ; We need to save this because we'll use it later to load more sectors.

    mov [boot_drive], dl        ; Save boot drive number to memory

    ; -----------------------------------------------------------------------
    ; STEP 5: Re-enable interrupts
    ; -----------------------------------------------------------------------
    ; Now that segments and stack are set up, it's safe to enable interrupts.
    ; We need interrupts enabled to use BIOS services (INT instructions).

    sti                         ; Set Interrupt Flag - enables interrupts

    ; -----------------------------------------------------------------------
    ; STEP 6: Print loading message
    ; -----------------------------------------------------------------------
    ; We use BIOS interrupt 0x10 (video services) to print text.
    ; This is one of the few ways to output text in Real Mode.

    mov si, msg_loading         ; SI = address of string to print
    call print_string           ; Call our print routine

    ; -----------------------------------------------------------------------
    ; STEP 7: Load Stage 2 from disk
    ; -----------------------------------------------------------------------
    ; Stage 2 is stored on disk immediately after the MBR (sectors 2-17).
    ; We use BIOS interrupt 0x13 (disk services) to read it into memory.
    ;
    ; BIOS INT 0x13, AH=0x02: Read Sectors
    ; Input:
    ;   AH = 0x02 (read sectors function)
    ;   AL = number of sectors to read
    ;   CH = cylinder number (low 8 bits)
    ;   CL = sector number (1-63) in bits 0-5, cylinder high bits in 6-7
    ;   DH = head number
    ;   DL = drive number
    ;   ES:BX = buffer address
    ; Output:
    ;   CF = 0 on success, 1 on error
    ;   AH = status code
    ;   AL = number of sectors actually read

    mov ah, 0x02                ; BIOS function: Read sectors
    mov al, STAGE2_SECTORS      ; Number of sectors to read (16 = 8KB)
    mov ch, 0                   ; Cylinder 0 (we're reading from start of disk)
    mov cl, 2                   ; Start from sector 2 (sector 1 = MBR)
                                ; Note: BIOS uses 1-indexed sector numbers!
    mov dh, 0                   ; Head 0
    mov dl, [boot_drive]        ; Drive number (saved earlier)

    ; Set up the destination buffer
    mov bx, STAGE2_SEGMENT      ; We want to load at 0x7E00
    mov es, bx                  ; ES = 0x07E0 (segment)
    xor bx, bx                  ; BX = 0 (offset)
                                ; Address = 0x07E0 * 16 + 0 = 0x7E00

    int 0x13                    ; Call BIOS disk services

    jc disk_error               ; If carry flag set, disk read failed

    ; -----------------------------------------------------------------------
    ; STEP 8: Verify we read the expected number of sectors
    ; -----------------------------------------------------------------------
    ; AL now contains the number of sectors actually read.
    ; We should verify it matches what we requested.

    cmp al, STAGE2_SECTORS      ; Did we read all sectors?
    jne disk_error              ; If not, something went wrong

    ; -----------------------------------------------------------------------
    ; STEP 9: Jump to Stage 2
    ; -----------------------------------------------------------------------
    ; Stage 2 is now loaded at 0x7E00. We jump there to continue booting.
    ; We use a far jump (specify both segment and offset) to ensure CS
    ; is set correctly.

    mov si, msg_jumping         ; Print message before jumping
    call print_string

    jmp 0x0000:0x7E00           ; Far jump to Stage 2
                                ; Sets CS = 0x0000, IP = 0x7E00

; ===========================================================================
; ERROR HANDLER
; ===========================================================================
; If disk read fails, we print an error message and halt.
; There's not much else we can do at this point!
; ===========================================================================

disk_error:
    mov si, msg_disk_error      ; Load error message address
    call print_string           ; Print it

    cli                         ; Disable interrupts
    hlt                         ; Halt the CPU

    jmp $                       ; Infinite loop (in case HLT doesn't stick)
                                ; '$' means "current address"

; ===========================================================================
; SUBROUTINE: print_string
; ===========================================================================
; Prints a null-terminated string to the screen using BIOS.
;
; Input:
;   SI = address of null-terminated string
;
; Uses:
;   BIOS INT 0x10, AH=0x0E (Teletype output)
;   This function prints a character and advances the cursor.
;
; Modifies:
;   AX, SI
; ===========================================================================

print_string:
    mov ah, 0x0E                ; BIOS function: Teletype output
    mov bh, 0                   ; Page number (0 = default)

.loop:
    lodsb                       ; Load byte from [SI] into AL, increment SI
                                ; This is equivalent to: MOV AL, [SI] / INC SI

    test al, al                 ; Check if AL is zero (null terminator)
                                ; TEST sets ZF if result is zero

    jz .done                    ; If zero, we've reached end of string

    int 0x10                    ; Call BIOS to print character in AL

    jmp .loop                   ; Continue with next character

.done:
    ret                         ; Return to caller

; ===========================================================================
; DATA SECTION
; ===========================================================================
; All our strings and variables go here.
; ===========================================================================

boot_drive:      db 0           ; Storage for boot drive number (1 byte)

msg_loading:     db "PeetOS Stage 1", 13, 10, 0
                                ; 13 = Carriage Return (CR)
                                ; 10 = Line Feed (LF)
                                ; 0 = Null terminator

msg_jumping:     db "Loading Stage 2...", 13, 10, 0

msg_disk_error:  db "Disk error!", 13, 10, 0

; ===========================================================================
; PADDING AND BOOT SIGNATURE
; ===========================================================================
; The MBR must be exactly 512 bytes, with the last two bytes being the
; boot signature 0x55 0xAA.
;
; TIMES fills the remaining bytes with zeros.
; $ = current address, $$ = start of section (0x7C00 for us)
; So ($-$$) = current size of our code/data
; 510 - ($-$$) = bytes remaining before the signature
; ===========================================================================

times 510 - ($ - $$) db 0       ; Pad with zeros to byte 510

dw 0xAA55                       ; Boot signature (little-endian: 55 AA)
                                ; BIOS checks for this signature!
                                ; Without it, BIOS won't boot this disk.

; ===========================================================================
; END OF STAGE 1
; ===========================================================================
; At this point, Stage 1 is complete. We're exactly 512 bytes.
; When assembled, this file produces a valid MBR that:
;   1. Sets up a Real Mode environment
;   2. Loads Stage 2 from disk
;   3. Jumps to Stage 2
;
; Next: See boot/stage2/stage2.asm for the continuation of the boot process.
; ===========================================================================
