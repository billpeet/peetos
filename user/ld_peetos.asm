BITS 64
GLOBAL _start

SECTION .text
_start:
    ; Stack layout on entry:
    ;   [rsp + 0x00] = argc
    ;   [rsp + 0x08] = argv[0]
    ;   ...
    ;   NULL
    ;   envp...
    ;   NULL
    ;   auxv pairs (type, value), terminated by AT_NULL
    mov rbx, rsp
    mov rcx, [rbx]              ; argc
    lea rbx, [rbx + 8]          ; argv start
    lea rbx, [rbx + rcx * 8 + 8] ; skip argv and trailing NULL

.skip_envp:
    mov rax, [rbx]
    add rbx, 8
    test rax, rax
    jne .skip_envp

.scan_auxv:
    mov rax, [rbx]              ; a_type
    mov rdx, [rbx + 8]          ; a_val
    cmp rax, 9                  ; AT_ENTRY
    je .jump_entry
    test rax, rax               ; AT_NULL
    je .fail
    add rbx, 16
    jmp .scan_auxv

.jump_entry:
    jmp rdx

.fail:
    mov rax, 60                 ; SYS_EXIT
    mov rdi, 127
    int 0x80
.halt:
    hlt
    jmp .halt
