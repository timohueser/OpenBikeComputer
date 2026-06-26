# LS021B7DD02 panel bring-up — protocol & DK pin spec (STATUS: L1 LANDED)

The **normative reference** for the Sharp **LS021B7DD02** bring-up on the nRF54L15-DK,
driven **M33-direct** (no FLPR yet). Epic [#139]; this doc is the deliverable of
**L0 [#140]** and the spec the firmware of L1–L4 is written against:

- **L1 [#141]** — free-running COM driver (`VCOM`/`VB`/`VA`, ~60 Hz). ✅ **DONE** (analyzer-
  verified 60.0 Hz / 50 %) — GPIO square wave on a timer; see [L1 COM driver](#l1-com-driver)
  below (and the **PWM-doesn't-route** gotcha there).
- **L2 [#142]** — power-on init → uniform **black** on glass.
- **L3 [#143]** — full-frame solid colour (white, then R/G/B).
- **L4 [#144]** — structured test pattern (colour bars + per-channel 4-level gradient).

Source: Sharp spec **LCP-0620032F**. Timing values cross-checked against the verified
bench notes (full-frame update ~53 ms, BCK ~0.758 MHz, f_V ~18.9 Hz). **Where a number
is read off a graphical timing chart it is flagged "(chart)"** — pinning these down on
the logic analyzer at a slow clock is exactly what L1–L3 do.

> **Why this is fragile, and the one rule that matters.** This is a Memory-in-Pixel
> reflective LCD with **no controller**: we generate every gate/source/COM waveform
> ourselves. Two ways to harm it — (1) **over-voltage** the rails (VDD1's abs-max is at
> 3.3 V, the rail we feed it from — see Power), and (2) **DC bias** the liquid crystal by
> leaving `VCOM`/`VA`/`VB` static or asymmetric, which causes image-sticking/degradation
> over time. **Rule: COM must free-run (alternate, ~60 Hz, ~50 % duty) whenever the panel
> is powered and being driven, even for a static image.** L0 never drives COM at all (it
> stays `Lo`); the COM driver is **L1 (landed)**, a GPIO square wave on a high-priority timer
> task so it is *always-on* and CPU-independent from L1 onward — see
> [L1 COM driver](#l1-com-driver).

## Panel overview

- **2.13″ 240×320 QVGA**, reflective colour, **64 colours = RGB222 by area gradation**.
  Each RGB sub-pixel splits into an **MSB block (2/3 area)** and **LSB block (1/3 area)**,
  each a 1-bit on/off (`Hi` = shown, `Lo` = black) → 4 levels/channel → 64 colours. This
  is the device-64 gamut the renderer already targets — RGB222 is the panel's *native*
  format. **Consequence: every gate line is written twice — an MSB sub-line then an LSB
  sub-line.**
- **6-bit parallel source bus, two pixels at once:** `R0/G0/B0` = the odd pixel, `R1/G1/B1`
  = the even pixel → **120 `BCK` per sub-line** for 240 columns.
- **No internal controller, no command set.** (The vendor EVK drives it through an Epson
  S1D13C00; we replace that with the M33.)
- **Slow, bit-bang-friendly:** `BCK` ≈ 0.746–0.758 MHz; frame `f_V` ≈ 18–18.9 Hz
  (`t_V` ~53 ms); `f_VCOM` 54–66 Hz. A full screen ≈ 640 gate-clocks (320 rows × MSB+LSB).

## Pinout (21-pin FPC) + DK pin map

21-pin FPC: 15 signals to drive + 2 rails + GND (NC: pins 2, 19, 21). **Verified on the
bench** with the `ls021_bringup` signal-walk (every DK pin lit up on its expected Pico
channel) + a meter on the rails. The harness wires each net **DK pin → panel pad → Pico LA
channel**, so any signal can be probed live on the analyzer; the table is ordered by Pico
channel (= the walk order). FPC pin numbers are from datasheet LCP-0620032F.

| Pico ch | Signal | DK pin | FPC # | Notes |
|---|---|---|---|---|
| D2 (GP2) | `GSP` | **P1.11** | 3 | gate start pulse (free P1) |
| D3 (GP3) | `GCK` | **P1.12** | 4 | gate clock, steps sub-lines (free P1) |
| D4 (GP4) | `GEN` | **P1.04** | 5 | gate output enable (UART1_TXD — device-driven, safe) |
| D5 (GP5) | `INTB` | **P1.06** | 6 | all-black init framing (UART1_RTS — free, HWFC off) |
| D6 (GP6) | `VB` | **P2.08** | 7 | COM, in-phase with `VCOM` (may strap to `VCOM`) |
| D7 (GP7) | `VA` | **P2.10** | 8 | COM, **inverse** phase |
| D8 (GP8) | `BSP` | **P1.07** | 11 | sub-line start pulse (UART1_CTS — free, HWFC off) |
| D9 (GP9) | `BCK` | **P2.06** | 12 | source/shift clock ~0.75 MHz; P2 trace pin (fast) |
| D10 (GP10) | `R0` | **P2.00** | 13 | odd-column R (freed ext-flash bus) |
| D11 (GP11) | `R1` | **P2.01** | 14 | even-column R |
| D12 (GP12) | `G0` | **P2.02** | 15 | odd-column G |
| D13 (GP13) | `G1` | **P2.03** | 16 | even-column G |
| D14 (GP14) | `B0` | **P2.04** | 17 | odd-column B |
| D15 (GP15) | `B1` | **P2.05** | 18 | even-column B |
| D16 (GP16) | `VCOM` | **P2.07** | 20 | common; in-phase with `VB` |
| D17 (GP17) | `VDD1` 3.3 V | DK **VDDM 3.3 V** | 9 | binary driver + pixel memory; reads constant HIGH = rail present |
| meter only | `VDD2` 5 V | DK **5 V / VBUS** | 1 | gate driver — ⛔ **never to the Pico** (5 V) |
| Pico GND | `VSS` | DK **GND** | 10 | common ground: DK ↔ Pico ↔ panel |

### DK pin-allocation rationale & cautions

- **`P2.00–P2.05`** are the freed external-flash pins (the old ST7789 bus) — fast MCU
  domain, freed by the Board Configurator's *external-memory → GPIO* setting. The ST7789
  is unplugged for this epic, so all six are ours for the source data; **`P2.06`** (a
  trace pin, same fast domain) carries `BCK`.
- **COM on `P2.07/08/10`** (trace pins): the COM lines are a real **56–77 nF** capacitive
  load each. Whether a bare GPIO (even high-drive) can slew them inside the ≤100 µs
  rise/fall is an **L1 open question** — external buffering is the fallback. Drive
  strength is moot at L0 (COM held `Lo`).
- **Do NOT drive `P1.05`** — it's the J-Link VCOM's **host-driven** UART RX line; an
  output there contends with the interface MCU. It is the one P1 pin we leave alone.
- Reusing the UART/RTS-CTS pins (`P1.04/06/07`) is safe **this epic only** — no VCOM UART
  and no SD bus run during bring-up. The eventual custom PCB has none of these
  constraints; this map is for the DK bench, not the product.
- **`BTN0` (P1.13)** is read (input) as the L0 "start LA test" gate; LEDs/other buttons
  stay on their on-board functions. `LED0` (P2.09) is the heartbeat.

## Power

- **Two rails, logic levels `0 / 3.2 V`:**
  - `VDD1` = **3.2 V** (3.1–3.3) — horizontal/binary driver **and the pixel memory**.
  - `VDD2` = **5.0 V** (4.85–5.15) — gate driver.
- **The 3.3 V caveat.** We feed `VDD1` from the DK's **VDDM 3.3 V** rail (already
  configured 3.3 V for the ST7789 build, via the Board Configurator). 3.3 V sits at
  `VDD1`'s **absolute-max ceiling**; driving 3.3 V logic into a 3.3 V-rail panel keeps
  logic at-rail (no overdrive), but **watch current**, and a clean **3.2 V** bench supply
  is the fallback if levels/edges misbehave. `VDD2` ← the DK 5 V (VBUS).
- **Board Configurator (persisted to the interface MCU, one-time):** *VDD/VDDM → 3.3 V*
  must be applied (reused from the ST7789 build — see the crate README).
- **Boot state = all panel inputs `Lo`.** At first power-on every one of the 15 signal
  pins is driven `Lo` (the datasheet "Boot" condition). The L0 firmware does exactly this
  before any test signal — see below.

## Power-on / power-off / mode-change sequence

```
POWER-ON
  1. rails up: VDD1 (3.2 V) and VDD2 (5.0 V); all inputs Lo.
  2. INTB-framed all-black init: run ≥1 full frame with INTB high, COM held Lo
     (this clears pixel memory to black). INTB high ~53.67 ms during init only (chart).
  3. wait T4 ≥ 30 µs.
  4. raise + start COM: VCOM/VB (in phase) and VA (inverse) begin free-running
     (~60 Hz, ~50 % duty). COM now runs forever.
  5. normal operation: write frames (gate-scan + source-shift, MSB then LSB per row).

POWER-OFF (reverse)
  1. stop COM with VCOM/VB/VA returned Lo.
  2. drive all inputs Lo.
  3. rails down (VDD2 then VDD1).
```

## Vertical (frame) timing

- `GSP` pulses **once per frame** to load the first gate.
- `GCK` steps the **~640 gate sub-lines** of a frame (320 rows × {MSB sub-line, LSB
  sub-line}). `GCK(1)` high must fall **within** `GSP` high (chart).
- `GEN` opens the **valid output window** for the addressed gate line; `GCK`↔`GEN`
  setup/hold ≥ **16.37 µs**, `GEN` high ≥ **24.56 µs** (chart).
- `INTB` is held high (~53.67 ms) **only during the init all-black frame**, not in normal
  operation.
- `f_V` ≈ **18–18.9 Hz** (`t_V` ~53 ms).

## Horizontal (line/source) timing

- `BSP` starts a **sub-line**; then **120 `BCK`** clock the 240 columns (two pixels per
  `BCK`). `BCK(1)` high must fall **within** `BSP` high (chart).
- Per `BCK`: present the **odd** pixel on `R0/G0/B0` and the **even** pixel on `R1/G1/B1`.
- **Each row = two sub-lines:** the **MSB** sub-line (the 2/3-area blocks) then the **LSB**
  sub-line (the 1/3-area blocks).

## Timing budget (the numbers L1–L3 must hit)

| Parameter | Value | Source |
|---|---|---|
| `BCK` frequency | 0.746–0.758 MHz (≥660 ns hi / ≥660 ns lo) | spec |
| Source data setup / hold | ~335 ns each | spec |
| `BCK(1)` hi | within `BSP` hi | chart |
| `GCK` hi / lo (normal) | 83 µs each | spec |
| `GCK` hi / lo (fast-forward) | ≥1 µs | spec |
| `GCK(1)` hi | within `GSP` hi | chart |
| `GCK`↔`GEN` setup / hold | ≥16.37 µs | chart |
| `GEN` hi | ≥24.56 µs | chart |
| `f_VCOM` | 54–66 Hz, 48–52 % duty, rise/fall ≤100 µs | spec |
| `f_V` (frame) | 18–18.9 Hz | spec |

## RP2040 logic-analyzer harness

The rig we trust for L1–L4: an **RP2040 flashed with sigrok-pico**, driven by
**`sigrok-cli`** (CLI — captures export to CSV/VCD as text we read directly) with
**PulseView** optional for eyeballing. Our signals are sub-MHz, so a comfortable
**~8–10 MHz** sample rate resolves `BCK` edges (~10× oversample) and everything slower.

**Channel plan:** the sigrok-pico **baseline** firmware exposes **21 digital channels**
(`D2–D22` = Pico `GP2–GP22`), enough to watch all 15 signals + the `VDD1` rail in one
capture — the harness map (above) assigns `D2..D16` to the signals and `D17` to `VDD1`.

> **Tooling note (macOS):** real `sigrok-cli`/PulseView is awkward here — Homebrew's stable
> libsigrok lacks the `raspberrypi-pico` driver and the HEAD build needs a newer Xcode than
> is installed. Captures are driven with **pysigrok** (`pysigrok-cli`, pure-Python; its
> 0.3.1 Pico driver needs a one-line patch for the v2 firmware). `-O srzip -o foo.sr`
> writes a session file stable PulseView can open for eyeballing. (Output formats are
> `bits` + `srzip`; the `measure.py` / `walk_check.py` helpers parse `bits`.)

Example capture (all 15 signals + rail, bits to stdout; min sample rate is 5 kHz):

```sh
pysigrok-cli -d "raspberrypi-pico:conn=/dev/cu.usbmodemXXXX" \
  -C D2,D3,D4,D5,D6,D7,D8,D9,D10,D11,D12,D13,D14,D15,D16,D17 \
  -c samplerate=10000 --samples 100000 -O bits
```

## L0 bench firmware (`obc-fw-nrf54l`, `ls021_bringup` bin)

The L0 deliverable that validates the LA rig and the safe power-on state. Build/flash:

```sh
cd firmware/obc-fw-nrf54l
cargo run --release --bin ls021_bringup --features ls021-bringup
```

Behaviour, **panel-safe by construction**:

1. **Boot → safe state.** Sets the M33 to 128 MHz, then drives **all 15 panel signal
   pins `Output(Lo)`** — the datasheet boot condition. Logs the safe-state prompt over
   RTT, blinks `LED0`, and **waits for `BTN0`**. This is the quiescent window to meter
   `VDD1`/`VDD2` and idle current with every input `Lo`.
2. **`BTN0` → signal walk.** Pulses **one line at a time** across all 15 signals in the
   harness order (`GSP GCK GEN INTB VB VA BSP BCK R0 R1 G0 G1 B0 B1 VCOM` → Pico D2..D16),
   the other 14 held `Lo`. `BCK` is pulsed at its real ~0.75 MHz (LA-calibrated busy-loop:
   `asm::delay` ≈ 3.96 cyc/count on this M33 @128 MHz); the rest blink at ~12 Hz. Because
   only one line moves at a time and COM lines are pulsed only briefly, there is **no
   sustained DC bias** even with the panel plugged in. The analyzer sees each
   DK→Pico→panel mapping light up in its own slot, so a swap / open / short is immediately
   visible — the `walk_check.py` helper checks the order + the `VDD1` rail-present channel
   (D17). (The earlier 2-signal form validated `BCK`≈744 kHz / `GSP`≈60 Hz on the LA.)

L1 (below) keeps this same boot-safe hold + `BTN0` gate, then starts the always-on COM
driver; from L1 onward COM must free-run per the rule at the top.

## L1 COM driver (`obc-fw-nrf54l/src/ls021.rs`)

The first stage that actually drives a panel signal: the **free-running COM waveform**,
`VCOM`/`VB`/`VA` (issue [#141]). It is the safety-critical, always-on signal — it must
alternate the whole time the panel is powered, even on a static image, or the
Memory-in-Pixel cells take a DC bias and stick. Lives in `src/ls021.rs` as the `com_task`
(L2–L4 grow the gate/source primitives alongside it); the `ls021_bringup` bin brings it up
on the bench.

> **⚠️ Gotcha — PWM does NOT drive the COM pins on this part.** The textbook approach is a
> PWM peripheral (zero-CPU, glitch-free, one counter → three channels with `VA` inverted via
> the polarity bit). It compiles and `SequencePwm` reports the sequence running, but on the
> analyzer **`PWM20` leaves `P2.07/08/10` dead `Lo`** — the PWM output does not route onto
> that GPIO port here, even though a plain `gpio::Output` toggles the *same* pins cleanly
> (L0's signal-walk already proved that). **Lesson for L2+: don't assume a peripheral can
> reach these pins just because it compiles — `BCK`/`GCK`/source clocks may need GPIO or
> GPIOTE, not PWM/SPIM-PSEL, on `P2`.** Confirm on the analyzer.

So COM is the **GRTC/timer-backed GPIO square wave** the issue sanctions as the fallback —
`com_task` flips the three `gpio::Output`s and `await`s half a period:

| Line | Phase |
|---|---|
| `VCOM` | the reference square |
| `VB` | toggled identically to `VCOM` — **in phase** |
| `VA` | toggled opposite — **exact inverse** (boots `Lo`, raised on the first half-period) |

**Non-blocking.** `com_task` runs on a **high-priority `InterruptExecutor` pended from
SWI00 (P3)**, so the GRTC wakeup preempts thread-mode and COM never stalls behind a busy
thread-mode loop. The L1 bin proves it by spinning a blocking CPU busy-loop forever while
COM keeps toggling. The crossings are three back-to-back register writes (tens of ns apart,
far below the ~100 µs edge spec) — no meaningful overlap glitch.

**Numbers:** half-period `1 / 60 / 2 ≈ 8333 µs` → **60.0 Hz, 50 % duty**, inside the
datasheet `f_VCOM` 54–66 Hz / 48–52 % window.

**Drive + load.** Each COM line is a real **56–77 nF** load, so the three are configured
**high-drive (H0H1)** GPIO (~2.5 mA) to slew inside the ≤100 µs rise/fall. The bench scope
showed clean edges into the real panel load — **no external buffering needed** (revisit
this note if a later panel/cable shows rounding).

**Enable model (for L2).** The COM pins boot `Output(Lo)` and stay `Lo` until `com_task` is
spawned — that *is* the "COM held `Lo` during init" state the power-on sequence needs. L2
spawns it after the `T4 ≥ 30 µs` wait; the bin auto-starts it after a brief settle window so
the bench capture is hands-free.

**Verified on the analyzer** (`com_check.py`, capture `VCOM` D16 / `VB` D6 / `VA` D7):

| Check | Result |
|---|---|
| Frequency (54–66 Hz) | `VCOM`/`VB`/`VA` all **60.00 Hz** ✓ |
| Duty (48–52 %) | **50.1 % / 50.1 % / 49.9 %** ✓ |
| `VB` in phase with `VCOM` | **100 %** of samples agree ✓ |
| `VA` inverse of `VCOM` | **100 %** of samples disagree ✓ |
| `VCOM`/`VA` overlap glitch | 0.005 % (LA sampling at crossings) — **glitch-free** ✓ |
| Toggles through a CPU busy-loop | yes (interrupt executor) ✓ |
