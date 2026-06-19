/* STM32F429ZI (F429I-DISC1): 2 MB flash, 192 KB contiguous SRAM. (There's also a
   separate 64 KB CCM at 0x10000000, but cortex-m-rt wants the stack above .bss and
   CCM sits below SRAM, so we keep the normal layout: everything in SRAM. The
   small-scratch renderer (~151 KB) + a small task arena + stack fit comfortably.) */
MEMORY
{
    FLASH : ORIGIN = 0x08000000, LENGTH = 2048K
    RAM   : ORIGIN = 0x20000000, LENGTH = 192K
}
