---
title: The display protocol
description: How the reflective memory LCD holds an image and how the device writes one.
---

# Display protocol

The device uses a Sharp LS021B7DD02 reflective memory-in-pixel LCD of 240 × 320 pixels. Every
subpixel has a one-bit latch on the glass, so the panel keeps its image with no scan traffic. That
is the property the device is built around: a map left on the screen costs almost nothing. The
framebuffer is RGB222, which is four levels per channel and 64 colors.

## Two signal paths

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<img src="../../assets/diagrams/hardware-display-protocol-01.svg" alt="Two independent paths. On the left, the intermittent image-write path — gate scan and source shift — pushes one bit into a subpixel latch on the glass. On the right, the continuous polarity path — VCOM, VB in phase, VA inverse, free-running at about 60 Hz — drives the liquid crystal. The stored bit only selects which rail (VA for white, VB for black) the subpixel follows." data-inline-svg>
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
<img src="../../assets/diagrams/hardware-display-protocol-02.svg" alt="One pixel cell drawn as a three by three grid: R, G, B columns by three stacked bands — top MSB, middle LSB, bottom MSB. The top and bottom MSB bands are wired together and form two thirds of the area; the middle LSB band is one third." data-inline-svg>
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
<img src="../../assets/diagrams/hardware-display-protocol-03.svg" alt="A timeline for one pixel row as one GCK period. GCK rises to advance to the row and open the MSB phase; while high, the MSB bit-plane is shifted in and a GEN pulse latches the two-thirds block. GCK then falls for the LSB phase on the same row; while low, the LSB bit-plane is shifted in and a second GEN pulse latches the one-third block. The next rising edge advances to the next row." data-inline-svg>
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
