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
real SPI/RRAMC/GPIO into that engine (`src/sd.rs`, `src/install.rs`, `src/led.rs`)
and acts on the outcome. The byte formats and the boot decision table are
normative in [`OBCU_Spec.md`](../../OBCU_Spec.md).

The RRAM layout (single source of truth for the app side:
`../obc-fw-nrf54l/build.rs`; this crate's static [`memory.x`](memory.x) mirrors it):

```
0x0000_0000  obc-boot          32 KB   (this crate; CI size-guards the budget)
0x0000_8000  app slot        1484 KB   (obc-fw-nrf54l, linked at 0x8000)
0x0017_B000  BOOT_STATE page    4 KB   (the obc-dfu handoff page)
0x0017_C000  SETTINGS page      4 KB   (the app's persistent settings, #193)
```

## LED codes (LED0 — the bootloader's entire UI)

| Pattern | Meaning |
| :-- | :-- |
| one short pulse | proof-of-life on every entry (then the app boots) |
| slow heartbeat | verifying the staged image (nothing written yet) |
| fast heartbeat | flashing the app slot / readback |
| **2 blinks**, then boot | staged image invalid — arm cleared, old app intact |
| **3 blinks**, pause, repeat | SD missing / read failing — retrying forever with backoff (reinsert card, or power cycle) |
| **SOS**, forever | readback never matched after retries — halted; state still `Armed`, so a power cycle retries the install |

Heartbeat rates scale with card throughput (a toggle per N 4 KB chunks); the
counted codes and SOS are fixed-timing. No display, ever.

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
cargo run --release        # probe-rs run --chip nRF54L15 --verify
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
- **No executor, no timers, no FAT, no FLPR**: blocking embassy-nrf HAL only
  (GPIO + blocking `Spim` + `Rramc`; the card delay source is a cycle-counted
  busy-wait). `embedded_sdmmc::SdCard` is used **without** `VolumeManager` —
  extents are pre-resolved absolute blocks, and `llvm-nm` on the release ELF
  must show no FAT/volume symbols. The app starts the FLPR itself.
- **Deliberate duplication**: SD pins/frequencies and the RRAM write idiom are
  copied from the board crate (`obc-fw-nrf54l/src/sd.rs`, `src/main.rs`,
  `src/settings.rs`) with cross-referencing comments — no shared pins module.
- `main.rs` stays a thin driver (bring-up + outcome dispatch); review is the
  verification for the wiring, the host tests are it for the sequencing.
