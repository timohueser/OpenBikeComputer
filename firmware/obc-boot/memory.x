/* obc-boot memory map — the bootloader's 32 KB slot at the bottom of the nRF54L15's RRAM.
 *
 * The full device layout (epic #615; the app side of it is emitted by
 * ../obc-fw-nrf54l/build.rs — keep the two in agreement):
 *
 *   0x0000_0000  obc-boot          32 KB   (this crate — FLASH below)
 *   0x0000_8000  app slot        1484 KB   (the board crate, linked at 0x8000)
 *   0x0017_B000  BOOT_STATE page    4 KB   (the obc-dfu handoff page — read here)
 *   0x0017_C000  SETTINGS page      4 KB   (the app's persistent settings, #193 — never ours)
 *
 * RAM is the full 256 KB: the bootloader runs alone (the app re-initialises RAM from its own
 * reset path after the jump, so nothing here needs to survive). No FLPR carve — the bootloader
 * never touches the FLPR coprocessor; the app starts it itself.
 *
 * Unlike the board crate (whose memory.x is generated into $OUT_DIR by build.rs), this file is
 * static and committed: build.rs copies it to $OUT_DIR for cortex-m-rt's `INCLUDE memory.x`.
 */
MEMORY
{
    FLASH      : ORIGIN = 0x00000000, LENGTH = 32K
    BOOT_STATE : ORIGIN = 0x0017B000, LENGTH = 4K   /* boot-state handoff page (OBCU_Spec.md §2) */
    RAM        : ORIGIN = 0x20000000, LENGTH = 256K
}

/* Base of the boot-state page — same symbol convention as the app's `__settings_base`
 * (the magic address lives only in the linker script; main.rs reads the symbol). */
PROVIDE(__boot_state_base = ORIGIN(BOOT_STATE));

/* Base of the app slot — the app's link origin (0x8000), which is exactly one past the
 * bootloader's own FLASH region. Derived so growing the bootloader (bump LENGTH(FLASH))
 * shifts the app base automatically; main.rs reads this symbol instead of a literal, the
 * same "magic address lives only in the linker script" rule as `__boot_state_base`. Mirrors
 * the board crate's `__app_slot_base` symbol (there FLASH *is* the app slot, so it PROVIDEs
 * `ORIGIN(FLASH)`; here FLASH is the bootloader, so the app base is its end). */
PROVIDE(__app_slot_base = ORIGIN(FLASH) + LENGTH(FLASH));
