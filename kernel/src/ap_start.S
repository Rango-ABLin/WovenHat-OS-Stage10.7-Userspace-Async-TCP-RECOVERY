// Copied to a reserved, usable page below 1 MiB. All data is image-relative.
.section .rodata.ap_start,"a"
.code16
.global ap_start, ap_end, ap_root, ap_stack, ap_entry, ap_gdt_ptr, ap_far, ap_long, ap_gdt
.set gdt_offset, ap_gdt_ptr - ap_start
.set root_offset, ap_root - ap_start
.set far_offset, ap_far - ap_start
ap_start:
    cli
    cld
    mov ax, cs
    mov ds, ax
    lgdt [gdt_offset]
    mov eax, 0x620
    mov cr4, eax
    mov eax, dword ptr [root_offset]
    mov cr3, eax
    mov ecx, 0xc0000080
    rdmsr
    or eax, 0x900
    wrmsr
    mov eax, cr0
    and eax, 0xfffffff3
    or eax, 0x80010023
    mov cr0, eax
    .byte 0x66, 0xff, 0x2e
    .word far_offset
.code64
ap_long:
    mov ax, 16
    mov ds, ax
    mov es, ax
    mov ss, ax
    xor eax, eax
    mov fs, ax
    mov gs, ax
    mov rsp, qword ptr [rip + ap_stack]
    xor rbp, rbp
    mov rax, qword ptr [rip + ap_entry]
    call rax
1:  cli
    hlt
    jmp 1b
.balign 8
ap_root: .quad 0
ap_stack: .quad 0
ap_entry: .quad 0
ap_far: .long 0
    .word 8
ap_gdt_ptr: .word 23
    .long 0
.balign 8
ap_gdt:
    .quad 0
    .quad 0x00af9a000000ffff
    .quad 0x00cf92000000ffff
ap_end:
.code64
