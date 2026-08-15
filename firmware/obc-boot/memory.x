/* obc-boot memory map — the bootloader's 32 KB slot at the bottom of the nRF54LM20's RRAM.
 *
 * The full device layout (epic #615; #1158 for the stage carve; the app side of it is emitted by
 * ../obc-fw-nrf54l/build.rs — keep the two in agreement):
 *
 *   0x0000_0000  obc-boot           32 KB   (this crate — FLASH below)
 *   0x0000_8000  app slot         1976 KB   (the board crate, linked at 0x8000)
 *   0x001F_6000  SEMMC_STAGE        20 KB   (the armer-staged sEMMC blob — OBCU_Spec.md §3, read here)
 *   0x001F_B000  BOOT_STATE page     4 KB   (the obc-dfu handoff page — read here)
 *   0x001F_C000  SETTINGS page       4 KB   (the app's persistent settings, #193 — never ours)
 *
 * RAM is 480 KB — the same extent the app links (#1158): the region above 0x2007_8000 is the
 * coprocessor territory this bootloader now genuinely uses. On the Install/Rollback paths it
 * copies the staged sEMMC image into the SEMMC carve and runs the FLPR there to reach the card
 * (semmc.rs), so the M33's stack must never overlap it — the pre-port note that boot's stack
 * could harmlessly start inside the carve is dead. The display FLPR's image/stack and the SHARED
 * handshake page above it stay untouched (the app owns the display blob's whole lifecycle), and
 * the top 4 KB VPR-context/ProtectedRAM reservation is never mapped by anyone:
 *
 *   0x2000_0000  M33 RAM           480 KB   (RAM below — .data/.bss/stack, boot runs alone)
 *   0x2007_8000  SEMMC carve        20 KB   (sEMMC image + VRI — INITPC target, not linked)
 *   0x2007_D000  FLPR display        4 KB   (the app's display blob — never ours)
 *   0x2007_E000  SHARED page         4 KB   (cross-core handshake — never ours)
 *   0x2007_F000  reserved            4 KB   (VPR context / ProtectedRAM)
 *
 * Unlike the board crate (whose memory.x is generated into $OUT_DIR by build.rs), this file is
 * static and committed: build.rs copies it to $OUT_DIR for cortex-m-rt's `INCLUDE memory.x`.
 */
MEMORY
{
    FLASH       : ORIGIN = 0x00000000, LENGTH = 32K
    SEMMC_STAGE : ORIGIN = 0x001F6000, LENGTH = 20K  /* armer-staged sEMMC blob (OBCU_Spec.md §3) */
    BOOT_STATE  : ORIGIN = 0x001FB000, LENGTH = 4K   /* boot-state handoff page (OBCU_Spec.md §2) */
    RAM         : ORIGIN = 0x20000000, LENGTH = 480K
}

/* Base of the boot-state page — same symbol convention as the app's `__settings_base`
 * (the magic address lives only in the linker script; main.rs reads the symbol). */
PROVIDE(__boot_state_base = ORIGIN(BOOT_STATE));

/* Base of the blob-stage carve (#1158, OBCU_Spec.md §3) — where the armer left the sEMMC
 * soft-peripheral image this bootloader boots the card through. Also the app slot's END:
 * the install engine's slot length is `__semmc_stage_base - __app_slot_base`, so nothing
 * the engine flashes can ever touch the carve. */
PROVIDE(__semmc_stage_base = ORIGIN(SEMMC_STAGE));

/* Base of the sEMMC execution carve in RAM — one past this crate's RAM region, mirroring the
 * app's build.rs contract (SEMMC_RAM_BASE = 0x2007_8000). semmc.rs copies the staged image
 * here and points VPR00.INITPC at it. */
PROVIDE(__semmc_ram_base = ORIGIN(RAM) + LENGTH(RAM));

/* Base of the app slot — the app's link origin (0x8000), which is exactly one past the
 * bootloader's own FLASH region. Derived so growing the bootloader (bump LENGTH(FLASH))
 * shifts the app base automatically; main.rs reads this symbol instead of a literal, the
 * same "magic address lives only in the linker script" rule as `__boot_state_base`. Mirrors
 * the board crate's `__app_slot_base` symbol (there FLASH *is* the app slot, so it PROVIDEs
 * `ORIGIN(FLASH)`; here FLASH is the bootloader, so the app base is its end). */
PROVIDE(__app_slot_base = ORIGIN(FLASH) + LENGTH(FLASH));
