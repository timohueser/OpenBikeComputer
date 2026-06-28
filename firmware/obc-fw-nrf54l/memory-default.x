/* nRF54L15 (nRF54L15-DK), `nrf54l15-app-s` secure-app core — the DEFAULT (no-FLPR) region map.

   ⚠️ This file is named `memory-default.x`, NOT `memory.x`, on purpose: cortex-m-rt's `INCLUDE
   memory.x` resolves from the linker's CWD (this crate root) *before* the `-L $OUT_DIR` path, so a
   crate-root `memory.x` would shadow the carved copy `build.rs` writes under the FLPR features and
   the carve would silently never apply (issue #165). build.rs `include_bytes!`es this file into
   `$OUT_DIR/memory.x` for the default build; the FLPR build emits its own carved map there instead.

   Base/length taken from embassy-nrf's `nrf54l15-app-s` memory.x.

   FLASH 1524K @ 0x0000_0000 — the application-core RRAM in the app-s partition.
   A future MCUboot retrofit re-partitions this (bootloader + image slots), and the
   nRF has no USB so the field-update path is DFU-over-BLE, not a UF2 drop. So do NOT
   hard-code any flash offset/size elsewhere — treat this purely as the whole-image
   budget for now (see epic #120).

   RAM 256K @ 0x2000_0000 — the full on-chip SRAM. embassy's example reserves the top
   128K for its FLPR coprocessor demo; the default map firmware reclaims all of it: the
   ST7789 display path runs on the M33 via SPIM-DMA (#122). The renderer scratch + caches
   are tight against this 256K — the board memory profile + budget assert is #124.

   The LS021 FLPR backend (epic #149) DOES carve the top 12K out for the coprocessor, but only
   under the `ls021-flpr` / `panel-ls021` features: build.rs emits a *carved* memory.x (RAM → 244K)
   in those builds instead of this file, so the default firmware here is unaffected. See
   firmware/docs/ls021-flpr.md + build.rs. A future BLE controller would carve here too. */
MEMORY
{
    FLASH : ORIGIN = 0x00000000, LENGTH = 1524K
    RAM   : ORIGIN = 0x20000000, LENGTH = 256K
}
