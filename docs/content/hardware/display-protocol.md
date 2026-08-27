---
title: The display protocol
description: The LS021B7DD02 pixel format, scan sequence, polarity waveform, and timing limits.
copy: ai
---

# Display protocol

The device uses a Sharp **LS021B7DD02** reflective memory-in-pixel LCD. The panel has 240 × 320 pixels.

Each subpixel has a one-bit latch. The panel retains the image without scan traffic.

The framebuffer uses **RGB222**. Each channel has four levels, and each pixel has 64 possible colors.

The FLPR writes pixels through a six-bit parallel bus. The M33 generates the continuous polarity waveform.

## Signal paths

<figure class="fig">
<svg viewBox="0 0 720 300" role="img" aria-label="Two independent paths. On the left, the intermittent image-write path — gate scan and source shift — pushes one bit into a subpixel latch on the glass. On the right, the continuous polarity path — VCOM, VB in phase, VA inverse, free-running at about 60 Hz — drives the liquid crystal. The stored bit only selects which rail (VA for white, VB for black) the subpixel follows.">
  <defs>
    <marker id="a1" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="22">Two independent signal paths</text>

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
  <text class="d-sub" x="232" y="88" text-anchor="middle">write 1 bit</text>

  <!-- centre: the latch -->
  <rect class="d-hot" x="290" y="102" width="120" height="74" rx="12" style="fill:#f8efe4" />
  <text class="d-title" x="350" y="130" text-anchor="middle" style="fill:#a9501c">subpixel latch</text>
  <text class="d-sub" x="350" y="148" text-anchor="middle">one bit, on glass</text>
  <text class="d-sub" x="350" y="162" text-anchor="middle">held without a scan</text>
  <line class="d-flow" x1="410" y1="139" x2="494" y2="139" marker-end="url(#a1)" />
  <text class="d-sub" x="452" y="131" text-anchor="middle">selects</text>
  <text class="d-sub" x="452" y="154" text-anchor="middle">a rail</text>

  <!-- Path B: polarity -->
  <text class="d-tag" x="500" y="58">Path B · polarity — ~60 Hz</text>
  <text class="d-sub" x="500" y="92" text-anchor="start">VCOM</text>
  <path d="M540 80 H566 V96 H600 V80 H634 V96 H668 V80 H694" fill="none" stroke="#3c6b39" stroke-width="1.8" />
  <text class="d-sub" x="500" y="130" text-anchor="start">VB</text>
  <path d="M540 118 H566 V134 H600 V118 H634 V134 H668 V118 H694" fill="none" stroke="#3c6b39" stroke-width="1.8" />
  <text class="d-sub" x="500" y="168" text-anchor="start">VA</text>
  <path d="M540 172 H566 V156 H600 V172 H634 V156 H668 V172 H694" fill="none" stroke="#cf6a2a" stroke-width="1.8" />
  <text class="d-sub" x="500" y="200" style="font-size:10px">VB in phase with VCOM · VA inverse</text>
  <text class="d-sub" x="500" y="216" style="font-size:10px">→ drives the LC (never DC)</text>
</svg>
<figcaption>Path A writes the pixel latches. Path B continuously drives the liquid crystal at approximately 60 Hz. A stored bit selects the <code>VA</code> or <code>VB</code> rail.</figcaption>
</figure>

Do not stop `VCOM`, `VA`, or `VB` while the panel has power. A DC bias can damage the liquid crystal.

The hardware timer generates this waveform independently of the scan operation.

## Pixel encoding

The panel makes four levels with area gradation. Each subpixel contains a large block and a small block.

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
<figcaption>The connected MSB bands cover two-thirds of the subpixel. The LSB band covers one-third. The two bits give four visible levels.</figcaption>
</figure>

Each two-bit channel has this mapping:

| Level `l` | (MSB, LSB) | Lit area | Appears as |
|-----------|------------|----------|--------------------|
| 0 | (0, 0) | 0 | black |
| 1 | (0, 1) | 1/3 | LSB / middle band |
| 2 | (1, 0) | 2/3 | MSB / top + bottom |
| 3 | (1, 1) | 3/3 | full (channel on) |

Use `msb = (l >> 1) & 1` and `lsb = l & 1`. The packer does not need a lookup table.

### Source-bus interleave

The six data lines carry one bit for each channel of two adjacent pixels.

| Lines | Pixel | Channel |
|---------------------|----------------|-----------|
| `R[0]` `G[0]` `B[0]` | **even** column | R / G / B |
| `R[1]` `G[1]` `B[1]` | **odd** column | R / G / B |

Each `BCK` edge clocks one pixel pair. Thus, 120 pairs require 60 `BCK` cycles.

Present a new pair before each rising and falling edge. Do not hold one pair for a complete cycle.

## Gate scan

The display has one gate line for each of its 320 rows. Write both area planes to the same gate line.

- `GCK` HIGH selects the two-thirds MSB block.
- `GCK` LOW selects the one-third LSB block.

One `GCK` period writes one row. The rising edge advances the gate. The falling edge keeps the same gate selected.

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
<figcaption>Send the MSB plane while <code>GCK</code> is high. Send the LSB plane while <code>GCK</code> is low. Pulse <code>GEN</code> after each plane.</figcaption>
</figure>

## Writing a frame

A frame scans the 320 rows. Keep `INTB` high for the complete scan.

```text
INTB = HIGH
GSP = pulse
send two leading dummy gate periods
for each row:
    set GCK HIGH
    shift the MSB plane
    pulse GEN
    set GCK LOW
    shift the LSB plane
    pulse GEN
send six trailing dummy gate periods
INTB = LOW
```

- `INTB` LOW holds the current image.
- One `GCK` period advances one row.
- Each row uses two `GEN` pulses.
- Each plane uses 120 data words and four dummy words.
- Pulse `GSP` at the start. Release it on the first `GCK` edge.

### Partial update

The presenter compares row hashes and sends changed row spans to the FLPR. The FLPR advances past unchanged rows without `GEN` pulses.

The FLPR writes all 240 columns of each changed row. It stops after the last changed row.

Thus, the scan time depends mainly on the number and position of changed rows.

## Power-on, power-off, and retention

- During power-on, write a black frame. Wait at least 30 µs, and then start the COM waveform.
- During power-off, write the safe state before you stop the COM waveform.
- Refresh a static image at least once every two days.

## Signal & timing reference

### Pins

| Signal | Group | Rail | Role |
|--------|-------|------|------|
| `GSP`  | Gate | VDD2 (~5.0 V) | Gate start pulse — once per frame |
| `GCK`  | Gate | VDD2 | Gate clock **and** MSB/LSB phase select (HIGH = MSB/⅔, LOW = LSB/⅓); one period per row |
| `GEN`  | Gate | VDD2 | Gate output enable — pulsed once per phase to latch the selected block |
| `INTB` | Gate | VDD2 | Frame envelope — HIGH for the whole frame; LOW = Hold (no write) |
| `BSP`  | Source | VDD1 (~3.2 V) | Sub-line (start-of-line) pulse |
| `BCK`  | Source | VDD1 | Source shift clock; 124 edge words per plane |
| `R[0]` `G[0]` `B[0]` | Source data | VDD1 | Even-column R/G/B bit |
| `R[1]` `G[1]` `B[1]` | Source data | VDD1 | Odd-column R/G/B bit |
| `VCOM` | COM | — | Common AC reference (free-running ~60 Hz) |
| `VB`   | COM | — | **In phase** with `VCOM` — the "black" rail |
| `VA`   | COM | — | **Inverse** of `VCOM` — the "white" rail |

### Timing

| Parameter | Value | Notes |
|-----------|-------|-------|
| `BCK` max frequency | 0.758 MHz | Source/shift clock ceiling |
| `BCK` minimum high / low | 660 ns each | Datasheet limit |
| COM frequency | 54–66 Hz (60 typ) | 48–52 % duty |
| COM edge (rise/fall) | ≤ 100 µs | Into ~56–77 nF per line; high-drive GPIO / buffer as needed |
| `GCK` ↔ `GEN` setup & hold | ≥ 16.37 µs | Keep `GEN` clear of `GCK` edges |
| `GEN` high width | ≥ 24.56 µs | Valid-output / latch window |
| Power-on settle (black → COM) | ≥ 30 µs | See power-on order above |
| Current full-frame scan | 44.1 ms | Measured on one development panel |

The current FLPR uses approximately 210 ns for each `BCK` half-period.
This gives a 420 ns period and approximately 2.38 MHz.
It exceeds the 0.758 MHz maximum and violates the 660 ns minimum high and low times.

This timing passed a visual test on one panel at room temperature. It is not production validation. See the [timing record](src:firmware/docs/flpr-timing.md).

## Power

The datasheet gives these typical module values. The values exclude COM capacitive-load current.

| State | Power |
|-------|------:|
| Hold / static | ≈ 32 µW |
| 1 Hz update | ≈ 45 µW |
| 30 Hz update | ≈ 290 µW |

Power increases with the update rate. Partial updates reduce the number of rows that the FLPR writes.

## Implementation

- COM driver: [`com.rs`](src:firmware/obc-fw-nrf54l/src/com.rs) and [`com_hw.rs`](src:firmware/obc-fw-nrf54l/src/com_hw.rs)
- FLPR scan: [`flpr_scan.c`](src:firmware/obc-fw-nrf54l/src/flpr/flpr_scan.c)
- Wire packer: [`wire.rs`](src:firmware/obc-display/src/ls021/wire.rs)
- Presenter: [Rendering pipeline](../../software/rendering/)
