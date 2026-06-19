/* Raspberry Pi Pico / generic RP2040 with a 2 MB QSPI flash (W25Q16).
   If your board has a different flash size, adjust FLASH LENGTH. */
MEMORY {
    BOOT2 : ORIGIN = 0x10000000, LENGTH = 0x100
    FLASH : ORIGIN = 0x10000100, LENGTH = 2048K - 0x100
    RAM   : ORIGIN = 0x20000000, LENGTH = 264K
}

/* The 256-byte second-stage bootloader (emitted by embassy-rp as a `.boot2`
   static) must sit in the first flash page; cortex-m-rt's link.x lays out .text
   after this, and the vector table at FLASH ORIGIN (0x10000100). */
SECTIONS {
    .boot2 ORIGIN(BOOT2) :
    {
        KEEP(*(.boot2));
    } > BOOT2
} INSERT BEFORE .text;
