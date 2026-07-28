# obc-fw-nrf54l — nRF54L15-DK firmware

The **real hardware target** for OpenBikeComputer: the shared `obc-app` running on
an nRF54L15-DK (Cortex-M33), with map/routes/tracks streamed from a microSD card —
load a route, ride it (driven by the **real SAM-M10Q GPS + BMP581 altimeter** on the shared I²C
bus, issue #218, or a `--features synth`/`debug-uart` stand-in for indoor work), and save the ride
as the durable `RDnn.ORD` ride object (GPX export happens in the companion app after sync — the
device writes no GPX). The firmware drives
the reflective **LS021B7DD02** memory LCD (the panel the project ships on) via the
nRF54L's **FLPR** (the VPR RISC-V coprocessor) — the only display path. The LS021 protocol
is on the [display-protocol docs page](https://timohueser.github.io/OpenBikeComputer/hardware/display-protocol/);
the `ls021-*` binaries below are its standalone bring-up benches.

See the module doc in [`src/main.rs`](src/main.rs) for the full peripheral/pin
plan; this README is the **board setup + build/flash** guide.

## One-time board configuration (nRF Connect **Board Configurator**)

These three settings are applied with the **Board Configurator** app (in *nRF Connect
for Desktop*), are written to the DK's interface MCU, and **persist across power
cycles** — do them once. After changing anything, click **Write config** (blue dots =
unwritten). No soldering / solder-bridge cuts are needed on current board revisions.

1. **VDD / VDDM → 3.3 V.** The default is 1.8 V, which is too low for the LS021's logic. (Also feed
   the panel's `Vin` from the DK's 5 V / VBUS so its on-board 3.3 V LDO has headroom.)
2. **External memory → OFF** ("external memory → GPIO on the P2 header"). This
   electronically disconnects the on-board QSPI flash, freeing **P2.00–P2.05** so the
   display can use them. We never use that flash (maps live on SD).
3. **VCOM hardware flow control (HWFC) → OFF.** *Required for the `debug-uart`
   build.* The DK's J-Link VCOM defaults to RTS/CTS flow control; with it on, the
   interface MCU gates **host→device** bytes on the device asserting RTS (P1.06) —
   which this firmware never does (it runs the VCOM 2-wire, and P1.06/P1.07 are reused
   as the SD bus). The symptom of leaving HWFC on: device→host **telemetry works** but
   injected GPS fixes / button presses are silently ignored.

## Wiring (DK headers)

Full detail is in the `src/main.rs` module doc. The **build drives the LS021 panel** — its FLPR
wiring is in the [LS021 section below](#ls021-flpr-builds--dk-wiring-issue-165). Also on the board:
a **microSD** breakout on the **P1** header (SCK P1_11 / MISO P1_07 / MOSI P1_06 / CS P0.00, with a
pull-up on MISO — see below); the four DK buttons and LED0 are on-board; the J-Link **VCOM**
(P1_04/P1_05) and RTT both ride the DK's USB.

### Full pin map (LS021 / FLPR build)

**Port P2 — MCU/fast domain (panel source bus + COM) — all 11 pins used:**

| Pin   | Signal | Notes                                   |
|-------|--------|-----------------------------------------|
| P2.00 | R0     | source data (odd-pixel R)               |
| P2.01 | R1     | source data (even-pixel R)              |
| P2.02 | G0     |                                         |
| P2.03 | G1     |                                         |
| P2.04 | B0     |                                         |
| P2.05 | B1     |                                         |
| P2.06 | BCK    | source shift clock                      |
| P2.07 | VCOM   | COM electrode (HighDrive); LED2 pin     |
| P2.08 | VB     | COM                                     |
| P2.09 | LED0   | per-frame heartbeat blink               |
| P2.10 | VA     | COM                                     |

**Port P1 — PERI domain ≤8 MHz (gate/BSP + SD + VCOM + buttons) — all broken-out pins used:**

| Pin   | Signal  | Notes                                  |
|-------|---------|----------------------------------------|
| P1.00 | GSP     | gate start pulse                       |
| P1.01 | GCK     | gate clock                             |
| P1.02 | —       | **NFC, off-limits**                    |
| P1.03 | —       | **NFC, off-limits**                    |
| P1.04 | VCOM TX | UARTE20 → host                         |
| P1.05 | VCOM RX | UARTE20 ← host (needs HWFC OFF)        |
| P1.06 | SD MOSI | SPIM22                                 |
| P1.07 | SD MISO | SPIM22 (external pull-up to 3V3)       |
| P1.08 | BTN2    | BACK                                   |
| P1.09 | BTN1    | DOWN                                   |
| P1.10 | INTB    | frame envelope; LED1 pin               |
| P1.11 | SD SCK  | SPIM22                                 |
| P1.12 | GEN     | gate enable (the freed SD-CS pin)      |
| P1.13 | BTN0    | UP                                     |
| P1.14 | BSP     | source sub-line start                  |

**Port P0 — low-power domain (P0.00–P0.04):**

| Pin   | Signal     | Notes                                          |
|-------|------------|------------------------------------------------|
| P0.00 | SD CS      | held LOW (freed P1.12 for GEN)                  |
| P0.01 | I²C SDA    | shared GPS + altimeter + compass bus (TWIM30, #218) |
| P0.02 | I²C SCL    | shared GPS + altimeter + compass bus (TWIM30, #218) |
| P0.03 | GPS TX-Ready | *optional* DDC data-ready IRQ (active-high)     |
| P0.04 | BTN3       | SELECT                                          |

The shared **I²C / Qwiic** bus on TWIM30 (the low-power-domain instance that reaches P0) carries the
real sensors (issue #218 + compass): the u-blox **SAM-M10Q** GNSS (DDC address `0x42`), the Bosch
**BMP581** altimeter (`0x47`, or `0x46` if the breakout straps `SDO` low), and an electronic compass
— the **AK09916** magnetometer inside a TDK **ICM-20948** (the IMU itself at `0x68`/`0x69`; the ICM is
put in **I²C bypass** so the magnetometer answers directly at `0x0C`). None of the addresses clash, so
all three share SDA/SCL with no extra pins. **Only the 3 magnetometer axes are used** — accel/gyro are
left asleep. The compass supplies the heading-up orientation while the rider is *stopped* (the GPS
reports no course below walking pace); once moving, the GPS course is the heading. Because the heading
is never logged, it's **decoupled from the GPS fix** — read on its own ~5 Hz cadence *while stationary*
(so the map stays lively as you turn the device, independent of a slow / power-saving fix rate), and
silent while moving or idle. A dead-band suppresses noise-only updates. The shipping board is expected
to drop the 9-axis IMU for a plain 3-axis magnetometer, which the [`obc_platform::compass`] (heading
geometry) / [`obc_platform::icm20948`] (chip register map) split is designed for.

The GPS **TX-Ready** line on P0.03 is an *optional* data-ready interrupt: when present it asserts as a
NAV-PVT message becomes ready and wakes the event-driven sensor task, so the bus does **zero** work
between fixes. **The SparkFun SAM-M10Q breakout (GPS-21834) does not break out TX-Ready** (it exposes
SDA/SCL/INT/SAFE/RST/PPS — where `INT` is the EXTINT *wake input*, not data-ready), so on that board
the task runs on its DDC-poll fallback: it reads the freshest NAV-PVT once per fix interval, sleeping
on a timer in between, so the M33 still wakes only ~once a second (the same cadence TX-Ready would
give at 1 Hz — no real cost). TX-Ready support stays in the driver for a board that *does* route it;
nothing to wire on the GPS-21834.

**Power tip:** wire **V_BCKP** on the SAM-M10Q to an always-on rail / supercap / coin cell. It backs
the receiver's RTC + ephemeris across a power-off, turning every cold ~30 s fix into a hot/warm fix
in seconds — the biggest UX win for a device switched off at each stop.

## Build & flash

**One-time prerequisite (#617): flash the bootloader.** The app is linked at `0x8000` —
the 32 KB below it belongs to [`obc-boot`](../obc-boot/README.md), which must be on the
chip once (`cd ../obc-boot && cargo run --release`; it survives every app reflash, since
probe-rs only writes each ELF's own address range). A device without it shows no LED
blink and never boots; the recovery recipe is in that README. Everything below — flashing,
RTT, `cargo rtt`, the flash-twice retry quirk — then works exactly as before.

From this crate directory (it's a standalone crate built for `thumbv8m.main-none-eabihf`;
`cargo run` flashes + streams defmt/RTT over the on-board J-Link via probe-rs):

```sh
# Default: full map + ride loop on the **LS021 panel via the FLPR** (issues #165 / #173),
# driving the **real SAM-M10Q GPS + BMP581 altimeter** on the shared I²C bus (issue #218),
# with **both companion transports up** — the BLE radio and the USB device plane (#889).
# Builds the RISC-V blob, so it needs an rv32emc gcc (below) + the LS021 wiring + Board-Configurator
# settings. With no Qwiic hardware attached it still boots and idles waiting for a fix (watch RTT).
cargo run --release

# Indoor / no-hardware: replace the real GPS with the on-board SynthLocation square loop, so the
# ride loop runs without a sky view (or any Qwiic sensors).
cargo run --release --features synth

# Indoor: stream a recorded ride from a host over the VCOM debug-sensor feed (issue #127) — needs
# HWFC OFF (above). Replaces the real sensors with the host feed.
cargo run --release --features debug-uart

# The BLE build (issue #270, epic #267): the same firmware with the nrf-sdc + MPSL + TrouBLE
# stack folded in (`src/ble/`), advertising as `OBC-XXXX` (S0 §2 — the FICR serial tail).
# Map + BLE run IN ONE IMAGE: the full map/ride app plus the companion link, sharing the SD
# card + RRAM settings behind one async mutex. `--no-default-features` is REQUIRED (it swaps
# the critical-section impl to MPSL's — a compile_error catches the wrong invocation).
# Composes with debug-uart/synth (headless ride beside a live link).
cargo run --release --no-default-features --features ble
```

**There is no `usb` feature.** The USB device plane (#889) is in every build above — see the
"USB device plane" section further down for the bring-up recipe.

### SD read throughput diagnostic

`sd_bench` measures the real SERIAL22 → embedded-sdmmc → card path against the first `.obcm` its
own scan finds in the card root — deliberately *not* the app's selected map (`/MAP.SEL`, #927): the
benchmark wants a large file to read, not the one the renderer would pick. It compares
CMD17-per-block reads with CMD18 batches across sequential, random 4 KB,
and random 512-byte shapes, and checks sequential passes against an FNV integrity hash:

```sh
cargo run --release --bin sd_bench
```

The harness is gated to the default non-BLE feature set; the BLE CI/build command above skips it
because that profile uses MPSL's mutually exclusive critical-section implementation.

(The standalone FLPR waveform bench bin `ls021_flpr_bringup` was retired in #177 once the app drove
the LS021 on glass; the M33-direct `ls021_bringup` bench was retired earlier in #176; the A1 BLE
spike bin `ble_spike` was retired at #270 when the stack moved into `main.rs`/`src/ble/`. All are
in git history if an isolation bring-up is ever needed again — the FLPR transport is
`src/ls021_flpr.rs`, exercised by the default build.)

### LS021 FLPR builds — DK wiring (issue #165)

The source bus + `BCK` + COM stay on **P2** (P2.00–06 data/clock, P2.07/08/10 COM, P2.09 heartbeat
LED); the four gate lines + `BSP` sit on **free P1 pins** — `GSP P1.00 / GCK P1.01 / GEN P1.12 /
INTB P1.10 / BSP P1.14` — deliberately **off** the SD-SPI bus (P1.06/07/11/12) and VCOM (P1.04/05)
the app needs.

The DK breaks out only **P1.00–14** (P1.02/03 are NFC), which is one pin short for everything the app
puts on P1 — so SD **`CS` moves from P1.12 to P0.00** (one jumper on the SD breakout; it's a plain
GPIO, and the M33 already drives P0 for BTN3). That frees P1.12 for `GEN`. The SD bus pins (SCK P1.11
/ MISO P1.07 / MOSI P1.06) are unchanged.

The five gate/`BSP` DK pins, the masks in `src/flpr/flpr_pingpong.c`, and the physical 21-pin FPC
harness must all agree; if a gate line stays dark on glass, confirm the pin is broken out on your DK
header and remap all three together. Full pin/protocol detail:
[firmware/docs/ls021-flpr.md](../docs/ls021-flpr.md).

#### COM on P2 is a DK artifact — route it onto GPIOTE pins on the production board

COM (`VCOM`/`VB`/`VA`) sits on **P2** only because the source bus + COM came up together on the P2 FPC
during bring-up — COM is a slow ~60 Hz wave that never needed P2's fast MCU domain. The cost: in
embassy-nrf 0.11 **P2 has no GPIOTE mapping** (on either the nRF54L15 or the nRF54LM20 — only P0→GPIOTE30
and P1/P3→GPIOTE20 are GPIOTE-capable), and PWM was proven dead on these exact pins (L1). So with COM
on P2 the anti-DC-bias wave **must** be generated by the M33 (`com::com_task`), which wakes the core
~120×/s regardless of the event-driven loop (issue #219) — capping the idle-power win.

The fix is a layout choice, not firmware: **on the production board (nRF54LM20A, `hardware/PCB/`) route
the three COM lines onto GPIOTE-capable P1/P3 pins** (all on GPIOTE20, so one DPPI channel toggles them
in lockstep). Then the **`com-hw`** build (`cargo build --release --features com-hw`) drives COM from a
zero-CPU **TIMER21 → DPPIC20 → GPIOTE20** toggle chain (`src/com_hw.rs`), the wave free-runs in
System-ON sleep, and the M33 can finally WFI between events. The placeholder COM pins in the `com-hw`
build (P1.04/05/15) are illustrative — reconcile them with the final schematic. On the DK, COM stays on
P2 and the default build keeps `com_task`; `com-hw` is **on-glass + logic-analyzer verification pending**
(no GPIOTE-COM board exists yet).

### FLPR toolchain

The FLPR backend cross-compiles a tiny freestanding C blob for the RISC-V coprocessor, so it
needs an `rv32emc`-capable GNU gcc — install once:

```sh
brew install riscv64-elf-gcc        # or set RISCV_GCC=<path> to an xPack / Zephyr-SDK toolchain
```

Every build needs it (the FLPR drives the panel on every build; `build.rs` always compiles the
blob). On Linux/CI the apt package `gcc-riscv64-unknown-elf` works too.

If `cargo run` prompts to pick a probe (e.g. another ST-LINK is attached), pass
`--probe <vid:pid:serial>` for the J-Link.

## BLE stack — dependency pins & gotchas (issues #269/#270, epic #267)

The A1 spike proved `nrf-sdc` (Nordic's closed-source SoftDevice Controller + MPSL bindings) +
`trouble-host` on this DK; A2 (#270) folded that stack into the real firmware as `src/ble/`
behind the `ble` feature (build command above). What the spike settled, for everything Track A
builds on it:

- **Versions.** The crates.io `nrf-sdc`/`nrf-mpsl` releases (0.3.x) predate nRF54L support and
  pin embassy-nrf 0.7 — useless here. The **git pin** (same rev as TrouBLE's `examples/nrf54`)
  targets exactly this crate's embassy set (embassy-nrf 0.11 / executor 0.10 / sync 0.8 /
  time 0.5.1), and `trouble-host` **0.7 from crates.io** matches too — no embassy patch or bump
  anywhere. Its heapless 0.9 coexists with our 0.8 (no types cross the boundary).
- **Critical section.** MPSL ships its own mandatory `critical-section` impl
  (`nrf-mpsl/critical-section-impl`) — global-interrupt-disable critical sections break its
  radio timing, and two impls are a duplicate-symbol link error. So
  `cortex-m/critical-section-single-core` moved behind the `cs-single-core` **default feature**,
  and BLE builds pass `--no-default-features`.
- **`central` is load-bearing.** trouble-host's Controller bound unconditionally requires
  `LeCreateConnCancel`, which only the multirole SDC lib variant exports — a peripheral-only
  `nrf-sdc` build is a link error. Costs flash only (the Builder never enables central roles).
- **⚠️ Sleep clock: internal RC, not the 32 k crystal (for now).** With
  `LfclkSource::ExternalXtal` the device advertises fine but **every connection dies at
  establishment** (HCI 0x3E sync timeout): the nRF54L's crystal **internal load capacitors**
  are never programmed (Nordic's DK config sets LFXO 15.5 pF / HFXO 15 pF; embassy-nrf 0.11's
  `internal_capacitors` knob is nRF5340-only and nrf-mpsl doesn't touch them), so the LFXO runs
  off-frequency and every anchor point is missed. The spike runs the LF **RC** oscillator with
  MPSL's recommended calibration (4 s cadence, ±500 ppm class) — solid, negotiates 2M PHY.
  Moving back to the xtal (better idle power) means writing the `OSCILLATORS` INTCAP registers
  before MPSL init — a filed follow-up.
- **Interrupts/peripherals MPSL+SDC claim.** Vectors: `RADIO_0`, `TIMER10`, `GRTC_3` (high-prio),
  `CLOCK_POWER`, and `SWI00` (low-prio scheduling) — which is why the firmware's high-priority
  `InterruptExecutor` (`src/planes.rs`) sits on **SWI01** (every build; the full priority ladder
  is in `main.rs`'s module doc). Peripherals owned outright: GRTC CH7–11, `TIMER10`, `TIMER20`, `TEMP`, `CRACEN`
  (LL crypto RNG), and a raft of PPI/PPIB channels (grouped in `main.rs`, consumed by
  `ble::run`). The HF **crystal** is an MPSL hard requirement (`HfclkSource::ExternalXtal`) —
  the `ble` build's boot config sets it; non-BLE builds keep the internal RC.
- **RAM.** The map plane compiles into every build now (#270), so on the `ble` build the map and
  BLE stack are resident together. The budget assert in `main.rs` sums the map-plane residents
  (`MAP_RESIDENT`) and the BLE stack's (`ble::RESIDENT_BYTES`) and fails a `ble`+map build on this
  DK at compile time if they overrun the carve. The one piece the `ble` build drops is the
  on-device router (`has_nav`): its ~14.3 KB of `NAV_*` statics don't fit beside the BLE stack on
  the 256 KB DK (see `build.rs`); the 512 KB LM20 relaxes the `has_nav` gate to run the router on
  every build too.
- **Stack: keep big values out of long-lived async bodies (#677).** Every sizeable value
  constructed inline in an async fn/block gets a construction-temporary slot in the generated
  poll function's **stack frame**, allocated at entry on *every* poll — `ble::run` once carried a
  30.5 KB poll frame this way, and SMP's synchronous software-P256 pairing chain (which runs in
  the host runner's rx path, on the main stack) overflowed the region into `defmt_rtt::BUFFER`:
  a HardFault with a corrupted backtrace on every pairing attempt. The discipline: big objects
  live in `.bss` statics built by dedicated `#[inline(never)]` init fns (transient frame, boot
  depth) and the async body holds only `&'static` handles — see `ble::run`'s doc. Three nets
  catch a regression: the compile-time budget assert (floors the stack region), the CI
  **poll-frame guard** on the ble ELF (largest `sub sp` in any `TaskStorage<F>::poll` ≤ 12 KB —
  the `ci.yml` embedded job), and **MSPLIM** (armed first thing in `main`): the ARMv8-M hardware
  stack-limit register turns any residual overflow into an immediate, precise fault instead of
  silent `.bss` corruption. To check a frame by hand, disassemble the release ELF
  (`cargo objdump --release --no-default-features --features ble -- -d --demangle`) and read the
  `sub sp` at each `TaskStorage<F>::poll` entry.

## BLE — board-specific notes & on-glass verification

The wire protocol (advertising policy, GATT services/UUIDs, CoC framing, object layouts, pairing
security) is canonical in [`obc-ble-interface-spec.md`](../../specs/obc-ble-interface-spec.md) — read it
there, not here. `src/ble/` implements it; the S0 descriptor codecs + transfer state machine are the
host-tested `obc-ble` crate (`cargo test -p obc-ble`, pinned to `specs/vectors/`). What's
**board/firmware-specific** and worth knowing:

- **DIS identity** — Firmware Revision is `<crate-semver>+<git-short>` (`build.rs` emits
  `OBC_FW_GIT`), Hardware Revision `nrf54l15-dk`, Serial Number the 16-hex FICR `DEVICEID` whose last
  four digits are the `OBC-XXXX` advertised name.
- **Storage lives on SD, ids are durable in filenames.** Uploaded routes land as 8.3 `RTnn.OBR`
  files (the `OBCR` magic held back as zeros until commit, so a power cut never leaves a half-route
  the boot scan accepts); the **map build's** catalog scan matches `*.OBR` beside `.obcr`, so an
  uploaded route shows in the on-device menu after a reflash. A **map** received over USB (#927) is
  the fourth member of that family: `/MPnn.OBM` in the card root, same durable-id-in-the-filename
  rule, and `OBM` is the 8.3-safe twin of `.obcm` the scan accepts beside it. It is the one upload
  that streams *straight into its final file* rather than through `UPLOAD.TMP` — a temp-then-copy
  promote would double both the minutes of writing and the free space needed — so the held-back
  magic is patched into the final file itself at commit, and `sweep_aborted_maps()` reclaims the
  zero-magic corpse of an interrupted transfer at the next boot. `/MAP.SEL` records which map the
  renderer streams from; a committed upload becomes that choice, effective at the next boot (the
  map's tables are parsed once at startup and held for the session). Every ride Finish on the map build
  writes `/tracks/RDnn.ORD` (byte-for-byte the S0 §7.2 ride object) — the only save artifact; the `ble`
  build just serves those. The id in each filename is recovered at boot and is what the app's
  synced-set keys on. A ride is deleted **only on the device** (its Rides screen, hold-to-delete);
  the wire `deleteObject` on a ride answers `notFound` deliberately, and app-side deletes are
  tombstoned in the app.
- **Config + bond persist in the RRAM SETTINGS carve** (`settings.rs`), which survives power cycles
  **and a firmware reflash** (it sits above the app image, so `probe-rs download` leaves it): the
  device name (config codec v3) @0, the boot counter @2048, the single 64-byte CRC-checked bond slot
  (LTK + peer IRK, `ObjectStore::{load,save,clear}_bond`) @`BOND_OFFSET`. Pairing is LESC passkey
  **display** — the 6-digit code renders on the app's passkey card (`Font::Huge`). One bond slot, and
  while it's occupied the device **rejects** every new pairing attempt (#455): the hold-guarded
  **Forget phone** in Settings ▸ Bluetooth is the only device-side clear, so physical possession
  guards the *clear* step (it no longer silently replaces the bond on a fresh pairing).

**Verify on glass** (nRF Connect is the pre-app oracle for A3–A4; the iOS app + harness for A5+):
- **nRF Connect** — service/char table matches the spec, DIS strings + serial real, BAS notifies,
  `protocolVersion` reads `1`, `psm` reads `0x0080`; negotiated MTU (247) + 2M PHY, interval settles
  to the idle set; disconnect/re-connect + walk out of range bumps the counters and always returns to
  advertising. As an *unbonded* stranger (post-A8): DIS/`protocolVersion` readable, access-denied on
  every gated char + the CoC.
- **Echo/transfer harness** `companion-ios/EchoHarness` (reuses the app's `BLEChannel`):
  `swift run echo-harness --count 1000 --size 32768` round-trips byte-identical; `--corrupt` expects a
  `crcMismatch`. `upload`/`list`/`detail`/`delete`/`abort-test` exercise the route plane.
- **E2E golden path** — share a GPX to the iOS app, upload (B5 sheet), reflash the **map** build; the
  route is in the device menu and rideable (SD persists across flashes). For sync: record 2–3 rides on
  the map build (`synth` is fine indoors), reflash `ble`, sync pulls them; spot-check a decoded ride's
  totals in the app against the device's Paused ledger. Ids must survive a power cycle; the boot
  counter must increment across them.
- **Pairing** — passkey card on the panel typed on the phone → bond lands; power-cycle / app
  restart / walk-away → silent reconnect, no dialog; reflash `ble` → still no dialog. A **second
  phone** is rejected while bonded (no passkey card, generic failure on the stranger). Re-pair path:
  device **Settings ▸ Bluetooth ▸ Forget phone** (hold) + app *Forget* + iOS Bluetooth forget → next
  contact pairs with a fresh passkey.

## The USB device plane (issue #889) — **in every build; cable-gated since #936**

Every build ships a second transport for the *same* companion protocol: the LM20's USBHS
behind one vendor-specific interface, so the web builder (WebUSB, Chromium) or the desktop app can
push a map / route / firmware image to a plugged-in device. It was briefly behind a `usb` Cargo
feature; **that feature is gone** — the plane is part of the device, not an option of it, and the
resource baseline in [`firmware/tools/resource_baseline.json`](../tools/resource_baseline.json) pins
the shape that includes it (+5,096 B resident and +45,688 B flash for the plane, +64 B and +288 B
more for #936's VBUS gate, then +8 B resident and **−80 B** flash for #937's event-driven park;
guarded poll frame 9,664 → 9,728 B against the unchanged 12,288 B limit, and unmoved since). The
wire protocol is canonical in
[`obc-ble-interface-spec.md`](../../specs/obc-ble-interface-spec.md) — USB is a transport under it, not a
second protocol. What is **board-specific** and worth knowing:

- **Zero GPIO cost.** D+/D−/VBUS/TXRTUNE are dedicated USBHS pins; nothing in the pin map above
  moves. The driver ships in embassy-nrf 0.11 (`src/usb/usbhs.rs`, a full `embassy_usb_driver::Driver`
  over the Synopsys OTG core, plus `vbus_detect.rs`), gated on the `nrf54lm20-app-s` feature this
  crate already selects. It needs AHB ≥ 30 MHz — the board runs 128.
- **Plug into J3 on the LM20-DK, not J4.** J4 is the on-board debugger (it is what `cargo run` and
  RTT use); **J3 is the USB connector wired to the SoC**. You want both cables: J4 to flash and
  watch RTT, J3 to the host that talks to the device. A production board with a routed USB port is
  *not* a prerequisite for bring-up.
- **VBUS gates everything, and J3 may be empty (#936).** Riding with no cable is the common case, so
  the plane arms VBUS detection (VREGUSB only), parks, and builds the driver **when a cable
  arrives** — then parks again on unplug and comes back on re-plug, any number of times. It is not
  silent about it: `usb: no VBUS on J3 — device plane parked …` at boot, `usb: VBUS present …` +
  `usb: device plane up …` when you plug in, `usb: VBUS removed …` when you pull it. Order no longer
  matters, in either direction. Before the gate, a cable-less boot **faulted the bus** partway
  through start-up (`Endpoint::wait_enabled` reads `USBHSCORE.DOEPCTL`, which does not answer while
  `USBHS.ENABLE.CORE = 0`) and took the debug port with it — probe-rs reported
  `DAP FAULT (sticky_err, sticky_orun)`, never a panic. If you ever see that signature again,
  suspect a *new* USBHS access that escaped the gate, not a stack or a panic handler.
- **The parked plane costs nothing (#937).** It waits on the VREGUSB interrupt, not a timer, so a
  ride with J3 empty produces **zero** USB wake-ups — the 500 ms poll #936 shipped is gone, and so
  is the 30 s fallback that briefly replaced it. There is **no timer here at all**: `wait_for_vbus`
  registers on `VBUS_WAKER` *before* reading the level, and the level is a level rather than a
  latched edge, so a wake cannot be lost and a periodic re-check could only ever help if one were.
  Confirmed on glass 2026-07-26. If plug-in is ever *not* immediate, that is a broken interrupt
  path to fix — not a latency to paper over.
- **Two vectors, no clashes:** `USBHS` and `VREGUSB`. MPSL takes `RADIO_0` / `TIMER10` / `GRTC_3` /
  `CLOCK_POWER` / `SWI00`, and the high-priority input executor is on `SWI01`. `VREGUSB` carries
  **two** handlers — ours (wake the park) and embassy's (clear the events, wake the driver's own
  bus waker) — bound in one `bind_interrupts!` arm. Ours reads and clears nothing, so the order
  between them does not matter; dropping embassy's would leave the events uncleared and storm.
- **Endpoint layout** (the host reads it off the descriptors): one interface, class `0xFF`, four
  bulk endpoints at the high-speed-mandated 512 B — `0x81/0x01` control frames, `0x82/0x02` the
  object stream. Control frames are `selector u8 · payload`, one frame per transfer.
- **Windows needs no driver install**: MS OS 2.0 BOS descriptors declare the `WINUSB` compatible id
  and a stable `DeviceInterfaceGUIDs` property, so Windows auto-binds WinUSB with no `.inf` and no
  Zadig.
- **VID/PID `1209:0001`** is pid.codes' *prototype* pair — deliberately the id that means "not
  allocated yet". Allocating a real PID is an owner action; when it lands, two constants change
  (`src/usb/mod.rs` and the web builder's `OBC_USB_FILTERS`).

### Bring-up recipe

Steps 1–2 are the **cable-less boot**, and they are the ones that matter most: that is how the
device is used. Steps 3–6 are the transfer path. Steps 1–3 were confirmed on glass under #936;
step 4 on is still **unverified**.

1. With **J3 empty**, flash `cargo run --release` over **J4** — the plain default build; no feature
   flag selects the plane any more (remember the flash-twice quirk and keep `--verify`; retry 2–3×
   if probe-rs errors).
2. **The board must reach the ride loop and stay there.** RTT shows
   `usb: no VBUS on J3 — device plane parked; it comes up when a cable is plugged in`, then the map
   renders and the session keeps logging. No `DAP FAULT`, no reset loop. This is the #936
   regression test: before the VBUS gate, this exact step killed the boot.
3. Now plug **J3** into the host. RTT: `usb: VBUS present — bringing the device plane up` followed by
   `usb: device plane up — 1209:0001, serial '…', HS bulk 512 B`. **Watch the clock here** — that is
   the #937 check. The defmt timestamp on `VBUS present` should be within a few milliseconds of the
   connector seating, because a VREGUSB interrupt woke the task. If instead it lands *seconds*
   later, or not at all, the VREGUSB wake is not arriving — there is no fallback timer to carry it
   any more, so this is a hard failure rather than a slow success. On macOS
   `system_profiler SPUSBDataType` (or Linux `lsusb -v -d 1209:0001`) should show
   `OpenBikeComputer`, the FICR serial as `iSerialNumber`, **Speed: Up to 480 Mb/s**, one
   vendor-specific interface and four bulk endpoints.
4. In Chromium, `chrome://device-log` shows the enumeration; the web builder's connect button opens
   the chooser and the identity + device-info reads answer. RTT should show
   `usb: [ctl] endpoint enabled` and `usb: [bulk] endpoint enabled` once the host claims the
   interface.
5. **Unplug J3 mid-ride.** RTT: `usb: VBUS removed — device plane parked, endpoints idle until a
   cable returns`, and the ride loop carries on untouched.
6. **Plug it back in.** `usb: VBUS back — device plane serving again`, the host re-enumerates, and a
   transfer works again — again within milliseconds, not seconds. Repeat a few times: the cable
   cycle is a loop, not a one-shot, and this is also where a lost VREGUSB edge would show up, since
   the park after an unplug is the one that has to be woken by an interrupt rather than entered
   with the answer already known.

**Known failure modes.** *No* `usb:` line at all → the task never started (it is spawned
unconditionally, so this is a real bring-up failure, not a missing build flag — check for a panic
before the spawn). `usb: no VBUS …` while a cable *is* in J3 → VBUS detection, not the plane: check
J3 really is the SoC connector on your DK revision and that `VREGUSB.TASKS_START` ran (that log
line is printed after it). A `DAP FAULT (sticky_err, sticky_orun)` that loses the target is the
#936 signature — a USBHS core access that escaped the VBUS gate; it is not a panic and not a stack
overflow, so look for a new register read, not a new buffer. A plug or re-plug that is **never
noticed** is the #937 signature: the park's VREGUSB wake is not arriving, and since the fallback
timer is gone nothing else will rescue it — check that both handlers are still on the `VREGUSB` arm
of `bind_interrupts!`. Enumeration at 12 Mb/s instead of 480 →
the PHY fell back to full speed, and the 512 B bulk descriptors are then illegal; that is a real
bug, not a slow link. A device that enumerates but whose transfers hang → look for `discarded N
unclaimed bytes while idle` (the host wrote payload before its descriptor was acked) or a `busy`
status (the cross-transport one-transfer gate — a BLE transfer is in flight). `Code 43` on Windows
means the MS OS 2.0 descriptor set was rejected; re-read it with `usbview`.

## Driving it from a host (`debug-uart`)

With the `debug-uart` firmware flashed and VCOM HWFC off, replay a recorded ride over
the VCOM from the desktop feeder (from the `firmware/` workspace dir):

```sh
cargo run -p obc-usb-host -- --gpx ../kandel.gpx        # add --port <VCOM tty> if not auto-detected
```

`obc-usb-host` streams the `.gpx` as fake GPS fixes (plus a baro/compass slider and an
on-screen button row that injects the four buttons' presses), and shows the device's
render-stats telemetry coming back. It's the same `obc-platform::debug_link` wire
protocol the simulator uses — only the transport differs (a VCOM UART here). `--list`
enumerates serial ports; the VCOM is the J-Link CDC port.

### Triggering a firmware update over the VCOM (`dfu-install`, S4 #619)

With an `UPDATE.BIN` (see `../README.md` §Firmware update images) in the card root, the
same link carries the DFU armer's trigger — no feeder GUI needed. Two hard-won gotchas
(both bit for real): the J-Link exposes **two** CDC ports and only one is live — on macOS
it's the `cu.usbmodem*133` one (`*131` silently swallows writes; and use `cu.*`, never
`tty.*`) — and plain `stty` + `printf`/`cat` does **not** work (macOS resets the termios
on every open/close, so the line never arrives at 115200 raw). Use pyserial:

```sh
uv venv /tmp/dfu-venv && uv pip install --python /tmp/dfu-venv/bin/python pyserial
/tmp/dfu-venv/bin/python - <<'EOF'
import serial, time
p = serial.Serial('/dev/cu.usbmodem<...>133', 115200, rtscts=False, timeout=1)
p.write(b'dfu-install\n')
end = time.time() + 90
while time.time() < end:                 # watch the `D …` status lines back
    if (line := p.readline()):
        print(line.decode(errors='replace').rstrip())
EOF
```

If nothing comes back (and RTT shows no `dfu:` line either), the J-Link CDC path is in
its known injection wedge — writes vanish while RTT keeps flowing; `probe-rs reset` does
NOT clear it, only a physical DK power-cycle does.

The device streams one `D` line per phase — scan result (staged version, size, extents),
rollback snapshot, `armed gen=N` — then resets into `obc-boot`, which installs the image
(LED codes: `../obc-boot/README.md`). Errors (`no UPDATE.BIN…`, `failed its CRC check…`,
`too fragmented…`) come back the same way and the device keeps running. The trigger is
refused mid-recording. Concept + formats: `OBCU_Spec.md`; the byte-identical request path
is what S5's on-device menu entry will post.
