BITS 64
GLOBAL _start

SECTION .text
_start:
    mov rax, 1          ; SYS_EXIT
    mov rdi, 42         ; exit code
    int 0x80
    hlt
    jmp $
