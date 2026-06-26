/* flpr.ld — linker script for the LS021 FLPR F0 blink blob (issue #150, epic #149).

   The FLPR (VPR RISC-V coprocessor) executes from on-chip SRAM at the *M33-visible*
   address (no remap on the nRF54L15). The M33 copies this image to the FLPR_RAM base and
   writes that base into VPR00.INITPC, so the entry (`_start`, in `.text.start`) MUST be
   the very first byte at the base address — KEEP it first below.

   Region matches the feature-gated `memory.x` carve-out (build.rs) and
   firmware/docs/ls021-flpr.md:

     FLPR_RAM 0x2003_8000 .. 0x2003_F000  (28 KB) — this image + the FLPR stack
     SHARED   0x2003_F000 .. 0x2004_0000  (4 KB)  — cross-core handshake (NOT linked here;
                                                    both cores reach it by hardcoded address)

   The blob is freestanding: -nostdlib -nostartfiles, integer ops only, so there is no
   libgcc/newlib and no init/fini arrays to run. */

MEMORY
{
    FLPR_RAM (rwx) : ORIGIN = 0x20038000, LENGTH = 28K
}

/* Stack grows down from the top of FLPR_RAM, i.e. the boundary with the SHARED block. */
_stack_top = ORIGIN(FLPR_RAM) + LENGTH(FLPR_RAM);

ENTRY(_start)

SECTIONS
{
    .text ORIGIN(FLPR_RAM) :
    {
        KEEP(*(.text.start))      /* _start MUST be first — INITPC points right here */
        *(.text .text.*)
        *(.rodata .rodata.*)
    } > FLPR_RAM

    .data : { *(.data .data.*) } > FLPR_RAM

    .bss :
    {
        _bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        _bss_end = .;
    } > FLPR_RAM

    /* Toolchain metadata the bare-metal blob does not need. */
    /DISCARD/ : { *(.eh_frame*) *(.comment) *(.riscv.attributes) *(.note*) }
}
