# obc-fw-nrf54l — nRF54L15-DK firmware

The **real hardware target** for OpenBikeComputer: the shared `obc-app` running on
an nRF54L15-DK (Cortex-M33), with map/routes/tracks streamed from a microSD card —
load a route, ride it (fake-sensor fed), and save a GPX. The default firmware
drives an Adafruit **ST7789** EYESPI panel (240×320) as the bring-up display; the
final panel is the reflective **LS021B7DD02** memory LCD, whose driver is brought
up in the separate `ls021-*` binaries below (protocol on the [display-protocol
docs page](https://timohueser.github.io/OpenBikeComputer/hardware/display-protocol/)).

See the module doc in [`src/main.rs`](src/main.rs) for the full peripheral/pin
plan; this README is the **board setup + build/flash** guide.

## One-time board configuration (nRF Connect **Board Configurator**)

These three settings are applied with the **Board Configurator** app (in *nRF Connect
for Desktop*), are written to the DK's interface MCU, and **persist across power
cycles** — do them once. After changing anything, click **Write config** (blue dots =
unwritten). No soldering / solder-bridge cuts are needed on current board revisions.

1. **VDD / VDDM → 3.3 V.** The default is 1.8 V, which is too low for the ST7789
   breakout's level shifters. (Also feed the panel's `Vin` from the DK's 5 V / VBUS so
   its on-board 3.3 V LDO has headroom.)
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

Full detail is in the `src/main.rs` module doc. In short: the **ST7789** sits on the
flash-freed **P2** header (SCK P2_01 / MOSI P2_02 / CS P2_05 / DC P2_03 / RST P2_00,
Vin←5 V, logic at 3.3 V); a **microSD** breakout on the **P1** header (SCK P1_11 /
MISO P1_07 / MOSI P1_06 / CS P1_12, with a pull-up on MISO); the four DK buttons and
LED0 are on-board. The J-Link **VCOM** (P1_04/P1_05) and RTT both ride the DK's USB.

## Build & flash

From this crate directory (it's a standalone crate built for `thumbv8m.main-none-eabihf`;
`cargo run` flashes + streams defmt/RTT over the on-board J-Link via probe-rs):

```sh
# Default: full map + ride loop, GPS faked by the on-board SynthLocation square loop
# (no host needed).
cargo run --release

# With the VCOM debug-sensor feed (issue #127) — needs HWFC OFF (above):
cargo run --release --features debug-uart

# Panel-only bring-up demo (font ladder + 64-colour gamut, no SD, no map):
cargo run --release --no-default-features --features glass-demo

# LS021B7DD02 panel bring-up bench (epic #139): M33-direct bit-bang driver —
# power-on init, solid colours, the 64-colour palette, and shapes on real glass.
# Protocol: the display-protocol docs page; bench notes: firmware/docs/ls021-bringup.md.
cargo run --release --bin ls021_bringup --features ls021-bringup

# LS021 FLPR backend bring-up (epic #149): moving the waveform generation onto the
# nRF54L FLPR (VPR RISC-V coprocessor). Needs a RISC-V gcc for the C blob (below).
# Notes: firmware/docs/ls021-flpr.md.
cargo run --release --bin ls021_flpr_bringup --features ls021-flpr

# The **real map/ride app on the reflective LS021 panel** via the FLPR (issue #165) — the
# same load→ride→save-GPX firmware as the default, but on the LS021 instead of the ST7789.
# Also needs the RISC-V gcc (it builds the FLPR blob). See firmware/docs/ls021-flpr.md.
cargo run --release --features panel-ls021,debug-uart
```

### LS021 FLPR builds — DK wiring (issue #165)

The `panel-ls021` app (and the `ls021_*` bring-up bins) drive the panel itself, not the ST7789.
The source bus + `BCK` + COM stay on **P2** (P2.00–06 data/clock, P2.07/08/10 COM, P2.09 heartbeat
LED); the four gate lines + `BSP` sit on **free P1 pins** — `GSP P1.00 / GCK P1.01 / GEN P1.12 /
INTB P1.10 / BSP P1.14` — deliberately **off** the SD-SPI bus (P1.06/07/11/12) and VCOM (P1.04/05)
the app needs.

The DK breaks out only **P1.00–14** (P1.02/03 are NFC), which is one pin short for everything the app
puts on P1 — so in the **`panel-ls021` build only**, SD **`CS` moves from P1.12 to P0.00** (one jumper
on the SD breakout; it's a plain GPIO, and the M33 already drives P0 for BTN3). That frees P1.12 for
`GEN`. The SD bus pins (SCK P1.11 / MISO P1.07 / MOSI P1.06) are unchanged; the default ST7789 build
keeps `CS` on P1.12.

The five gate/`BSP` DK pins, the masks in `src/flpr/flpr_pingpong.c`, and the physical 21-pin FPC
harness must all agree; if a gate line stays dark on glass, confirm the pin is broken out on your DK
header and remap all three together. Full pin/protocol detail:
[firmware/docs/ls021-flpr.md](../docs/ls021-flpr.md).

### FLPR toolchain (only for the `ls021-flpr` build)

The FLPR bring-up cross-compiles a tiny freestanding C blob for the RISC-V coprocessor, so it
needs an `rv32emc`-capable GNU gcc — install once:

```sh
brew install riscv64-elf-gcc        # or set RISCV_GCC=<path> to an xPack / Zephyr-SDK toolchain
```

It's only needed for `--features ls021-flpr`; every other build (the default map firmware, the
`glass-demo` and `ls021-bringup` bins) needs no RISC-V toolchain.

If `cargo run` prompts to pick a probe (e.g. another ST-LINK is attached), pass
`--probe <vid:pid:serial>` for the J-Link.

## Driving it from a host (`debug-uart`)

With the `debug-uart` firmware flashed and VCOM HWFC off, replay a recorded ride over
the VCOM from the desktop feeder (from the `firmware/` workspace dir):

```sh
cargo run -p obc-usb-host -- --gpx ../kandel.gpx        # add --port <VCOM tty> if not auto-detected
```

`obc-usb-host` streams the `.gpx` as fake GPS fixes (plus a baro/compass slider and an
on-screen button row that injects encoder/Back presses), and shows the device's
render-stats telemetry coming back. It's the same `obc-platform::debug_usb` wire
protocol the simulator and the STM32 prototype use — only the transport differs (VCOM
UART here vs. USB-CDC on the STM32). `--list` enumerates serial ports; the VCOM is the
J-Link CDC port.
