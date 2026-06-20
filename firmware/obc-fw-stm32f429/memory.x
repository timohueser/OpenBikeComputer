/* STM32F429ZI (F429I-DISC1): 2 MB flash, 192 KB contiguous SRAM at 0x20000000.
   (There is also a separate 64 KB CCM at 0x10000000, not declared here.)

   The display framebuffer does NOT live in these regions: it sits in the external
   8 MB FMC SDRAM at 0xD0000000, which is not memory-mapped until the FMC is brought
   up at runtime. cortex-m-rt must not try to zero/copy into it, so SDRAM is managed
   manually (a raw slice constructed after `Fmc::init`), never via a linker section. */
MEMORY
{
    FLASH : ORIGIN = 0x08000000, LENGTH = 2048K
    RAM   : ORIGIN = 0x20000000, LENGTH = 192K
}
