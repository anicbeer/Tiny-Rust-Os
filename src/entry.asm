    .section .text.entry
    .globl _start
    .align 2
_start:
    # a0 = hartid, a1 = dtb pointer
    # Only hart 0 runs; others park.
    bnez a0, park

    # Set stack pointer (top of a 128KB stack)
    la sp, boot_stack_top

    # Save dtb pointer for later
    mv s1, a1

    # Clear BSS
    la t0, bss_start
    la t1, bss_end
1:
    bgeu t0, t1, 2f
    sd zero, (t0)
    addi t0, t0, 8
    j 1b
2:
    # Call rust_main(hartid, dtb)
    mv a0, zero
    mv a1, s1
    call rust_main

park:
    wfi
    j park

    .section .bss
    .align 12
boot_stack:
    .space 131072
boot_stack_top:
