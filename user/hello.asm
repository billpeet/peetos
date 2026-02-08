BITS 64
GLOBAL _start

SECTION .text
_start:
    mov rax, 60         ; SYS_EXIT (Linux x86_64)
    mov rdi, 42         ; exit code
    int 0x80
    hlt
    jmp $
