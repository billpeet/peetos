; ===========================================================================
; PeetOS VESA/VBE Support - vesa.asm
; ===========================================================================
;
; WHAT IS THIS FILE?
; ==================
; This file implements VESA BIOS Extensions (VBE) support for setting up
; a high-resolution linear framebuffer for graphics output.
;
; VBE OVERVIEW:
; =============
; VBE is a standard interface between software and the video BIOS that
; provides access to extended video modes beyond the standard VGA modes.
;
; Key VBE features:
; - Resolution support beyond 640x480
; - Linear Frame Buffer (LFB) for direct pixel access
; - True color (24/32-bit) support
;
; VESA DETECTION FLOW:
; ====================
;   1. Get VBE Controller Info (INT 0x10, AX=0x4F00)
;      - Verify VBE is supported
;      - Get pointer to mode list
;
;   2. Search mode list for 1024x768x32
;      - For each mode, get mode info (INT 0x10, AX=0x4F01)
;      - Check resolution and color depth
;
;   3. Set VESA mode with LFB (INT 0x10, AX=0x4F02)
;      - Mode number OR'd with 0x4000 for LFB
;
;   4. Store status for kernel
;      - Framebuffer address, resolution, etc.
;
; IMPORTANT:
; ==========
; This code MUST run in Real Mode (16-bit) because VBE functions use
; BIOS interrupts (INT 0x10) which are not available in Protected Mode.
;
; ===========================================================================

[BITS 16]

; ===========================================================================
; SUBROUTINE: vesa_init
; ===========================================================================
; Attempts to set VESA mode 1024x768x32. Falls back to VGA text mode if
; VESA is not available or the desired mode is not found.
;
; Input:
;   None (reads constants from constants.inc)
;
; Output:
;   VESA_STATUS_ADDR filled with status structure
;   CF = 0 on success (VESA or VGA fallback)
;   CF = 1 on fatal error (unlikely)
;
; Registers Modified:
;   AX, BX, CX, DX, DI, SI, ES, FS
;
; ===========================================================================

vesa_init:
    push bp
    mov bp, sp
    push es
    push fs
    push di
    push si
    push bx
    push cx
    push dx

    ; -----------------------------------------------------------------------
    ; Initialize status to "no VESA"
    ; -----------------------------------------------------------------------
    xor ax, ax
    mov es, ax
    mov di, VESA_STATUS_ADDR
    mov byte [es:di], 0             ; Available = 0 (VGA fallback)
    mov byte [es:di + 1], 0         ; Reserved
    mov word [es:di + 2], 0         ; Mode number = 0
    mov dword [es:di + 4], 0        ; Framebuffer address = 0

    ; Print detection message
    mov si, msg_vesa_detecting
    call serial_print_16

    ; -----------------------------------------------------------------------
    ; STEP 1: Get VBE Controller Info
    ; -----------------------------------------------------------------------
    ; INT 0x10, AX=0x4F00
    ; ES:DI = Pointer to 512-byte buffer for VbeInfoBlock
    ;
    ; On success: AX=0x004F (AL=0x4F means function supported, AH=0x00 success)
    ; On failure: AX != 0x004F
    ;
    ; We write "VBE2" at the start of the buffer to request VBE 2.0+ info.
    ; -----------------------------------------------------------------------

    ; Prepare buffer with VBE2 signature
    mov di, VESA_INFO_ADDR
    mov dword [es:di], 'VBE2'       ; Request VBE 2.0+ information

    ; Call VBE function 0x00 - Get Controller Info
    mov ax, VBE_GET_CONTROLLER_INFO
    int 0x10

    ; Check for success
    cmp ax, 0x004F
    jne .vesa_not_supported

    ; Verify VESA signature in response
    mov di, VESA_INFO_ADDR
    cmp dword [es:di], 'VESA'
    jne .vesa_not_supported

    ; Print VBE version
    mov si, msg_vesa_found
    call serial_print_16
    mov ax, [es:di + 4]             ; VBE version (BCD)
    call vesa_print_version

    ; -----------------------------------------------------------------------
    ; STEP 2: Search mode list for 1024x768x32
    ; -----------------------------------------------------------------------
    ; The mode list is a far pointer at offset 14-17 of VbeInfoBlock
    ; Format: Segment at offset 16, Offset at offset 14
    ; Mode list is a list of 16-bit mode numbers, terminated by 0xFFFF
    ; -----------------------------------------------------------------------

    ; Get mode list pointer
    mov si, [es:VESA_INFO_ADDR + 14]    ; Offset of mode list
    mov ax, [es:VESA_INFO_ADDR + 16]    ; Segment of mode list
    mov fs, ax                           ; FS:SI = mode list

    mov si, msg_vesa_searching
    push si
    mov si, [es:VESA_INFO_ADDR + 14]    ; Restore SI
    pop dx
    push si
    mov si, dx
    call serial_print_16
    pop si

.search_mode_loop:
    ; Read mode number from mode list
    mov cx, [fs:si]

    ; Check for end of list (0xFFFF)
    cmp cx, 0xFFFF
    je .mode_not_found

    ; Save current position in mode list
    push si

    ; Get mode info for this mode
    mov ax, VBE_GET_MODE_INFO
    mov di, VESA_MODE_INFO_ADDR
    int 0x10

    ; Check if function succeeded
    cmp ax, 0x004F
    jne .next_mode

    ; Check mode attributes
    ; Bit 0: Mode supported by hardware
    ; Bit 4: Graphics mode (not text)
    ; Bit 7: Linear frame buffer available
    mov ax, [es:VESA_MODE_INFO_ADDR + VBE_MODE_ATTRIBUTES]
    test ax, 0x0001                 ; Mode supported?
    jz .next_mode
    test ax, 0x0010                 ; Graphics mode?
    jz .next_mode
    test ax, 0x0080                 ; LFB available?
    jz .next_mode

    ; Check resolution - Width
    mov ax, [es:VESA_MODE_INFO_ADDR + VBE_MODE_WIDTH]
    cmp ax, VESA_DESIRED_WIDTH
    jne .next_mode

    ; Check resolution - Height
    mov ax, [es:VESA_MODE_INFO_ADDR + VBE_MODE_HEIGHT]
    cmp ax, VESA_DESIRED_HEIGHT
    jne .next_mode

    ; Check bits per pixel
    mov al, [es:VESA_MODE_INFO_ADDR + VBE_MODE_BPP]
    cmp al, VESA_DESIRED_BPP
    jne .next_mode

    ; Found our mode!
    pop si                          ; Restore mode list position
    mov si, msg_vesa_mode_found
    call serial_print_16

    ; Print the mode number we found
    mov ax, cx
    call vesa_print_hex16
    mov si, msg_newline
    call serial_print_16

    jmp .set_mode

.next_mode:
    pop si                          ; Restore mode list position
    add si, 2                       ; Move to next mode in list
    jmp .search_mode_loop

.mode_not_found:
    mov si, msg_vesa_mode_not_found
    call serial_print_16
    jmp .use_vga_fallback

    ; -----------------------------------------------------------------------
    ; STEP 3: Set VESA mode with Linear Frame Buffer
    ; -----------------------------------------------------------------------
    ; INT 0x10, AX=0x4F02
    ; BX = Mode number OR'd with 0x4000 to enable LFB
    ;
    ; On success: AX=0x004F
    ; -----------------------------------------------------------------------

.set_mode:
    ; CX still contains the mode number from search
    mov bx, cx
    or bx, VBE_MODE_LFB_BIT         ; Enable Linear Frame Buffer

    mov si, msg_vesa_setting
    call serial_print_16

    ; Set the mode
    mov ax, VBE_SET_MODE
    int 0x10

    ; Check for success
    cmp ax, 0x004F
    jne .set_mode_failed

    ; -----------------------------------------------------------------------
    ; STEP 4: Store status for kernel
    ; -----------------------------------------------------------------------

    mov di, VESA_STATUS_ADDR
    mov byte [es:di], 1             ; Available = 1 (VESA mode set)
    and cx, 0x3FFF                  ; Clear flags, keep mode number
    mov word [es:di + 2], cx        ; Store mode number

    ; Get framebuffer physical address from mode info
    mov eax, [es:VESA_MODE_INFO_ADDR + VBE_MODE_PHYS_BASE]
    mov [es:di + 4], eax            ; Store framebuffer address

    ; Print success message with framebuffer address
    mov si, msg_vesa_success
    call serial_print_16

    mov si, msg_vesa_fb_addr
    call serial_print_16
    mov eax, [es:VESA_STATUS_ADDR + 4]
    call vesa_print_hex32
    mov si, msg_newline
    call serial_print_16

    ; Print resolution
    mov si, msg_vesa_resolution
    call serial_print_16
    movzx eax, word [es:VESA_MODE_INFO_ADDR + VBE_MODE_WIDTH]
    call vesa_print_dec16
    mov si, msg_vesa_x
    call serial_print_16
    movzx eax, word [es:VESA_MODE_INFO_ADDR + VBE_MODE_HEIGHT]
    call vesa_print_dec16
    mov si, msg_vesa_x
    call serial_print_16
    movzx eax, byte [es:VESA_MODE_INFO_ADDR + VBE_MODE_BPP]
    call vesa_print_dec16
    mov si, msg_newline
    call serial_print_16

    ; Success!
    clc
    jmp .done

.vesa_not_supported:
    mov si, msg_vesa_not_supported
    call serial_print_16
    jmp .use_vga_fallback

.set_mode_failed:
    mov si, msg_vesa_set_failed
    call serial_print_16
    ; Fall through to VGA fallback

.use_vga_fallback:
    ; VGA text mode is already active, just mark status
    mov si, msg_vesa_vga_fallback
    call serial_print_16

    ; Status already initialized to 0 (VGA fallback)
    clc                             ; Indicate success (VGA is acceptable)
    jmp .done

.done:
    pop dx
    pop cx
    pop bx
    pop si
    pop di
    pop fs
    pop es
    pop bp
    ret

; ===========================================================================
; HELPER SUBROUTINES (prefixed with vesa_ to avoid conflicts)
; ===========================================================================

; ---------------------------------------------------------------------------
; vesa_print_version
; ---------------------------------------------------------------------------
; Print VBE version number (BCD format)
; Input: AX = Version (e.g., 0x0300 for VBE 3.0)
; ---------------------------------------------------------------------------
vesa_print_version:
    push ax
    push bx

    mov si, msg_vesa_version
    call serial_print_16

    ; High byte = major version
    mov bl, ah
    and bl, 0x0F
    add bl, '0'
    mov al, bl
    call vesa_putchar

    mov al, '.'
    call vesa_putchar

    ; Low byte = minor version
    pop bx
    pop ax
    push ax
    mov bl, al
    shr bl, 4
    and bl, 0x0F
    add bl, '0'
    mov al, bl
    call vesa_putchar

    mov al, al                      ; Get low nibble
    pop ax
    and al, 0x0F
    add al, '0'
    call vesa_putchar

    mov si, msg_newline
    call serial_print_16

    ret

; ---------------------------------------------------------------------------
; vesa_print_hex16
; ---------------------------------------------------------------------------
; Print 16-bit value in hex
; Input: AX = value to print
; ---------------------------------------------------------------------------
vesa_print_hex16:
    push ax
    push bx
    push cx

    mov cx, 4                       ; 4 hex digits

.loop:
    rol ax, 4                       ; Rotate left by 4 bits
    mov bx, ax
    and bx, 0x000F                  ; Get low nibble
    cmp bl, 10
    jl .digit
    add bl, 'A' - 10
    jmp .print
.digit:
    add bl, '0'
.print:
    push ax
    mov al, bl
    call vesa_putchar
    pop ax
    loop .loop

    pop cx
    pop bx
    pop ax
    ret

; ---------------------------------------------------------------------------
; vesa_print_hex32
; ---------------------------------------------------------------------------
; Print 32-bit value in hex
; Input: EAX = value to print
; ---------------------------------------------------------------------------
vesa_print_hex32:
    push eax
    push ebx
    push ecx

    mov ecx, 8                      ; 8 hex digits

.loop:
    rol eax, 4                      ; Rotate left by 4 bits
    mov ebx, eax
    and ebx, 0x0000000F             ; Get low nibble
    cmp bl, 10
    jl .digit
    add bl, 'A' - 10
    jmp .print
.digit:
    add bl, '0'
.print:
    push eax
    mov al, bl
    call vesa_putchar
    pop eax
    loop .loop

    pop ecx
    pop ebx
    pop eax
    ret

; ---------------------------------------------------------------------------
; vesa_print_dec16
; ---------------------------------------------------------------------------
; Print 16-bit value in decimal
; Input: AX = value to print
; ---------------------------------------------------------------------------
vesa_print_dec16:
    push ax
    push bx
    push cx
    push dx

    mov bx, 10
    xor cx, cx                      ; Digit counter

.divide_loop:
    xor dx, dx
    div bx                          ; AX = AX / 10, DX = remainder
    push dx                         ; Save digit
    inc cx
    test ax, ax
    jnz .divide_loop

.print_loop:
    pop ax
    add al, '0'
    call vesa_putchar
    loop .print_loop

    pop dx
    pop cx
    pop bx
    pop ax
    ret

; ---------------------------------------------------------------------------
; vesa_putchar
; ---------------------------------------------------------------------------
; Print single character to serial port
; Input: AL = character
; ---------------------------------------------------------------------------
vesa_putchar:
    push ax
    push dx

    mov dx, 0x3F8 + 5               ; Line status register
.wait:
    in al, dx
    test al, 0x20                   ; Transmit buffer empty?
    jz .wait

    mov dx, 0x3F8                   ; Data register
    pop ax                          ; Restore character (was in AL)
    push ax
    out dx, al

    pop dx
    pop ax
    ret

; ===========================================================================
; DATA SECTION
; ===========================================================================

msg_vesa_detecting:     db "[SERIAL] VESA: Detecting VBE support...", 13, 10, 0
msg_vesa_found:         db "[SERIAL] VESA: VBE controller found, version ", 0
msg_vesa_version:       db "VBE ", 0
msg_vesa_searching:     db "[SERIAL] VESA: Searching for 1024x768x32 mode...", 13, 10, 0
msg_vesa_mode_found:    db "[SERIAL] VESA: Found matching mode: 0x", 0
msg_vesa_setting:       db "[SERIAL] VESA: Setting graphics mode...", 13, 10, 0
msg_vesa_success:       db "[SERIAL] VESA: Graphics mode set successfully", 13, 10, 0
msg_vesa_fb_addr:       db "[SERIAL] VESA: Framebuffer at 0x", 0
msg_vesa_resolution:    db "[SERIAL] VESA: Resolution: ", 0
msg_vesa_x:             db "x", 0
msg_vesa_not_supported: db "[SERIAL] VESA: VBE not supported", 13, 10, 0
msg_vesa_mode_not_found:db "[SERIAL] VESA: 1024x768x32 mode not found", 13, 10, 0
msg_vesa_set_failed:    db "[SERIAL] VESA: Failed to set graphics mode", 13, 10, 0
msg_vesa_vga_fallback:  db "[SERIAL] VESA: Using VGA text mode fallback", 13, 10, 0
msg_newline:            db 13, 10, 0
