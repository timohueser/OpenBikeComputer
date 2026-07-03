# obc-fw-nrf54l — nRF54L15-DK firmware

The **real hardware target** for OpenBikeComputer: the shared `obc-app` running on
an nRF54L15-DK (Cortex-M33), with map/routes/tracks streamed from a microSD card —
load a route, ride it (driven by the **real SAM-M10Q GPS + BMP581 altimeter** on the shared I²C
bus, issue #218, or a `--features synth`/`debug-uart` stand-in for indoor work), and save a GPX. The default firmware drives
the reflective **LS021B7DD02** memory LCD (the panel the project ships on) via the
nRF54L's **FLPR** (the VPR RISC-V coprocessor); the Adafruit **ST7789** EYESPI panel
(240×320) is kept as the opt-in `--features tft` bring-up backend. The LS021 protocol
is on the [display-protocol docs page](https://timohueser.github.io/OpenBikeComputer/hardware/display-protocol/);
the `ls021-*` binaries below are its standalone bring-up benches.

See the module doc in [`src/main.rs`](src/main.rs) for the full peripheral/pin
plan; this README is the **board setup + build/flash** guide.

## One-time board configuration (nRF Connect **Board Configurator**)

These three settings are applied with the **Board Configurator** app (in *nRF Connect
for Desktop*), are written to the DK's interface MCU, and **persist across power
cycles** — do them once. After changing anything, click **Write config** (blue dots =
unwritten). No soldering / solder-bridge cuts are needed on current board revisions.

1. **VDD / VDDM → 3.3 V.** The default is 1.8 V, which is too low for both panels (the
   LS021's logic and the ST7789 breakout's level shifters). (Also feed the panel's `Vin`
   from the DK's 5 V / VBUS so its on-board 3.3 V LDO has headroom.)
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

Full detail is in the `src/main.rs` module doc. The **default build drives the LS021
panel** — its FLPR wiring is in the [LS021 section below](#ls021-flpr-builds--dk-wiring-issue-165).
Common to both panels: a **microSD** breakout on the **P1** header (SCK P1_11 / MISO P1_07
/ MOSI P1_06 / CS P1_12, with a pull-up on MISO; the LS021 build moves CS to P0.00 — see
below); the four DK buttons and LED0 are on-board; the J-Link **VCOM** (P1_04/P1_05) and
RTT both ride the DK's USB.

For the opt-in **`--features tft`** build, the **ST7789** sits on the flash-freed **P2**
header (SCK P2_01 / MOSI P2_02 / CS P2_05 / DC P2_03 / RST P2_00, Vin←5 V, logic at 3.3 V)
and SD `CS` stays on P1.12.

### Full pin map (default LS021 / FLPR build)

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
| P1.09 | BTN1    | NEXT                                   |
| P1.10 | INTB    | frame envelope; LED1 pin               |
| P1.11 | SD SCK  | SPIM22                                 |
| P1.12 | GEN     | gate enable (the freed SD-CS pin)      |
| P1.13 | BTN0    | PREV                                   |
| P1.14 | BSP     | source sub-line start                  |

**Port P0 — low-power domain (P0.00–P0.04):**

| Pin   | Signal     | Notes                                          |
|-------|------------|------------------------------------------------|
| P0.00 | SD CS      | moved here in the FLPR build (held LOW)         |
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

From this crate directory (it's a standalone crate built for `thumbv8m.main-none-eabihf`;
`cargo run` flashes + streams defmt/RTT over the on-board J-Link via probe-rs):

```sh
# Default: full map + ride loop on the **LS021 panel via the FLPR** (issues #165 / #173),
# driving the **real SAM-M10Q GPS + BMP581 altimeter** on the shared I²C bus (issue #218).
# Builds the RISC-V blob, so it needs an rv32emc gcc (below) + the LS021 wiring + Board-Configurator
# settings. With no Qwiic hardware attached it still boots and idles waiting for a fix (watch RTT).
cargo run --release

# Indoor / no-hardware: replace the real GPS with the on-board SynthLocation square loop, so the
# ride loop runs without a sky view (or any Qwiic sensors).
cargo run --release --features synth

# Indoor: stream a recorded ride from a host over the VCOM debug-sensor feed (issue #127) — needs
# HWFC OFF (above). Replaces the real sensors with the host feed.
cargo run --release --features debug-uart

# The same map/ride app on the **ST7789** bring-up panel instead of the LS021 (opt-in
# backend) — no FLPR, no RISC-V gcc, links the full 256 KB. Real sensors still apply. ST7789 wiring (below).
cargo run --release --features tft

# The BLE build (issue #270, epic #267): the same firmware with the nrf-sdc + MPSL + TrouBLE
# stack folded in (`src/ble.rs`), advertising as `OBC-XXXX` (S0 §2 — the FICR serial tail). On
# this 256 KB DK it compiles the MAP PLANE OUT (~128 KB freed) and boots a text-only BLE status
# UI on the LS021 instead; SD, RRAM settings, buttons, and the real sensors all stay up.
# `--no-default-features` is REQUIRED (it swaps the critical-section impl to MPSL's — a
# compile_error catches the wrong invocation). Doesn't combine with tft/synth/debug-uart.
cargo run --release --no-default-features --features ble
```

(The standalone FLPR waveform bench bin `ls021_flpr_bringup` was retired in #177 once the app drove
the LS021 on glass; the M33-direct `ls021_bringup` bench was retired earlier in #176; the A1 BLE
spike bin `ble_spike` was retired at #270 when the stack moved into `main.rs`/`src/ble.rs`. All are
in git history if an isolation bring-up is ever needed again — the FLPR transport is
`src/ls021_flpr.rs`, exercised by the default build.)

### LS021 FLPR builds — DK wiring (issue #165)

The default build drives the LS021 panel itself, not the ST7789.
The source bus + `BCK` + COM stay on **P2** (P2.00–06 data/clock, P2.07/08/10 COM, P2.09 heartbeat
LED); the four gate lines + `BSP` sit on **free P1 pins** — `GSP P1.00 / GCK P1.01 / GEN P1.12 /
INTB P1.10 / BSP P1.14` — deliberately **off** the SD-SPI bus (P1.06/07/11/12) and VCOM (P1.04/05)
the app needs.

The DK breaks out only **P1.00–14** (P1.02/03 are NFC), which is one pin short for everything the app
puts on P1 — so in the **default (LS021) build**, SD **`CS` moves from P1.12 to P0.00** (one jumper
on the SD breakout; it's a plain GPIO, and the M33 already drives P0 for BTN3). That frees P1.12 for
`GEN`. The SD bus pins (SCK P1.11 / MISO P1.07 / MOSI P1.06) are unchanged; the opt-in `tft` build
keeps `CS` on P1.12.

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

### FLPR toolchain (the default build)

The FLPR backend cross-compiles a tiny freestanding C blob for the RISC-V coprocessor, so it
needs an `rv32emc`-capable GNU gcc — install once:

```sh
brew install riscv64-elf-gcc        # or set RISCV_GCC=<path> to an xPack / Zephyr-SDK toolchain
```

It's needed by the **default** (FLPR) map firmware. Only the opt-in **`--features tft`** ST7789
firmware needs **no** RISC-V toolchain (CI installs the gcc only on the FLPR legs; `build.rs` keys the
blob on the absence of `tft`). On Linux/CI the apt package `gcc-riscv64-unknown-elf` works too.

If `cargo run` prompts to pick a probe (e.g. another ST-LINK is attached), pass
`--probe <vid:pid:serial>` for the J-Link.

## BLE stack — dependency pins & gotchas (issues #269/#270, epic #267)

The A1 spike proved `nrf-sdc` (Nordic's closed-source SoftDevice Controller + MPSL bindings) +
`trouble-host` on this DK; A2 (#270) folded that stack into the real firmware as `src/ble.rs`
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
  `CLOCK_POWER`, and `SWI00` (low-prio scheduling) — which is why `main.rs`'s high-priority
  `InterruptExecutor` sits on **SWI01** (every build; the full priority ladder is in `main.rs`'s
  module doc). Peripherals owned outright: GRTC CH7–11, `TIMER10`, `TIMER20`, `TEMP`, `CRACEN`
  (LL crypto RNG), and a raft of PPI/PPIB channels (grouped in `main.rs`, consumed by
  `ble::run`). The HF **crystal** is an MPSL hard requirement (`HfclkSource::ExternalXtal`) —
  the `ble` build's boot config sets it; non-BLE builds keep the internal RC.
- **RAM.** The `ble` build's statics end ~104 KB in (vs ~210 KB for the map build in 244 KB) —
  the map plane's exclusion is what buys that. The budget assert in `main.rs` counts the BLE
  residents (`ble::RESIDENT_BYTES`) and fails a `ble`+map build on this DK at compile time; the
  512 KB LM20 relaxes the `has_map` line in `build.rs` to run both planes together.

## Connection lifecycle (A3, #271)

`ble::run` is a loop with **no terminal state**: advertise → serve the link → re-advertise,
forever, unattended. Any disconnect (any reason) drops straight back to advertising; even an
advertise *error* only pauses a beat before retrying. What A3 added on top of A2's bare
connect/hold:

- **Advertising interval policy (S0 §2)** — *fast* (40 ms) for 30 s after boot and after every
  disconnect, then *slow* (1000 ms) indefinitely. Legacy connectable adv doesn't self-terminate,
  so the fast→slow switch is a host-side timer (`select` against the advertiser), not the HCI
  duration field.
- **Parameter negotiation on connect (S0 §3.4)** — the device *requests* 2M PHY, data-length
  extension (251-byte PDUs), and a relaxed idle connection-parameter set; iOS accepts what the OS
  allows. Every request is a **preference** (the protocol is correct at any MTU/PHY, just slower),
  so each is `with_timeout`-bounded and best-effort — a stalled or rejected procedure is logged
  and skipped, never a reason to drop the link. `conn_params(true)` pins the *fast* set A5's data
  plane switches to during transfers.
- **Watchdog policy** — **no hardware WDT (yet)**. The lifecycle is a *structural* watchdog: every
  host op is timeout-bounded and the loop has no path that can block permanently, so a stuck
  procedure degrades to a reconnect rather than a hang. A hardware `WDT` petted from the host
  runner is deferred to A9, where it can be co-designed with the idle/WFI wake pattern.
- **Telemetry** — connects / disconnects / last disconnect reason (named + numeric) / negotiated
  MTU + PHY, all logged over RTT and shown on the status screen (the `link c/d xNN` and
  `NNms 2M mMMM` lines) — the raw material for the A9 soak assertions.

**Verify** with nRF Connect (the A1–A4 oracle): connect and confirm the negotiated MTU (247),
2M PHY, and the interval settling to the idle set; disconnect/re-connect and walk out of range —
the counters bump, the reason logs, and it always returns to advertising.

## GATT control plane (A4, #272)

`ble::run` now serves the **real** control plane the iOS app discovers on connect
(`obc-ble-interface-spec.md` §3, the S0-frozen UUIDs). One `#[gatt_server]` in `src/ble.rs` holds
three services — mirroring the spec section so there's one place to diff:

- **DIS** (`0x180A`) — Firmware Revision (`<crate-semver>+<git-short>`, e.g. `0.1.0+ca9b336`;
  `build.rs` emits `OBC_FW_GIT`), Hardware Revision (`nrf54l15-dk`), Serial Number (the 16-hex FICR
  `DEVICEID`, whose last four digits are the `OBC-XXXX` advertised name).
- **BAS** (`0x180F`) — Battery Level, read + notify, fed from the `FuelGauge` seam (the status plane
  publishes each poll via `ble::publish_battery`; a `battery_task` re-notifies on a slow cadence).
- **OBC Control** (`3C92XXXX-9916-4EBA-ABC2-342FE08F6B10`) — all eight characteristics: `command`,
  `status` (notify), `objectStore`, `config`, `transferControl` (write+notify), `diagnostics`
  (reserved, reads 0 bytes), `psm`, `protocolVersion` (= 1). The 128-bit service UUID is advertised
  (name moves to the scan response) so the app's `scanForPeripherals(withServices:)` filter matches.

Control-plane writes are answered with the S0-typed `status` envelope — never a hang or a bare ATT
failure: `command` → `commandResult` (`deleteObject` is real since A6), `transferControl` →
`transferResult` (typed `busy`/`notFound`/`error` rejects; valid transfers are armed to the data
plane), `config` is validated + **persisted** (A6: RRAM settings, see below).

A4 also stands up a **minimal L2CAP CoC**: the SPSM `0x0080` is registered on the stack and
published in `psm`, and `serve_coc` accepts the channel and **drains/discards** its bytes. That is
just enough for the iOS app's `connect()` to complete (it gates completion on the L2CAP channel
opening); the framing crate + transfer state machine + real object payloads are A5/A6, bonding A8.

**Verify** — nRF Connect: the full service/characteristic table matches S0, DIS strings + serial are
real, BAS notifies, `protocolVersion` reads `1`, `psm` reads `0x0080`, and a write to
`command`/`transferControl` yields a `status` notification. Then the iOS companion (Debug build,
`-OBCTransport ble`): `connect()` completes end-to-end and the device row shows the real firmware
revision — first app↔firmware contact.

## CoC data plane + echo loopback (A5, #273)

A5 turns that drain into a **real bulk-transfer data plane**, driven by the host-tested `obc-ble`
workspace crate (the S0 descriptor codecs + the whole-object transfer state machine — `cargo test -p
obc-ble`, pinned to `protocol-vectors/` alongside the Swift side). The control plane and the CoC are
separate futures coordinating through one `Signal`:

- A `transfer_control` write is decoded (`obc_ble::TransferControl`) and classified (`classify_transfer`):
  an `echo` **upload** (S0 type 8) is *armed* — its descriptor signalled to the CoC task and answered by
  the data plane; real routes/rides/diagnostics ride the same arming path since A6/A7; anything
  nonsensical still gets an immediate S0-typed `transferResult(error)`, an `abort` an `aborted`.
- `serve_coc` → `run_echo` feeds the CoC bytes through an `obc_ble::Receiver` (a running CRC-32, **no**
  reassembly buffer, S0 §5) and streams each SDU straight back byte-for-byte, verifying **one**
  whole-object CRC at the end and notifying `committed` / `crcMismatch`. Zero storage involvement — the
  loopback that proves the data plane end to end (real objects → SD are A6). On the first transfer the
  link is asked for the fast `conn_params` set; the kB/s is logged over RTT.

**Verify** — the Mac echo rig `companion-ios/EchoHarness` (reuses the app's `BLEChannel` + CoC byte
layer): `swift run echo-harness --count 1000 --size 32768` round-trips 1000 × 32 KB byte-identical;
`--corrupt` flips a byte per object and expects the device to reject it with `crcMismatch`. Watch the
per-echo throughput + `committed`/`crcMismatch` in the RTT log.

## Route object plane (A6, #274)

A6 wires that data plane to real storage — the epic's golden path (komoot GPX → app → upload →
**ride it**). The pieces (`object_store.rs` + the A6 half of `sd.rs`):

- **Upload → SD.** A route upload streams into `/routes/UPLOAD.TMP` (running CRC, no reassembly
  buffer); commit verifies the transfer CRC **and** that the bytes parse as OBCR, then promotes.
  embedded-sdmmc has no `rename`, so atomicity is a copy with the 4-byte `OBCR` magic **held back
  as zeros**, patched in as the last write — a power cut at any point leaves only files every
  header read rejects, never a half-route in the catalog; the boot scan sweeps that exact
  signature. Uploads get 8.3 `RTnn.OBR` names (LFN creation isn't available); the catalog scan —
  including the **map build's** — matches `*.OBR` beside `.obcr`, so an uploaded route appears in
  the on-device Route menu after a reflash. Replace-by-id deletes the old copy only after the new
  bytes validate. Uploads are **not resumable** (S0 §1 principle 4): an interrupted upload (a drop
  or an `op=3` abort) discards the partial and the app re-sends the object from the start — trivial
  for a tens-of-kB route.
- **List / detail / delete.** `routeList` is built from the stored OBCR headers; a route detail
  download streams the stored file verbatim (whole-object CRC pre-pass, then raw CoC chunks);
  `deleteObject` removes the file. Object ids are **durable across reboots** (the identity
  rework, #289): an uploaded route's id lives in its `RTnn.OBR` filename and is recovered at the
  boot scan; only side-loaded `.obcr` files get session-scoped ids from the reserved `0xFF00`
  band. Every store movement notifies `storeChanged` + the refreshed `objectStore` digest.
- **Config ↔ settings.** The `config` characteristic round-trips through the persisted settings
  (codec v3 adds the device name to the RRAM blob): a rename survives a power cycle and replaces
  the advertised `OBC-XXXX` on the next advertise cycle — no reboot; an empty name clears back to
  factory.

**Verify** — the E2E golden path: share a GPX to the iOS app on a phone, upload (B5 sheet), reflash
the **map** build, and the route is in the device menu and rideable (SD persists across flashes).
List/detail/delete + the mid-upload abort-and-re-upload are exercised from the Mac harness
(`companion-ios/EchoHarness`: `upload`/`list`/`detail`/`delete`/`abort-test`) — the app's
list/detail screens land on the B track.

## Ride download + diagnostics (A7, #275)

The reverse direction, and it's mostly *already built*: every ride Finish on the **map** build
writes — beside the `.gpx` — a second file `/tracks/RDnn.ORD` that is **byte-for-byte the S0 §7.2
ride object** (header with the ride totals + wall-clock start, 14-byte points; encoded in one
streaming pass over the track log, the version byte held back as the commit point like the route
upload's magic). The `ble` build then just scans `/tracks` at boot, serves `rideList` from the
stored headers, and streams a requested ride verbatim through the same CRC-pre-pass + chunked
download path a route detail uses. The durable ride id in the filename is what the app's
synced-set keys on: sync pulls only rides it hasn't landed, and a ride deleted *in the app* is
tombstoned there — never deleted here (`deleteObject` on a ride answers `notFound`, deliberately;
the device keeps every ride until a future device-side management UI). `diagnostics` (type 4)
downloads an honest text blob: fw/hw/serial, an RRAM-persisted **boot counter** (one 16-byte line
in the SETTINGS carve, bumped every boot on every build), uptime, the A3 link counters, and the
store counts — readable with no SD card, because that's when you want it.

**Verify** — record 2–3 rides on the map build (`synth` indoors is fine: load a route, ride, Finish),
reflash `ble`, and the app's sync pulls them; spot-check a decoded ride against the `.gpx` twin the
same Finish wrote. Ids must survive a device power cycle (list → reboot → same ids); the boot
counter must increment across power cycles (diagnostics read, or the `boot #N` RTT line).

## Pairing + bonding (A8, #276)

A8 flips security on (S0 §8): the whole OBC Control service becomes
`permissions(authenticated)` — an **encrypted, LESC-authenticated** (MITM) link — except
`protocolVersion`, which stays open for the connect-time version check (DIS/BAS stay open too). The
CoC accept is refused on an unencrypted link. Pairing is **LESC passkey display**: IO capability
`DisplayOnly`, so the device shows a 6-digit code (the status screen's big-font marquee, `Font::Huge`)
that the rider types into the phone's system dialog. The whole thing rides `trouble-host`'s
`security` feature (P-256 ECDH in-host) — the SMP CSPRNG **auto-seeds from the controller's `LeRand`**
at host init, so there's no entropy to plumb; nrf-sdc exports every security HCI command
(LTK reply, enable-encryption, the resolving-list set).

The single bonded peer (LTK + peer identity/IRK + level) persists in the **RRAM SETTINGS carve** — a
64-byte CRC-checked slot at `BOND_OFFSET` (`settings.rs`), clear of the settings slot @0 and the boot
counter @2048, reached through `ObjectStore::{load,save,clear}_bond`. It survives power cycles **and a
firmware reflash** (the carve sits above the application image — `probe-rs download` of the app region
leaves it). At boot the bond is handed to the host (`add_bond_information`) so the controller's
resolving list resolves the phone's rotating RPA and re-encrypts with the stored LTK — silent
reconnect, no dialog. The device keeps its **stable** static-random address (no device privacy), which
is what the phone's background reconnect keys on. `set_bondable(true)` per link lets a pairing persist
keys (trouble's default is *not* bondable).

**Single-peer policy:** one bond slot; a fresh passkey pairing replaces it — the on-screen passkey is
the anti-stranger control, so there's no device-side "clear bond" gesture. When the phone forgets the
device and re-pairs, trouble raises `BondLost`; we clear the stale bond and store the fresh one.

**Verify** (on glass, with the iPhone app): (1) fresh pair — passkey on the panel (webcam), typed on
the phone, bond lands; a wrong/declined code → clean re-advertise. (2) Power-cycle the device → the
phone reconnects with no dialog; same after an app restart or a walk-away. (3) Reflash `ble` → still
no dialog (bond survived). (4) App *Forget* + iOS *Settings ▸ Bluetooth* forget → next contact
re-pairs with a fresh passkey. (5) A6 upload + A7 sync re-run over the encrypted link. (6) nRF Connect
as an unbonded stranger: reads DIS/`protocolVersion`, access-denied on every gated char + the CoC.

## Driving it from a host (`debug-uart`)

With the `debug-uart` firmware flashed and VCOM HWFC off, replay a recorded ride over
the VCOM from the desktop feeder (from the `firmware/` workspace dir):

```sh
cargo run -p obc-usb-host -- --gpx ../kandel.gpx        # add --port <VCOM tty> if not auto-detected
```

`obc-usb-host` streams the `.gpx` as fake GPS fixes (plus a baro/compass slider and an
on-screen button row that injects encoder/Back presses), and shows the device's
render-stats telemetry coming back. It's the same `obc-platform::debug_link` wire
protocol the simulator uses — only the transport differs (a VCOM UART here). `--list`
enumerates serial ports; the VCOM is the J-Link CDC port.
