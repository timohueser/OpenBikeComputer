# obc-fw-nrf54l — nRF54LM20-DK firmware

`obc-app` runs on the nRF54LM20-DK. Maps, routes and rides use microSD. The FLPR coprocessor
drives the LS021B7DD02 memory LCD.

[`src/board.rs`](src/board.rs) is the canonical peripheral and pin ledger and the constructors in
`src/main.rs` are the executable authority; if this README disagrees with them, fix both. The
LS021 protocol is on the
[display-protocol page](https://openbikecomputer.com/hardware/display-protocol/).

## One-time board configuration (nRF Connect Board Configurator)

Three settings, written to the DK's interface MCU, which persist across power cycles. Click
**Write config** after each change (blue dots mean unwritten). No soldering is needed.

1. **VDD / VDDM → 3.3 V.** The 1.8 V default is too low for the LS021's logic. Feed the panel's
   `Vin` from the DK's 5 V / VBUS so its 3.3 V LDO has headroom.
2. **External memory → OFF.** This disconnects the on-board QSPI flash and frees P2.00–P2.05 for
   the microSD card in native 4-bit SD mode. The flash is never used.
3. **VCOM hardware flow control → OFF.** Required for `debug-uart` builds. With HWFC on, the
   interface MCU gates host-to-device bytes on RTS, which this firmware never asserts. The symptom
   is that telemetry comes back but injected fixes and button presses are ignored.

## Full pin map

**Port P2 — MCU/fast domain.** All 11 pins are used: the microSD bus and the panel's source bus,
two of them shared. The card's six pads are fixed by Nordic's sEMMC soft peripheral.

| Pin   | sEMMC | Display | Notes                                                        |
|-------|-------|---------|--------------------------------------------------------------|
| P2.00 | D3    | **B0**  | shared — `CTRLSEL` per mode; internal pull-up in storage mode |
| P2.01 | CLK   | —       | card only; parked as an input in display mode                 |
| P2.02 | D0    | —       | card only; parked as an input in display mode                 |
| P2.03 | D2    | —       | card only; parked as an input in display mode                 |
| P2.04 | D1    | **B1**  | shared — `CTRLSEL` per mode; internal pull-up in storage mode |
| P2.05 | CMD   | —       | card only; parked as an input in display mode                 |
| P2.06 | —     | R0      | source data (even-`x` R)                                      |
| P2.07 | —     | BCK     | source shift clock                                            |
| P2.08 | —     | R1      | source data (odd-`x` R)                                       |
| P2.09 | —     | G0      | source data (even-`x` G)                                      |
| P2.10 | —     | G1      | source data (odd-`x` G)                                       |

The packed wire word is `DATA_MASK = 0x751` (`B0`→0, `B1`→4, `R0`→6, `R1`→8, `G0`→9, `G1`→10).
`even`/`odd` is 0-based `x`, so the `*0` lines carry `x = 0, 2, 4, …`; the panel datasheet numbers
columns from 1 and calls that same line the *odd* column.

**Pad configuration per mode** (`src/semmc.rs`, `configure_storage_pads` / `configure_display_pads`):

| | the six card pads | the four card-only pads |
|---|---|---|
| **storage** | Output, input Disconnect, E0/E1 drive, `CTRLSEL = VPR`, `GPIOHSPADCTRL.BIAS = 2`; internal pull-up on `D3`/`D1` only | same — all six are the card's |
| **display** | `P2.00`/`P2.04` → Output, S drive, no pull, `CTRLSEL = GPIO`, `GPIOHSPADCTRL.BIAS = 2` | Input, no pull, `CTRLSEL = GPIO`; the external pull-ups hold the bus idle-high and the card stays inert |

`GPIOHSPADCTRL.BIAS` is port-global, not per-pin, so both modes set the same constant 2
(`semmc::HS_PAD_BIAS`). Only `D3`/`D1` get an internal pull-up, because this desk breakout carries
its own resistors on the rest. **The production board should fit external 10–100 kΩ pull-ups on
`CMD`/`DAT0–3` (none on `CLK`) and turn all internal pulls off.**

**Port P1 — PERI domain, ≤8 MHz** (gate/BSP, sensors, COM, VCOM and buttons):

| Pin   | Signal       | Notes                                                     |
|-------|--------------|-----------------------------------------------------------|
| P1.03 | I²C SCL      | shared GPS + altimeter + compass bus (TWIM22)             |
| P1.04 | I²C SDA      | same bus                                                   |
| P1.05 | GPS TX-Ready | optional DDC data-ready IRQ (active-high)                  |
| P1.06 | piezo A      | **PROVISIONAL** — PWM21 ch0, the note's frequency          |
| P1.07 | piezo B      | **PROVISIONAL** — PWM21 ch1; opposite phase on Loud, low on Quiet |
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
| P1.27 | backlight    | **PROVISIONAL** — PWM20 ch0, 1 kHz; also DK LED2           |

**Port P0 — low-power domain:** `P0.05` is `BTN3` (SELECT).

**Schematic-time: the backlight gate needs an external pull-down.** P1.27 idles low, but only
after `PanelBacklight::new` has run (`src/panel_power.rs`). Before that, and through all of
`obc-boot`, the pin is input, no pull, high impedance, and System OFF stops the PWM wherever the
waveform left the line. A floating MOSFET gate is not a defined lamp state.

## Build and flash

**First, flash the bootloader once.** The app is linked at `0x8000`; the 32 KB below it belongs to
[`obc-boot`](../obc-boot/README.md) (`cd ../obc-boot && cargo run --release`). It survives every
app reflash. A device without it shows no LED blink and never boots.

Every build compiles the RISC-V blob, so it needs an `rv32emc`-capable GNU gcc: `brew install
riscv64-elf-gcc`, or the apt package `gcc-riscv64-unknown-elf`, or `RISCV_GCC=<path>`.

Run `cargo run --release` here or `obc flash` from the checkout root.
Select a probe with `PROBE_RS_PROBE=VID:PID:SERIAL`. BLE and USB are always enabled.

For finished test rides, run `obc flash seed-rides` over J4. Wait for `demo rides: complete`.
This adds three 30-minute GPS loops: GPS, heart rate, and heart rate with power, dated the
previous three days. The two sensor rides record max HR 185 and FTP 250. Existing names are
skipped. Existing objects remain; unformatted cards and active recordings are refused. Press
Ctrl-C, then run `obc flash` to restore normal firmware. The rides remain on the card.

| Feature | What it does |
| :-- | :-- |
| `synth` | Replaces the real GPS and altimeter with the built-in square-loop source. |
| `debug-uart` | Streams GPS, altimeter and compass from a host over VCOM, and enables the VCOM word-tag commands. Takes precedence over the real sensors and `synth`. |
| `com-hw` | Drives the COM wave from a zero-CPU TIMER21 → DPPIC20 → GPIOTE20 chain instead of `com::com_task`. Off by default until it is verified on glass and a logic analyzer. |
| `sd-bench` | Adds SD read counters and one `map SD bench:` RTT line per map redraw. Use with `synth`. |
| `seed-rides` | Adds finished demo rides. Use `obc flash seed-rides`. |
| `peak-view-demo` | Seeds a Kleine Scheidegg fix and opens Peak View at boot. |
| `resource-report` | Adds the `.obc_resources` table for `firmware/tools/resource_guard.py`. Diagnostic only — never flash or package this image as the shipping artifact. |
| `flat-store-reset` | Destructive maintenance mode for `flat_store_bench` only. |

## The display: LS021 on the FLPR

The M33 renders a full 240 × 320 RGB222 frame into one resident plane, publishes the framebuffer
address and at most 16 dirty-row spans in the shared control page, and rings the FLPR. A full
320-row present takes about 44 ms and a 64-row partial about 9 ms. `src/flpr/flpr_scan.c` owns the
timing policy, including what to re-check on a new panel; `build.rs` generates the M33 `memory.x`,
the FLPR linker script and the shared Rust and C constants from one contract. **Never commit a
`memory.x` in the crate root**: the linker finds it before the generated carve.

The source bus and `BCK` are on P2 (see [the pin map](#full-pin-map)); the gate lines and `BSP`
are `GSP P1.10 / GCK P1.11 / GEN P1.12 / INTB P1.13 / BSP P1.14`, and COM is on P1.22–24. The gate
pins, the masks in `flpr_scan.c` and the physical 21-pin FPC harness must all agree. If a gate
line stays dark on glass, confirm the pin is broken out on your DK header and remap all three.

## microSD over sEMMC

**There is no SPI.** The card runs in native 4-bit SD mode on the same FLPR that drives the panel,
through Nordic's sEMMC soft peripheral — a 13,636 B position-independent RISC-V image (vendored at
`vendor/semmc/`, `LicenseRef-Nordic-5-Clause`) that turns P2.00–05 into an SD host controller.
Display and storage never run at the same instant: `src/flpr_mux.rs` time-multiplexes the one
FLPR, at 29 µs into storage and 138 µs back, lazily, so a run of reads or of frames pays it once.

Six jumpers, no chip select: `P2.00 D3 · P2.01 CLK · P2.02 D0 · P2.03 D2 · P2.04 D1 · P2.05 CMD`,
plus GND and 3V3. The assignment is fixed by the soft peripheral. Pull-ups are in
[the pad table](#full-pin-map).

**Writes cap at 21.3 MHz.** 32 MHz writes fail card-side CRC on the jumper harness — a clean
failure, with nothing programmed — while 32 MHz reads are spotless. The clock is per transaction,
so mixed rates are free. Re-test on soldered hardware.

If the card does not come up, the RTT log names the rung that failed (`SemmcError`) and decodes
the aborted transfer. A `data CRC` at 32 MHz on a long harness is the one worth trying
`Semmc::set_read_delay` for.

## Sensors on the shared I²C bus

TWIM22 carries the u-blox **SAM-M10Q** GNSS (DDC `0x42`), the Bosch **BMP581** altimeter (`0x47`,
or `0x46` if the breakout straps `SDO` low), and an **AK09916** magnetometer inside a TDK
**ICM-20948** (IMU at `0x68`/`0x69`, in I²C bypass so the magnetometer answers at `0x0C`). No
addresses clash. Only the three magnetometer axes are used; accel and gyro stay asleep.

- **GPS TX-Ready is optional.** The SparkFun SAM-M10Q breakout (GPS-21834) does not break it out,
  so the task falls back to DDC polling at the same ~1 Hz cadence.
- **Wire V_BCKP to an always-on rail, supercap or coin cell.** It backs the receiver's RTC and
  ephemeris across a power-off and turns every cold ~30 s fix into a warm fix in seconds.
- The receiver attempts acquisition for up to 150 s at boot; after that, GPS power follows
  recording.
- **An older image can leave the receiver in indefinite software standby**, which I²C traffic
  cannot wake. Remove its power before starting this image. This image sends controlled stop on
  idle and START at startup, so an MCU reset recovers it. See the
  [SAM-M10Q integration manual §3.3 and §3.5.3.3](https://content.u-blox.com/sites/default/files/documents/SAM-M10Q_IntegrationManual_UBX-22020019.pdf).

## BLE — board-specific notes

The radio's contract is canonical in
[`obc-ble-interface-spec.md`](../../specs/obc-ble-interface-spec.md), and the object surface is
[`FLAT_Store_Protocol.md`](../../specs/FLAT_Store_Protocol.md). What is board-specific:

- **⚠️ The sleep clock is the internal RC, not the 32 k crystal.** With `LfclkSource::ExternalXtal`
  the device advertises but every connection dies at establishment (HCI 0x3E sync timeout),
  because the nRF54L's crystal internal load capacitors are never programmed on the DK. The build
  runs the LF RC oscillator with MPSL's calibration. Moving back to the crystal means writing the
  `OSCILLATORS` INTCAP registers before MPSL init.
- **Two link-error traps.** `nrf-sdc`'s `central` feature is load-bearing, because trouble-host's
  Controller bound needs `LeCreateConnCancel`, which only the multirole variant exports. And MPSL
  owns the one `critical-section` impl in the tree; a second one is a duplicate symbol.
- **Config and the bond live in the RRAM SETTINGS carve** (`src/settings.rs`), which survives a
  power cycle and a reflash, because it sits above the app image. There is one bond slot; while it
  is occupied the device rejects every new pairing. The hold-guarded **Forget phone** in
  Settings ▸ Bluetooth is the only device-side clear.
- **DIS Firmware Revision is the installed OBCU container's version**, falling back to the bare
  git short hash `build.rs` emits. A probe-flashed board reports a hash, not a version, so no host
  offers it an auto-update.
- **Keep big values out of long-lived async bodies.** A value built inline in an async fn takes a
  permanent slot in the generated poll frame. Big objects belong in `.bss` statics built by
  `#[inline(never)]` init functions. The CI poll-frame guard and MSPLIM catch a regression.

## The USB device plane

Every build ships a second transport for the same object protocol: the LM20's USBHS behind one
vendor-specific interface, so the web builder (WebUSB, Chromium) or the desktop app can push a
map, route or firmware image to a plugged-in device.

- **Plug into J3, not J4.** J4 is the on-board debugger, which `cargo run` and RTT use; J3 is the
  USB connector wired to the SoC. You want both cables.
- **Zero GPIO cost.** D+/D−/VBUS/TXRTUNE are dedicated USBHS pins. The driver needs AHB ≥ 30 MHz;
  the board runs 128. **VBUS gates everything**: the plane parks on the VREGUSB interrupt, not a
  timer, so a ride with J3 empty produces no USB wake-ups.
- **The bulk OUT endpoint is armed in bursts**, which is why `embassy-usb-synopsys-otg` is
  vendored under `vendor/`. The dial is `BULK_OUT_BURST_PACKETS` in `src/usb/mod.rs`; both RAM
  baselines move with it.
- **VID/PID `1209:0001`** is pid.codes' prototype pair. Allocating a real PID is an owner action.
  Windows needs no driver install: MS OS 2.0 BOS descriptors bind WinUSB automatically.

### Bring-up recipe

1. With **J3 empty**, flash `cargo run --release` over **J4**. The board must reach the ride loop
   and stay there: RTT shows `usb: no VBUS on J3 …`, then the map renders, with no `DAP FAULT` and
   no reset loop. This cable-less boot matters most, because it is how the device is used.
2. Plug **J3** into the host. RTT: `usb: VBUS present …`, then `usb: device plane up …`, within a
   few milliseconds of the connector seating. There is no fallback timer, so a late line is a hard
   failure. `system_profiler SPUSBDataType` (or `lsusb -v -d 1209:0001`) should show
   `OpenBikeComputer`, the FICR serial, **Speed: Up to 480 Mb/s** and four bulk endpoints.
3. In Chromium, the web builder's connect button opens the chooser and the first `LIST` returns.
4. **Unplug J3 mid-ride, then plug it back in**, a few times. The ride loop must carry on
   untouched and transfers must work again. A lost VREGUSB edge shows up here.
5. **Recovery path:** boot with an unreadable or absent map, connect the builder, and `PUT` a
   valid map. Restart and confirm the normal application starts.

### Failure signatures

| What you see | What it means |
| :-- | :-- |
| no `usb:` line at all | The task never started. It is spawned unconditionally, so look for a panic before the spawn. |
| `usb: no VBUS …` with a cable in J3 | VBUS detection, not the plane. Check J3 really is the SoC connector on your DK revision. |
| `DAP FAULT (sticky_err, sticky_orun)` losing the target | A USBHS core access escaped the VBUS gate. Look for a new register read, not a new buffer. |
| a plug that is never noticed | The park's VREGUSB wake is not arriving. Both handlers must stay on the `VREGUSB` arm of `bind_interrupts!`. |
| enumeration at 12 Mb/s | The PHY fell back to full speed, which makes the 512 B bulk descriptors illegal. |

## Board connection and recovery

Run `obc board doctor` before reflashing a board that appears unresponsive. It reports probes,
serial ports and their owners, and native USB enumeration, and it resets nothing. macOS and Linux.

All `obc` flash/debug/RTT commands and both standalone Cargo runners use
[`tools/board.py`](../../tools/board.py), which selects `nRF54LM20A` and holds one per-user lock
across worktrees. Direct probe-rs and SEGGER commands do not share that lock, so stop those
sessions first. One board session at a time; select a probe with `PROBE_RS_PROBE=VID:PID:SERIAL`
or `--probe VID:PID:SERIAL`. The runner keeps `--verify` on and passes
`--disable-double-buffering`, the workaround
[probe-rs #3775](https://github.com/probe-rs/probe-rs/issues/3775) reports for nRF54LM20A/J-Link
corruption; keep it after a probe-rs upgrade.

`obc rtt [ELF] --log /tmp/obc-rtt.log` attaches without compiling, programming or resetting. **The
ELF must be the exact one installed, built with the same features and `DEFMT_LOG`**, or the decode
is garbage. `cargo rtt` can rebuild before attaching but never programs the device.

| Symptom | Check and recovery |
| --- | --- |
| Probe busy / exclusive-access error | Run `obc board doctor`. Stop the owning RTT, debugger or programmer session with Ctrl-C, wait for it to exit, then retry. Do not kill all probe processes. |
| Flash read-back mismatch | Keep verification enabled and double buffering disabled. Keep the failing output and ELF. Do not erase all or run an unverified image. |
| RTT stops or cannot decode | Confirm the ELF and firmware version match, then close and reattach. An idle device may legitimately produce no logs. |
| VCOM writes succeed but commands have no effect | J4 carries VCOM. Confirm the `debug-uart` image, the correct CDC port, the baud and HWFC OFF. Close any other serial owner. A write return value does not prove delivery. |
| VCOM stays unresponsive after those checks | Power-cycle the DK. A target reset does not reset the J-Link bridge. With both cables connected, unplugging one may leave the board powered. |
| J3 absent from host USB enumeration | J3 is the separate native device cable, VID:PID `1209:0001`. Check the RTT VBUS lines, then reconnect J3. J4 serial ports do not prove J3 is working. |
| J3 enumerates but the application cannot connect | Close the desktop or browser session that owns the interface, then reconnect J3. |
| You need to restart a board and read its boot log | `obc board run ELF --preverify` reads the image back, programs nothing when the board already holds it, then resets and streams RTT. One session does both. An ELF that differs is programmed as usual. `--preverify` is for `run` and `download` only. |

## Driving it from a host (`debug-uart`)

With the `debug-uart` firmware flashed and VCOM HWFC off, open the desktop feeder from the
repository root. A GPX path is optional.

```sh
obc flash debug-uart
obc uart                              # or: obc uart path/to/ride.gpx
obc debug                             # flash, then open the feeder
```

The feeder injects the four buttons and a baro/compass slider, and shows the device's telemetry
coming back. `--list` enumerates serial ports. Unplug J3 when the test also needs BLE.

**Two VCOM gotchas.** The J-Link exposes two CDC ports and only one is live: on macOS the
`cu.usbmodem*133` one, because `*131` silently swallows writes. Use `cu.*`, never `tty.*`. And
`stty` with `printf`/`cat` does not work, because macOS resets the termios on every open and
close — use pyserial.

**Firmware update over the VCOM.** With a **signed** update package in the store (see
[`../README.md`](../README.md); an unsigned container is refused), the same link carries the armer's
trigger. Send `dfu-install\n` over the
live CDC port with pyserial and read the `D` status lines back:

```sh
uv venv /tmp/dfu-venv && uv pip install --python /tmp/dfu-venv/bin/python pyserial
/tmp/dfu-venv/bin/python -c "import serial; p = serial.Serial('/dev/cu.usbmodem<...>133', 115200, \
  rtscts=False, timeout=90); p.write(b'dfu-install\n'); [print(p.readline()) for _ in range(40)]"
```

The device streams one `D` line per phase, then resets into `obc-boot`, which installs the image
(LED codes in [`../obc-boot/README.md`](../obc-boot/README.md)). The trigger is refused during a
recording. If nothing comes back and RTT shows no `dfu:` line either, the J-Link CDC path is in
its known injection wedge: writes vanish while RTT keeps flowing, and only a physical DK power
cycle clears it.

## The flat store bench

```sh
cargo run --release --bin flat_store_bench          # measure
cargo run --release --bin flat_store_bench --features flat-store-reset   # erase, then park
```

The bench measures every figure `specs/FLAT_Store_Format.md` states. Its module docs describe the
phases and the figures they report.

> ⚠️ **DESTRUCTIVE.** The flat store owns the raw card from LBA 0, so either command destroys the
> partition table and every object on the card, not only benchmark routes. The bench refuses a
> card that carries a flat store under another `StoreId`; `FORCE_REINIT` overrides that.

## Peak View

Peak View is part of the normal firmware; the menu entry appears when the selected map — the
lowest-ID Map object — holds current indexed terrain. Behaviour and controls are in
[the simulator README](../../apps/obc-sim/README.md). Generation runs in the shared 128 KiB
scratch arena, so Back must release it before navigation, map rendering or USB can use it.
`obc-bake bake` writes the surface index; standalone `obc-dem bake` keeps native terrain, so
convert it with `obc-dem surface native.obcd indexed.obcd`.

```sh
# Open an extra map object instead of the lowest-ID one. Ignored without peak-view-demo, and the
# screen still waits for a fresh fix, so send an `F` fix over the debug link.
OBC_TEST_MAP_OBJECT_ID=201 cargo run --release --features debug-uart,peak-view-demo
```
