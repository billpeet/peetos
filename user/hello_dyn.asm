BITS 64
GLOBAL _start

SECTION .text
_start:
    mov rax, 1          ; SYS_WRITE
    mov rdi, 1          ; stdout
    lea rsi, [rel msg]
    mov rdx, msg_len
    int 0x80

    mov rax, 60         ; SYS_EXIT
    xor rdi, rdi
    int 0x80

.halt:
    hlt
    jmp .halt

SECTION .rodata
msg: db "hello from dynamic user image", 10
msg_len: equ $ - msg
