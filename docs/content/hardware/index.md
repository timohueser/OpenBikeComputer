---
title: Hardware
description: The OpenBikeComputer hardware — reflective memory-LCD, nRF54L microcontroller, schematic and PCB. Documentation coming soon.
---

# Hardware

> **Under survey.** The hardware documentation isn't written yet. The device is being developed firmware-first on a prototype board, with the production hardware still taking shape — so this page is a map of what's coming rather than a finished guide.

<figure class="fig">
<svg viewBox="0 0 720 240" role="img" aria-label="A stylised outline of the device seen from the front: a portrait reflective display in a rounded body, with an Up and a Down button on the left flank and a Select and a Back button on the right.">
  <defs>
    <pattern id="grid" width="22" height="22" patternUnits="userSpaceOnUse">
      <path d="M22 0 L0 0 0 22" fill="none" stroke="#2c5e54" stroke-opacity="0.12" stroke-width="1" />
    </pattern>
  </defs>
  <rect x="0" y="0" width="720" height="240" fill="url(#grid)" />
  <!-- device body -->
  <rect x="276" y="24" width="168" height="192" rx="24" class="d-panel" style="fill:#f7f4e6" />
  <!-- screen -->
  <rect x="296" y="46" width="128" height="132" rx="8" style="fill:#e7ead8;stroke:#3c6b39;stroke-width:1.5" />
  <text class="d-sub" x="360" y="106" text-anchor="middle">240 × 320</text>
  <text class="d-sub" x="360" y="122" text-anchor="middle">64 colours</text>
  <text class="d-sub" x="360" y="202" text-anchor="middle" style="letter-spacing:2px">OBC</text>
  <!-- the four buttons: Up / Down on the left flank, Select / Back on the right -->
  <g style="fill:#eae4cb;stroke:#5f7d3d;stroke-width:1.5">
    <rect x="266" y="72"  width="14" height="30" rx="4" />
    <rect x="266" y="112" width="14" height="30" rx="4" />
    <rect x="440" y="72"  width="14" height="30" rx="4" />
    <rect x="440" y="112" width="14" height="30" rx="4" />
  </g>
  <text class="d-sub" x="258" y="92"  text-anchor="end">up</text>
  <text class="d-sub" x="258" y="132" text-anchor="end">down</text>
  <text class="d-sub" x="462" y="92">select</text>
  <text class="d-sub" x="462" y="132">back</text>
  <!-- annotations -->
  <line class="d-stroke" x1="490" y1="176" x2="556" y2="176" />
  <text class="d-label" x="564" y="173">reflective MIP panel</text>
  <text class="d-sub" x="564" y="189">sunlight-readable</text>
  <line class="d-stroke" x1="230" y1="176" x2="164" y2="176" />
  <text class="d-label" x="156" y="173" text-anchor="end">nRF54LM20</text>
  <text class="d-sub" x="156" y="189" text-anchor="end">Cortex-M33 · BLE</text>
</svg>
<figcaption>The shape of the thing: a portrait reflective display and four buttons — up and down on one flank, select and back on the other — no touchscreen. Final hardware specifics are still being finalised.</figcaption>
</figure>

## What this section will cover

### The display
A **reflective memory-LCD (MIP) panel** in the LS021B7DD02 class — 240×320 portrait, 64 colours, matte and sunlight-readable, and able to hold its image without power. Designing for it (flat fills, dithered shading, crisp 1px lines, redraw-only-on-change) shapes the whole software look.

### The microcontroller
A **Nordic nRF54LM20** (Cortex-M33 with BLE) running the firmware. How it drives the panel, stores maps and routes, and talks to a companion app.

### Schematic
The full schematic — power, the display interface, the four buttons, sensors, and connectivity — with the design rationale.

### PCB
The board layout, footprints, and the manufacturing files.

### Power
The battery, charging, and the power budget that turns the low-power display and MCU into *days* of runtime.

---

The firmware runs on an nRF54LM20 development kit; the [system architecture](../software/architecture/) page explains how the board-agnostic core stays unchanged behind a handful of board seams.
