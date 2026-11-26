# Firmware

Every crate that the device image can reach, and three standalone Cargo roots: the board image
for the nRF54LM20 development kit, the 32 KB bootloader and the BLE sensor simulator.
`tools/` holds the guards of these crates. The device core (`obc-app`) runs the same code on the
board, in the simulators and in tests.

Not here: host tools, bakers and test oracles (`host/`), the simulators and the render bench
(`sim/`), the web app and its WebAssembly bridges (`builder/`). The site explains how the system
works: <https://openbikecomputer.com/>.

## Where to start

| Group | Crates |
| --- | --- |
| Foundations: no product policy, `no_std` | `obc-formats` (the code copy of `specs/`, with `ByteSource` and `ByteSink`), `obc-crc` (the one CRC-32), [`obc-ports`](obc-ports/README.md) (the traits between the app and a host), `obc-map-scene` (the interface between a map source and the renderer) |
| Core | `obc-reader` (OBCM), `obc-elevation` (OBCT), `obc-route` (OBCR, GPX), [`obc-render`](obc-render/README.md), `obc-link` (the protocol v4 codec and engine), `obc-ble` (BLE policy with no radio), `obc-dfu` (OBCU, the armer, the boot decision) |
| Platform | `obc-display` (draw targets, the LS021 row diff and wire bits), `obc-platform` (board adapters, sound), `obc-sensors` (chip decoders), `obc-storage` (the flat store on the raw card) |
| App | `obc-app`: the device core. `build.rs` generates the texts from `i18n/*.toml`. |
| Standalone roots | [`obc-fw-nrf54l`](obc-fw-nrf54l/README.md) (the board image), [`obc-boot`](obc-boot/README.md), [`obc-sensor-sim`](obc-sensor-sim/README.md) |

`obc-app/src` is one crate with several subsystems. The file names do not show the grouping:

| Subsystem | Files of `obc-app/src` |
| --- | --- |
| Runtime | `app`, `device_core`, `arena_gate`, `placement`, `host`, `wall_clock`, `settings*`, `harness` |
| Rider interface | `screen`, `ui_runtime`, `input*`, `dirty`, `render_key`, `hold_hint`, `map_icons`, `stat_fields`, `i18n`, `cues`, `distress`, `alert`, `fault`, `device_status` |
| Navigation | `navigator`, `route`, `corridor`, `next_ahead`, `whats_next`, `easier`, `breadcrumb` |
| Card and library | `catalog_state`, `trip`, `ride`, `card_scheduler`, `map_catalog` |
| Ride recording | `recorder`, `metadata`, `upload_facts`, `effort`, `altitude`, `activity` |
| Discovery | `landmarks`, `photo`, `settlements`, `peak_view`, `find_place` |
| Edges | `sensors`, `ble`, `dfu` |

In `obc-fw-nrf54l`, the drivers are `vendor/` and, in `src/`, `ble/`, `usb/`, `flpr/`,
`semmc.rs`, `ls021_flpr.rs`, `com.rs`, `sensors.rs`, `buzzer.rs`, `panel_power.rs`,
`flpr_mux.rs`, `card_io.rs` and `board.rs`. The rest is glue: the tasks in `main.rs`, the shared RAM block
(`arena.rs`), the one task that writes to the card, and the settings in RRAM.

## Run and test

```sh
obc test -p CRATE            # a workspace crate
obc check device             # obc-app and obc-link compile for thumbv8m.main-none-eabihf
obc flash                    # build and flash the board image; obc flash-boot once before it
obc check obc-fw-nrf54l      # clippy and the release build; also obc-boot, obc-sensor-sim
obc shot NAME                # one UI frame; --check, --accept
obc fmt                      # the workspace and the three standalone roots
```

The workspace is a virtual manifest: a plain `cargo fmt` formats nothing, and `cargo fmt --all`
skips the three standalone roots. `obc fmt` runs all four.

Tests of the workspace crates need GEOS: a dev-dependency may reach a host crate, because it
never enters the `no_std` build. The board image needs the `thumbv8m.main-none-eabihf` target
and a RISC-V gcc for the display coprocessor; `obc doctor` checks both. The board image and the
bootloader have no host tests; their READMEs have the procedures on the board.
`obc board`, `obc rtt`, `obc uart` and `obc debug` talk to a board.

A UI frame is a row of `ui-frames.toml`, and `ui-snapshots.sha256` holds one PNG digest for each.
After an intended change of a screen: `obc shot --all`, look at the changed frames, then
`obc shot --accept`.

`tools/resource_guard.py` holds a release image to `tools/resource_baseline.json`: RAM, frames,
the boot chain and the flash size. It runs in CI. Resource figures come from one build of the
head, compared with the baseline; never build the base again for a comparison. A build with
`--features resource-report` is for diagnosis: never flash or package it.

## Firmware update images (OBCU)

A field update is an OBCU container a client uploads as an update-package object. The byte format
is [`obcu-update.md`](../specs/obcu-update.md); the codec and boot decision live in `obc-dfu`, and
the producer is `obc-mkimage`. The pipeline is objcopy, then wrap and sign:

```sh
# From the board crate, whose .cargo/config.toml selects the nRF54L target.
# cargo-binutils provides `cargo objcopy`; -O binary emits the raw image in LMA order.
cd obc-fw-nrf54l
cargo objcopy --release -- -O binary app.bin

# Wrap and sign. On a dev machine the committed test seed is the right key; CI passes
# --sign-seed-env instead.
cargo run -p obc-mkimage -- wrap \
    --bin app.bin \
    --version "$(git describe --always --dirty)" \
    --out UPDATE.BIN \
    --sign-seed ../obc-dfu/keys/test/obcu-test.seed

# Decode and verify both CRCs and the signature. Non-zero exit if invalid.
cargo run -p obc-mkimage -- inspect UPDATE.BIN
```

`wrap` refuses an image over `MAX_IMAGE_LEN`. It warns if the binary's first word is not a
plausible initial stack pointer, which usually means an ELF or a wrong section order.

**Signing is not optional.** `wrap` needs a seed, and the armer rejects an unsigned container.
`obc-mkimage sign` replaces the trailer of an already-wrapped container, so you can wrap with the
test seed on one machine and re-sign on the one that holds the release key; `keygen` makes a
keypair. Keys, the `OBCU_SIGNING_SEED` secret and the **rotation still owed before the first
real release** are in [`obc-dfu/keys/README.md`](obc-dfu/keys/README.md).

To install, upload the container and confirm on the device, or trigger the armer over the debug
VCOM link ([board README](obc-fw-nrf54l/README.md#host-input-and-firmware-update)). The armer
validates the object, writes the running image into a rollback reserve, arms the boot-state page
and resets into `obc-boot` ([its README](obc-boot/README.md) has the LED codes).

## Gotchas

- Build a standalone root from inside its directory. Cargo finds the MCU target in
  `.cargo/config.toml` by working directory, so `--manifest-path` from elsewhere builds for the
  host and fails. `obc-sensor-sim` is the exception: its device build names the target.
- Never commit a `memory.x` in the board crate. `build.rs` writes the linker map; a file in the crate
  root shadows it, and the stack then grows into the coprocessor image. `obc-boot/memory.x` is
  committed and is kept equal to `obc_dfu::layout` by hand.
- Keep `+strict-align`. OBCM navigation records hold multi-byte fields at odd offsets, and an
  unaligned load faults. A job-wide `RUSTFLAGS` shadows the flag.
- A large part of `App` uses the placement constructors (`init_in_place`). An `App` by value is
  larger than the device stack.
- The shared RAM block (`arena.rs`) is one union: the board pays for its largest arm. Growth
  below the largest arm is free, and `const` asserts hold that each arm fits. A guard is never
  held across an `.await`. Map and route caches
  are never arms. Peak View gives the block back on Back.
- The display coprocessor (FLPR) is shared in time between the panel scan and the card host.
  Never stop it in the middle of a scan or switch it in the middle of a transfer.
- The bootloader is flashed once and never updated. So the OBCU header stays at version 1: a
  new field would brick every device in the field. The armer in the app checks the signature.
- `obc-dfu/keys/obcu-release.pub` is still the test key. [`obc-dfu/keys/README.md`](obc-dfu/keys/README.md)
  has the rotation that is owed before the first real release.
- The settings blob in RRAM (`obc-app/src/settings.rs`) is the one format with users in the
  field. Its versioned codec stays; it may become simpler, never go. Its layout is the row
  order of `settings_table.rs`.
- The Bluetooth host (`trouble-host`) gets no logging feature: its logs would hold bond keys.
  cargo-deny holds this (`tools/licenses/board-features.toml`).
- A new workspace crate goes in one group of `tools/dependency_rules.json`, or
  `tools/check_dependencies.py` fails. Production dependencies point downward; a foundation
  crate takes only `heapless`, `libm` and other foundation crates. Dev-dependencies are not
  checked.
- Latitude and longitude are integers of microdegrees in every crate.
- A "source" in these crates gives bytes (`ByteSource`) or map features (`MapScene`). It is not
  a source data set such as OpenStreetMap.
- A map `Style` stays at 12 bytes or less, because a map source keeps 256 of them in RAM.
- The companion app has a Swift copy of the CRC-32 of `obc-crc`. The two must give the same
  result.

## Decisions

- One device core for the board, the simulators and the tests. The `obc-ports` traits are
  synchronous and own no driver: a sensor poll takes the newest value from a mailbox. So the
  app needs no executor and cannot tell a simulator from hardware.
- The specs are the contract, and `obc-formats` is only their code copy. Bakers and readers
  share one set of constants. The crate has no reader, because a reader carries policy: what to
  cache, what to skip. One reader crate for each format would make a host producer take a
  device reader with it.
- `ByteSource` reads a byte range on request. A reader keeps small tables in RAM and streams
  the rest, so a map can be larger than the RAM. The device cannot load a whole file or map it
  into memory.
- One renderer for every host, generic over the draw target, so a preview cannot differ from
  the panel. The walk methods of `MapScene` are generic, not trait objects: the renderer calls
  them for each feature of a frame, and the hot loop pays no dynamic dispatch.
- The CRC is its own crate with no dependency, because the bootloader links it. The
  `slice-by-8` feature costs 7 KiB of flash for speed; the app turns it on, the bootloader
  does not. `obc-dfu` uses only `core` for the same reason.
- One RAM block for users that are never active together (render, navigation, USB, Peak View,
  photos), so the board pays for the largest and not for the sum.
- `obc-fw-nrf54l/build.rs` is the one definition of the contract between the two cores. It
  writes the Rust, C and linker files, so a change of one side only is not possible.
- Render and present are separate calls, so an object operation never waits behind a panel scan.
- The app chooses which sound cue plays (`Cue` crosses the port); each platform chooses how it
  sounds.
