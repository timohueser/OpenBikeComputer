---
title: The display protocol
description: How the reflective memory LCD holds an image and how the device writes one.
copy: ai
---

# Display protocol

The device uses a Sharp LS021B7DD02 reflective memory-in-pixel LCD of 240 × 320 pixels. Every
subpixel has a one-bit latch on the glass, so the panel keeps its image with no scan traffic. That
is the property the device is built around: a map left on the screen costs almost nothing. The
framebuffer is RGB222, which is four levels per channel and 64 colors.

## Two signal paths

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
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
  <text class="d-sub" x="500" y="200" style="font-size:12px">VB in phase with VCOM · VA inverse</text>
  <text class="d-sub" x="500" y="216" style="font-size:12px">→ drives the LC (never DC)</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Path A writes the pixel latches. Path B continuously drives the liquid crystal at approximately 60 Hz. A stored bit selects the <code>VA</code> or <code>VB</code> rail.</figcaption>
</figure>

Writing an image and driving the liquid crystal are separate. The write is intermittent and pushes
one bit into each subpixel latch. The polarity waveform is continuous, and the stored bit selects
which rail the subpixel follows.

Never stop that waveform while the panel has power: a liquid crystal held at a DC bias is damaged.
A hardware timer generates it, so a busy or sleeping processor cannot interrupt it.

## Pixel encoding

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
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
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The connected MSB bands cover two-thirds of the subpixel. The LSB band covers one-third. The two bits give four visible levels.</figcaption>
</figure>

The panel makes four levels per channel by area: each subpixel has a large block and a small block,
two thirds and one third of its area. The two bits of a channel are those two blocks, so a level
needs no lookup table.

## Gate scan

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
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
  <text class="d-sub" x="360" y="250" text-anchor="middle" style="font-size:12px">one gate advance per pixel row · two GEN pulses per row</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Send the MSB plane while <code>GCK</code> is high. Send the LSB plane while <code>GCK</code> is low. Pulse <code>GEN</code> after each plane.</figcaption>
</figure>

A row is written in one clock period, in two phases: the larger block while the clock is high and
the smaller while it is low, with a latch pulse after each. One period advances one row. A frame
raises the envelope signal, writes the rows, and drops it again; with the envelope low the panel
holds what it has.

### Partial update

The presenter compares row hashes and sends only the changed row spans. The scan advances past
unchanged rows without latching them and stops after the last changed row, so a frame costs what
changed and not what the screen holds. That is how a clock digit costs a few rows.

## Power-on, power-off, and retention

Write a black frame at power-on, settle, then start the waveform. Write the safe state at power-off
before the waveform stops. Refresh a static image at least every two days.

## Timing

The signal names, their rails, and every timing limit are the panel datasheet's, and the
implementation holds them in one place: the [wire packer](src:firmware/obc-display/src/ls021/wire.rs)
builds the bus words, the [COM driver](src:firmware/obc-fw-nrf54l/src/com.rs) makes the polarity
waveform, and the [scan program](src:firmware/obc-fw-nrf54l/src/flpr/flpr_scan.c) runs on the
coprocessor that writes the panel.

The current scan clocks the source bus faster than the datasheet maximum. It passed a visual test
on one panel at room temperature, which is not production validation.

Panel power follows the update rate, and partial updates are what keep the average near the cost of
holding an image.

## Implementation

- COM driver: [`com.rs`](src:firmware/obc-fw-nrf54l/src/com.rs), [`com_hw.rs`](src:firmware/obc-fw-nrf54l/src/com_hw.rs)
- Scan program: [`flpr_scan.c`](src:firmware/obc-fw-nrf54l/src/flpr/flpr_scan.c)
- Wire packer: [`wire.rs`](src:firmware/obc-display/src/ls021/wire.rs)
- Presenter: [rendering pipeline](../../software/rendering/)
