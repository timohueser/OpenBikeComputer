# obc-fw-nrf54l — nRF54LM20-DK firmware

`obc-app` runs on the nRF54LM20-DK. Maps, routes and rides use microSD.
The FLPR drives the LS021B7DD02 memory LCD. Check pins against
[`src/board.rs`](src/board.rs) and constructors against [`src/main.rs`](src/main.rs).
See the [display protocol](https://openbikecomputer.com/hardware/display-protocol/).

## One-time board configuration

In nRF Connect Board Configurator, set these values. Click **Write config** after
each change; blue dots mean unwritten. Settings persist across power cycles.

1. **VDD / VDDM → 3.3 V.** The default 1.8 V is too low for panel logic.
   Feed panel `Vin` from 5 V / VBUS for its 3.3 V LDO.
2. **External memory → OFF.** This frees P2.00–P2.05 from QSPI for native SD.
3. **VCOM hardware flow control → OFF.** `debug-uart` never asserts RTS.
   With HWFC on, telemetry works but injected fixes and buttons do not.

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
| P1.06 | unused PWM pad | held low; do not wire |
| P1.07 | unused PWM pad | held low; do not wire |
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

**Ports P0 and P3:**

| Pin   | Signal  | Notes |
|-------|---------|-------|
| P0.05 | BTN3    | SELECT; low-power domain |
| P3.00 | piezo A | **PROVISIONAL** — PWM21 ch0; header P5 |
| P3.01 | piezo B | **PROVISIONAL** — PWM21 ch1; header P5; wire a passive piezo between A and B, not to ground |

**Schematic-time: the backlight gate needs an external pull-down.** P1.27 idles low, but only
after `PanelBacklight::new` has run (`src/panel_power.rs`). Before that, and through all of
`obc-boot`, the pin is input, no pull, high impedance, and System OFF stops the PWM wherever the
waveform left the line. A floating MOSFET gate is not a defined lamp state.

## Build and flash

Flash [`obc-boot`](../obc-boot/README.md) first: run `obc flash-boot` from the
checkout root, or `cargo run --release` in `../obc-boot`. The app starts at
`0x8000`, above the bootloader's 32 KB. Without it, the device never boots.
App flashes preserve it.

Builds need an `rv32emc` GNU compiler: `brew install riscv64-elf-gcc`, the apt
package `gcc-riscv64-unknown-elf`, or `RISCV_GCC=<path>`.
Run `cargo run --release` here or `obc flash` from the root.
Select a probe with `PROBE_RS_PROBE=VID:PID:SERIAL`. BLE and USB are enabled.

`obc flash factory-demo-ride` over J4 writes the Grimsel Pass demo as a ride,
dated 1 January of the current production year, with synthetic sensors.
Wait for `factory demo ride: complete`, stop the probe, then run `obc flash`.
The factory image stays idle. Normal firmware never seeds rides.
Existing objects stay. Unformatted cards, active recordings and conflicting names
are refused. Set `OBC_DEMO_RIDE_FILE` to an absolute path for another marked demo.

| Feature | Purpose |
| --- | --- |
| `synth` | Square-loop GPS and altimeter source. |
| `debug-uart` | Host sensors and commands over VCOM; overrides real sensors and `synth`. |
| `com-hw` | TIMER21 → DPPIC20 → GPIOTE20 COM wave; verify on glass and a logic analyzer before use. |
| `sd-bench` | SD counters and `map SD bench:` RTT lines; use with `synth`. |
| `factory-demo-ride` | Write a demo ride, then stay idle. |
| `peak-view-demo` | Seed a Kleine Scheidegg fix and open Peak View. |
| `resource-report` | `.obc_resources` for the resource guard; never flash or package this diagnostic image. |
| `flat-store-reset` | Destructive `flat_store_bench` maintenance mode. |

## Display and microSD

The M33 renders one 240 × 320 RGB222 plane and sends up to 16 dirty-row spans to
the FLPR. [`flpr_scan.c`](src/flpr/flpr_scan.c) owns panel timing.
`build.rs` generates the memory layout, linker script and shared constants.
Never commit a crate-root `memory.x`: it overrides the generated layout.

The pin ledger, scan masks and physical FPC harness must agree. If a gate line
stays dark, check that the DK header exposes it before remapping all three.

The card uses Nordic's native 4-bit sEMMC peripheral in `vendor/semmc/`
(`LicenseRef-Nordic-5-Clause`). [`flpr_mux.rs`](src/flpr_mux.rs) switches the FLPR
between display and storage; they cannot run together.
Wire the six card pins in [the pin map](#full-pin-map), plus GND and 3V3.
There is no chip select. Use the pull-ups specified in the pad table.

Writes cap at 21.3 MHz; high-speed reads use 32 MHz. Re-test signal integrity on
soldered hardware. RTT reports `SemmcError` and the aborted transfer.
For read CRC errors at 32 MHz on a long harness, try `Semmc::set_read_delay`.

## Sensors

TWIM22 connects these devices. Only magnetometer axes are used; accel and gyro sleep.

| Device | I²C address |
| --- | --- |
| u-blox SAM-M10Q GNSS, DDC | `0x42` |
| Bosch BMP581 | `0x47`, or `0x46` with SDO low |
| TDK ICM-20948 | `0x68` / `0x69`; bypass exposes AK09916 at `0x0C` |

GPS TX-Ready is optional. The SparkFun GPS-21834 breakout omits it;
the task falls back to DDC polling at about 1 Hz.
Wire `V_BCKP` to an always-on rail, supercap or coin cell to preserve RTC and
ephemeris for warm fixes. Acquisition runs for up to 150 s at boot;
then GPS power follows recording.

Indefinite software standby needs receiver power removal; I²C traffic cannot wake it.
The firmware uses controlled stop while idle and START at startup, so its standby
recovers on an MCU reset. See the
[SAM-M10Q integration manual](https://content.u-blox.com/sites/default/files/documents/SAM-M10Q_IntegrationManual_UBX-22020019.pdf), sections 3.3 and 3.5.3.3.

## BLE

See the [BLE contract](../../specs/obc-ble-interface-spec.md) and
[object protocol](../../specs/FLAT_Store_Protocol.md).

- Use the internal LF RC clock with MPSL calibration. `ExternalXtal` connections
  fail with HCI `0x3E` unless the DK's `OSCILLATORS` INTCAP registers are set
  before MPSL starts.
- Keep `nrf-sdc`'s `central` feature for `LeCreateConnCancel`. MPSL must own the
  sole `critical-section` implementation.
- Settings and one bond slot live in RRAM above the image and survive reflashes.
  An occupied slot refuses new pairings. Clear it with **Forget phone** in
  Settings → Connections, or a factory reset.
- DIS reports the installed OBCU version, or the build's git hash. Hosts do not
  offer automatic updates to probe-flashed images that report a hash.
- Build large async values in `.bss` statics with `#[inline(never)]` initializers
  to avoid permanent poll-frame slots. CI and MSPLIM guard their size.

## Native USB

J3 is the SoC USB connector; J4 is the debugger and VCOM. Use both cables.
USBHS needs AHB ≥ 30 MHz; this board uses 128 MHz. VBUS gates core access,
and VREGUSB wakes the task without polling when J3 is empty.
The vendored USB driver arms bulk OUT in bursts. Changing
`BULK_OUT_BURST_PACKETS` in `src/usb/mod.rs` changes RAM use.
Prototype VID/PID is `1209:0001`. MS OS 2.0 descriptors bind WinUSB on Windows.

### Bring-up

1. With J3 empty, flash over J4. RTT must report `usb: no VBUS on J3 …` and
   reach the ride loop without `DAP FAULT` or repeated resets.
2. Connect J3. RTT must promptly report VBUS and `usb: device plane up …`.
   Check `system_profiler SPUSBDataType` or `lsusb -v -d 1209:0001` for
   OpenBikeComputer, the FICR serial, 480 Mb/s and four bulk endpoints.
3. Connect in Chromium's web builder and confirm `LIST` works.
4. Disconnect and reconnect J3 during a ride. Recording and transfers must continue.
5. Boot without a readable map, upload a valid map with `PUT`, then restart.

| Failure | Check |
| --- | --- |
| No `usb:` line | Panic before the unconditional task spawn. |
| No VBUS with J3 connected | Cable, connector and DK revision. |
| `DAP FAULT` | USBHS register access outside the VBUS gate. |
| Plug is not detected | Both VREGUSB interrupt handlers remain bound. |
| Enumeration at 12 Mb/s | Full-speed fallback; 512 B bulk descriptors are invalid. |

## Board connection and recovery

`obc board doctor` reports probes, serial owners and USB enumeration on macOS
and Linux. It resets nothing. All `obc` board commands and standalone Cargo
runners use [`tools/board.py`](../../tools/board.py) and one per-user lock across
worktrees. Stop direct probe-rs or SEGGER sessions first; they bypass the lock.
Use `--probe VID:PID:SERIAL` or `PROBE_RS_PROBE` to select a probe.
Keep `--verify` and `--disable-double-buffering` after upgrades; see the
[probe-rs corruption issue](https://github.com/probe-rs/probe-rs/issues/3775).

`obc rtt [ELF] --log /tmp/obc-rtt.log` attaches without building, programming or
resetting. Use the exact installed ELF, features and `DEFMT_LOG` for decoding.
`cargo rtt` can rebuild but does not program the result.

| Symptom | Recovery |
| --- | --- |
| Probe busy | Use `obc board doctor`; stop the owner with Ctrl-C and wait. Do not kill all probe processes. |
| Flash read-back mismatch | Keep verification and disabled double buffering. Preserve the output and ELF; do not erase all or run an unverified image. |
| RTT decode fails | Match ELF to firmware, close and reattach. An idle device can produce no logs. |
| VCOM commands have no effect | Check `debug-uart`, J4 CDC port, baud, HWFC OFF and other owners. A successful write does not prove delivery. |
| VCOM remains stuck | Physically power-cycle the DK and J-Link bridge. Disconnect both cables. |
| J3 does not enumerate | Check native cable, VID/PID and RTT VBUS lines. J4 serial does not prove native USB works. |
| J3 enumerates but connection fails | Close the desktop or browser interface owner, then reconnect. |
| Restart and capture boot logs | `obc board run ELF --preverify` verifies, resets and streams RTT. It programs only if the installed image differs. `--preverify` applies to `run` and `download`. |

## Host input and firmware update

Flash `debug-uart` and disable VCOM HWFC. From the repository root:

```sh
obc flash debug-uart
obc uart                              # optional: path/to/ride.gpx
obc debug                             # flash and open the feeder
```

The feeder sends buttons and sensor values, and displays telemetry.
`--list` shows serial ports. Disconnect J3 if the test also needs BLE.
On macOS, use `cu.usbmodem*133`, not `*131` or `tty.*`. Use pyserial;
`stty` with `printf` or `cat` loses termios settings across opens.

For a [signed update package](../README.md) in the store, send `dfu-install\n`
to the live CDC port and read the `D` status lines:

```sh
uv venv /tmp/dfu-venv && uv pip install --python /tmp/dfu-venv/bin/python pyserial
/tmp/dfu-venv/bin/python -c "import serial; p = serial.Serial('/dev/cu.usbmodem<...>133', 115200, \
  rtscts=False, timeout=90); p.write(b'dfu-install\n'); [print(p.readline()) for _ in range(40)]"
```

The device resets into `obc-boot` to install it; see its README for LED codes.
Unsigned packages and triggers during recording are refused. If VCOM returns
nothing and RTT has no `dfu:` line, power-cycle the DK to clear the CDC bridge.

## Flat store bench

```sh
cargo run --release --bin flat_store_bench
cargo run --release --bin flat_store_bench --features flat-store-reset
```

**Both commands destroy the partition table and every object from raw LBA 0.**
The bench refuses another `StoreId` unless `FORCE_REINIT` overrides it.
See the [bench module](src/bin/flat_store_bench.rs) for phases and output.

## Peak View

The menu appears when the selected map (lowest-ID Map object) has indexed terrain.
See [simulator controls](../../apps/obc-sim/README.md). Generation uses the shared
128 KiB scratch arena; Back must release it before navigation, rendering or USB.
`obc-bake bake` writes the surface index. Convert standalone native terrain with
`obc-dem surface native.obcd indexed.obcd`.

```sh
# Requires peak-view-demo and a fresh F fix over the debug link.
OBC_TEST_MAP_OBJECT_ID=201 cargo run --release --features debug-uart,peak-view-demo
```
