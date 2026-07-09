# obc-boot — the nRF54L bootloader

The 32 KB first-stage bootloader for the SD-staged DFU path (epic #615): it reads
the `BOOT_STATE` RRAM page, decodes it with the shared, host-tested
[`obc-dfu`](../obc-dfu) crate (anything torn or blank decodes to `Idle`), blinks
LED0 once as proof-of-life, and jumps to the app at `0x8000`. **S2 (#617) ships no
install logic** — every decision currently resolves to the jump; S3 (#618) adds
the verify → flash → trial → rollback engine. The byte formats and the boot
decision table are normative in [`OBCU_Spec.md`](../../OBCU_Spec.md).

The RRAM layout (single source of truth for the app side:
`../obc-fw-nrf54l/build.rs`; this crate's static [`memory.x`](memory.x) mirrors it):

```
0x0000_0000  obc-boot          32 KB   (this crate; CI size-guards the budget)
0x0000_8000  app slot        1484 KB   (obc-fw-nrf54l, linked at 0x8000)
0x0017_B000  BOOT_STATE page    4 KB   (the obc-dfu handoff page)
0x0017_C000  SETTINGS page      4 KB   (the app's persistent settings, #193)
```

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
# measured with rtt OFF):
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
- **Never panics**: all decode/decision logic lives upstream in `obc-dfu` (torn
  page ⇒ `Idle` ⇒ jump), and unimplemented decision arms fall through to the jump
  rather than `todo!()`.
- **No executor, no timers, no FAT, no FLPR**: blocking embassy-nrf HAL only
  (GPIO now; S3 adds blocking `Spim` + `Rramc`). The app starts the FLPR itself.
- `main.rs` stays small (~150 lines) — review is the verification; there is no
  on-target test harness.
