/* start.S — FLPR reset entry for the LS021 F0 blink blob (issue #150, epic #149).

   The M33 sets VPR00.INITPC to the FLPR_RAM base and releases the core, so the very first
   instruction the FLPR fetches is `_start` here (placed first by flpr.ld via `.text.start`).
   There is no C runtime (-nostartfiles): set the stack pointer, zero .bss, call flpr_main.

   RV32E (-mabi=ilp32e) has only x0..x15; we touch x0(zero)/x1(ra)/x2(sp)/x5(t0)/x6(t1),
   all inside the E register set. */

    .section .text.start, "ax"
    .global _start
_start:
    la   sp, _stack_top         /* stack grows down from the top of FLPR_RAM */

    /* Zero .bss. Empty for this blob (_bss_start == _bss_end → the loop runs 0 times),
       but it keeps the C contract honest for when the FLPR firmware grows. */
    la   t0, _bss_start
    la   t1, _bss_end
1:  bgeu t0, t1, 2f
    sw   zero, 0(t0)
    addi t0, t0, 4
    j    1b

2:  call flpr_main              /* never returns */
3:  j    3b                     /* spin if it ever does (keeps the core off the weeds) */
