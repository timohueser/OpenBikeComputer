# obc-fw-nrf54l — nRF54L15-DK firmware

The **real hardware target** for OpenBikeComputer: the shared `obc-app` running on
an nRF54L15-DK (Cortex-M33), with map/routes/tracks streamed from a microSD card —
load a route, ride it (fake-sensor fed), and save a GPX. The default firmware drives
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

| Pin   | Signal   | Notes                                 |
|-------|----------|---------------------------------------|
| P0.00 | SD CS    | moved here in the FLPR build (held LOW)|
| P0.01 | **free** | →                                     |
| P0.02 | **free** | → spare for expansion (I²C below)     |
| P0.03 | **free** | →                                     |
| P0.04 | BTN3     | SELECT                                |

## Build & flash

From this crate directory (it's a standalone crate built for `thumbv8m.main-none-eabihf`;
`cargo run` flashes + streams defmt/RTT over the on-board J-Link via probe-rs):

```sh
# Default: full map + ride loop on the **LS021 panel via the FLPR** (issues #165 / #173),
# GPS faked by the on-board SynthLocation square loop (no host needed). Builds the RISC-V
# blob, so it needs an rv32emc gcc (below) + the LS021 wiring + Board-Configurator settings.
cargo run --release

# Default + the VCOM debug-sensor feed (issue #127) — needs HWFC OFF (above):
cargo run --release --features debug-uart

# The same map/ride app on the **ST7789** bring-up panel instead of the LS021 (opt-in
# backend) — no FLPR, no RISC-V gcc, links the full 256 KB. ST7789 wiring (below).
cargo run --release --features tft
```

(The standalone FLPR waveform bench bin `ls021_flpr_bringup` was retired in #177 once the app drove
the LS021 on glass; the M33-direct `ls021_bringup` bench was retired earlier in #176. Both are in git
history if a panel-isolation bring-up is ever needed again — the FLPR transport is `src/ls021_flpr.rs`,
exercised by the default build.)

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
