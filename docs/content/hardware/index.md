---
title: Hardware
description: The OpenBikeComputer hardware — reflective memory-LCD, nRF54L microcontroller, schematic and PCB. Documentation coming soon.
---

# Hardware

> **Under survey.** The hardware documentation isn't written yet. The device is being developed firmware-first on a prototype board, with the production hardware still taking shape — so this page is a map of what's coming rather than a finished guide.

<figure class="fig">
<svg viewBox="0 0 720 240" role="img" aria-label="A stylised outline of the device: a portrait reflective display above a board with a rotary encoder and a back button.">
  <defs>
    <pattern id="grid" width="22" height="22" patternUnits="userSpaceOnUse">
      <path d="M22 0 L0 0 0 22" fill="none" stroke="#2c5e54" stroke-opacity="0.12" stroke-width="1" />
    </pattern>
  </defs>
  <rect x="0" y="0" width="720" height="240" fill="url(#grid)" />
  <!-- device body -->
  <rect x="276" y="24" width="168" height="192" rx="16" class="d-panel" style="fill:#f7f4e6" />
  <!-- screen -->
  <rect x="296" y="42" width="128" height="118" rx="6" style="fill:#e7ead8;stroke:#3c6b39;stroke-width:1.5" />
  <text class="d-sub" x="360" y="98" text-anchor="middle">240 × 320</text>
  <text class="d-sub" x="360" y="114" text-anchor="middle">64 colours</text>
  <!-- encoder + back -->
  <circle cx="324" cy="190" r="15" style="fill:#eae4cb;stroke:#5f7d3d;stroke-width:1.5" />
  <rect x="386" y="178" width="26" height="24" rx="6" style="fill:#eae4cb;stroke:#5f7d3d;stroke-width:1.5" />
  <text class="d-sub" x="324" y="222" text-anchor="middle">encoder</text>
  <text class="d-sub" x="399" y="222" text-anchor="middle">back</text>
  <!-- annotations -->
  <line class="d-stroke" x1="424" y1="101" x2="540" y2="101" />
  <text class="d-label" x="548" y="98">reflective MIP panel</text>
  <text class="d-sub" x="548" y="114">sunlight-readable · holds its image</text>
  <line class="d-stroke" x1="296" y1="60" x2="180" y2="60" />
  <text class="d-label" x="172" y="57" text-anchor="end">nRF54L</text>
  <text class="d-sub" x="172" y="73" text-anchor="end">Cortex-M33 · BLE</text>
</svg>
<figcaption>The shape of the thing: a portrait reflective display, one rotary encoder, one back button — no touchscreen. Final hardware specifics are still being finalised.</figcaption>
</figure>

## What this section will cover

### The display
A **reflective memory-LCD (MIP) panel** in the LS021B7DD02 class — 240×320 portrait, 64 colours, matte and sunlight-readable, and able to hold its image without power. Designing for it (flat fills, dithered shading, crisp 1px lines, redraw-only-on-change) shapes the whole software look.

### The microcontroller
A **Nordic nRF54L** (Cortex-M33 with BLE) running the firmware. How it drives the panel, stores maps and routes, and talks to a companion app.

### Schematic
The full schematic — power, the display interface, the encoder and button, sensors, and connectivity — with the design rationale.

### PCB
The board layout, footprints, and the manufacturing files.

### Power
The battery, charging, and the power budget that turns the low-power display and MCU into *days* of runtime.

---

The firmware is being proven on an STM32F429 prototype today; the [system architecture](../../software/architecture/) page explains how the same software targets both the prototype and the eventual nRF54L hardware unchanged.
