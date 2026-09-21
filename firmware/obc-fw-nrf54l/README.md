# obc-fw-nrf54l — nRF54LM20-DK firmware

The real hardware target: `obc-app` on an nRF54LM20-DK (Cortex-M33), with map, routes and tracks
on a microSD card. It drives the reflective **LS021B7DD02** memory LCD through the nRF54L's
**FLPR** RISC-V coprocessor — the only display path — and records rides directly as flat-store
Ride objects. GPX export happens in the companion app after sync; the device writes no GPX.

[`src/board.rs`](src/board.rs) is the canonical peripheral and pin ledger. This README is the
board setup and build/flash guide. The LS021 protocol is on the
[display-protocol page](https://openbikecomputer.com/hardware/display-protocol/); the
cross-core display architecture is in [firmware/docs/ls021-flpr.md](../docs/ls021-flpr.md).

## One-time board configuration (nRF Connect Board Configurator)

Three settings, written to the DK's interface MCU, persisting across power cycles. Click
**Write config** after changing anything (blue dots = unwritten). No soldering needed.

1. **VDD / VDDM → 3.3 V.** The 1.8 V default is too low for the LS021's logic. Feed the panel's
   `Vin` from the DK's 5 V / VBUS so its 3.3 V LDO has headroom.
2. **External memory → OFF.** This disconnects the on-board QSPI flash, freeing **P2.00–P2.05**
   for the microSD card in native 4-bit SD mode. Maps live on the card; the flash is never used.
3. **VCOM hardware flow control → OFF.** Required for `debug-uart` builds. With HWFC on, the
   interface MCU gates host→device bytes on RTS, which this firmware never asserts. Symptom:
   device→host telemetry works, but injected fixes and button presses are silently ignored.

## Wiring (DK headers)

The build drives the LS021 panel. Also on the board: a microSD breakout on P2.00–05 (native
4-bit SD, no SPI), the four DK buttons, and the J-Link VCOM and RTT over the DK's USB.

### Full pin map

**Port P2 — MCU/fast domain. All 11 pins used: the microSD bus and the panel's source bus, two
of them shared.** The card's six pads are fixed by Nordic's sEMMC soft peripheral; the display's
six data lines take the four pins the retired SD-SPI path freed plus the two shared pads, whose
`CTRLSEL` flips per mode. Display and storage never run at the same instant —
`src/flpr_mux.rs` time-multiplexes the one FLPR.

| Pin   | sEMMC | Display | Notes                                                        |
|-------|-------|---------|--------------------------------------------------------------|
| P2.00 | D3    | **B0**  | **shared** — `CTRLSEL` per mode; internal pull-up in storage mode |
| P2.01 | CLK   | —       | card only; parked as an input in display mode                 |
| P2.02 | D0    | —       | card only; parked as an input in display mode                 |
| P2.03 | D2    | —       | card only; parked as an input in display mode                 |
| P2.04 | D1    | **B1**  | **shared** — `CTRLSEL` per mode; internal pull-up in storage mode |
| P2.05 | CMD   | —       | card only; parked as an input in display mode                 |
| P2.06 | —     | R0      | source data (even-`x` R)                                      |
| P2.07 | —     | BCK     | source shift clock                                            |
| P2.08 | —     | R1      | source data (odd-`x` R)                                       |
| P2.09 | —     | G0      | source data (even-`x` G)                                      |
| P2.10 | —     | G1      | source data (odd-`x` G)                                       |

The packed wire word is `DATA_MASK = 0x751` (`B0`→0, `B1`→4, `R0`→6, `R1`→8, `G0`→9, `G1`→10),
pinned from both sides by `obc_display::ls021::wire`'s goldens and by a test that parses
`src/flpr/flpr_scan.c`. `even`/`odd` is **0-based `x`**: the `*0` lines carry `x = 0, 2, 4, …`.
The panel datasheet numbers columns from 1 and calls that same line the *odd* column.

**Pad configuration per mode** (`src/semmc.rs`, `configure_storage_pads` / `configure_display_pads`):

| | the six card pads | the four card-only pads |
|---|---|---|
| **storage** | Output, input Disconnect, **E0/E1** drive, `CTRLSEL = VPR`, `GPIOHSPADCTRL.BIAS = 2`; internal pull-up on `D3`/`D1` only | (same — all six are the card's) |
| **display** | `P2.00`/`P2.04` → Output, S drive, no pull, `CTRLSEL = GPIO`, `GPIOHSPADCTRL.BIAS = 2` | Input, no pull, `CTRLSEL = GPIO` — the external pull-ups hold the bus idle-high and the card stays inert |

`GPIOHSPADCTRL.BIAS` is **port-global**, not per-pin, so it is not restored per mode. Both
configurations set the same constant 2 (`semmc::HS_PAD_BIAS`): Nordic's value for the card at
32 MHz, and the panel's ≤0.758 MHz `BCK` is indifferent to it.

Only `D3`/`D1` get an internal pull-up; this desk breakout carries its own resistors on
`CLK`/`D0`/`D2`/`CMD`, and 13 kΩ ∥ 10 kΩ would sit under the SD spec's floor. **The production
board should fit external 10–100 kΩ pull-ups on `CMD`/`DAT0–3` (none on `CLK`) and run all
internal pulls off.**

**Port P1 — PERI domain ≤8 MHz (gate/BSP + sensors + COM + VCOM + buttons):**

| Pin   | Signal       | Notes                                                     |
|-------|--------------|-----------------------------------------------------------|
| P1.03 | I²C SCL      | shared GPS + altimeter + compass bus (TWIM22)             |
| P1.04 | I²C SDA      | same bus                                                   |
| P1.05 | GPS TX-Ready | *optional* DDC data-ready IRQ (active-high)                |
| P1.08 | BTN2         | BACK                                                       |
| P1.09 | BTN1         | DOWN                                                       |
| P1.10 | GSP          | gate start pulse                                           |
| P1.11 | GCK          | gate clock                                                 |
| P1.12 | GEN          | gate enable                                                |
| P1.13 | INTB         | frame envelope                                             |
| P1.14 | BSP          | source sub-line start (the lone P1 source line)            |
| P1.16 | VCOM TX      | UARTE20 → host (`debug-uart` builds only)                  |
| P1.17 | VCOM RX      | UARTE20 ← host (`debug-uart` only; needs HWFC OFF)         |
| P1.22 | VCOM (COM)   | COM electrode, HighDrive — or a GPIOTE toggle on `com-hw`  |
| P1.23 | VB           | COM electrode                                              |
| P1.24 | VA           | COM electrode (inverse phase)                              |
| P1.25 | LED1         | liveness heartbeat                                         |
| P1.26 | BTN0         | UP                                                         |
| P1.27 | backlight    | **PROVISIONAL** — PWM20 ch0, 1 kHz; also DK LED2; needs a gate pull-down |

**Port P0 — low-power domain:** `P0.05` is `BTN3` (SELECT).

> **These tables mirror the canonical `src/board.rs` ledger and the constructors in
> `src/main.rs`.** If they disagree, the constructors are the executable authority and both
> documents must be fixed.

### The provisional backlight pin

The panel is reflective with no light of its own, and no front light is fitted. What is wired is
the seam: `obc_ports::Backlight` on a real PWM output, so the quick drawer's brightness control
has hardware behind it and a later driver is a new impl of the same trait.

**P1.27, PWM20 channel 0, 1 kHz** (`src/panel_power.rs`). On the shipping board the net is the
gate of a low-side MOSFET switching the front light. On the DK it doubles as the buffered LED2
net, so the five-step duty ladder is visible on the desk without a logic analyzer.

**Schematic-time: the gate needs an external pull-down.** The pin is push-pull and idles low,
but only once `PanelBacklight::new` has run. Before that it is in its GPIO reset state — input,
no pull, high impedance — and that window covers all of `obc-boot`, so a DFU install spends its
entire duration there. System OFF is the same from the other end: the PWM stops wherever the
waveform left the line. A floating MOSFET gate is not a defined lamp state.

The level → duty ladder is `obc_platform::backlight`, board-agnostic and host-tested. It is
square-law (`40 · (level + 1)²` per mille, countertop 1,000) because evenly spaced duty is not
evenly spaced perceived brightness. There is no off step.

### Sensors on the shared I²C bus

TWIM22 carries the u-blox **SAM-M10Q** GNSS (DDC `0x42`), the Bosch **BMP581** altimeter
(`0x47`, or `0x46` if the breakout straps `SDO` low), and an **AK09916** magnetometer inside a
TDK **ICM-20948** (IMU at `0x68`/`0x69`, put in I²C bypass so the magnetometer answers directly
at `0x0C`). No addresses clash, so all three share SDA/SCL with no extra pins.

Only the three magnetometer axes are used; accel and gyro stay asleep. The compass supplies
heading-up orientation while the rider is stopped, on its own ~5 Hz cadence; once moving, the
GPS course is the heading. The shipping board is expected to drop the 9-axis IMU for a plain
3-axis magnetometer, which the `obc_platform::compass` / `obc_platform::icm20948` split allows.

**GPS TX-Ready is optional.** When wired it asserts as a NAV-PVT message becomes ready, so the
bus does zero work between fixes. The SparkFun SAM-M10Q breakout (GPS-21834) does not break it
out, so the task falls back to DDC polling once per fix interval — the same ~1 Hz wake cadence.
Nothing to wire on that board.

**Power tip:** wire **V_BCKP** to an always-on rail, supercap or coin cell. It backs the
receiver's RTC and ephemeris across a power-off, turning every cold ~30 s fix into a warm fix in
seconds.

If GPS does not answer the first probe, the sensor task retries during the 150-second boot
acquisition window and sends configuration before reading fixes. The startup sensor-warning
bundle publishes on first response or at the deadline. This is a one-time startup result, not a
report of live availability.

Idle sends `UBX-CFG-RST` controlled GNSS stop; tracking resumes with controlled start. Startup
also sends START after the receiver answers, so an MCU reset recovers a receiver left stopped by
the previous session. The receiver does not acknowledge CFG-RST; new NAV-PVT epochs confirm
acquisition. An older image can leave the receiver in indefinite software standby, which I²C
traffic cannot wake — remove its power before starting this image. See the
[SAM-M10Q integration manual §3.3 and §3.5.3.3](https://content.u-blox.com/sites/default/files/documents/SAM-M10Q_IntegrationManual_UBX-22020019.pdf).

## Build and flash

**One-time prerequisite: flash the bootloader.** The app is linked at `0x8000`; the 32 KB below
it belongs to [`obc-boot`](../obc-boot/README.md), which must be on the chip once
(`cd ../obc-boot && cargo run --release`). It survives every app reflash, since probe-rs only
writes each ELF's own address range. A device without it shows no LED blink and never boots; the
recovery recipe is in that README.

From this crate directory (a standalone crate for `thumbv8m.main-none-eabihf`; `cargo run`
flashes and streams defmt/RTT over the on-board J-Link):

```sh
# Default: full map + ride loop on the LS021 via the FLPR, real SAM-M10Q GPS + BMP581 on the
# shared I²C bus, and both companion transports up (BLE radio and USB device plane). Builds the
# RISC-V blob, so it needs an rv32emc gcc. With no Qwiic hardware attached it still boots and
# idles waiting for a fix.
cargo run --release

# Indoor / no hardware: the on-board SynthLocation square loop replaces the real GPS.
cargo run --release --features synth

# Indoor: stream a recorded ride from a host over the VCOM debug-sensor feed. Needs HWFC OFF.
cargo run --release --features debug-uart
```

**There is no `ble` feature and no `usb` feature.** The nrf-sdc + MPSL + TrouBLE stack
(`src/ble/`) and the USB device plane are in every build: the full map/ride app, the companion
link over both transports, and one SD card plus RRAM settings behind one async mutex, in one
image. The device advertises as `OBC-XXXX` (the FICR serial tail).

### The flat store bench

```sh
cargo run --release --bin flat_store_bench
```

It measures every figure `specs/FLAT_Store_Format.md` states: §8 initialization, §5.6 mount at an
empty / 300-entry / 1024-entry catalog and with a ride recording, §5.5's commit at each of those,
§7.2's checkpoint cadence, and §6.1's read path over a 2 GiB object with its read amplification,
which must be 1.00. Every timed figure is reported as three terms — the card's write half, its
read half, and what was left for the M33 — measured inside the block-device adapter.

> **DESTRUCTIVE.** The flat store owns the raw card from LBA 0, so a run destroys the partition
> table. Anything on the card is gone. It refuses a card that already carries a flat store under
> another `StoreId`; override with `FORCE_REINIT`.

Phase one takes about 44 minutes on a 64 GB card; phase two takes 3 seconds. Which phase runs is
decided by what is on the card. A card that is not this bench's store gets phase one, ending with
a ride left **recording**. `probe-rs reset` then runs phase two on that ride — §7.3 recovery
through `recovered_ride`, §7.2's ride end, and the whole ride read back byte for byte. A third
reset finds no ride recording and starts phase one over.

To erase the corpus without recreating it, use the reset-only mode. It initializes one empty
store and parks before serial ingest and before the measurement phase:

```sh
cargo run --release --bin flat_store_bench --features flat-store-reset
```

This destroys every card object, not only benchmark routes. Wait for RTT to print
`RESET ONLY complete`, stop the runner, then flash the normal app image, which can receive maps
and routes over its USB device plane.

## Board connection diagnostics and recovery

Run `obc board doctor` before reflashing a board that appears unresponsive. It reports probe-rs
version, connected probes, serial ports and their owners, active probe-rs processes, and native
USB enumeration. It does not reset the device or send UART commands. macOS and Linux only.

All `obc` flash/debug/RTT commands and both standalone Cargo runners use
[`tools/board.py`](../../tools/board.py). The runner selects `nRF54LM20A`, prints the ELF path and
SHA-256, and holds one per-user lock across worktrees for the duration of flash, reset or RTT. A
busy lock reports its owner instead of killing it. Direct probe-rs and SEGGER commands do not
share this lock; stop those sessions first. One board session at a time, even with several probes
attached. Select one with `PROBE_RS_PROBE=VID:PID:SERIAL` or `obc board … --probe VID:PID:SERIAL`;
selection is noninteractive, so an ambiguous selection fails rather than waiting for input.

`obc rtt [ELF] --log /tmp/obc-rtt.log` attaches without compiling, programming or resetting. Use
the exact ELF for the installed image, including its compile-time `DEFMT_LOG`. Without an ELF it
uses this worktree's last release build. `cargo rtt` can rebuild before attaching; it never
programs the device, so a rebuilt ELF can be the wrong decoder for the installed image.

The runner keeps `--verify` enabled and passes `--disable-double-buffering`, which avoids
concurrent debugger RAM writes during RRAM programming — the workaround
[probe-rs #3775](https://github.com/probe-rs/probe-rs/issues/3775) reports for
nRF54LM20A/J-Link corruption. A failed verification stops the command; the runner does not retry
it or start an unverified image. Keep the workaround after a probe-rs upgrade.

| Symptom | Check and recovery |
| --- | --- |
| Probe busy / exclusive-access error | Use `obc board doctor`. Stop the owning RTT, debugger or programmer session with Ctrl-C. Wait for it to exit, then retry. Do not kill all probe processes. |
| Flash read-back mismatch | Keep verification enabled and double buffering disabled. Preserve the failing output and ELF. Do not use an erase-all or run an unverified image. |
| RTT stops or cannot decode | Confirm the ELF and firmware version match. Close and reattach RTT. Use `obc board reset` only when restarting the application is intended; an idle device may legitimately produce no logs. |
| VCOM writes succeed but commands have no effect | J4 carries VCOM. Confirm the `debug-uart` image, correct CDC port, baud, and HWFC OFF. Close any other serial owner. Open the port once for the session. A write return value alone does not prove delivery. |
| VCOM remains unresponsive after those checks | Power-cycle the DK. A target reset does not reset the J-Link bridge. With both cables connected, unplugging one may leave the board powered. |
| J3 absent from host USB enumeration | J3 is the separate native device cable, VID:PID `1209:0001`. Check the RTT VBUS lines, then reconnect J3. J4 serial ports do not prove J3 is working. |
| J3 enumerates but the application cannot connect | Close the desktop or browser session that owns the interface. Reconnect J3 and open a fresh session. Enumeration alone does not prove a transfer works. |

### Serial map ingest — putting a real map on the card

Before either bench phase, the bench advertises on the DK's VCOM for ten seconds. If a host
answers it takes objects over the cable instead of measuring anything, then parks. This exists
because a board session needs a real packed map on a real flat store, and the bench image is
storage and nothing else — no USB plane, no radio. The wire (magic, kind, length, CRC, then acked
chunks) is documented in the binary's module docs. The CRC is verified before the commit, so a bad
transfer publishes nothing and a retry is a fresh put with a new `ObjectId`.

Start the host **first** — it blocks on the device's advertisement — then flash and run:

```bash
# shell 1, from the repo root
python3 tools/bench_ingest.py --port /dev/cu.usbmodem*133 \
    --file "$(python3 tools/fixtures.py resolve monaco-upahead | awk '/^map/ {print $2}')" \
    --kind map --name monaco.obcm

# shell 2, from this directory. Stop an existing RTT session with Ctrl-C first.
cargo run --release --bin flat_store_bench
```

To download and attach separately, from the repository root:

```bash
obc board download firmware/obc-fw-nrf54l/target/thumbv8m.main-none-eabihf/release/flat_store_bench
obc board reset
obc rtt firmware/obc-fw-nrf54l/target/thumbv8m.main-none-eabihf/release/flat_store_bench
```

`sim-monaco`'s `monaco.obcm` is 718,336 B, about **63 s** at the default 115,200 8N1.
`INGEST_BAUD` takes that to ~7.5 s at `Baud1m` (pass `--baud 1000000` to match), at the cost of
finding out mid-session whether this J-Link's CDC will carry a megabaud. RTT prints the
`ObjectId` and a full catalog census after every commit, which is the acceptance evidence.

**A `--baud` mismatch is silent, and it wipes the card.** Check it first. The host transmits only
after decoding a valid READY, so at the wrong rate it sends nothing and waits — while the device
sees an idle line, concludes nobody is there, and starts the destructive run. Neither side errors.
The signature is that exact pair: **RTT says `nobody answered` while the host says it is still
waiting.** The device prints its own baud next to `nobody answered` for this reason.

**If the host reports no answer** and RTT does *not* say `nobody answered`, the device sent a
`GONE` frame and the host prints its reason:

- `the window closed` — the host started after the ten-second window and the **destructive**
  measurement suite is now running. Reset immediately.
- `the session is over` — the board took its last object and parked. Reset to re-arm.
- `reservation held` — a commit was refused and its extents are held until a remount. Reset.
- `could not frame` — something else is driving this tty. The bench **refused** the measurement
  run, so the card is untouched; clear the line and reset. This is not the baud case, which is
  silent rather than noisy.

Two never reach the wire: the baud mismatch, and a card carrying a foreign `StoreId` (refused
before ingest is offered; `FORCE_REINIT` is the override).

Only if no `GONE` arrives, RTT shows the bench advertising, and the baud is confirmed has the
J-Link's VCOM wedged: host writes succeed, RTT keeps flowing, nothing reaches the device. A
physical power-cycle is the only fix; `probe-rs reset` does not clear it.

## LS021 FLPR wiring

The source bus and `BCK` stay on P2 (see [the pin map](#full-pin-map)); the four gate lines and
`BSP` sit on `GSP P1.10 / GCK P1.11 / GEN P1.12 / INTB P1.13 / BSP P1.14`, and COM on P1.22–24.
The display shares P2 with the microSD card: `R0/R1/G0/G1` on `P2.06/.08/.09/.10`, and `B0/B1`
time-share `P2.00/.04` with the card's `D3/D1`.

The gate and `BSP` pins, the masks in `src/flpr/flpr_scan.c`, and the physical 21-pin FPC harness
must all agree. If a gate line stays dark on glass, confirm the pin is broken out on your DK
header and remap all three together.

### COM on P1.22–24: one wiring contract, two drivers

The harness routes COM (`VCOM`/`VB`/`VA`) to P1.22/P1.23/P1.24. The default build owns those three
nets as high-drive GPIO outputs and `com::com_task` toggles them at ~60 Hz, waking the M33 about
120 times per second. The opt-in **`com-hw`** build owns the same pins as GPIOTE20 channels and
drives the same waveform from a zero-CPU **TIMER21 → DPPIC20 → GPIOTE20** chain. `com-hw` is
on-glass and logic-analyzer verification pending, so the shipping default stays on the M33 driver.

### FLPR toolchain

The FLPR backend cross-compiles a freestanding C blob for the RISC-V coprocessor, so it needs an
`rv32emc`-capable GNU gcc. Every build needs it — `build.rs` always compiles the blob.

```sh
brew install riscv64-elf-gcc        # or set RISCV_GCC=<path> to an xPack / Zephyr-SDK toolchain
```

On Linux and CI the apt package `gcc-riscv64-unknown-elf` works too. If `cargo run` prompts to
pick a probe, pass `--probe <vid:pid:serial>` for the J-Link.

## microSD over sEMMC — the storage transport

**There is no SPI.** The card runs in native 4-bit SD mode on the same FLPR that drives the panel,
through Nordic's **sEMMC soft peripheral** — a 13,636 B position-independent RISC-V image
(vendored at `vendor/semmc/`, `LicenseRef-Nordic-5-Clause`) that turns P2.00–05 into a real SD
host controller. The M33 fills a register block in the image's RAM carve and pokes VPR tasks; the
coprocessor does the clocking, CRC and framing.

**Wiring.** Six jumpers, no chip-select: `P2.00 D3 · P2.01 CLK · P2.02 D0 · P2.03 D2 · P2.04 D1 ·
P2.05 CMD`, plus GND and 3V3. The assignment is fixed by the soft peripheral. Pull-ups per
[the pad table](#full-pin-map).

**Sharing the coprocessor.** `src/flpr_mux.rs` time-multiplexes it: a switch to storage is 29 µs
(park the hart, flip the pads, warm-boot the resident image, power it on), a switch back 138 µs
(quiesce, park, flip, relaunch the display blob). The card keeps its `tran` and High-Speed state
across a switch and is never re-initialised. The mode is lazy, so a run of reads pays 29 µs once
and a run of frames 138 µs once; the panel keeps getting frames throughout a multi-megabyte upload
because storage sessions never outlive one synchronous burst.

**Writes cap at 21.3 MHz.** 32 MHz writes fail card-side CRC on the jumper harness — a clean
failure, nothing programmed — while 32 MHz reads are spotless. The clock is per-transaction, so
mixed-rate is free. Re-test on soldered hardware.

**If the card does not come up**, the RTT log names which rung failed (`SemmcError`), and an
aborted transfer is decoded (`command timeout` / `command CRC` / `data CRC (clock too high for the
wiring?)` / `retries exceeded` / `protocol error`). A `data CRC` at 32 MHz on a long harness is
the one worth trying `Semmc::set_read_delay` for.

To profile the real map renderer against the card, build `cargo run --release --features
synth,sd-bench`. The image boots directly to Map and emits one `map SD bench:` RTT line per
redraw. `sd-bench` is absent from normal builds and adds no counters to shipping firmware.

> ⚠️ **The bootloader cannot read the card.** `obc-boot` still carries the SPI path and there is
> no room in its 32 KB carve for the 13.6 KB soft-peripheral image, so **SD-staged DFU (install
> and rollback) does not work on this hardware** until the bootloader gets a storage story. It
> fails safely — bring-up reports no card and the old app boots — but it does fail. See
> `obc-boot/src/sd.rs`.

## BLE — board-specific notes

The radio's contract — advertising policy, GATT services and UUIDs, CoC framing, pairing security,
and the `command` / `status` / `config` characteristics — is canonical in
[`obc-ble-interface-spec.md`](../../specs/obc-ble-interface-spec.md). The object surface is
[`FLAT_Store_Protocol.md`](../../specs/FLAT_Store_Protocol.md), implemented by the host-tested
`obc-link` crate (`cargo test -p obc-link`). What is board-specific:

- **DIS identity.** Firmware Revision is the installed OBCU container's version string, read off
  the DFU boot-state page at boot, falling back to `OBC_FW_GIT` — the bare git short hash
  `build.rs` emits — on a probe-flashed board that has never installed a container. So a device
  flashed over SWD reports `ca9b336`, not a version, and no host offers it an auto-update. To see
  a real version on glass, install a wrapped `UPDATE.BIN` (`obc-mkimage`). The same string answers
  the USB EP0 vendor request. Hardware Revision is `nrf54l15-dk`; Serial Number is the 16-hex FICR
  `DEVICEID` whose last four digits are the advertised `OBC-XXXX` name.
- **A FAT card is not a supported runtime store.** Boot rejects it as unformatted and offers the
  recovery link. There is no FAT ride recorder, scan, conversion or recovery.
- **⚠️ Sleep clock: internal RC, not the 32 k crystal.** With `LfclkSource::ExternalXtal` the
  device advertises but **every connection dies at establishment** (HCI 0x3E sync timeout): the
  nRF54L's crystal internal load capacitors are never programmed, so the LFXO runs off-frequency
  and every anchor point is missed. The build runs the LF RC oscillator with MPSL's recommended
  calibration (4 s cadence, ±500 ppm class) and negotiates 2M PHY. Moving back to the crystal
  means writing the `OSCILLATORS` INTCAP registers before MPSL init.
- **`central` is load-bearing.** trouble-host's Controller bound unconditionally requires
  `LeCreateConnCancel`, which only the multirole SDC variant exports; a peripheral-only `nrf-sdc`
  build is a link error. It costs flash only — the builder never enables central roles.
- **Critical section.** MPSL ships its own mandatory `critical-section` impl. Global-interrupt-
  disable critical sections break its radio timing, and two impls are a duplicate-symbol link
  error. It is the only impl in the tree, and the radio is in every build, so a plain
  `cargo build --release` gets the right one.
- **MPSL/SDC hardware.** `board.rs` owns the five production MPSL vectors and 31 MPSL/SDC
  timing/PPI claims. `main.rs` retains CRACEN so store-epoch minting can reborrow it before
  `ble::run` consumes it as the link layer's crypto RNG.
- **RAM.** The map plane and the BLE stack are both in every build and resident together. The
  budget assert in `main.rs` sums `MAP_RESIDENT` and `ble::RESIDENT_BYTES` and fails the build at
  compile time if they overrun the carve. The on-device router drops out on the 256 KB DK — its
  ~14.3 KB of `NAV_*` statics do not fit beside the BLE stack (see `build.rs`); the 512 KB LM20
  relaxes the `has_nav` gate.
- **Config and bond persist in the RRAM SETTINGS carve** (`settings.rs`), which survives power
  cycles **and a reflash**, since it sits above the app image: the device name @0, the boot counter
  @2048, and the single 64-byte CRC-checked bond slot (LTK + peer IRK) @`BOND_OFFSET`. Pairing is
  LESC passkey display. There is one bond slot, and while it is occupied the device **rejects**
  every new pairing attempt; the hold-guarded **Forget phone** in Settings ▸ Bluetooth is the only
  device-side clear, so physical possession guards it.

### Stack discipline: keep big values out of long-lived async bodies

Every sizeable value constructed inline in an async fn or block gets a construction-temporary slot
in the generated poll function's **stack frame**, allocated at entry on *every* poll. `ble::run`
once carried a 30.5 KB poll frame this way, and SMP's synchronous software-P256 pairing chain
overflowed the region into `defmt_rtt::BUFFER` — a HardFault with a corrupted backtrace on every
pairing attempt.

The discipline: big objects live in `.bss` statics built by dedicated `#[inline(never)]` init
functions, and the async body holds only `&'static` handles. Three nets catch a regression — the
compile-time budget assert, the CI poll-frame guard on the release ELF (largest `sub sp` in any
`TaskStorage<F>::poll` ≤ 12 KB), and **MSPLIM**, armed first thing in `main`, which turns any
residual overflow into a precise fault instead of silent `.bss` corruption. To check a frame by
hand, disassemble the release ELF (`cargo objdump --release -- -d --demangle`) and read the
`sub sp` at each `TaskStorage<F>::poll` entry.

### Verify on glass

nRF Connect is the pre-app oracle; the iOS app covers the rest.

- **nRF Connect** — the service and characteristic table matches the spec, DIS strings and serial
  are real, BAS notifies, `protocolVersion` reads two bytes `4`, `psm` reads `0x0080`, negotiated
  MTU is 247 on 2M PHY, and the interval settles to the idle set. Disconnect, reconnect and walking
  out of range bump the counters and always return to advertising. As an unbonded stranger:
  DIS and `protocolVersion` readable, access denied on every gated characteristic and the CoC.
- **End-to-end** — share a GPX to the iOS app, upload, then reflash: the route is in the device
  menu and rideable. Record 2–3 rides (`synth` is fine indoors), sync them, and spot-check a
  decoded ride's totals against the device's Paused ledger. Ids must survive a power cycle and the
  boot counter must increment.
- **Single-file map** — put a packed `.obcm` on the card, boot the default image, and ride a route
  end to end at fine zoom and then zoomed out. Roads, route ink, rider marker and guidance stay
  continuous. Walk the ordinary route-load → ride → finish/save path with `STKOF`/HardFault in
  view and measure the deep path here.
- **Pairing** — passkey card on the panel typed on the phone, bond lands; power-cycle, app restart
  and walk-away all reconnect silently, and so does a reflash. A second phone is rejected while
  bonded. Re-pair path: device **Settings ▸ Bluetooth ▸ Forget phone** (hold), app *Forget*, and
  iOS Bluetooth forget.

## Protocol v4 on both links

The object surface is [`FLAT_Store_Protocol.md`](../../specs/FLAT_Store_Protocol.md) §5 on both
the radio (§5.1) and the cable (§5.2). Over the cable a device serves **object transfer and
device information, and nothing else**; route retention, ride acknowledgement, clock setting,
bond forgetting and the settings blob are BLE-only.

The engine lives in the flat store's `storage_task`, not in either transport: the store seam is
synchronous and the card has exactly one writing execution context. The transports are record
shippers and are the same code below the records, so the two links cannot drift into two answers
to the same question. **The storage task is spawned on every card**, which is what makes these
one code path rather than a branch:

| Card in the slot | What a v4 client gets |
| :-- | :-- |
| **A flat store** | Full service: `LIST`, `GET`, `PUT`, `REMOVE`, `CANCEL`. `ARM` answers `rejected` — this build has no update policy wired. |
| **A FAT card** | Every opcode answers `readOnly` with detail `unformatted 3`. §5.6 step 1 classifies a FAT card as not a flat store, and §3.9 specifies exactly this answer, including for reads, because there is nothing to read. |
| **A broken flat store** | Boot shows STORAGE FAULT before any plane comes up. |

## The USB device plane

Every build ships a second transport for the same object protocol: the LM20's USBHS behind one
vendor-specific interface, so the web builder (WebUSB, Chromium) or the desktop app can push a
map, route or firmware image to a plugged-in device. It is part of the device, not an option of
it. What is board-specific:

- **Zero GPIO cost.** D+/D−/VBUS/TXRTUNE are dedicated USBHS pins; nothing in the pin map moves.
  The driver ships in embassy-nrf 0.11 (`src/usb/usbhs.rs`, plus `vbus_detect.rs`) and needs
  AHB ≥ 30 MHz — the board runs 128.
- **Plug into J3, not J4.** J4 is the on-board debugger, which `cargo run` and RTT use. **J3 is
  the USB connector wired to the SoC.** You want both cables. A production board with a routed
  USB port is not a prerequisite for bring-up.
- **VBUS gates everything, and J3 may be empty.** Riding with no cable is the common case, so the
  plane arms VBUS detection, parks, and builds the driver when a cable arrives — then parks again
  on unplug and returns on re-plug, any number of times. RTT says so each way. Before the gate, a
  cable-less boot faulted the bus partway through start-up and took the debug port with it;
  probe-rs reported `DAP FAULT (sticky_err, sticky_orun)`, never a panic.
- **The parked plane costs nothing.** It waits on the VREGUSB interrupt, not a timer, so a ride
  with J3 empty produces zero USB wake-ups. `wait_for_vbus` registers on `VBUS_WAKER` *before*
  reading the level, and the level is a level rather than a latched edge, so a wake cannot be lost.
  If plug-in is ever not immediate, that is a broken interrupt path, not a latency to paper over.
- **Two vectors, no clashes:** `USBHS` and `VREGUSB`. MPSL takes `RADIO_0` / `TIMER10` / `GRTC_3`
  / `CLOCK_POWER` / `SWI00`, and the high-priority input executor is on `SWI01`. `VREGUSB` carries
  two handlers — ours (wake the park) and embassy's (clear the events) — bound in one
  `bind_interrupts!` arm. Ours reads and clears nothing, so order does not matter; dropping
  embassy's would leave the events uncleared and storm.
- **Endpoint layout.** One interface, class `0xFF` with `bInterfaceProtocol = 5`, four bulk
  endpoints at the high-speed-mandated 512 B: `0x81/0x01` for §3 control records, `0x82/0x02` for
  §3.8 stream records. Both pairs carry the same framing — a `record_length u32`, exactly that
  many frame bytes, then zero padding to four-byte alignment. A packet boundary means nothing to
  the protocol. The ceilings are constants of the binding (§5.2): 8,208 B device→host on either
  channel and host→device on the stream channel, and 256 B for a host→device control record.
  Device identity and the firmware/hardware/serial strings are one EP0 vendor request
  (`bmRequestType 0xC1`, `bRequest 0x20`, §5.2.1), readable once the interface is claimed.
- **The bulk OUT endpoint is armed in bursts**, which is why `embassy-usb-synopsys-otg` is
  vendored under `vendor/`. Stock, it arms one packet and re-arms only after the firmware task has
  copied it out, so the endpoint NAKs for a whole scheduler round trip per 512 B. The dial is
  `BULK_OUT_BURST_PACKETS` in `src/usb/mod.rs`; the sweep recipe is on the constant, and both RAM
  baselines move with it.
- **Uploads use DMA at both ends.** The vendored OTG driver runs in buffer-DMA mode. For a map
  `PUT`, eight 8 KiB v4 records fill one of two 64 KiB scratch-arena banks; the flat store then
  starts one 128-block deferred card DMA while USB reception and CRC folding fill the other bank.
  The storage task borrows only the inactive bank, and every completion, refusal, cancellation or
  unplug joins the card DMA before releasing the arena grant. The two banks cost no additional
  resident RAM.
- **A whole-payload CRC-32 is checked before every commit, maps included.** The device verifies
  the declared length and CRC over the whole payload, runs the kind's validator, and only then
  commits, so a mismatch is `checksumFailure` and nothing is published. The board enables
  `obc-crc`'s slicing-by-8 implementation and overlaps the fold with card DMA.
- **Windows needs no driver install.** MS OS 2.0 BOS descriptors declare the `WINUSB` compatible
  id and a stable `DeviceInterfaceGUIDs` property, so Windows auto-binds WinUSB.
- **VID/PID `1209:0001`** is pid.codes' prototype pair — deliberately the id that means "not
  allocated yet". Allocating a real PID is an owner action; two constants change when it lands.

### The RTT watch-list

| RTT line | What it means |
| :-- | :-- |
| `ep_out buffer overflow index=2` (driver) | The burst reader armed with bytes still staged. Structurally impossible — `arm_transfer` asserts against it — so a panic is the expected form. Treat any occurrence as possible silent loss. |
| the endpoint goes dead after exactly one burst | The core cleared `EPENA` on transfer completion and the stock `DOEPTSIZ`+`CNAK` re-arm is not enough on this part. Nothing else looks like this. |
| `usb: [rec] record length N is outside this channel's ceiling C` | A host framing bug or a desynchronised record stream: the length prefix is being read where payload is. The reader ends that record stream rather than guessing. |
| `usb: [v4] a stream record arrived unadmitted …` | §3.6's admission race lost: the `PUT`'s first stream record beat its control record by more than the 250 ms hold. One per transfer at boot-time contention is survivable; every transfer means the control pump is being starved. |
| `usb: [v4] no upload staging arm granted …` | The transfer could not claim the scratch arena within one second. Still correct, but falls back to short synchronous writes and is visibly slower. Recovery boot pre-grants the idle arena, so this must not appear on the first upload after format. |
| `usb: [v4] staged … kB/s, full CRC + card DMA` | End-to-end device staging rate for this upload — the device-side acceptance measurement. Host timing remains useful for separating browser overhead. |
| `usb: [v4] the store's write half is not armed` | The plane came up without an engine. Object service is down for this boot; it is a storage-task failure, not a USB one. |

Two cases are worth constructing deliberately, because natural traffic almost never produces
them: a record whose length is an exact multiple of 512 (no short packet anywhere, so the final
burst stays armed across the gap to the next record), and an unplug in the middle of a burst.

### Bring-up recipe

Steps 1–2 are the cable-less boot, and they matter most: that is how the device is used.
Steps 3–6 are the ordinary transfer path; step 7 is recovery.

1. With **J3 empty**, flash `cargo run --release` over **J4**. If it fails, use
   `obc board doctor` before another attempt.
2. **The board must reach the ride loop and stay there.** RTT shows `usb: no VBUS on J3 — device
   plane parked …`, then the map renders and the session keeps logging. No `DAP FAULT`, no reset
   loop.
3. Plug **J3** into the host. RTT: `usb: VBUS present …` then `usb: device plane up — 1209:0001,
   serial '…', HS bulk 512 B`. **Watch the clock here.** The timestamp should be within a few
   milliseconds of the connector seating, because a VREGUSB interrupt woke the task. If it lands
   seconds later, or not at all, the wake is not arriving — there is no fallback timer, so this is
   a hard failure rather than a slow success. `system_profiler SPUSBDataType` (or `lsusb -v -d
   1209:0001`) should show `OpenBikeComputer`, the FICR serial as `iSerialNumber`, **Speed: Up to
   480 Mb/s**, one vendor-specific interface and four bulk endpoints.
4. In Chromium, `chrome://device-log` shows the enumeration; the web builder's connect button
   opens the chooser, the EP0 device-info read answers, and the first `LIST` comes back. RTT shows
   `usb: [v4] endpoints enabled — control 4112 B, stream 4112 B` once the host claims the
   interface. There is no version handshake to watch: the major was settled by
   `bInterfaceProtocol` before the chooser opened.
5. **Unplug J3 mid-ride.** RTT: `usb: VBUS removed …`, and the ride loop carries on untouched.
6. **Plug it back in.** `usb: VBUS back …`, the host re-enumerates, and a transfer works again —
   within milliseconds. Repeat a few times: the cable cycle is a loop, not a one-shot, and this is
   where a lost VREGUSB edge would show up.
7. **Recovery path:** boot once with an unreadable or absent map. Keep the fault screen visible,
   connect the builder, and `PUT` a valid map; USB must enumerate and run at the ordinary map rate
   even though the ride loop and BLE were never spawned. Restart and confirm the normal
   application starts, which proves the replacement parsed and mounted rather than merely
   committed.

**Known failure modes.** No `usb:` line at all → the task never started; it is spawned
unconditionally, so check for a panic before the spawn. `usb: no VBUS …` while a cable *is* in J3
→ VBUS detection, not the plane: check J3 really is the SoC connector on your DK revision.
`DAP FAULT (sticky_err, sticky_orun)` that loses the target → a USBHS core access that escaped the
VBUS gate; look for a new register read, not a new buffer. A plug that is never noticed → the
park's VREGUSB wake is not arriving; check that both handlers are still on the `VREGUSB` arm of
`bind_interrupts!`. Enumeration at 12 Mb/s instead of 480 → the PHY fell back to full speed and
the 512 B bulk descriptors are then illegal. Transfers that hang → look for
`a stream record arrived unadmitted`, or a `busy` error response, since the one-transfer rule is
shared across links. `Code 43` on Windows means the MS OS 2.0 descriptor set was rejected.

## Driving it from a host (`debug-uart`)

With the `debug-uart` firmware flashed and VCOM HWFC off, open the desktop feeder from the
repository root. A GPX path is optional:

```sh
obc flash debug-uart
obc uart                              # or: obc uart path/to/ride.gpx
obc debug                             # flash, then open the feeder
```

The feeder provides a baro/compass slider and a button row that injects the four buttons, and
shows the device's render-stats telemetry coming back. It is the same `obc-platform::debug_link`
wire protocol the simulator uses; only the transport differs. `--list` enumerates serial ports;
the VCOM is the J-Link CDC port. The J3 cable is not involved and should be unplugged when the
test also needs BLE.

### Triggering a firmware update over the VCOM

With a **signed** `UPDATE.BIN` in the card root (see [`../README.md`](../README.md); an unsigned
container is refused since OBCU v2), the same link carries the DFU armer's trigger. Two gotchas,
both of which bit for real: the J-Link exposes **two** CDC ports and only one is live — on macOS
the `cu.usbmodem*133` one, since `*131` silently swallows writes, and use `cu.*`, never `tty.*` —
and plain `stty` + `printf`/`cat` does not work, because macOS resets the termios on every
open/close. Use pyserial:

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

If nothing comes back and RTT shows no `dfu:` line either, the J-Link CDC path is in its known
injection wedge: writes vanish while RTT keeps flowing, and only a physical DK power-cycle clears
it.

The device streams one `D` line per phase — scan result, rollback snapshot, `armed gen=N` — then
resets into `obc-boot`, which installs the image (LED codes in
[`../obc-boot/README.md`](../obc-boot/README.md)). Errors come back the same way and the device
keeps running. The trigger is refused mid-recording. Formats are in `OBCU_Spec.md`.

### Driving the ride-recovery acceptance over the VCOM

Four `debug-uart`-only word-tag commands, sent over the same pyserial link. They let the
damaged-`RECORDING` recovery path run on a real card without direct edits to the card contents.
All four are behind `#[cfg(feature = "debug-uart")]` and are absent from the release image.

| Command | What it does |
| :-- | :-- |
| `ride-damage payload` | Opens a `RECORDING` object through the production seam and journals a **7-byte** append. On the next mount the bytes are not a ride-v3 sample/footer boundary, so recovery classifies `payload` damage. |
| `ride-damage metadata` | The same, but journals a **20-byte** zeroed sample and an all-zero resume image. The sample boundary stays valid, so the refusal comes from the continuation metadata. |
| `ride-repair-fail` | Arms a one-shot: the next exact removal answers as a media failure would, **without issuing the commit**. The card is never touched. |
| `store-census` | Prints one RTT line per catalog entry, then the entry count, listing completeness and free extents — the before/after proof that a repair moved exactly one object. |

`ride-damage` deliberately does not self-reset, because the reset would race the confirmation line
off the UART. Issue `probe-rs reset` after you see
`flat ride: fabricated a damaged RECORDING <id> (<kind>)`.

There is no `ride-damage catalog`. That cause needs a real catalog read failure, which cannot be
fabricated on a working card without risking it; it is pinned host-side instead.

## Peak View on the selected map

Peak View is part of the normal firmware. The menu entry appears when the selected map contains
current indexed terrain. The selected map is the lowest-ID Map object; replace that object when
testing a new regional map. The screen's behaviour and controls are in
[the simulator README](../../apps/obc-sim/README.md), and its terrain data in
[the terrain page](https://openbikecomputer.com/software/terrain/).

Board-specific notes:

- The receiver attempts acquisition at boot for up to 150 seconds even when no ride is recording.
  After that window GPS power follows recording, and opening Peak View does not itself wake a
  sleeping receiver.
- Generation runs in the existing 128 KiB scratch arena, so Back must release it before
  navigation, map rendering or USB can use it.
- RTT reports panorama generation time and time to the first partial frame.

For an indoor test, build from this directory and inject GPS/compass data through VCOM:

```sh
cargo run --release --features debug-uart
```

The optional `peak-view-demo` feature seeds a cached Kleine Scheidegg position and opens the
screen at boot. It still reads only the selected map. To test an additional map object without
replacing the lowest-ID map, set an explicit build-time object ID:

```sh
OBC_TEST_MAP_OBJECT_ID=201 cargo run --release --features debug-uart,peak-view-demo
```

Use the ID returned by the map upload. The override is ignored without `peak-view-demo`, and an
invalid or absent object fails to open rather than falling back to another map. The screen waits
for a fresh fix, so send an `F` fix through the debug link to start generation; the cached demo
position does not satisfy GPS acquisition.

The normal `obc-bake bake` terrain stage writes the surface index. Re-bake published terrain and
geometry cells with the current baker before publication; catalog generation rejects mixed native
and indexed terrain. Standalone `obc-dem bake` keeps native terrain, so convert its output
explicitly when needed:

```sh
target/release/obc-dem surface native.obcd indexed.obcd
```

## Landmark photo demo

A display-only demo showing Aare Gorge, Reichenbach Falls and Dunlough Castle. It shares the
simulator's accepted 216 × 240 ordered-dither RGB222 assets and uses the production framebuffer
and FLPR presenter. It does not initialize or write the SD card, and GPS, navigation and BLE are
not part of it. It replaces the application image until normal firmware is flashed again.

```sh
cargo run --release --bin display_test --features landmark-photo-demo
```

Up or Down changes the place. Select cycles through photo credit, source URL, licence URL and
photo; Back returns to the photo. The pixels are embedded in firmware flash, so this is not an
SD-card loading test. The COM task continues while the image is stationary. To restore the normal
application, stop the demo's RTT session and run `cargo run --release --bin obc-fw-nrf54l`.

See the [photo study and attribution](../../docs/assets/ride-assistant/landmark-photos/README.md).

### Map-backed landmark photo acceptance

Hardware acceptance is pending. Use the normal application with the selected landmark's packed
OBCM photo, and record the firmware commit, map hash, QID, map revision, decode time and final
frame hash with the result.

- Open a photo from the installed map. Compare the image rectangle at `(12, 40)`, size
  `216 × 240`, with a fresh simulator capture from that map.
- Open a drawer during loading. Check that no image pixels overwrite the drawer, then enter its
  Brightness page to force a base redraw. Close the drawer and check the complete image returns.
- Open Sources and return to the photo. Check the image is reconstructed without old page pixels.
- Leave the photo during loading, then open another landmark. Check no pixels from the first
  photo appear in the second.
- Replace or remove the selected map. Check the previous image disappears and the source is shown
  as unavailable.
- Repeat after a map render or route search has used the shared arena. Check the image is
  complete, the controls respond and the COM task continues.
