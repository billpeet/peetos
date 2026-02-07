; ===========================================================================
; A20 Line Enablement - a20.asm
; ===========================================================================
;
; WHAT IS THE A20 LINE?
; =====================
; The A20 line is the 21st address line of the system bus (bit 20, counting
; from 0). It controls whether the CPU can access memory above 1 MB.
;
; HISTORICAL CONTEXT:
; ===================
; The original IBM PC (8086) had 20 address lines (A0-A19), allowing access
; to 1 MB of memory (2^20 = 1,048,576 bytes).
;
; When IBM released the 80286 with 24 address lines, they discovered a
; compatibility problem: some programs relied on the "wrap-around" behavior
; of the 8086, where addresses above 1 MB would wrap back to 0.
;
; IBM's solution was to add a gate (controlled by the keyboard controller!)
; that could force address line A20 to zero, making the system behave like
; an 8086 for compatibility.
;
; WHY WE NEED TO ENABLE IT:
; =========================
; By default on many systems, A20 is disabled. This means:
;   - Any address with bit 20 set will have that bit masked to 0
;   - Address 0x100000 (1 MB) becomes 0x000000 (0)
;   - Address 0x100001 becomes 0x000001
;   - etc.
;
; Our kernel will be loaded at 1 MB (0x100000), so we MUST enable A20
; to access it properly.
;
; METHODS TO ENABLE A20:
; ======================
; There are several methods, listed in order of preference:
;   1. BIOS INT 15h, AX=2401h (most modern, cleanest)
;   2. Keyboard Controller method (works on older hardware)
;   3. Fast A20 via port 0x92 (quick but not universal)
;
; We try them in order until one succeeds.
;
; ===========================================================================

[BITS 16]                       ; This code runs in 16-bit Real Mode

; ===========================================================================
; SUBROUTINE: enable_a20
; ===========================================================================
; Attempts to enable the A20 line using multiple methods.
;
; Output:
;   CF = 0 on success, CF = 1 on failure
;
; Clobbers:
;   AX, BX, CX
; ===========================================================================

enable_a20:
    call check_a20              ; First, check if A20 is already enabled
    jnz .already_enabled        ; If ZF=0, A20 is already on

    ; A20 is not enabled, try various methods

    ; Method 1: BIOS
    call enable_a20_bios
    call check_a20
    jnz .success

    ; Method 2: Keyboard controller
    call enable_a20_keyboard
    call check_a20
    jnz .success

    ; Method 3: Fast A20 gate
    call enable_a20_fast
    call check_a20
    jnz .success

    ; All methods failed
    stc                         ; Set carry flag to indicate failure
    ret

.already_enabled:
.success:
    clc                         ; Clear carry flag to indicate success
    ret

; ===========================================================================
; SUBROUTINE: check_a20
; ===========================================================================
; Checks if the A20 line is enabled.
;
; How it works:
;   We compare memory at 0x0000:0x0500 with 0xFFFF:0x0510.
;   Due to segment:offset addressing:
;     0xFFFF * 16 + 0x0510 = 0xFFFF0 + 0x0510 = 0x100500
;
;   If A20 is DISABLED, 0x100500 wraps to 0x000500 (same location!)
;   If A20 is ENABLED, they are different memory locations.
;
;   We write a test value and check if it appears at both locations.
;
; Output:
;   ZF = 1 if A20 is disabled (addresses wrapped)
;   ZF = 0 if A20 is enabled (addresses distinct)
;
; Clobbers:
;   AX, BX, ES
; ===========================================================================

check_a20:
    pushf                       ; Save flags
    push ds
    push es
    push di
    push si

    cli                         ; Disable interrupts for this test

    ; Set up segment registers
    xor ax, ax                  ; AX = 0
    mov es, ax                  ; ES = 0x0000

    mov ax, 0xFFFF
    mov ds, ax                  ; DS = 0xFFFF

    ; ES:DI points to 0x0000:0x0500 = 0x00500
    mov di, 0x0500

    ; DS:SI points to 0xFFFF:0x0510 = 0x100500 (or 0x00500 if A20 disabled)
    mov si, 0x0510

    ; Save original values
    mov al, byte [es:di]        ; Save byte at 0x00500
    push ax
    mov al, byte [ds:si]        ; Save byte at 0x100500
    push ax

    ; Write test values
    mov byte [es:di], 0x00      ; Write 0x00 to 0x00500
    mov byte [ds:si], 0xFF      ; Write 0xFF to 0x100500

    ; Read back from first location
    cmp byte [es:di], 0xFF      ; Did 0xFF appear at 0x00500?
                                ; If yes, A20 is disabled (they're the same addr)
                                ; If no, A20 is enabled (distinct addresses)

    ; Restore original values
    pop ax
    mov byte [ds:si], al
    pop ax
    mov byte [es:di], al

    ; At this point:
    ; ZF = 1 means the compare was equal (A20 disabled)
    ; ZF = 0 means the compare was not equal (A20 enabled)

    pop si
    pop di
    pop es
    pop ds
    sti                         ; Re-enable interrupts
    popf                        ; Restore original flags

    ret

; ===========================================================================
; SUBROUTINE: enable_a20_bios
; ===========================================================================
; Enables A20 using BIOS INT 15h, AX=2401h.
;
; This is the cleanest method, supported by modern BIOSes.
;
; BIOS INT 15h, AX=2401h - Enable A20 Gate:
;   Input:  AX = 0x2401
;   Output: CF = 0 on success, 1 on failure
;           AH = status
;
; ===========================================================================

enable_a20_bios:
    mov ax, 0x2401              ; BIOS A20 enable function
    int 0x15                    ; Call BIOS

    ; CF is set by BIOS if function not supported or failed
    ; We don't need to do anything else
    ret

; ===========================================================================
; SUBROUTINE: enable_a20_keyboard
; ===========================================================================
; Enables A20 through the keyboard controller.
;
; HISTORICAL NOTE:
; ================
; The keyboard controller (Intel 8042) has an output port with several
; control lines. Bit 1 of this output port controls the A20 gate.
; This is a legacy hack from the original IBM AT design.
;
; The keyboard controller is accessed through I/O ports:
;   0x60 - Data port (read/write data)
;   0x64 - Command/Status port (read=status, write=command)
;
; To enable A20:
;   1. Wait for controller to be ready (input buffer empty)
;   2. Send command 0xAD (disable keyboard) to port 0x64
;   3. Wait for controller ready
;   4. Send command 0xD0 (read output port) to port 0x64
;   5. Wait for data available
;   6. Read data from port 0x60
;   7. Save this value
;   8. Wait for controller ready
;   9. Send command 0xD1 (write output port) to port 0x64
;  10. Wait for controller ready
;  11. Write saved value OR'd with 0x02 (A20 enable bit) to port 0x60
;  12. Wait for controller ready
;  13. Send command 0xAE (enable keyboard) to port 0x64
;  14. Wait for controller ready
;
; ===========================================================================

enable_a20_keyboard:
    cli                         ; Disable interrupts during this process

    ; Disable keyboard
    call .wait_input
    mov al, 0xAD                ; Command: Disable keyboard
    out 0x64, al

    ; Read output port
    call .wait_input
    mov al, 0xD0                ; Command: Read output port
    out 0x64, al

    ; Read the current output port value
    call .wait_output
    in al, 0x60                 ; Read current value
    push ax                     ; Save it

    ; Write output port
    call .wait_input
    mov al, 0xD1                ; Command: Write output port
    out 0x64, al

    ; Write new value with A20 enabled
    call .wait_input
    pop ax                      ; Get saved value
    or al, 0x02                 ; Set bit 1 (A20 enable)
    out 0x60, al                ; Write new value

    ; Re-enable keyboard
    call .wait_input
    mov al, 0xAE                ; Command: Enable keyboard
    out 0x64, al

    call .wait_input            ; Wait for command to complete

    sti                         ; Re-enable interrupts
    ret

; Wait for keyboard controller input buffer to be empty
; (Ready to accept commands)
.wait_input:
    in al, 0x64                 ; Read status register
    test al, 0x02               ; Check bit 1 (input buffer full)
    jnz .wait_input             ; If set, keep waiting
    ret

; Wait for keyboard controller output buffer to have data
; (Ready to read)
.wait_output:
    in al, 0x64                 ; Read status register
    test al, 0x01               ; Check bit 0 (output buffer full)
    jz .wait_output             ; If clear, keep waiting
    ret

; ===========================================================================
; SUBROUTINE: enable_a20_fast
; ===========================================================================
; Enables A20 using the Fast A20 Gate (port 0x92).
;
; This method is faster but not universally supported.
; Some systems use port 0x92 for other purposes, so this method
; can cause problems. We use it as a last resort.
;
; Port 0x92:
;   Bit 0: System reset (DON'T set this!)
;   Bit 1: A20 gate enable
;
; ===========================================================================

enable_a20_fast:
    in al, 0x92                 ; Read current value
    test al, 0x02               ; Is A20 already enabled?
    jnz .done                   ; If yes, we're done

    or al, 0x02                 ; Set bit 1 (A20 enable)
    and al, 0xFE                ; Clear bit 0 (DON'T reset!)
    out 0x92, al                ; Write new value

.done:
    ret
