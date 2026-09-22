# obc-boot — the nRF54L bootloader

The 32 KB first-stage bootloader for the SD-staged DFU path. It reads the `BOOT_STATE` RRAM page,
decodes it with the shared [`obc-dfu`](../obc-dfu) crate, and runs `obc_dfu::engine`: an `Armed`
page verifies the staged image's CRC, flashes the app slot through RRAMC, verifies the readback,
writes `Trial` and jumps into the new image; a `Trial` still present at a later entry rolls back.
This crate only wires the card transport, RRAMC and GPIO into that engine (`src/semmc.rs`,
`src/install.rs`, `src/led.rs`). The byte formats and the boot decision table are normative in
[`OBCU_Spec.md`](../../specs/OBCU_Spec.md).

The card transport is the sEMMC soft peripheral. The card exists only behind a ~13.6 KB
coprocessor image this crate cannot embed, so the app-side armer stages that image into the
`SEMMC_STAGE` RRAM carve before every arm and `src/semmc.rs` validates it before it boots it on
the FLPR.

RRAM layout ([`memory.x`](memory.x), which must agree with `../obc-fw-nrf54l/build.rs`):

| Address | Region | Size |
| --- | --- | ---: |
| `0x0000_0000` | `obc-boot` (this crate) | 32 KB |
| `0x0000_8000` | app slot (`obc-fw-nrf54l`) | 1976 KB |
| `0x001F_6000` | `SEMMC_STAGE` (the staged sEMMC blob) | 20 KB |
| `0x001F_B000` | `BOOT_STATE` page | 4 KB |
| `0x001F_C000` | `SETTINGS` page (the app's) | 4 KB |

## LED codes (LED0 — the bootloader's entire UI)

| Pattern | Meaning |
| :-- | :-- |
| one short pulse | proof of life on every entry, then the app boots |
| slow heartbeat | verifying the staged image; nothing is written yet |
| fast heartbeat | flashing the app slot and reading it back |
| 2 blinks, then boot | the arm was abandoned and the old app is intact: the staged image or the staged sEMMC blob failed verification, or the card stayed unreadable past the retry budget |
| 3 blinks, pause, repeat | the card is missing or reads are failing; retrying with backoff. Reinsert the card or power-cycle. A pre-erase `Armed` arm gives up after about a minute; a `Rollback` or a mid-flash error retries forever |
| SOS, forever | the readback never matched. Halted, still `Armed`, so a power cycle retries the install |

The panel is not dark during an install: the app paints a static "Installing update" card as its
last frame, and `src/com.rs` keeps the memory-in-pixel glass alive with a software COM wave.

## Build

Standalone, workspace-excluded crate. **Build it from inside this directory.** The
`thumbv8m.main-none-eabihf` target comes from the crate-local `.cargo/config.toml`, which cargo
finds by working directory: building through `--manifest-path` from elsewhere silently targets the
host and fails.

```sh
cd firmware/obc-boot
cargo build --release

# Debug only: defmt over RTT, plus the install throughput report. Never the shipping shape —
# the 32 KB budget is measured with rtt off.
cargo build --release --features rtt
```

## Flash — once

probe-rs writes each ELF at its linked address, so flash `obc-boot` once and then iterate on the
app; an app reflash never touches the bootloader's 32 KB.

```sh
cd firmware/obc-boot
cargo run --release        # or, from the repo root: obc flash-boot
```

A power cycle then shows one short LED0 blink, and the app boots. If a flash fails, keep the
output and run `obc board doctor` before the next attempt; see the
[board connection guide](../obc-fw-nrf54l/README.md#board-connection-and-recovery).

## Recovering a device with no bootloader

A chip-erased or fresh device needs both images. Order does not matter, because each flash writes
only its own address range.

```sh
cd firmware/obc-boot      && cargo run --release   # bootloader @ 0x0
cd ../obc-fw-nrf54l       && cargo run --release   # app @ 0x8000
```

No LED blink and no boot means there is no bootloader at `0x0`. One blink and then nothing means
there is no app at `0x8000`.

## Bring-up: the install path runs only on glass

The `Idle` fast path is the only path an ordinary boot takes, and no host test can run this crate
at all. The card, the FLPR, the RRAM writes, the trial boot and the rollback therefore have one
verification: a board. Run it after any change to `src/semmc.rs`, `src/install.rs` or the engine
wiring.

1. Build the app twice, with version strings that differ. Wrap and sign both with `obc-mkimage`.
2. Write the second container to the card as a kind-7 update object with the WebUSB client.
3. On the device, open Settings ▸ System ▸ Update and install it. Watch the bootloader over RTT
   (`cargo run --release --features rtt`): the verify, flash and readback lines must all appear,
   and the app must confirm the trial on the next boot.
4. For the rollback, wrap an image with an invalid reset vector. The bootloader installs it, the
   trial boot faults, the trial watchdog resets the board, and the next entry restores the
   reserve.

A fault on this path leaves the `Armed` page behind, so every later boot repeats it. Reflash both
images to recover.

## Constraints

- The image must fit **32 KB** of flash. `firmware/tools/resource_guard.py boot` gates it in CI,
  against the default shape. The `rtt` feature must never be load-bearing for the budget.
- There is no executor, no timer, no interrupt and no FAT here: blocking HAL plus polled MMIO,
  and every wait is a DWT-cycle deadline.
