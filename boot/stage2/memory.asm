; ===========================================================================
; Memory Detection - memory.asm
; ===========================================================================
;
; Implements memory detection using BIOS INT 15h, E820h.
;
; ===========================================================================

[BITS 16]

; ===========================================================================
; SUBROUTINE: detect_memory
; ===========================================================================
; Standard E820 memory detection implementation.
; Based on the canonical E820 implementation from OSDev Wiki.
; ===========================================================================

detect_memory:
    push es
    push di
    push ebx
    push ecx
    push edx
    push bp

    ; Set ES:DI to point to buffer
    xor ax, ax
    mov es, ax
    mov di, MMAP_ADDR

    ; Initialize count
    xor bp, bp                  ; BP = entry count
    xor ebx, ebx                ; Must be 0 for first call

.e820_loop:
    mov edx, 0x534D4150         ; 'SMAP'
    mov eax, 0xE820             ; E820 function
    mov ecx, 24                 ; Ask for 24 bytes

    int 0x15

    jc .e820_check_end          ; Carry might mean end of list
    mov edx, 0x534D4150         ; Some BIOSes trash EDX
    cmp eax, edx                ; EAX should = 'SMAP' on success
    jne .e820_failed

    ; Got a valid entry - count it first, then check for more
    inc bp                      ; Count this entry
    add di, 24                  ; Move to next slot

    test ebx, ebx               ; EBX = 0 means no more entries
    jz .e820_done

    cmp bp, MMAP_MAX_ENTRIES    ; Check for buffer overflow
    jl .e820_loop
    jmp .e820_done              ; Buffer full, stop

.e820_check_end:
    ; CF was set - if we have entries, this means end of list (success)
    test bp, bp
    jnz .e820_done
    jmp .e820_failed            ; First call failed

.e820_done:
    ; Store final count
    mov [MMAP_COUNT_ADDR], bp

    ; Success
    clc

    pop bp
    pop edx
    pop ecx
    pop ebx
    pop di
    pop es
    ret

.e820_failed:
    ; Store count (might be 0 or partial)
    mov [MMAP_COUNT_ADDR], bp

    ; Failure
    stc

    pop bp
    pop edx
    pop ecx
    pop ebx
    pop di
    pop es
    ret

; ===========================================================================
; SUBROUTINE: print_memory_map
; ===========================================================================

print_memory_map:
    push si
    push di
    push cx
    push eax

    mov si, msg_mmap_header
    call serial_print_16

    mov cx, [MMAP_COUNT_ADDR]
    test cx, cx
    jz .no_entries

    ; Print count
    mov si, msg_mmap_count
    call serial_print_16
    movzx eax, cx
    call serial_print_dec_16
    mov si, msg_mmap_entries
    call serial_print_16

    mov di, MMAP_ADDR

.print_loop:
    mov si, msg_mmap_base
    call serial_print_16
    mov eax, [di + 4]
    call serial_print_hex_16
    mov eax, [di]
    call serial_print_hex_16

    mov si, msg_mmap_len
    call serial_print_16
    mov eax, [di + 12]
    call serial_print_hex_16
    mov eax, [di + 8]
    call serial_print_hex_16

    mov si, msg_mmap_type
    call serial_print_16
    mov eax, [di + 16]
    cmp eax, 1
    je .type_usable
    cmp eax, 2
    je .type_reserved
    jmp .type_other

.type_usable:
    mov si, msg_type_usable
    jmp .print_type
.type_reserved:
    mov si, msg_type_reserved
    jmp .print_type
.type_other:
    mov si, msg_type_other

.print_type:
    call serial_print_16

    add di, 24
    dec cx
    jnz .print_loop
    jmp .done

.no_entries:
    mov si, msg_mmap_none
    call serial_print_16

.done:
    pop eax
    pop cx
    pop di
    pop si
    ret

; Print 32-bit hex
serial_print_hex_16:
    push eax
    push ecx
    push edx

    mov edx, eax
    mov ecx, 8

.hex_loop:
    rol edx, 4
    mov al, dl
    and al, 0x0F
    cmp al, 10
    jl .digit
    add al, 'A' - 10
    jmp .print_char
.digit:
    add al, '0'
.print_char:
    call serial_putc_16
    dec ecx
    jnz .hex_loop

    pop edx
    pop ecx
    pop eax
    ret

; Print decimal
serial_print_dec_16:
    push ax
    push bx
    push cx
    push dx

    mov bx, 10
    xor cx, cx

.div_loop:
    xor dx, dx
    div bx
    push dx
    inc cx
    test ax, ax
    jnz .div_loop

.print_digits:
    pop ax
    add al, '0'
    call serial_putc_16
    dec cx
    jnz .print_digits

    pop dx
    pop cx
    pop bx
    pop ax
    ret

; ===========================================================================
; DATA
; ===========================================================================

msg_mmap_header:    db "[SERIAL] Memory Map (E820):", 13, 10, 0
msg_mmap_count:     db "[SERIAL]   Found ", 0
msg_mmap_entries:   db " memory regions", 13, 10, 0
msg_mmap_base:      db "[SERIAL]   Base: 0x", 0
msg_mmap_len:       db " Len: 0x", 0
msg_mmap_type:      db " ", 0
msg_type_usable:    db "(Usable)", 13, 10, 0
msg_type_reserved:  db "(Reserved)", 13, 10, 0
msg_type_other:     db "(Other)", 13, 10, 0
msg_mmap_none:      db "[SERIAL]   No entries found!", 13, 10, 0
