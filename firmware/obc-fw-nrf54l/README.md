# obc-fw-nrf54l — nRF54LM20-DK firmware

The **real hardware target** for OpenBikeComputer: the shared `obc-app` running on
an nRF54LM20-DK (Cortex-M33), with map/routes/tracks streamed from a microSD card —
load a route, ride it (driven by the **real SAM-M10Q GPS + BMP581 altimeter** on the shared I²C
bus, issue #218, or a `--features synth`/`debug-uart` stand-in for indoor work), and record the ride
directly as a flat-store Ride object (GPX export happens in the companion app after sync — the
device writes no GPX or FAT conversion artifact). The firmware drives
the reflective **LS021B7DD02** memory LCD (the panel the project ships on) via the
nRF54L's **FLPR** (the VPR RISC-V coprocessor) — the only display path. The LS021 protocol
is on the [display-protocol docs page](https://openbikecomputer.com/hardware/display-protocol/);
the `ls021-*` binaries below are its standalone bring-up benches.

See the canonical hardware ledger in [`src/board.rs`](src/board.rs) for the full peripheral/pin
plan; this README is the **board setup + build/flash** guide.

## One-time board configuration (nRF Connect **Board Configurator**)

These three settings are applied with the **Board Configurator** app (in *nRF Connect
for Desktop*), are written to the DK's interface MCU, and **persist across power
cycles** — do them once. After changing anything, click **Write config** (blue dots =
unwritten). No soldering / solder-bridge cuts are needed on current board revisions.

1. **VDD / VDDM → 3.3 V.** The default is 1.8 V, which is too low for the LS021's logic. (Also feed
   the panel's `Vin` from the DK's 5 V / VBUS so its on-board 3.3 V LDO has headroom.)
2. **External memory → OFF** ("external memory → GPIO on the P2 header"). This
   electronically disconnects the on-board QSPI flash, freeing **P2.00–P2.05**. Since the storage
   pivot (#1158) those six pads carry the **microSD card** in native 4-bit SD mode, not the display;
   either way we never use that flash (maps live on the card).
3. **VCOM hardware flow control (HWFC) → OFF.** *Required for the `debug-uart`
   build.* The DK's J-Link VCOM defaults to RTS/CTS flow control; with it on, the
   interface MCU gates **host→device** bytes on the device asserting RTS — which this
   firmware never does (it runs the VCOM 2-wire). The symptom of leaving HWFC on:
   device→host **telemetry works** but injected GPS fixes / button presses are silently ignored.

## Wiring (DK headers)

Full detail is in `src/board.rs`. The **build drives the LS021 panel** — its FLPR
wiring is in the [LS021 section below](#ls021-flpr-builds--dk-wiring-issue-165). Also on the board: a
**microSD** breakout on **P2.00–05** (native 4-bit SD, no SPI — see
[microSD over sEMMC](#microsd-over-semmc-the-storage-transport)); the four DK buttons; the J-Link
**VCOM** and RTT both ride the DK's USB.

### Full pin map (LS021 / FLPR build)

**Port P2 — MCU/fast domain. All 11 pins used: the microSD bus and the panel's source bus, two of
them shared** (epic #1158). The card's six pads are *fixed* by Nordic's sEMMC soft peripheral; the
display's six data lines take the four pins the retired SD-SPI path freed plus the two shared pads,
whose `CTRLSEL` flips per mode (`GPIO` for the display blob, `VPR` for the soft peripheral). Display
and storage never run at the same instant — `src/flpr_mux.rs` time-multiplexes the one FLPR.

| Pin   | sEMMC | Display | Notes                                                        |
|-------|-------|---------|--------------------------------------------------------------|
| P2.00 | D3    | **B0**  | **shared** — `CTRLSEL` per mode; internal pull-up in storage mode |
| P2.01 | CLK   | —       | card only; parked as an input in display mode                 |
| P2.02 | D0    | —       | card only; parked as an input in display mode                 |
| P2.03 | D2    | —       | card only; parked as an input in display mode                 |
| P2.04 | D1    | **B1**  | **shared** — `CTRLSEL` per mode; internal pull-up in storage mode |
| P2.05 | CMD   | —       | card only; parked as an input in display mode                 |
| P2.06 | —     | R0      | source data (even-`x` R) — was SD-SPI SCK                     |
| P2.07 | —     | BCK     | source shift clock (unchanged)                                |
| P2.08 | —     | R1      | source data (odd-`x` R) — was SD-SPI MOSI                     |
| P2.09 | —     | G0      | source data (even-`x` G) — was SD-SPI MISO                    |
| P2.10 | —     | G1      | source data (odd-`x` G) — was SD-SPI CS                       |

The packed wire word is therefore `DATA_MASK = 0x751` (`B0`→0, `B1`→4, `R0`→6, `R1`→8, `G0`→9,
`G1`→10) — pinned from both sides by `obc_display::ls021::wire`'s goldens and by a test that parses
`src/flpr/flpr_scan.c`.

`even`/`odd` above is **0-based `x`**, matching those goldens: the `*0` lines carry `x = 0, 2, 4, …`.
The panel datasheet numbers columns from 1 and so calls that same physical line the *odd* column —
same wire, different origin.

**Pad configuration per mode** (`src/semmc.rs`, `configure_storage_pads` / `configure_display_pads`):

| | the six card pads | the four card-only pads |
|---|---|---|
| **storage** | Output, input Disconnect, **E0/E1** drive, `CTRLSEL = VPR`, `GPIOHSPADCTRL.BIAS = 2`; internal pull-up on `D3`/`D1` only | (same — all six are the card's) |
| **display** | `P2.00`/`P2.04` → Output, S drive, no pull, `CTRLSEL = GPIO`, `GPIOHSPADCTRL.BIAS = 2` | Input, no pull, `CTRLSEL = GPIO` — the external pull-ups hold the bus idle-high and the card stays inert |

`GPIOHSPADCTRL.BIAS` is a **port-global** trim, not per-pin, so it is not restored per mode — both
configurations set the same constant 2 (`semmc::HS_PAD_BIAS`). 2 is Nordic's value for the card at
32 MHz and the panel's ≤0.758 MHz `BCK` is indifferent to it; writing it from both sides is what
keeps the value independent of whether a card access has happened yet.

Only `D3`/`D1` get an *internal* pull-up: this desk breakout carries its own resistors on
`CLK`/`D0`/`D2`/`CMD`, and 13 kΩ ∥ 10 kΩ would sit under the SD spec's floor. **The production board
should fit external 10–100 kΩ pull-ups on `CMD`/`DAT0–3` (none on `CLK`) and then run all internal
pulls off.**

**Port P1 — PERI domain ≤8 MHz (gate/BSP + sensors + COM + VCOM + buttons):**

| Pin   | Signal       | Notes                                                     |
|-------|--------------|-----------------------------------------------------------|
| P1.03 | I²C SCL      | shared GPS + altimeter + compass bus (TWIM22, #218)        |
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
| P1.27 | backlight    | **PROVISIONAL** — PWM20 ch0, 1 kHz (#1558); also DK LED2    |

**Port P0 — low-power domain:**

| Pin   | Signal | Notes    |
|-------|--------|----------|
| P0.05 | BTN3   | SELECT   |

> **This table mirrors the canonical `src/board.rs` ledger and the constructors in `src/main.rs`.**
> If they ever disagree, the constructors are the executable authority and both documents must be fixed.

### The provisional backlight pin (#1558)

The panel is reflective and has **no light of its own**, and no front light is fitted yet. What is
wired is the *seam*: `obc_ports::Backlight` on a real PWM output, so the quick drawer's brightness
control has hardware behind it and a later driver is a new impl of the same trait.

**P1.27, PWM20 channel 0, 1 kHz** (`src/panel_power.rs`). On the shipping board the net is meant to
be the gate of a low-side MOSFET switching the front light — push-pull, idles low, so a disabled PWM
is a dark lamp. On the DK it doubles as the buffered **LED2** net, which is deliberate: the five-step
duty ladder is visible on the desk without a logic analyzer, and probe-able with one.

Why not the others: **P0.01–03** are held for the I²C sensor expansion (and no PWM instance reaches
P0 — PWM20/21/22 are PERI-domain, P1/P3 only). **P2** is full (microSD + the panel's source bus), has
no PWM mapping, and PWM20 measured dead there during bring-up. **P3** is an embassy-nrf 0.11 landmine
(any P3 GPIO faults — see the COM notes). **P1.07/.18** are free but are *dedicated clock pins*, kept
for a future SPIM SCK / TWIM SCL; **P1.18/.19** also carry the DK's UART1 flow control, **P1.20/.21**
the 32 kHz crystal, and **P1.01/.02** are NFC-strapped through 0 Ω resistors. That leaves
P1.00/.06/.15/.27/.28/.29–.31, of which P1.27 is the one that also lights an LED.

The **level → duty ladder** is not here: it is `obc_platform::backlight`, board-agnostic and
host-tested, so the same five brightnesses survive a change of driver. It is square-law
(`40 · (level + 1)²` per mille, countertop 1,000) because evenly spaced duty is not evenly spaced
*perceived* brightness, and there is no off step.

The shared **I²C / Qwiic** bus on TWIM22 (the instance the SD freed when storage moved onto the FLPR)
carries the
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

The GPS **TX-Ready** line is an *optional* data-ready interrupt: when present it asserts as a
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
```

**There is no `ble` feature and no `usb` feature.** The nrf-sdc + MPSL + TrouBLE stack
(`src/ble/`, issue #270, epic #267) and the USB device plane (#889) are in every build above: the
full map/ride app, the companion link over both transports, and one SD card + RRAM settings behind
one async mutex, in one image. The device advertises as `OBC-XXXX` (S0 §2 — the FICR serial tail).
See the "USB device plane" section further down for the bring-up recipe.

```bash
# The OBC2 media bench (#1354). Storage only — no display, no app, no BLE, no sensors — and
# DESTRUCTIVE: it deletes and rebuilds /OBC2 on the card in the slot. It decides the two §1.1
# volume-geometry preconditions for this card, times the §12 skeleton initialization, records the
# exact sectors a gated sync writes (the §13.1 clean-flush obligation), times the commit cycle, and
# then verifies the recovery decision across resets. Run it, then `probe-rs reset` (or re-run) for
# each further recovery cycle; the on-card journal is the state that carries across.
cargo run --release --bin obc2_media_bench
```

Its first cycle is destructive and its later ones are not: the bench resumes the journal it finds
and appends exactly one record per boot. Flip `FORCE_REINIT` in the source for one flash to get the
destructive first-cycle path back on a card it has already initialized.

```bash
# The OBC2 store bench (#1359) — the layer above the media bench. Same bring-up (storage only, no
# display/app/BLE/sensors) and the same destructive posture, but it drives the whole DOS3 kernel
# transaction on the card: §12 mount classification and initialization with lazy shards, then one
# upload lifecycle (claim → append → seal → validate → publish → QueryOperation), then the sectors
# that publish wrote, the resident footprint, and the stack high-water. Reset the board and run it
# again: the store must remount from the card alone with the head, its payload bytes and the
# retained result intact, and then commit one more object.
cargo run --release --bin obc2_store_bench
```

It reinitializes only when §12 refuses to mount what it finds (or when `FORCE_REINIT` is set), so
consecutive resets accumulate objects and journal records — which is the point, since the mount cost
it reports grows with the replay suffix. It refuses to write to a store whose `StoreId` is not its
own.

```bash
# The flat store bench (#1386) — `obc_storage::flat` on the card, and the successor to both benches
# above. It measures every figure `specs/FLAT_Store_Format.md` states: §8 initialization, §5.6 mount
# at an empty / 300-entry / 1024-entry catalog and with a ride recording, §5.5's commit at each of
# those, §7.2's checkpoint cadence, and §6.1's read path over a 2 GiB object with the read
# amplification device blocks read / payload blocks required, which must be 1.00.
#
# MORE destructive than the OBC2 benches: the flat store owns the RAW CARD FROM LBA 0, so a run
# destroys the partition table too. Anything on the card is gone. It refuses a card that already
# carries a flat store under another `StoreId` (override with `FORCE_REINIT`).
#
# Phase one takes ~44 minutes on a 64 GB card: ~8 of them are the commit ladder, ~5 the 2 GiB
# write, ~3 the sweep, and ~20 the two CRC-32 folds over those 2 GiB — `obc-crc` folds at
# 3.4 MB/s here, which is a measurement of its own and the reason the folds are timed apart.
# Phase two takes 3 seconds.
#
# Every timed figure is reported as three terms — the card's write half, its read half, and what
# was left for the M33 — measured inside the block-device adapter. A commit at 300 entries writes
# 79 blocks and reads 156 of them, so one number for it would say nothing about which to fix.
cargo run --release --bin flat_store_bench
```

To erase the benchmark corpus without immediately recreating it, use the
bench's explicit **reset-only** maintenance mode. It initializes one empty flat
store and parks before serial ingest and before the `fs4-ladder` measurement
phase:

```bash
cargo run --release --bin flat_store_bench --features flat-store-reset
```

This destroys every card object (maps, routes, trips, rides, weather, and
updates), not only benchmark routes. Wait for RTT to print `RESET ONLY complete`,
then stop the runner and flash the normal `obc-fw-nrf54l` app image. The normal
image can receive the desired map and routes over its protocol-v4 USB Device
page; do not reset or rerun `flat_store_bench` after uploading them.

It runs in two phases, and which one it runs is decided by what is on the card. A card that is not
this bench's store gets phase one: initialize, then every measurement above, ending with a ride left
**recording**. `probe-rs reset` then runs phase two on that ride — §7.3 recovery through the store's
own `recovered_ride`, §7.2's ride end, and the whole ride read back byte for byte against the payload
phase one generated. A third reset finds no ride recording and starts phase one over.

#### Serial map ingest — putting a real map on the card

Before either phase, the bench advertises on the DK's VCOM UART for ten seconds. If a host answers
it takes objects over the cable instead of measuring anything, and then parks — the measurement run
would allocate the card out from under what it just accepted. Nothing answers, the window expires,
and the bench is what it always was.

This exists because a board session needs a **real packed map** on a real flat store and nothing
this binary brings up can put one there. The bench image is storage and nothing else — it spawns no
USB plane and no radio — so the app build's v4 `PUT` is out of reach from inside it, BLE v4's phone
client is not ready, and the host has no card reader. (Since FS7.5-c3b the *app* build does serve a
map over the cable; running the bench through it would mean bringing up the plane the bench exists
to stay out of the way of.) The wire — magic, kind, length, CRC, then acked chunks — is
documented in full in the binary's module docs; the CRC is verified before the commit, so a bad
transfer publishes nothing and a retry is simply a fresh put with a new `ObjectId`.

Start the host **first** — it blocks on the device's advertisement — then flash and run:

```bash
# shell 1, from the repo root
python3 tools/bench_ingest.py --port /dev/cu.usbmodem*133 \
    --file "$(python3 tools/fixtures.py resolve monaco-upahead | awk '/^map/ {print $2}')" \
    --kind map --name monaco.obcm

# shell 2, from this directory. The committed runner is `probe-rs run --chip nRF54LM20A --verify`,
# so this flashes WITH the RRAM read-back check .cargo/config.toml explains — keep it.
pkill probe-rs
cargo run --release --bin flat_store_bench
```

`--verify` is not optional on this part: probe-rs 0.31's program path corrupts the first write after
a code change often enough to matter, and on an RRAM device that is a boot HardFault at a random PC.
The check turns it into an immediate, loud failure — if it trips, just run it again. (There is no
`cargo run --verify`; `--verify` lives in the runner string, not in cargo's argument list. To pass
anything to probe-rs from cargo it would have to be `cargo run -- …`.)

If you would rather flash and attach as two steps — reflashing without dropping an RTT session, say
— run probe-rs directly and keep the flag:

```bash
probe-rs download --chip nRF54LM20A --verify target/thumbv8m.main-none-eabihf/release/flat_store_bench
probe-rs run      --chip nRF54LM20A target/thumbv8m.main-none-eabihf/release/flat_store_bench
```

`sim-monaco`'s `monaco.obcm` is 718,336 B, which is about **63 s** at the wire's default 115,200
8N1. `INGEST_BAUD` in the binary takes that to ~7.5 s at `Baud1m` (pass `--baud 1000000` to match),
at the cost of finding out mid-session whether this J-Link's CDC will carry a megabaud. RTT prints
the `ObjectId` and a full catalog census after every commit, which is the acceptance evidence.

**A `--baud` mismatch is silent, and it wipes the card.** Check it first. The host transmits only
after it has decoded a valid READY, so at the wrong rate it decodes nothing, sends nothing, and
waits — while the device sees an idle line, concludes nobody is there, and starts the destructive
run. Neither side errors. The signature is that exact pair: **RTT says `nobody answered` while the
host says it is still waiting.** Nothing else produces it, and the device prints a line naming its
own baud right next to `nobody answered` for this reason.

**If the host reports the device is not answering** and RTT does *not* say `nobody answered`, the
device sends a `GONE` frame on every exit path and the host prints its reason. Four causes, four
different fixes:

- `the window closed` — the host started after the ten-second window, and the bench is now running
  the **destructive** measurement suite. Reset it immediately;
- `the session is over` — the board took its last object and parked. Reset to re-arm;
- `reservation held` — a commit was refused and its extents are held until a remount. Reset;
- `could not frame` — something else is driving this tty (a `screen`/`minicom` at another rate, a
  second talker, a failing cable). The bench **refused** the measurement run, so the card is
  untouched; clear the line and reset. Note this is *not* the baud case above — a mismatched host is
  silent, not noisy.

Two more never reach the wire at all: the baud mismatch, and a card carrying a foreign `StoreId`
(`run` refuses before the ingest is ever offered — RTT says so, and `FORCE_REINIT` is the override).

Only if **no** `GONE` arrives, RTT shows the bench advertising, and the baud is confirmed has the
J-Link's VCOM wedged: host writes succeed, RTT keeps flowing, nothing reaches the device. A physical
power-cycle of the DK is the only fix; `probe-rs reset` does not clear it.

(The standalone FLPR waveform bench bin `ls021_flpr_bringup` was retired in #177 once the app drove
the LS021 on glass; the M33-direct `ls021_bringup` bench was retired earlier in #176; the A1 BLE
spike bin `ble_spike` was retired at #270 when the stack moved into `main.rs`/`src/ble/`; the
`sd_bench` raw-throughput harness went with the SPI storage path it measured, at #1158. Its sEMMC
successor is the nonshipping `sd-bench` feature described below: it profiles the real map renderer
rather than adding another standalone binary. All retired bins remain in git history if an isolation
bring-up is ever needed again — the FLPR transport is `src/ls021_flpr.rs`, exercised by the default
build.)

### LS021 FLPR builds — DK wiring

The source bus + `BCK` stay on **P2** (see [the pin map](#full-pin-map-ls021--flpr-build)); the four
gate lines + `BSP` sit on `GSP P1.10 / GCK P1.11 / GEN P1.12 / INTB P1.13 / BSP P1.14`, and COM on
`P1.22–24`. Since #1158 the display shares P2 with the microSD card rather than with an SPI bus:
`R0/R1/G0/G1` took `P2.06/.08/.09/.10` — the four pins the retired SD-SPI path freed — and `B0/B1`
time-share `P2.00/.04` with the card's `D3/D1`.

The gate/`BSP` pins, the masks in `src/flpr/flpr_scan.c`, and the physical 21-pin FPC harness must
all agree; if a gate line stays dark on glass, confirm the pin is broken out on your DK header and
remap all three together. Cross-core display architecture and protocol:
[firmware/docs/ls021-flpr.md](../docs/ls021-flpr.md).

### microSD over sEMMC — the storage transport

**There is no SPI.** The card runs in **native 4-bit SD mode** on the same FLPR that drives the
panel, through Nordic's **sEMMC soft peripheral** — a 13,636 B position-independent RISC-V image
(vendored at `vendor/semmc/`, `LicenseRef-Nordic-5-Clause`) that turns `P2.00–05` into a real SD host
controller. The M33 fills in a register block in the image's RAM carve and pokes VPR tasks; the
coprocessor does the clocking, CRC and framing. Measured on glass 2026-08-05/06 (LM20-DK + SanDisk
SDXC 64 GB), against the SPI path this replaced:

| | sEMMC | SPI (retired) |
| :-- | --: | --: |
| read, CMD18 × 256 blocks @ 32 MHz 4-bit | **14.7 MB/s** | 1.07 MB/s |
| write, CMD25 × 256 blocks @ 21.3 MHz | **8.2 MB/s** | ~1.1 MB/s |
| single-block read | 430 µs | 1.1 ms |

The bulk number is not the whole map-rendering story. The 2026-08-07 on-device profile used the
shipping FAT extent path and a **three-file volume set** (394,075,657 B across its geometry files,
32 KiB clusters) — **a card that predates the volume set's removal** (#1420: FS7.5b2 took the
producers, FS7.5c3b the reader), since nothing produces or mounts one any more; repeating this
measurement means keeping that card, or re-running it against a single-file map and recording that
the shape of the read changed with it — while `SynthLocation` moved the camera. Counters at the `ByteSource` and concrete `BlockDevice`
boundaries separate logical reader requests from physical sEMMC commands; the time includes FLPR
mode acquisition, the card commands and any alignment-bounce copy.

| Live map frame | Before reader tuning | Final warm result | Change |
| :-- | --: | --: | --: |
| four chunks | 36 commands / 18,432 B / **12.48–12.70 ms** | **0 commands / 0 B / 0 ms** on most frames | storage sleeps between refreshes |
| five chunks | ~66 commands / ~30.8 KiB / **25.1–25.7 ms** | **0 commands / 0 B / 0 ms** once resident | eliminates steady five-chunk thrash |

The raw bus was already healthy; command latency was the bottleneck. Pass B had been walking the
quadtree again to reconstruct a leaf bbox even when pass A's geometry chunk was resident. The
repeated ordered scans could also cyclically thrash both the index and geometry caches. Cached
chunks now keep their leaf anchor; both caches use scan-resistant RRIP; the otherwise-idle first
4 KiB of the oversized decode scratch acts as a fifth geometry slot; and two bounded leaf lists
prefetch 1/8 of a viewport around the current view. The latter replaces one tagged index sector,
so the complete change leaves `MapCache` at the same 37,084-byte resource baseline.

Zero is the steady state, not a claim that panning never reads storage: crossing into two new
chunks measured **4.45 ms**, while a periodic expanded-index refresh measured **5.9–7.6 ms** and
was followed by zero-command frames again. A measured 1 KiB index-window experiment was rejected:
despite turning reads into CMD18, it raised the earlier four-chunk warm result from 4.55–4.81 ms to
8.6–9.0 ms by halving useful cache capacity.

To reproduce, build and flash `cargo run --release --features synth,sd-bench`. The benchmark image
boots directly to Map and emits one `map SD bench:` RTT line per redraw. `sd-bench` is absent from
normal builds and adds no counters or automatic map boot to shipping firmware.

**Wiring.** Six jumpers, no chip-select: `P2.00 D3 · P2.01 CLK · P2.02 D0 · P2.03 D2 · P2.04 D1 ·
P2.05 CMD`, plus GND and 3V3. The pin assignment is fixed by the soft peripheral, not by us. Pull-ups
per [the pad table above](#full-pin-map-ls021--flpr-build).

**Sharing the coprocessor.** `src/flpr_mux.rs` time-multiplexes it: a switch to storage is 29 µs
(park the hart, flip the pads, warm-boot the resident image, power it on), a switch back 138 µs
(quiesce, park, flip, relaunch the display blob). The card keeps its `tran` + High-Speed state across
a switch and is **never** re-initialised — measured 12/12 rounds. The mode is *lazy*, so a run of
reads pays 29 µs once and a run of frames 138 µs once; the panel keeps getting frames throughout a
multi-megabyte upload because storage sessions never outlive one synchronous burst.

**Writes cap at 21.3 MHz.** 32 MHz writes fail card-side CRC on the jumper harness — a clean failure,
nothing programmed — while 32 MHz reads are spotless. The clock is per-transaction, so mixed-rate is
free. Re-test on soldered hardware.

**If the card does not come up**, the RTT log names which rung failed (`SemmcError`), and an aborted
transfer is decoded (`command timeout` / `command CRC` / `data CRC (clock too high for the wiring?)`
/ `retries exceeded` / `protocol error`). A `data CRC` at 32 MHz on a long harness is the one worth
trying `Semmc::set_read_delay` for.

> ⚠️ **The bootloader cannot read the card.** `obc-boot` still carries the SPI path and there is no
> room in its 32 KB carve for the 13.6 KB soft-peripheral image, so **SD-staged DFU (install and
> rollback) does not work on this hardware** until the epic gives the bootloader a storage story. It
> fails safely — bring-up reports no card and the old app boots — but it does fail. See
> `obc-boot/src/sd.rs`.

#### COM on P1.22–24: one wiring contract, two drivers

The current nRF54LM20-DK harness routes COM (`VCOM`/`VB`/`VA`) to **P1.22/P1.23/P1.24**. The default
build owns those three nets as high-drive GPIO outputs and `com::com_task` toggles them at ~60 Hz,
waking the M33 about 120 times per second. The opt-in **`com-hw`** build owns the same pins as GPIOTE20
channels and drives the same waveform from a zero-CPU **TIMER21 → DPPIC20 → GPIOTE20** chain.

Historically, the first display harness put COM on P2.07/P2.08/P2.10 beside the source bus. P2 has no
GPIOTE mapping and PWM was measured dead on that retired routing, so #1158 rehomed those nets to the
current P1 pins when P2 became the display-source + native-SD bus. That history does not describe the
current wiring. `com-hw` remains **on-glass + logic-analyzer verification pending**, so the shipping
default continues to use the M33 driver on P1.22–24.

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
`trouble-host` on this DK; A2 (#270) folded that stack into the real firmware as `src/ble/`, which
is in every build (the build command is above). What the spike settled, for everything Track A
builds on it:

- **Versions.** The crates.io `nrf-sdc`/`nrf-mpsl` releases (0.3.x) predate nRF54L support and
  pin embassy-nrf 0.7 — useless here. The **git pin** (same rev as TrouBLE's `examples/nrf54`)
  targets exactly this crate's embassy set (embassy-nrf 0.11 / executor 0.10 / sync 0.8 /
  time 0.5.1), and `trouble-host` **0.7 from crates.io** matches too — no embassy patch or bump
  anywhere. Its heapless 0.9 coexists with our 0.8 (no types cross the boundary).
- **Critical section.** MPSL ships its own mandatory `critical-section` impl
  (`nrf-mpsl/critical-section-impl`) — global-interrupt-disable critical sections break its
  radio timing, and two impls are a duplicate-symbol link error. It is the **only** impl in the
  tree: `cortex-m/critical-section-single-core` lived behind a `cs-single-core` feature for a
  radio-less build that stopped compiling and that nothing in CI built, and #931 removed it. The
  radio is in every build, so the plain `cargo build --release` gets the right impl and there is no
  invocation to remember.
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
- **MPSL/SDC hardware.** `board.rs` owns the five production MPSL vectors and 31 MPSL/SDC
  timing/PPI claims. `main.rs` intentionally retains CRACEN so store-epoch minting can reborrow it
  before `ble::run` consumes it as the link layer's crypto RNG; `ble::run` owns runtime/stack policy.
- **RAM.** The map plane and the BLE stack are both in every build (#270, #1530), so they are
  resident together. The budget assert in `main.rs` sums the map-plane residents (`MAP_RESIDENT`)
  and the BLE stack's (`ble::RESIDENT_BYTES`) and fails the build on this DK at compile time if
  they overrun the carve. The one piece that drops out on the DK is the on-device router
  (`has_nav`): its ~14.3 KB of `NAV_*` statics don't fit beside the BLE stack on the 256 KB DK
  (see `build.rs`); the 512 KB LM20 relaxes the `has_nav` gate to run the router as well.
- **Stack: keep big values out of long-lived async bodies (#677).** Every sizeable value
  constructed inline in an async fn/block gets a construction-temporary slot in the generated
  poll function's **stack frame**, allocated at entry on *every* poll — `ble::run` once carried a
  30.5 KB poll frame this way, and SMP's synchronous software-P256 pairing chain (which runs in
  the host runner's rx path, on the main stack) overflowed the region into `defmt_rtt::BUFFER`:
  a HardFault with a corrupted backtrace on every pairing attempt. The discipline: big objects
  live in `.bss` statics built by dedicated `#[inline(never)]` init fns (transient frame, boot
  depth) and the async body holds only `&'static` handles — see `ble::run`'s doc. Three nets
  catch a regression: the compile-time budget assert (floors the stack region), the CI
  **poll-frame guard** on the release ELF (largest `sub sp` in any `TaskStorage<F>::poll` ≤ 12 KB —
  the `ci.yml` embedded job), and **MSPLIM** (armed first thing in `main`): the ARMv8-M hardware
  stack-limit register turns any residual overflow into an immediate, precise fault instead of
  silent `.bss` corruption. To check a frame by hand, disassemble the release ELF
  (`cargo objdump --release -- -d --demangle`) and read the
  `sub sp` at each `TaskStorage<F>::poll` entry.

## BLE — board-specific notes & on-glass verification

The radio's own contract — advertising policy, GATT services/UUIDs, CoC framing, pairing security,
and the `command` / `status` / `config` characteristics — is canonical in
[`obc-ble-interface-spec.md`](../../specs/obc-ble-interface-spec.md); read it there, not here. The
**object** surface is not in that document any more on either link: it is
[`FLAT_Store_Protocol.md`](../../specs/FLAT_Store_Protocol.md), implemented by the host-tested
`obc-link` crate (`cargo test -p obc-link`) and reached through `src/ble/` and `src/usb/`. What's
**board/firmware-specific** and worth knowing:

- **DIS identity** — Firmware Revision is the **installed OBCU container's version string**, read
  off the DFU boot-state page at boot (`dfu::seed_firmware_revision` → `link::identity`), falling
  back to `OBC_FW_GIT` — the bare git short hash `build.rs` emits — on a probe-flashed board that has
  never installed a container. So a device you flashed over SWD reports `ca9b336`, not a version, and
  no host offers it an auto-update (the dialect + why is in the BLE spec §3.1); to see a real version
  on glass, install a wrapped `UPDATE.BIN` (`obc-mkimage`). The same string answers the USB
  §5.2.1 EP0 vendor request. Hardware Revision `nrf54l15-dk`, Serial Number the 16-hex FICR `DEVICEID`
  whose last four digits are the `OBC-XXXX` advertised name.
- **A FAT card is no longer a supported runtime store.** Boot rejects it as unformatted and offers
  the recovery link; there is no FAT ride recorder, ride scan, conversion, recovery, or
  `RD*.ORD` ownership fallback.
- **The volume set is gone from the card, reader side included.** A map is one OBCM file with its
  terrain inside it (`OBCM_Spec.md` §1.3), so the manifest, the shards, the set-wide mount and the
  `.bss` shard table all went with FS7.5b's producers and FS7.5-c3b's reader. One consequence is
  worth carrying to the bench: the map scan's shard-name exclusion went too, so a card still holding
  an old set now lists **each shard as its own map** — every one of them is a real OBCM file, and a
  geometry shard opened alone has no roads and no POIs. That card wants re-sending onto a flat store,
  not diagnosing.
- **Config + bond persist in the RRAM SETTINGS carve** (`settings.rs`), which survives power cycles
  **and a firmware reflash** (it sits above the app image, so `probe-rs download` leaves it): the
  device name (config codec v3) @0, the boot counter @2048, the single 64-byte CRC-checked bond slot
  (LTK + peer IRK, `ObjectStore::{load,save,clear}_bond`) @`BOND_OFFSET`. Pairing is LESC passkey
  **display** — the 6-digit code renders on the app's passkey card (`Font::Huge`). One bond slot, and
  while it's occupied the device **rejects** every new pairing attempt (#455): the hold-guarded
  **Forget phone** in Settings ▸ Bluetooth is the only device-side clear, so physical possession
  guards the *clear* step (it no longer silently replaces the bond on a fresh pairing).

**Verify on glass** (nRF Connect is the pre-app oracle for A3–A4; the iOS app covers A5+):
- **nRF Connect** — service/char table matches the spec, DIS strings + serial real, BAS notifies,
  `protocolVersion` reads two bytes, `4`, `psm` reads `0x0080`; negotiated MTU (247) + 2M PHY, interval settles
  to the idle set; disconnect/re-connect + walk out of range bumps the counters and always returns to
  advertising. As an *unbonded* stranger (post-A8): DIS/`protocolVersion` readable, access-denied on
  every gated char + the CoC.
- **E2E golden path** — share a GPX to the iOS app, upload (B5 sheet), then reflash; the route is in
  the device menu and rideable (SD persists across flashes). For sync: record 2–3 rides (`synth` is
  fine indoors), then sync pulls them; spot-check a decoded ride's totals in the app against the
  device's Paused ledger. Ids must survive a power cycle; the boot counter must increment across
  them.
- **Single-file map path** — the volume-set run this item used to describe (#1033) is retired with
  the set itself (#1420 FS7.5b/c3b); what replaces it is one map file and the whole of it. Put a
  packed `.obcm` on the card, boot the default image, and ride a route from end to end at fine zoom
  and then zoomed out: roads, route ink, rider marker and guidance stay continuous, because there is
  no longer a boundary to cross. The RAM acceptance the set run carried is still owed against this
  shape and its figures do not carry over — the numbers it quoted (53,400 B above linked residents,
  17,592 B beyond a 35,808 B deep-path peak) were measured with an eleven-shard mount in `.bss` — so
  re-measure the deep path here rather than re-quoting them, walking the ordinary route-load → ride →
  finish/save path with `STKOF`/HardFault in view.
- **Pairing** — passkey card on the panel typed on the phone → bond lands; power-cycle / app
  restart / walk-away → silent reconnect, no dialog; reflash → still no dialog. A **second
  phone** is rejected while bonded (no passkey card, generic failure on the stranger). Re-pair path:
  device **Settings ▸ Bluetooth ▸ Forget phone** (hold) + app *Forget* + iOS Bluetooth forget → next
  contact pairs with a fresh passkey.

## Protocol v4 on both links (FS7.5-c3a/c3b, epic #1256) — what a client sees on each card

The object surface is [`FLAT_Store_Protocol.md`](../../specs/FLAT_Store_Protocol.md) §5, and since
c3b that is true of the cable as well as the radio:

- **BLE** is §5.1: `objectControl` (`3C920009`, Write Request + confirmed indication) carries
  control frames, the L2CAP CoC carries the byte stream of consecutive §3.8 stream records — a
  record is recovered from its **own header** and may cross SDU boundaries, because CoreBluetooth
  exposes a CoC as a stream whose write may be accepted in pieces — and `protocolVersion` reads two
  bytes, `4`. `command` / `status` / `config` are untouched and still governed by
  [`obc-ble-interface-spec.md`](../../specs/obc-ble-interface-spec.md).
- **USB** is §5.2: both bulk endpoint pairs carry §3 frames, each USB-binding-v5 record a
  `record_length u32`, frame bytes and zero alignment padding. The binding is settled by descriptor
  matching before a record moves (`bInterfaceProtocol = 5`, `bcdDevice = 0x0500`), independently of
  §3's protocol-v4 frame major. The cable's non-object surface is one EP0 vendor
  request (§5.2.1); it carries nothing else. The endpoint section further down has the board detail.

Over the cable a device therefore serves **object transfer and device information, and nothing
else**. Route retention, ride acknowledgement, clock setting, bond forgetting and the settings blob
are BLE-only imperatives — §5.2.2 is the table of where each retired selector went, and the short
version is that the two that act on the store became `REMOVE` and `ARM` and the rest kept the BLE
characteristics they always had.

The engine lives in the flat store's `storage_task`, not in either transport — the store seam is
synchronous, and the card has exactly one writing execution context. The transports are record
shippers, and they are deliberately the same code below the records: one `Lane` into one engine, so
the two links cannot drift into two answers to the same question.

**The storage task is spawned on every card**, and that is what makes the two cases below one code
path rather than a branch:

| Card in the slot | What a v4 client gets |
| :-- | :-- |
| **A flat store** | Full service: `LIST` from the catalog, `GET`, `PUT`, `REMOVE`, `CANCEL`. `ARM` answers `rejected` — this build has no update policy wired (§4 needs `obc-dfu`, the RRAM boot page and a reboot). |
| **A FAT card** | Every opcode answers `readOnly` with detail `unformatted 3`. That is not a fallback or a stub: §5.6 step 1 classifies a FAT card as **not a flat store**, and §3.9 already specifies exactly this answer for one — including for the reads, because there is nothing to read. |
| **A card that carries a broken flat store** | Boot shows STORAGE FAULT before any plane comes up; the honesty rules in `obc-app` are unchanged. |

Three client-visible facts worth knowing before you debug a client against this:

- **Open the CoC before writing `objectControl`** (BLE only). The driver owns the channel, so a
  control write arriving with no channel up is refused at the ATT layer
  (`PROCEDURE_ALREADY_IN_PROGRESS`) rather than staged for an answer that would arrive whenever a
  channel happened to open. Lifting this needs the driver split from the channel owner and is a
  named follow-up.
- **Ride recording is flat-store native.** Samples are their final served bytes, checkpointed by
  the tail-in-slot journal and completed by one footer plus one commit clearing `RECORDING`.
- **On-glass map-transfer progress survived the cutover, and it no longer comes from a transport.**
  The card the rider watches during an upload is fed from the engine
  (`obc_link::flat::Engine::live_upload` / `take_upload_end`, published beside the engine in
  `flat_store::publish_upload`), because §5 forbids an adapter from parsing a payload and the kind
  and declared length are payload. A rider sees a card for a **map** and nothing else: a route lands
  in a second and a weather bundle is invisible by design.

On-glass checks this section is waiting for: a real phone and a real cable each completing a `PUT`
and a `GET`, the admission race (stream record ahead of its control record) surviving a ride-loop
render pass, and a `CANCEL` landing mid-download.

This section is the **transport cutover only**. It does not close FS6 or FS7: the remaining
on-device map, route, trip, and weather consumers and their legacy FAT removal stay tracked by
#1388 and #1389.

## The USB device plane (issue #889) — **in every build; cable-gated since #936**

Every build ships a second transport for the *same* object protocol: the LM20's USBHS
behind one vendor-specific interface, so the web builder (WebUSB, Chromium) or the desktop app can
push a map / route / firmware image to a plugged-in device. It was briefly behind a `usb` Cargo
feature; **that feature is gone** — the plane is part of the device, not an option of it, and the
resource baseline in [`firmware/tools/resource_baseline.json`](../tools/resource_baseline.json) pins
the shape that includes it (+5,096 B resident and +45,688 B flash for the plane, +64 B and +288 B
more for #936's VBUS gate, then +8 B resident and **−80 B** flash for #937's event-driven park;
guarded poll frame 9,664 → 9,728 B against the unchanged 12,288 B limit, then FS7.5-c3b's cutover,
which re-pins the whole `usb_named` block — the v4 adapter's three buffers replace the v1 plane's
staging pair *and* the arena arm it wrote through). The wire protocol is canonical in
[`FLAT_Store_Protocol.md`](../../specs/FLAT_Store_Protocol.md) §5.2 — USB is a transport under it,
not a second protocol. What is **board-specific** and worth knowing:

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
- **Endpoint layout** (the host reads it off the descriptors): one interface, class `0xFF` with
  `bInterfaceProtocol = 5`, four bulk endpoints at the high-speed-mandated 512 B — `0x81/0x01` §3
  control records, `0x82/0x02` §3.8 stream records. **Both pairs carry the same framing**: a
  `record_length u32`, exactly that many frame bytes, then zero padding to four-byte alignment. A
  packet boundary means nothing to the protocol — an 8 KiB stream payload spans seventeen packets
  with its header and prefix, which is why `MAX_PACKET` is no longer a frame ceiling. The ceilings
  are constants of the binding (§5.2): 8,208 B device→host on either channel and host→device on the
  stream channel (a 16-byte stream frame plus 8,192 payload bytes, and the `LIST` page ceiling at 92
  entries), and 256 B for a host→device
  control record, because §3's widest request is the 100-byte `PUT`. Device identity and the
  firmware/hardware/serial strings are **not** on these endpoints: they are one EP0 vendor request,
  `bmRequestType 0xC1`, `bRequest 0x20` (§5.2.1), readable the moment the interface is claimed.
- **The bulk OUT endpoint is armed in bursts** (#1173), which is why
  `embassy-usb-synopsys-otg` is vendored under `vendor/` — stock, it arms one packet and re-arms
  only after the firmware task has copied it out, so the endpoint NAKs for a whole scheduler round
  trip per 512 B (~342 µs measured on glass 2026-08-07, capping uploads at ~1.4 MB/s). The dial is
  `BULK_OUT_BURST_PACKETS` in `src/usb/mod.rs`; the sweep recipe is on the constant, and both RAM
  baselines move with it. A completed staged map upload now prints
  `usb: [v4] staged … B in … ms (… kB/s, full CRC + card DMA)`, so that line is the device-side
  acceptance measurement; host timing remains useful for separating browser overhead. The
  watch-list — each line says something different went wrong:

  | RTT line | What it means |
  | :-- | :-- |
  | `ep_out buffer overflow index=2` (driver) | **The burst reader armed with bytes still staged.** Should be structurally impossible — `arm_transfer` asserts against it — so a panic is the expected form. Treat any occurrence as possible silent loss. |
  | the endpoint goes dead after exactly one burst | The core cleared `EPENA` on transfer completion and the stock `DOEPTSIZ`+`CNAK` re-arm is not enough on this part. Nothing else looks like this. |
  | `usb: [rec] record length N is outside this channel's ceiling C` | A host framing bug or a desynchronised record stream: the length prefix is being read where payload is. The reader ends that record stream rather than guessing. |
  | `usb: [v4] a stream record arrived unadmitted — delivering after the hold window` | §3.6's admission race lost: the `PUT`'s first stream record beat its control record by more than the 250 ms hold. One per transfer at boot-time contention is survivable noise; every transfer means the control pump is being starved. |
  | `usb: [v4] no upload staging arm granted — using narrow card writes` | The transfer could not claim the scratch arena within one second. It remains correct, but falls back to short synchronous writes and will be visibly slower. Recovery boot pre-grants the otherwise-idle arena, so this must not appear on the first upload after format. |
  | `usb: [v4] staged … kB/s, full CRC + card DMA` | End-to-end device staging rate for this upload. Use a large map and compare it with the 7–7.9 MB/s hardware target; the flat-store bench's separate CRC timing identifies a checksum ceiling. |
  | `usb: [v4] the store's write half is not armed` | The plane came up without an engine — object service is down for this boot, and it is a storage-task failure, not a USB one. |

  Two cases worth constructing deliberately, because natural traffic almost never produces them:
  a record whose length is an **exact multiple of 512** (no short packet anywhere, so the final
  burst stays armed across the gap to the next record), and an **unplug** in the middle of a burst.
- **Uploads use DMA at both ends.** The vendored OTG driver runs in buffer-DMA mode, including
  aligned IN bounce storage and burst-sized OUT DMA. For a map `PUT`, eight 8 KiB v4 records fill
  one of two 64 KiB scratch-arena banks; the flat store then starts one 128-block deferred card DMA
  while USB reception and CRC folding fill the other bank. The storage task borrows only the
  inactive bank — never the whole arm while DMA owns its opposite half — and every completion,
  refusal, cancellation or unplug joins the card DMA before releasing the arena grant. The two
  banks cost no additional resident RAM: render already sets the same 131,072 B arena ceiling.
- **A whole-payload CRC-32 is checked before every commit, maps included.** The v1 plane exempted
  map-shaped objects from the device-side fold and leaned on USB packet CRC/retry, the card's block
  CRC/ECC and a magic-last commit instead; §3.6 retires that policy outright — the device verifies
  the declared length and the declared CRC over the whole payload, runs the kind's validator, and
  only then commits, so a mismatch is `checksumFailure` and nothing is published. The board enables
  `obc-crc`'s slicing-by-8 implementation and overlaps that fold with card DMA; compact consumers
  retain the single-table implementation. The flat-store bench times the fold independently.
- **The hardware target is the historical 7.3–7.9 MB/s class.** Those figures were measured on
  2026-08-07 against a 73.4 MB builder set using the same 2 × 64 KiB USB/card pipeline, although the
  old FAT path skipped the map CRC. The restored flat-store pipeline keeps full v4 verification.
  A conservative card-command model (`1,470 µs + 127 × 72 µs` for 64 KiB) predicts about
  **6.17 MB/s**; that is a model, not the acceptance result. Flash the firmware, upload a large map,
  and use the staged-rate RTT line above for the real on-glass number. Approximately 7–7.9 MB/s is
  the target; a result near the former ~0.5 MB/s means the staged path was not active.
- **Windows needs no driver install**: MS OS 2.0 BOS descriptors declare the `WINUSB` compatible id
  and a stable `DeviceInterfaceGUIDs` property, so Windows auto-binds WinUSB with no `.inf` and no
  Zadig.
- **VID/PID `1209:0001`** is pid.codes' *prototype* pair — deliberately the id that means "not
  allocated yet". Allocating a real PID is an owner action; when it lands, two constants change
  (`src/usb/mod.rs` and the web builder's `OBC_USB_FILTERS`).

### Bring-up recipe

Steps 1–2 are the **cable-less boot**, and they are the ones that matter most: that is how the
device is used. Steps 3–6 are the ordinary transfer path; step 7 is recovery. All seven were
confirmed on glass through 2026-08-07 — but against the v1 plane. **Enumeration and the cable cycle
(1–3, 5, 6) are unchanged by FS7.5-c3b; everything that moves an object (4 and 7) is owed a fresh
run**, because the plane under it is a different one.

1. With **J3 empty**, flash `cargo run --release` over **J4** — the plain default build; no feature
   flag selects the plane any more (remember the flash-twice quirk and keep `--verify`; retry 2–3×
   if probe-rs errors).
2. **The board must reach the ride loop and stay there.** RTT shows
   `usb: no VBUS on J3 — device plane parked; it comes up when a cable is plugged in`, then the map
   renders and the session keeps logging. No `DAP FAULT`, no reset loop. This is the #936
   regression test: before the VBUS gate, this exact step killed the boot.
3. Now plug **J3** into the host. RTT: `usb: VBUS present — BLE radio parked; bringing the device plane up` followed by
   `usb: device plane up — 1209:0001, serial '…', HS bulk 512 B`. **Watch the clock here** — that is
   the #937 check. The defmt timestamp on `VBUS present` should be within a few milliseconds of the
   connector seating, because a VREGUSB interrupt woke the task. If instead it lands *seconds*
   later, or not at all, the VREGUSB wake is not arriving — there is no fallback timer to carry it
   any more, so this is a hard failure rather than a slow success. On macOS
   `system_profiler SPUSBDataType` (or Linux `lsusb -v -d 1209:0001`) should show
   `OpenBikeComputer`, the FICR serial as `iSerialNumber`, **Speed: Up to 480 Mb/s**, one
   vendor-specific interface and four bulk endpoints.
4. In Chromium, `chrome://device-log` shows the enumeration; the web builder's connect button opens
   the chooser, the EP0 device-info read (§5.2.1) answers with the firmware/hardware/serial strings,
   and the first `LIST` comes back. RTT should show
   `usb: [v4] endpoints enabled — control 4112 B, stream 4112 B` once the host claims the interface
   (that pair is what the *device* may send; the 256 B narrowing is the host→device control reader's,
   and it is not in the line).
   There is no identity read to check for and no version handshake to watch: the major was settled by
   `bInterfaceProtocol` before the chooser opened.
5. **Unplug J3 mid-ride.** RTT: `usb: VBUS removed — device plane parked, endpoints idle until a
   cable returns`, and the ride loop carries on untouched.
6. **Plug it back in.** `usb: VBUS back — BLE radio parked; device plane serving again`, the host re-enumerates, and a
   transfer works again — again within milliseconds, not seconds. Repeat a few times: the cable
   cycle is a loop, not a one-shot, and this is also where a lost VREGUSB edge would show up, since
   the park after an unplug is the one that has to be woken by an interrupt rather than entered
   with the answer already known.
7. **Recovery path:** boot once with an unreadable or absent map. Keep the fault screen visible,
   connect the builder, and `PUT` a valid map; USB must enumerate and run at the ordinary map rate
   even though the ride loop and BLE were never spawned. Restart and confirm the normal ride
   application starts — that proves the replacement parsed and mounted rather than merely committed.

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
bug, not a slow link. A device that enumerates but whose transfers hang → look for
`usb: [v4] a stream record arrived unadmitted` (the host's payload beat its own `PUT` by more than
the 250 ms hold, so §3.8 discarded it and the upload died on a gap at offset zero) or a `busy` error
response (§1's one-transfer rule is shared across links — a BLE transfer is live). `Code 43` on
Windows means the MS OS 2.0 descriptor set was rejected; re-read it with `usbview`.

## Driving it from a host (`debug-uart`)

With the `debug-uart` firmware flashed and VCOM HWFC off, open the desktop feeder from the
repository root. A GPX path is optional:

```sh
obc flash debug-uart
obc uart                              # or: obc uart path/to/ride.gpx
# One command to flash and then open the feeder:
obc debug
```

`obc-usb-host` can stream the `.gpx` as fake GPS fixes, or keep a stationary fix fresh at decimal
latitude/longitude entered in its **Fixed GPS location** panel. That panel also has a user-triggered
place search (for example `Munich` or `Sydney`) whose result fills the coordinates; it uses the
public OpenStreetMap Nominatim service only when **Search** is pressed, caches repeated queries for
the session, and can be pointed at another compatible endpoint with `OBC_GEOCODER_URL`. Enable
**Send stationary fix every second** to keep the device's normal GPS freshness gate satisfied —
useful for weather tests anywhere in the world without manufacturing a GPX.

The feeder also provides a baro/compass slider and an on-screen button row that injects the four
buttons' presses, and shows the device's render-stats telemetry coming back. It's the same
`obc-platform::debug_link` wire protocol the simulator uses — only the transport differs (a VCOM
UART here). `--list` enumerates serial ports; the VCOM is the J-Link CDC port. The J3 USB device
cable is not involved and should be unplugged when the test also needs BLE.

### Triggering a firmware update over the VCOM (`dfu-install`, S4 #619)

With a **signed** `UPDATE.BIN` (see `../README.md` §Firmware update images — an unsigned
container is refused since OBCU v2, #997) in the card root, the
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
`is not signed…`, `signature is not valid for this device`, `too fragmented…`) come back the
same way and the device keeps running. The trigger is
refused mid-recording. Concept + formats: `OBCU_Spec.md`; the byte-identical request path
is what S5's on-device menu entry will post.
