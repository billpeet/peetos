; ===========================================================================
; 16-bit Print Utilities - print16.asm
; ===========================================================================
;
; WHAT IS THIS FILE?
; ==================
; This file contains subroutines for printing text to the screen while
; in 16-bit Real Mode. We use BIOS interrupts for this since we don't
; have access to graphics drivers yet.
;
; BIOS VIDEO SERVICES (INT 0x10):
; ===============================
; The BIOS provides various video services through interrupt 0x10.
; We primarily use:
;   - AH=0x0E: Teletype output (print character and advance cursor)
;   - AH=0x03: Get cursor position
;   - AH=0x02: Set cursor position
;
; ===========================================================================

[BITS 16]                       ; This code runs in 16-bit Real Mode

; ===========================================================================
; SUBROUTINE: print_string_16
; ===========================================================================
; Prints a null-terminated string to the screen.
;
; Input:
;   SI = pointer to null-terminated string
;
; Clobbers:
;   AX, BX, SI
;
; Example usage:
;   mov si, my_string
;   call print_string_16
; ===========================================================================

print_string_16:
    pusha                       ; Save all general-purpose registers
                                ; This pushes AX, CX, DX, BX, SP, BP, SI, DI

    mov ah, 0x0E                ; BIOS teletype function
    mov bh, 0                   ; Display page 0 (the visible page)
    mov bl, 0x07                ; Text attribute (light gray on black)
                                ; This is used by some BIOS implementations

.loop:
    lodsb                       ; Load byte from DS:SI into AL, increment SI
                                ; This is a string instruction that automatically
                                ; advances the source pointer

    test al, al                 ; Check if we've hit the null terminator
                                ; TEST performs AND but doesn't store result
                                ; It just sets flags (ZF if result is 0)

    jz .done                    ; If AL is 0, we're done (jump if zero)

    int 0x10                    ; Call BIOS to print the character
                                ; AH=0x0E tells BIOS to use teletype mode
                                ; which prints the character and advances cursor

    jmp .loop                   ; Process next character

.done:
    popa                        ; Restore all general-purpose registers
    ret                         ; Return to caller

; ===========================================================================
; SUBROUTINE: print_char_16
; ===========================================================================
; Prints a single character to the screen.
;
; Input:
;   AL = character to print
;
; Clobbers:
;   AH, BX
;
; Example usage:
;   mov al, 'X'
;   call print_char_16
; ===========================================================================

print_char_16:
    pusha                       ; Save all registers
    mov ah, 0x0E                ; BIOS teletype function
    mov bh, 0                   ; Page 0
    int 0x10                    ; Print character in AL
    popa                        ; Restore registers
    ret

; ===========================================================================
; SUBROUTINE: print_newline_16
; ===========================================================================
; Prints a newline (CR + LF) to move cursor to start of next line.
;
; In DOS/BIOS text mode, a "newline" consists of two characters:
;   - Carriage Return (CR, 0x0D or 13): Moves cursor to column 0
;   - Line Feed (LF, 0x0A or 10): Moves cursor down one row
;
; Clobbers:
;   AX, BX
; ===========================================================================

print_newline_16:
    pusha
    mov ah, 0x0E
    mov bh, 0

    mov al, 13                  ; Carriage Return (CR)
    int 0x10

    mov al, 10                  ; Line Feed (LF)
    int 0x10

    popa
    ret

; ===========================================================================
; SUBROUTINE: print_hex_16
; ===========================================================================
; Prints a 16-bit value as a 4-digit hexadecimal number.
;
; Input:
;   AX = 16-bit value to print
;
; Example:
;   mov ax, 0x1234
;   call print_hex_16
;   ; Prints: 1234
;
; How it works:
;   We need to convert each 4-bit nibble to its ASCII representation.
;   For 0-9: ASCII = nibble + '0' (0x30)
;   For A-F: ASCII = nibble - 10 + 'A' (0x41)
;
; Clobbers:
;   AX, BX, CX, DX
; ===========================================================================

print_hex_16:
    pusha
    mov cx, 4                   ; We have 4 nibbles to print (4 hex digits)

.loop:
    rol ax, 4                   ; Rotate left by 4 bits
                                ; This brings the high nibble to the low position
                                ; First iteration: nibble 3 (highest)
                                ; Second: nibble 2
                                ; Third: nibble 1
                                ; Fourth: nibble 0 (lowest)

    mov dx, ax                  ; Save AX (we need to preserve it for next iteration)

    and al, 0x0F                ; Mask off the low nibble (0-15)

    cmp al, 10                  ; Is it >= 10 (A-F)?
    jb .digit                   ; If below 10, it's a digit (0-9)

    add al, 'A' - 10            ; Convert 10-15 to 'A'-'F'
    jmp .print

.digit:
    add al, '0'                 ; Convert 0-9 to '0'-'9'

.print:
    mov ah, 0x0E                ; BIOS teletype
    mov bh, 0
    int 0x10                    ; Print the character

    mov ax, dx                  ; Restore AX for next nibble
    loop .loop                  ; Decrement CX and loop if not zero

    popa
    ret

; ===========================================================================
; SUBROUTINE: print_hex_byte_16
; ===========================================================================
; Prints an 8-bit value as a 2-digit hexadecimal number.
;
; Input:
;   AL = 8-bit value to print
;
; Example:
;   mov al, 0x4F
;   call print_hex_byte_16
;   ; Prints: 4F
; ===========================================================================

print_hex_byte_16:
    pusha
    mov cx, 2                   ; 2 nibbles for a byte

    ; Move the byte to AH to start with, we'll shift it
    mov ah, al
    shr ah, 4                   ; Get high nibble

.high_nibble:
    mov al, ah
    and al, 0x0F

    cmp al, 10
    jb .high_digit
    add al, 'A' - 10
    jmp .print_high
.high_digit:
    add al, '0'
.print_high:
    push ax                     ; Save for low nibble
    mov ah, 0x0E
    mov bh, 0
    int 0x10
    pop ax

.low_nibble:
    ; AL still has original value on stack, get it again
    popa
    pusha
    and al, 0x0F                ; Get low nibble

    cmp al, 10
    jb .low_digit
    add al, 'A' - 10
    jmp .print_low
.low_digit:
    add al, '0'
.print_low:
    mov ah, 0x0E
    mov bh, 0
    int 0x10

    popa
    ret
