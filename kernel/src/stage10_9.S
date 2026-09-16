.section .rodata.wovenhat_stage10_9, "a"
.global wovenhat_stage10_9_start
.global wovenhat_stage10_9_end
wovenhat_stage10_9_start:
    lea rbx, [rsp - 512]
    mov eax, 89
    xor edi, edi
    int 0x80
    cmp rax, -1
    je .Lfail
    mov r12, rax
    mov eax, 90
    mov rdi, r12
    mov esi, 1
    int 0x80
    test rax, rax
    jne .Lfail
    mov eax, 91
    mov rdi, r12
    int 0x80
    cmp rax, -1
    je .Lfail
    mov r13, rax
    mov eax, 66
    mov rdi, r13
    mov rsi, rbx
    int 0x80
    cmp rax, 1
    jne .Lfail
    cmp qword ptr [rbx + 32], 1
    jne .Lfail
    mov eax, 93
    mov rdi, r12
    int 0x80
    test rax, rax
    jne .Lfail
    mov eax, 87
    mov edi, 2
    xor esi, esi
    int 0x80
    cmp rax, -1
    je .Lfail
    mov r12, rax
    mov eax, 88
    mov rdi, r12
    int 0x80
    cmp rax, -1
    je .Lfail
    mov r13, rax
    mov eax, 66
    mov rdi, r13
    mov rsi, rbx
    int 0x80
    cmp rax, 1
    jne .Lfail
    cmp qword ptr [rbx + 32], 1
    jne .Lfail
    mov eax, 92
    mov rdi, r12
    int 0x80
    test rax, rax
    jne .Lfail
    xor edi, edi
    mov eax, 3
    int 0x80
.Lfail:
    mov edi, 1
    mov eax, 3
    int 0x80
    ud2
wovenhat_stage10_9_end:
.previous
