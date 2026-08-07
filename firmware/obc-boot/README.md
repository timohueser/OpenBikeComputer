# obc-boot — the nRF54L bootloader

The 32 KB first-stage bootloader for the SD-staged DFU path (epic #615): it reads
the `BOOT_STATE` RRAM page, decodes it with the shared, host-tested
[`obc-dfu`](../obc-dfu) crate (anything torn or blank decodes to `Idle`), and runs
the **install engine** (S3, #618): `Armed` → CRC-verify the staged image over its
raw SD block extents → flash the app slot via RRAMC → readback-verify → write
`Trial` → jump straight into the new image (the one trial boot — never a reset,
which would re-enter the bootloader and read the fresh `Trial` as unconfirmed);
a `Trial` still present at a *later* entry rolls back the same way. All install
*sequencing* — pass ordering, retry counts, every failure edge — lives in
`obc_dfu::engine` and is unit-tested there with mock IO; this crate only wires
the real card transport/RRAMC/GPIO into that engine (`src/semmc.rs`,
`src/install.rs`, `src/led.rs`) and acts on the outcome. The byte formats and the
boot decision table are normative in [`OBCU_Spec.md`](../../specs/OBCU_Spec.md).

**The card transport is the sEMMC soft peripheral** (#1158 — the SPI path is
deleted; its pins are the display's now): the card only exists behind a ~13.6 KB
coprocessor image this crate cannot afford to embed, so the app-side armer stages
that image into the `SEMMC_STAGE` RRAM carve before every arm (`OBCU_Spec.md` §3)
and `src/semmc.rs` validates it — the CRC frame plus the image's own metadata,
through the shared `obc_dfu::blobstage` — before booting it on the FLPR. An
`Armed` page whose carve fails validation is **abandoned** like DR3's unreadable
card (the slot is untouched, no retry can heal a bad carve); a `Rollback` whose
carve fails validation parks on SOS (a power cycle retries — near-unreachable, by
the armer's stage-before-arm ordering).

The RRAM layout (single source of truth for the app side:
`../obc-fw-nrf54l/build.rs`; this crate's static [`memory.x`](memory.x) mirrors it):

```
0x0000_0000  obc-boot           32 KB   (this crate; CI size-guards the budget)
0x0000_8000  app slot         1976 KB   (obc-fw-nrf54l, linked at 0x8000)
0x001F_6000  SEMMC_STAGE        20 KB   (the armer-staged sEMMC blob, #1158)
0x001F_B000  BOOT_STATE page     4 KB   (the obc-dfu handoff page)
0x001F_C000  SETTINGS page       4 KB   (the app's persistent settings, #193)
```

## LED codes (LED0 — the bootloader's entire UI)

| Pattern | Meaning |
| :-- | :-- |
| one short pulse | proof-of-life on every entry (then the app boots) |
| slow heartbeat | verifying the staged image (nothing written yet) |
| fast heartbeat | flashing the app slot / readback |
| **2 blinks**, then boot | arm cleared, old app intact — the staged image failed verification, or (DR3) an `Armed` card stayed unreadable past the retry budget, or (#1158) the staged sEMMC blob failed validation; the untouched arm was abandoned |
| **3 blinks**, pause, repeat | SD missing / read failing — retrying with backoff (reinsert card, or power cycle). A pre-erase `Armed` arm gives up after ~a minute (`ARM_ABANDON_ROUNDS`) and abandons it (→ 2 blinks, old app boots); a `Rollback` or a mid-flash error retries forever (never abandon a touched slot) |
| **SOS**, forever | readback never matched after retries — halted; state still `Armed`, so a power cycle retries the install |

Heartbeat rates scale with card throughput (a toggle per N 4 KB chunks); the
counted codes and SOS are fixed-timing. The bootloader never *draws* — but the
panel isn't dark during an install: the app paints a static "Installing update"
card as its last frame before the arm's reset, and this crate keeps it alive
(`src/com.rs`) by parking the LS021's scan pins driven-low and free-running the
anti-DC-bias COM wave in software, paced off the DWT cycle counter from the same
chokepoints that pet the watchdog. The memory-in-pixel glass holds the frame; the
LED stays the only *output* the bootloader produces.

## Build

Standalone crate, workspace-excluded like the board crate. **Build it from inside
this directory** — the `thumbv8m.main-none-eabihf` target comes from the
crate-local `.cargo/config.toml`, which cargo discovers by **working directory**:
building via `--manifest-path` from elsewhere silently targets the **host** and
fails on embassy-nrf.

```sh
cd firmware/obc-boot
cargo build --release

# Debug build with defmt over RTT (never the shipping shape — the 32 KB budget is
# measured with rtt OFF). Also enables the DWT-based install throughput report
# (verify/flash/readback wall time + KiB/s per phase).
cargo build --release --features rtt
```

## Flash — once

`probe-rs` flashes ELFs at their linked addresses, so the workflow is: flash
`obc-boot` **once**, then iterate on the app exactly as before (its ELF links at
`0x8000`, and flashing it never touches the bootloader's 32 KB).

```sh
cd firmware/obc-boot
cargo run --release        # probe-rs run --chip nRF54LM20A --verify  (or: obc flash-boot)
```

**The flash-twice DK quirk applies here too:** the first probe-rs flash after
powering the DK up often fails (or `--verify` reports a mismatch) — just run the
command again. `--verify` is what turns the silent RRAM corruption into a loud
failure, so a retry is always safe.

After flashing, a power cycle should show one short LED0 blink (the bootloader)
and then the app booting exactly as before.

## Recovering a device with no bootloader

A chip-erased or fresh device (or one where only the old, `0x0`-linked app is
present) needs **both** images — order doesn't matter, each flash only writes its
own address range:

```sh
cd firmware/obc-boot      && cargo run --release   # bootloader @ 0x0
cd ../obc-fw-nrf54l       && cargo run --release   # app @ 0x8000
```

Symptoms of a missing piece: no LED blink and no boot at all → no bootloader at
`0x0`; one blink then nothing → no (or a pre-#617, `0x0`-linked) app at `0x8000`.

## Design constraints (hold these in review)

- **≤ 32 KB** flash (`.text` + `.rodata` + `.data`), CI-guarded via `llvm-size`.
  The `rtt` feature must never be load-bearing for the budget.
- **Never panics**: all decode/decision/sequencing logic lives upstream in
  `obc-dfu` (torn page ⇒ `Idle` ⇒ jump; the engine is total over any page
  content), and the IO adapters here carry no unwraps.
- **Verify before erase**: the engine never writes the app slot until the staged
  image's CRC has passed over the full extent chain — a bad `UPDATE.BIN` costs
  nothing (host-tested, epic invariant 1).
- **Watchdog-aware, never watchdog-started for an install** (DR1, #729): the arm
  path enters through a warm reset that carries the app's live 24 s WDT, so the
  bootloader adopts and pets it (engine progress, the SD retry loops, and the SOS
  park — the parks must stay parks); before jumping into a **trial** boot it
  starts the identical dog so a wedged trial resets into the rollback. A plain
  `Idle` boot never touches the WDT, and a cold-boot install stays dog-less until
  that trial jump. The config contract lives on `obc_dfu::WDT_TIMEOUT_TICKS`.
  Flip side: every trial boot now runs its bring-up under a counting dog, so the
  app must reach its own WDT adoption well inside one 24 s period (invariant
  comment at the app's WDT setup in `obc-fw-nrf54l/src/main.rs`).
- **No executor, no timers, no FAT, no interrupts**: blocking embassy-nrf HAL
  (GPIO + `Rramc`) plus raw, polled MMIO for the sEMMC transport — every wait is
  a DWT-cycle-bounded deadline, and the VPR00 completion vector is never bound.
  Extents are pre-resolved absolute blocks and `llvm-nm` on the release ELF must
  show no FAT/volume symbols. The **one** coprocessor use is deliberate and
  scoped (#1158): the card only exists behind the sEMMC soft peripheral, so the
  Install/Rollback paths boot the armer-staged image on the FLPR and park the
  hart + reset the pads again before the jump; the display blob stays entirely
  the app's. The panel keep-alive (`src/com.rs`) holds the line: no display
  driver, no framebuffer — just parked pins and a CYCCNT-paced GPIO toggle woven
  into the existing waits.
- **Deliberate duplication**: the sEMMC driver is a subtractive port of the
  board crate's (`obc-fw-nrf54l/src/semmc.rs` — laws, barrier, CMD8 workaround
  inherited verbatim; `src/semmc.rs`'s module doc lists exactly what was dropped
  and why), and the panel/COM pins + the RRAM write idiom are copied from
  `src/main.rs` / `src/settings.rs` with cross-referencing comments — no shared
  pins module. The carve geometry both crates must agree on lives in the shared
  `obc_dfu::blobstage` constants and the two `memory.x` maps.
- `main.rs` stays a thin driver (bring-up + outcome dispatch); review is the
  verification for the wiring, the host tests are it for the sequencing.
