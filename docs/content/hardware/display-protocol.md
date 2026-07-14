---
title: The display protocol
description: How the firmware drives the LS021B7DD02 reflective memory-in-pixel LCD — the two independent signal paths, RGB222 area-gradation pixels, the one-gate-line/two-area-block write, the frame algorithm, and the signal/timing reference.
---

# Driving the display

The device's screen is a Sharp **LS021B7DD02** — a 2.13″, **240 × 320** reflective **memory-in-pixel (MIP)** LCD. "Memory-in-pixel" is the whole story: **every subpixel has a 1-bit latch on the glass.** Write a bit and the panel holds that image with *zero* bus traffic — you only spend power when the picture actually changes, which is exactly what an always-on, battery-bound device wants.

Colour is **RGB222** — 2 bits per channel → **64 colours** — and the panel is **normally black** (an unwritten, all-zero panel is dark). Two facts about *how you talk to it* surprise most people and shape everything below:

1. **The interface is parallel, not SPI.** Pixel data goes out on a **6-bit parallel source bus** plus a handful of control lines.
2. **The stored bits don't drive the liquid crystal directly.** A separate, *continuously running* AC waveform does; the stored bit only chooses which way each subpixel leans.

This page is the protocol — conceptual model first, then the mechanical detail and the signal reference. The firmware that implements it is split three ways: the M33 [COM-waveform driver](src:firmware/obc-fw-nrf54l/src/com.rs), the [FLPR scan blob](src:firmware/obc-fw-nrf54l/src/flpr/flpr_scan.c) that clocks the gate and source buses, and the host-tested [wire-format packer](src:firmware/obc-platform/src/ls021/wire.rs).

## Two signal paths that never meet

The single most useful way to think about this panel is that **two unrelated jobs run on two different sets of pins, and they don't talk to each other.**

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="Two independent paths. On the left, the intermittent image-write path — gate scan and source shift — pushes one bit into a subpixel latch on the glass. On the right, the continuous polarity path — VCOM, VB in phase, VA inverse, free-running at about 60 Hz — drives the liquid crystal. The stored bit only selects which rail (VA for white, VB for black) the subpixel follows.">
  <defs>
    <marker id="a1" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="22">Two signal paths — they don't talk to each other</text>

  <!-- Path A: write -->
  <text class="d-tag" x="30" y="58">Path A · image write — intermittent</text>
  <rect class="d-panel-2" x="30" y="72" width="160" height="46" rx="9" />
  <text class="d-label" x="110" y="92" text-anchor="middle">gate scan</text>
  <text class="d-sub" x="110" y="108" text-anchor="middle">GSP · GCK · GEN · INTB</text>
  <rect class="d-panel-2" x="30" y="130" width="160" height="46" rx="9" />
  <text class="d-label" x="110" y="150" text-anchor="middle">source shift</text>
  <text class="d-sub" x="110" y="166" text-anchor="middle">BSP · BCK · R/G/B[0:1]</text>
  <line class="d-flow" x1="190" y1="96" x2="286" y2="122" marker-end="url(#a1)" />
  <line class="d-flow" x1="190" y1="153" x2="286" y2="140" marker-end="url(#a1)" />
  <text class="d-sub" x="232" y="106" text-anchor="middle">write 1 bit</text>

  <!-- centre: the latch -->
  <rect class="d-hot" x="290" y="102" width="120" height="74" rx="12" style="fill:#f8efe4" />
  <text class="d-title" x="350" y="130" text-anchor="middle" style="fill:#a9501c">subpixel latch</text>
  <text class="d-sub" x="350" y="148" text-anchor="middle">one bit, on glass</text>
  <text class="d-sub" x="350" y="162" text-anchor="middle">held with no power</text>
  <line class="d-flow" x1="410" y1="139" x2="494" y2="139" marker-end="url(#a1)" />
  <text class="d-sub" x="452" y="131" text-anchor="middle">selects</text>
  <text class="d-sub" x="452" y="154" text-anchor="middle">a rail</text>

  <!-- Path B: polarity -->
  <text class="d-tag" x="500" y="58">Path B · polarity — continuous ~60 Hz</text>
  <text class="d-sub" x="500" y="92" text-anchor="start">VCOM</text>
  <path d="M540 80 H566 V96 H600 V80 H634 V96 H668 V80 H694" fill="none" stroke="#3c6b39" stroke-width="1.8" />
  <text class="d-sub" x="500" y="130" text-anchor="start">VB</text>
  <path d="M540 118 H566 V134 H600 V118 H634 V134 H668 V118 H694" fill="none" stroke="#3c6b39" stroke-width="1.8" />
  <text class="d-sub" x="500" y="168" text-anchor="start">VA</text>
  <path d="M540 172 H566 V156 H600 V172 H634 V156 H668 V172 H694" fill="none" stroke="#cf6a2a" stroke-width="1.8" />
  <text class="d-sub" x="540" y="200" style="font-size:10px">VB in phase with VCOM · VA the exact inverse → drives the LC (never DC)</text>
</svg>
<figcaption><b>Path A</b> (the gate scan + source shift) pushes one bit per subpixel into the on-glass latches — it runs only while you update the image, then goes quiet and the picture is retained. <b>Path B</b> (<code>VCOM</code>/<code>VA</code>/<code>VB</code>) is a free-running ~60 Hz square wave that physically drives the liquid crystal; the stored bit just routes each subpixel to follow the <code>VA</code> rail (→ white) or the <code>VB</code> rail (→ black).</figcaption>
</figure>

The consequence for firmware is that **COM is its own concern.** The liquid crystal must **never see a DC bias** — a steady field degrades the cells — so `VCOM`/`VA`/`VB` have to keep alternating the *entire* time the panel is powered, even on a perfectly static image. Generate them on a hardware timer or dedicated task that runs autonomously and never stalls behind the image write. The write path may be slow and bursty; the polarity path cannot pause.

## Pixels: RGB222 by area gradation

Each channel carries **2 bits**, but the panel has no analog grey — it fakes four levels per channel with **area gradation.** Every subpixel is physically split into two 1-bit on/off blocks of different size:

<figure class="fig">
<svg viewBox="0 0 720 280" role="img" aria-label="One pixel cell drawn as a three by three grid: R, G, B columns by three stacked bands — top MSB, middle LSB, bottom MSB. The top and bottom MSB bands are wired together and form two thirds of the area; the middle LSB band is one third.">
  <text class="d-tag" x="20" y="22">One pixel cell — three stacked bands</text>

  <!-- column headers -->
  <text class="d-label" x="300" y="52" text-anchor="middle" style="fill:#a9501c">R</text>
  <text class="d-label" x="372" y="52" text-anchor="middle" style="fill:#3c6b39">G</text>
  <text class="d-label" x="444" y="52" text-anchor="middle" style="fill:#33575b">B</text>

  <!-- 3 columns x 3 rows; cell 72 wide; rows: 60/60/60 -->
  <!-- MSB top -->
  <rect x="264" y="62" width="72" height="60" style="fill:#e6c2b3;stroke:#3c6b39;stroke-width:1.2" />
  <rect x="336" y="62" width="72" height="60" style="fill:#cadcb6;stroke:#3c6b39;stroke-width:1.2" />
  <rect x="408" y="62" width="72" height="60" style="fill:#b6cdd3;stroke:#3c6b39;stroke-width:1.2" />
  <!-- LSB middle -->
  <rect x="264" y="122" width="72" height="60" style="fill:#eed6cc;stroke:#3c6b39;stroke-width:1.2" />
  <rect x="336" y="122" width="72" height="60" style="fill:#dde9cf;stroke:#3c6b39;stroke-width:1.2" />
  <rect x="408" y="122" width="72" height="60" style="fill:#cfdee2;stroke:#3c6b39;stroke-width:1.2" />
  <!-- MSB bottom -->
  <rect x="264" y="182" width="72" height="60" style="fill:#e6c2b3;stroke:#3c6b39;stroke-width:1.2" />
  <rect x="336" y="182" width="72" height="60" style="fill:#cadcb6;stroke:#3c6b39;stroke-width:1.2" />
  <rect x="408" y="182" width="72" height="60" style="fill:#b6cdd3;stroke:#3c6b39;stroke-width:1.2" />

  <!-- band labels inside -->
  <text class="d-sub" x="372" y="96" text-anchor="middle">MSB band</text>
  <text class="d-sub" x="372" y="156" text-anchor="middle">LSB band</text>
  <text class="d-sub" x="372" y="216" text-anchor="middle">MSB band</text>

  <!-- annotations: MSB (top + bottom) and LSB (middle) -->
  <line x1="480" y1="92" x2="500" y2="116" style="stroke:#9aa884;stroke-width:1.3" />
  <line x1="480" y1="212" x2="500" y2="116" style="stroke:#9aa884;stroke-width:1.3" />
  <text class="d-label" x="506" y="112">MSB plane</text>
  <text class="d-sub" x="506" y="128">2/3 area · top + bottom</text>
  <line x1="480" y1="152" x2="500" y2="172" style="stroke:#cf6a2a;stroke-width:1.3" />
  <text class="d-label" x="506" y="174" style="fill:#a9501c">LSB plane</text>
  <text class="d-sub" x="506" y="190">1/3 area · middle</text>
</svg>
<figcaption>Physically a pixel cell is three stacked bands — <b>MSB (top) · LSB (middle) · MSB (bottom)</b>. The two MSB bands are wired together into one block covering <b>2/3</b> of the area; the middle band is the <b>1/3</b> LSB block. Turning a channel's MSB and LSB bits on/off lights 0, 1/3, 2/3, or the full area — four perceived levels per channel.</figcaption>
</figure>

A channel's 2-bit level therefore maps to a (MSB, LSB) pair and a lit area:

| Level `l` | (MSB, LSB) | Lit area | Appears as |
|-----------|------------|----------|--------------------|
| 0 | (0, 0) | 0 | black |
| 1 | (0, 1) | 1/3 | LSB / middle band |
| 2 | (1, 0) | 2/3 | MSB / top + bottom |
| 3 | (1, 1) | 3/3 | full (channel on) |

Bit extraction from a level is just `msb = (l >> 1) & 1`, `lsb = l & 1` — so an RGB222 framebuffer maps onto the wire with no lookup.

### The source bus is odd/even interleaved

The 6 data lines carry **two adjacent pixels at once** — one bit each for the odd and the even column:

| Lines | Pixel | Channel |
|---------------------|----------------|-----------|
| `R[0]` `G[0]` `B[0]` | **odd** column  | R / G / B |
| `R[1]` `G[1]` `B[1]` | **even** column | R / G / B |

So each `BCK` *edge* clocks **one pixel pair** (two columns) — the panel latches the source bus on **both** edges of `BCK` (DDR). Drive a *distinct* pair on the rising edge and the next pair on the falling edge, and the 240 columns ship in **60 `BCK` cycles** of data per sub-line.

> **Both edges, learned the hard way.** It's natural to assume one pair per `BCK` *cycle* (120 cycles for 240 columns) and hold the data steady across the whole period. Do that and the panel captures each pair *twice* — every pair lands in four columns, so the left half of the frame stretches to fill the screen, the right half drops, and the 64-colour gamut collapses to 32. It's invisible on solid fills (uniform either way), which is why it can hide for a long time; fine vertical detail (1-px lines, text) is what exposes it. The fix is to feed a new pair on each edge (DDR), which also clocks the line out ~2× faster — and is why the panel's ~53 ms full-frame spec is actually achievable at the rated `BCK`.

## One gate line, two area blocks

Here is the part the datasheet hides in its charts, and the key to the whole protocol:

> **A pixel row is ONE gate line.** The display has exactly **320 gate lines (`L1…L320`), one per pixel row.** The "three bands" above are the area layout *inside a single cell* — **not** three separate gate lines.

To write a row you transfer the MSB data **and** the LSB data into that **same** gate line. The only thing that routes a latch to the 2/3 (MSB) block versus the 1/3 (LSB) block is the **level of `GCK`**:

- **`GCK` HIGH → MSB phase** → the latch writes the **2/3-area** cells.
- **`GCK` LOW → LSB phase** → the latch writes the **1/3-area** cells.

There is **no separate MSB/LSB select pin** — `GCK` level *is* the area-block selector. So one pixel row is **one `GCK` period**: a rising edge that both advances the gate and opens the MSB phase, then a falling edge that holds the same row for the LSB phase.

<figure class="fig">
<svg viewBox="0 0 720 270" role="img" aria-label="A timeline for one pixel row as one GCK period. GCK rises to advance to the row and open the MSB phase; while high, the MSB bit-plane is shifted in and a GEN pulse latches the two-thirds block. GCK then falls for the LSB phase on the same row; while low, the LSB bit-plane is shifted in and a second GEN pulse latches the one-third block. The next rising edge advances to the next row.">
  <text class="d-tag" x="20" y="22">One pixel row = one GCK period</text>

  <!-- GCK -->
  <text class="d-sub" x="34" y="92" text-anchor="start">GCK</text>
  <path d="M80 122 H150 V70 H360 V122 H570 V70 H694" fill="none" stroke="#3c6b39" stroke-width="2" />
  <!-- phase labels -->
  <text class="d-label" x="255" y="98" text-anchor="middle" style="fill:#a9501c">MSB phase · GCK HIGH</text>
  <text class="d-sub" x="255" y="114" text-anchor="middle">shift MSB plane → 2/3 block</text>
  <text class="d-label" x="465" y="98" text-anchor="middle" style="fill:#a9501c">LSB phase · GCK LOW</text>
  <text class="d-sub" x="465" y="114" text-anchor="middle">shift LSB plane → 1/3 block</text>

  <!-- GEN -->
  <text class="d-sub" x="34" y="196" text-anchor="start">GEN</text>
  <path d="M80 200 H220 V172 H260 V200 H430 V172 H470 V200 H694" fill="none" stroke="#cf6a2a" stroke-width="2" />
  <text class="d-sub" x="240" y="222" text-anchor="middle">latch 2/3</text>
  <text class="d-sub" x="450" y="222" text-anchor="middle">latch 1/3</text>

  <!-- edge annotations -->
  <line class="d-stroke" x1="150" y1="70" x2="150" y2="44" style="stroke:#9aa884" />
  <text class="d-sub" x="150" y="38" text-anchor="middle">advance + MSB</text>
  <line class="d-stroke" x1="570" y1="70" x2="570" y2="44" style="stroke:#9aa884" />
  <text class="d-sub" x="570" y="38" text-anchor="middle">next row</text>
  <text class="d-sub" x="360" y="250" text-anchor="middle" style="font-size:11px">one gate advance per pixel row · two GEN pulses per row</text>
</svg>
<figcaption>A row's data is two <b>bit-planes on the same 6 wires</b>: the MSB plane goes out while <code>GCK</code> is high (latched into the 2/3 block by a <code>GEN</code> pulse), then the LSB plane while <code>GCK</code> is low (latched into the 1/3 block). The gate advances on the <code>GCK</code> rising edge, so a single gate line stays selected across both phases of its period.</figcaption>
</figure>

## Writing a frame

A frame is bracketed by `INTB` and is a gate scan over the 320 rows. Two non-obvious rules are called out in the pseudocode:

```
# RULE 1: INTB must be HIGH for the whole frame, or nothing latches.
INTB = HIGH
GSP  = pulse                          # frame start; loads the first gate
leading dummy gate advances           # pipeline fill (a few GCK periods, no GEN/BCK);
                                      # release GSP on the very first GCK edge
for each of 320 pixel rows:           # one GCK PERIOD per row — RULE 2
    GCK = HIGH                        #   rising edge advances the gate to this row
    shift MSB plane  (BSP + 60 DDR BCK) #  2/3-area data, a pair per BCK edge
    GEN = pulse                       #   latch the 2/3 (MSB) block  [GCK still HIGH]
    GCK = LOW                         #   same gate line, NOT an advance
    shift LSB plane  (BSP + 60 DDR BCK) #  1/3-area data
    GEN = pulse                       #   latch the 1/3 (LSB) block  [GCK LOW]
trailing dummy gate advances          # flush / "necessary signal" blank
INTB = LOW                            # end of frame; panel now holds the image
```

What the counts mean:

- **`INTB` HIGH = "write enabled" for the whole frame.** `INTB` LOW is the inter-frame **Hold** state: the panel ignores the gate/source scan and keeps its current memory.
- **One `GCK` *period* per pixel row → 320 gate advances**, not 640. The vertical-timing chart shows ~640 `GCK` marks because those are **edges** (320 periods × 2 phases); with the bracketing dummies it totals ~648 edges.
- **Two `GEN` pulses per row** — one in the MSB (high) phase, one in the LSB (low) phase.
- **120 pixel-pair columns per sub-line** + a few dummy/flush columns that push the last columns through the source shift register. Because the panel is **DDR** (a pair per `BCK` *edge*, see above), those 120 columns are clocked in **~60 `BCK` cycles**, not 120.
- **`GSP`** is pulsed once at frame start and released on the first `GCK` edge so its high overlaps `GCK(1)`.

### Partial update — a span-masked gate scan

Because pixel memory is retained you do **not** have to rewrite the whole frame, and the firmware doesn't. The FLPR backend drives a **span-masked scan**: given the row-spans that changed, it fast-forwards `GCK` over every clean row — `GEN` inactive, so nothing latches — does the shift-and-latch work only on the dirty rows, and **stops early** after the last one (drop `INTB`; the panel holds everything below). A skipped row costs one fast `GCK` advance instead of two full sub-line writes, so a frame scales with the number of *changed rows*, not a flat 320.

The grain is a **whole row**, though: touching any row re-latches all 240 of its columns — the source shift register feeds the entire line — so there is no cheap "just these few columns." A partial update is a set of full-width row-spans; a 16-px-wide right-edge overlay still rewrites its rows full-width, and is cheap only because it touches *few rows*, not few columns.

The hold-progress bulge rides exactly this: as it animates, only its rows re-push — a fraction of a full-frame scan — while the rest of the map stays untouched and asleep. And the **map present** rides it too: it keeps a per-row hash of the last-pushed frame and feeds the masked scan only the rows that actually changed, so an idle Home clock ticking a minute repaints its clock band and nothing else — no per-screen code, the change detected automatically in the present layer. It's the renderer's [redraw-only-what-changed](../../software/rendering/) design carried onto the glass, and for a UI that changes a few fields per second, a large power win.

## Power-on, power-off, and retention

The never-DC rule (Path B) dictates a strict order:

- **Power-on:** write an **all-black frame first** (Path A), *then* start the `VCOM`/`VA`/`VB` waveform (Path B), with a small settle (datasheet ≥ 30 µs) before COM begins. The panel is now in a known, bias-free state before any AC drive.
- **Power-off:** **reverse it** — bring the content to the safe state before stopping COM. Never cut COM while a biased image is held.
- **Retention:** the image holds indefinitely for practical purposes, but refresh it at least every **~2 days** rather than holding a single static frame longer than that.

## Signal & timing reference

### Pins

| Signal | Group | Rail | Role |
|--------|-------|------|------|
| `GSP`  | Gate | VDD2 (~5.0 V) | Gate start pulse — once per frame |
| `GCK`  | Gate | VDD2 | Gate clock **and** MSB/LSB phase select (HIGH = MSB/⅔, LOW = LSB/⅓); one period per row |
| `GEN`  | Gate | VDD2 | Gate output enable — pulsed once per phase to latch the selected block |
| `INTB` | Gate | VDD2 | Frame envelope — HIGH for the whole frame; LOW = Hold (no write) |
| `BSP`  | Source | VDD1 (~3.2 V) | Sub-line (start-of-line) pulse |
| `BCK`  | Source | VDD1 | Source shift clock — 124 per sub-line (120 data + 4 dummy) |
| `R[0]` `G[0]` `B[0]` | Source data | VDD1 | Odd-column R/G/B bit |
| `R[1]` `G[1]` `B[1]` | Source data | VDD1 | Even-column R/G/B bit |
| `VCOM` | COM | — | Common AC reference (free-running ~60 Hz) |
| `VB`   | COM | — | **In phase** with `VCOM` — the "black" rail |
| `VA`   | COM | — | **Inverse** of `VCOM` — the "white" rail |

### Timing (datasheet values are the envelope, not a target)

| Parameter | Value | Notes |
|-----------|-------|-------|
| `BCK` max frequency | 0.758 MHz | Source/shift clock ceiling |
| `BCK` min high / low | 660 ns each | Honour as minimum widths |
| COM frequency | 54–66 Hz (60 typ) | 48–52 % duty |
| COM edge (rise/fall) | ≤ 100 µs | Into ~56–77 nF per line; high-drive GPIO / buffer as needed |
| `GCK` ↔ `GEN` setup & hold | ≥ 16.37 µs | Keep `GEN` clear of `GCK` edges |
| `GEN` high width | ≥ 24.56 µs | Valid-output / latch window |
| Power-on settle (black → COM) | ≥ 30 µs | See power-on order above |
| Full-frame update | ~18 Hz (~55.6 ms) | At datasheet-nominal clocking |

A bring-up driver may deliberately clock far *slower* than these maxima (e.g. `BCK` in the low-hundreds-of-kHz) so a logic analyzer resolves every edge — that's fine; the table is the envelope. The minimum widths and the `GCK`↔`GEN` setup/hold are the relationships that actually matter for correct latching.

## Power — the figures

This is why the panel suits an always-on device. Typical module power (datasheet, **excludes** the COM capacitive-load current):

| State | Power |
|-------|------:|
| Hold / static | ≈ 32 µW |
| 1 Hz update | ≈ 45 µW |
| 30 Hz update | ≈ 290 µW |

The panel itself is nearly free at rest; cost scales with how often you rewrite — which is exactly why the partial-update strategy above matters.

---

## Where this lives

- The COM polarity waveform on the M33 — and its zero-CPU [`TIMER→DPPI→GPIOTE`](src:firmware/obc-fw-nrf54l/src/com_hw.rs) variant: [`obc-fw-nrf54l/src/com.rs`](src:firmware/obc-fw-nrf54l/src/com.rs)
- The gate/source scan that writes a frame — the [`FLPR`](src:firmware/obc-fw-nrf54l/src/ls021_flpr.rs) coprocessor blob: [`flpr/flpr_scan.c`](src:firmware/obc-fw-nrf54l/src/flpr/flpr_scan.c)
- The host-tested wire-format pack (the area-gradation bit packing this page describes): [`obc-platform/src/ls021/wire.rs`](src:firmware/obc-platform/src/ls021/wire.rs)
- How a rendered frame reaches the panel — the banded push and the RGB222 framebuffer: [rendering pipeline](../../software/rendering/)
- The hardware overview: [Hardware](../)
