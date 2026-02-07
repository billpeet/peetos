; ===========================================================================
; Serial Port Output - serial.asm
; ===========================================================================
;
; WHAT IS THIS FILE?
; ==================
; This file provides serial port (COM1) output for debugging.
; Serial output is much easier to capture than VGA when running in QEMU
; with -serial stdio or -nographic.
;
; SERIAL PORT BASICS:
; ===================
; The PC's COM1 serial port is accessed through I/O ports:
;   0x3F8 - Data register (read/write data)
;   0x3F9 - Interrupt enable register
;   0x3FA - Interrupt identification / FIFO control
;   0x3FB - Line control register (data format)
;   0x3FC - Modem control register
;   0x3FD - Line status register (check if ready to send)
;   0x3FE - Modem status register
;   0x3FF - Scratch register
;
; ===========================================================================

; COM1 port base address
COM1_PORT       equ 0x3F8

; Line Status Register bits
LSR_THRE        equ 0x20        ; Transmitter Holding Register Empty

; ===========================================================================
; 16-BIT SERIAL ROUTINES
; ===========================================================================

[BITS 16]

; ---------------------------------------------------------------------------
; serial_init_16: Initialize COM1 serial port
; ---------------------------------------------------------------------------
; Sets up COM1 at 9600 baud, 8N1 (8 data bits, no parity, 1 stop bit)
; ---------------------------------------------------------------------------
serial_init_16:
    push ax
    push dx

    ; Disable interrupts on the serial port
    mov dx, COM1_PORT + 1       ; Interrupt enable register
    xor al, al
    out dx, al

    ; Enable DLAB (Divisor Latch Access Bit) to set baud rate
    mov dx, COM1_PORT + 3       ; Line control register
    mov al, 0x80                ; DLAB = 1
    out dx, al

    ; Set baud rate divisor (115200 / baud_rate)
    ; For 9600 baud: divisor = 115200 / 9600 = 12
    ; For 115200 baud: divisor = 1
    mov dx, COM1_PORT + 0       ; Divisor latch low byte
    mov al, 1                   ; 115200 baud (faster for debugging)
    out dx, al
    mov dx, COM1_PORT + 1       ; Divisor latch high byte
    xor al, al
    out dx, al

    ; Set line control: 8 bits, no parity, 1 stop bit (8N1)
    mov dx, COM1_PORT + 3       ; Line control register
    mov al, 0x03                ; 8N1, DLAB = 0
    out dx, al

    ; Enable FIFO, clear them, with 14-byte threshold
    mov dx, COM1_PORT + 2       ; FIFO control register
    mov al, 0xC7
    out dx, al

    ; Enable IRQs, set RTS/DSR
    mov dx, COM1_PORT + 4       ; Modem control register
    mov al, 0x0B
    out dx, al

    pop dx
    pop ax
    ret

; ---------------------------------------------------------------------------
; serial_putc_16: Output a character to COM1
; ---------------------------------------------------------------------------
; Input: AL = character to send
; ---------------------------------------------------------------------------
serial_putc_16:
    push ax
    push dx

    mov ah, al                  ; Save character

    ; Wait for transmitter to be ready
.wait:
    mov dx, COM1_PORT + 5       ; Line status register
    in al, dx
    test al, LSR_THRE           ; Check if THR is empty
    jz .wait

    ; Send the character
    mov dx, COM1_PORT           ; Data register
    mov al, ah                  ; Restore character
    out dx, al

    pop dx
    pop ax
    ret

; ---------------------------------------------------------------------------
; serial_print_16: Print a null-terminated string to COM1
; ---------------------------------------------------------------------------
; Input: SI = pointer to string
; ---------------------------------------------------------------------------
serial_print_16:
    push ax
    push si

.loop:
    lodsb                       ; Load character from [SI] into AL
    test al, al                 ; Check for null terminator
    jz .done
    call serial_putc_16
    jmp .loop

.done:
    pop si
    pop ax
    ret

; ===========================================================================
; 32-BIT SERIAL ROUTINES
; ===========================================================================

[BITS 32]

; ---------------------------------------------------------------------------
; serial_putc_32: Output a character to COM1 (32-bit)
; ---------------------------------------------------------------------------
; Input: AL = character to send
; ---------------------------------------------------------------------------
serial_putc_32:
    push eax
    push edx

    mov ah, al                  ; Save character

.wait:
    mov dx, COM1_PORT + 5
    in al, dx
    test al, LSR_THRE
    jz .wait

    mov dx, COM1_PORT
    mov al, ah
    out dx, al

    pop edx
    pop eax
    ret

; ---------------------------------------------------------------------------
; serial_print_32: Print a null-terminated string to COM1 (32-bit)
; ---------------------------------------------------------------------------
; Input: ESI = pointer to string
; ---------------------------------------------------------------------------
serial_print_32:
    push eax
    push esi

.loop:
    lodsb
    test al, al
    jz .done
    call serial_putc_32
    jmp .loop

.done:
    pop esi
    pop eax
    ret

; ===========================================================================
; 64-BIT SERIAL ROUTINES
; ===========================================================================

[BITS 64]

; ---------------------------------------------------------------------------
; serial_putc_64: Output a character to COM1 (64-bit)
; ---------------------------------------------------------------------------
; Input: DIL = character to send
; ---------------------------------------------------------------------------
serial_putc_64:
    push rax
    push rdx
    push rcx

    mov cl, dil                 ; Save character in CL

.wait:
    mov dx, COM1_PORT + 5
    in al, dx
    test al, LSR_THRE
    jz .wait

    mov dx, COM1_PORT
    mov al, cl                  ; Get character from CL
    out dx, al

    pop rcx
    pop rdx
    pop rax
    ret

; ---------------------------------------------------------------------------
; serial_print_64: Print a null-terminated string to COM1 (64-bit)
; ---------------------------------------------------------------------------
; Input: RSI = pointer to string
; ---------------------------------------------------------------------------
serial_print_64:
    push rax
    push rsi

.loop:
    lodsb
    test al, al
    jz .done
    mov dil, al
    call serial_putc_64
    jmp .loop

.done:
    pop rsi
    pop rax
    ret
