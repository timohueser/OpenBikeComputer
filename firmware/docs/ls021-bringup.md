# LS021B7DD02 panel bring-up — protocol & DK pin spec (STATUS: L0 LANDED)

The **normative reference** for the Sharp **LS021B7DD02** bring-up on the nRF54L15-DK,
driven **M33-direct** (no FLPR yet). Epic [#139]; this doc is the deliverable of
**L0 [#140]** and the spec the firmware of L1–L4 is written against:

- **L1 [#141]** — free-running COM driver (`VCOM`/`VB`/`VA`, ~60 Hz).
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
> stays `Lo`); the COM driver is L1, and it is *always-on* from L1 onward.

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

21-pin FPC: 15 signals to drive + 2 rails + GND (+ NC). The **DK pin** column is the L0
proposal, grounded in the nRF54L15-DK connector table; the **FPC #** column is left to
confirm against the datasheet pin-assignment page and the breakout silkscreen during the
L0 continuity check — fill it in there.

| Group | Signal | Dir | FPC # | DK pin | Notes |
|---|---|---|---|---|---|
| Source | `BCK` | in | _tbd_ | **P2.06** | source/shift clock ~0.75 MHz; P2 MCU-fast |
| Source data (odd) | `R0` `G0` `B0` | in | _tbd_ | **P2.00 P2.02 P2.04** | odd-column pixel |
| Source data (even) | `R1` `G1` `B1` | in | _tbd_ | **P2.01 P2.03 P2.05** | even-column pixel |
| Source | `BSP` | in | _tbd_ | **P1.07** | sub-line start pulse (UART1_CTS — free, HWFC off) |
| Gate | `GSP` | in | _tbd_ | **P1.11** | frame/gate start pulse (free P1) |
| Gate | `GCK` | in | _tbd_ | **P1.12** | gate clock, steps sub-lines (free P1) |
| Gate | `GEN` | in | _tbd_ | **P1.04** | gate output enable (UART1_TXD — device-driven, safe) |
| Init | `INTB` | in | _tbd_ | **P1.06** | all-black init framing (UART1_RTS — free, HWFC off) |
| COM | `VCOM` | in | _tbd_ | **P2.07** | common; in-phase with `VB` |
| COM | `VB` | in | _tbd_ | **P2.08** | COM, in-phase with `VCOM` (may strap to `VCOM`) |
| COM | `VA` | in | _tbd_ | **P2.10** | COM, **inverse** phase |
| Power | `VDD2` | — | _tbd_ | **DK 5 V / VBUS** | 5.0 V (4.85–5.15) gate driver |
| Power | `VDD1` | — | _tbd_ | **DK VDDM 3.3 V** | 3.2 V (3.1–3.3) binary driver + pixel memory |
| Power | `VSS` | — | _tbd_ | **GND** | |

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

**Channel-grouping plan** (the simple LA firmwares expose ~8 channels; we have ~14 lines
to watch, so capture in two passes):

1. **Clocks + control first:** `BCK`, `BSP`, `GSP`, `GCK`, `GEN`, `INTB` (+ a COM line at
   L1). This is the protocol skeleton — phase/order/timing.
2. **Data lines next:** `R0 R1 G0 G1 B0 B1` — pixel content, once the skeleton checks out.

Example capture (sub-MHz, two channels, CSV to stdout):

```sh
sigrok-cli --driver sigrok-pico:conn=/dev/tty.usbmodemXXXX \
  --channels D0=BCK,D1=GSP --config samplerate=8m --samples 2000000 -O csv
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
2. **`BTN0` → LA test.** A cycle-counted busy-loop toggles **two logic lines only** —
   `BCK` (P2.06) at ~0.75 MHz and `GSP` (P1.11) at ~60 Hz — with the other 13 pins held
   `Lo`. **COM is never toggled**, so there is no DC bias and no pixel latch even with the
   panel plugged in. The two pins are intentionally chosen to validate one fast P2 pin +
   one P1 control pin (i.e. both channel groups). Frequencies are approximate by design —
   the LA *measures* the real values; the goal is "sigrok reads both cleanly."

The next stage (L1) replaces this with the always-on COM driver; from L1 onward COM must
free-run per the rule at the top.
