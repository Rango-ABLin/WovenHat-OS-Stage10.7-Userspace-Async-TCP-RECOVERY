.section .rodata.wovenhat_stage10_8, "a"
.global wovenhat_stage10_8_start
.global wovenhat_stage10_8_end
wovenhat_stage10_8_start:
    lea rbx, [rsp - 512]
    lea rbp, [rsp - 640]
    mov eax, 82
    int 0x80
    cmp rax, -1
    je .Lport_fail
    mov r12, rax
    mov eax, 64
    mov edi, 3
    int 0x80
    mov r13, rax
    mov eax, 84
    mov rdi, r13
    mov rsi, r12
    mov edx, 11
    int 0x80
    test rax, rax
    jne .Lport_fail
    mov eax, 68
    mov rdi, r13
    xor esi, esi
    mov edx, 101
    int 0x80
    test rax, rax
    jne .Lport_fail
    // Complete a second operation before association.
    mov eax, 64
    mov edi, 3
    int 0x80
    mov r14, rax
    mov eax, 68
    mov rdi, r14
    xor esi, esi
    mov edx, 202
    int 0x80
    test rax, rax
    jne .Lport_fail
    mov eax, 84
    mov rdi, r14
    mov rsi, r12
    mov edx, 22
    int 0x80
    test rax, rax
    jne .Lport_fail
    mov eax, 85
    mov rdi, r12
    mov rsi, -1
    mov edx, 8
    int 0x80
    cmp rax, -1
    jne .Lport_fail
    mov eax, 85
    mov rdi, r12
    mov rsi, rbx
    mov edx, 8
    int 0x80
    cmp rax, 2
    jne .Lport_fail
    cmp [rbx], r13
    jne .Lport_fail
    cmp qword ptr [rbx + 8], 11
    jne .Lport_fail
    cmp qword ptr [rbx + 32], 101
    jne .Lport_fail
    cmp [rbx + 40], r14
    jne .Lport_fail
    cmp qword ptr [rbx + 48], 22
    jne .Lport_fail
    cmp qword ptr [rbx + 72], 202
    jne .Lport_fail
    // Notifications do not consume the original operation result.
    mov eax, 65
    mov rdi, r13
    mov rsi, rbx
    int 0x80
    cmp rax, 1
    jne .Lport_fail
    mov eax, 65
    mov rdi, r14
    mov rsi, rbx
    int 0x80
    cmp rax, 1
    jne .Lport_fail
    mov eax, 64
    mov edi, 3
    int 0x80
    mov r15, rax
    mov eax, 84
    mov rdi, r15
    mov rsi, r12
    mov edx, 33
    int 0x80
    test rax, rax
    jne .Lport_fail
    mov eax, 67
    mov rdi, r15
    int 0x80
    test rax, rax
    jne .Lport_fail
    mov qword ptr [rbp], 1
    mov qword ptr [rbp + 8], 0
    mov eax, 86
    mov rdi, r12
    mov rsi, rbx
    mov rdx, rbp
    int 0x80
    cmp rax, 1
    jne .Lport_fail
    cmp dword ptr [rbx + 20], 1
    jne .Lport_fail
    cmp dword ptr [rbx + 24], -2
    jne .Lport_fail
    // A finite wait on an empty port must return without polling in Ring 3.
    mov eax, 31
    int 0x80
    mov r15, rax
    mov qword ptr [rbp + 8], 3
    mov eax, 86
    mov rdi, r12
    mov rsi, rbx
    mov rdx, rbp
    int 0x80
    test rax, rax
    jne .Lport_fail
    mov eax, 31
    int 0x80
    sub rax, r15
    cmp rax, 3
    jb .Lport_fail
    mov eax, 83
    mov rdi, r12
    int 0x80
    test rax, rax
    jne .Lport_fail
    mov eax, 85
    mov rdi, r12
    mov rsi, rbx
    mov edx, 1
    int 0x80
    cmp rax, -1
    jne .Lport_fail
    // Abandon a port and operation to normal process teardown.
    mov eax, 82
    int 0x80
    mov r12, rax
    mov eax, 64
    mov edi, 3
    int 0x80
    mov rdi, rax
    mov rsi, r12
    xor edx, edx
    mov eax, 84
    int 0x80
    test rax, rax
    jne .Lport_fail
    xor edi, edi
    mov eax, 3
    int 0x80
.Lport_fail:
    mov edi, 1
    mov eax, 3
    int 0x80
    ud2
wovenhat_stage10_8_end:
.previous
