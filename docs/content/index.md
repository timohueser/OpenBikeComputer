---
title: Overview
description: How the OpenBikeComputer project fits together, and where to find what — hardware, software internals, and a build guide.
---

# OpenBikeComputer documentation

A from-scratch bikepacking computer: offline vector maps in a custom binary format, GPX route navigation with live map-matching, and ride recording — with the **firmware and a desktop simulator sharing one rendering path**. These docs explain how it all fits together.

These pages are a **conceptual companion to the code**, not an API reference. The source is already documented in depth; what's hard to reconstruct by reading files one at a time is the *shape* of the thing — the pipelines, the boundaries, the why. That's what lives here.

<figure class="fig">
<svg viewBox="0 0 840 430" role="img" aria-label="System map: OSM data and GPX routes are packed into the OBCM and OBCR binary formats, which feed one shared app and render path that runs on both the desktop simulator and the device firmware. On the device, live sensors — GPS, barometer and compass — also feed the app.">
  <defs>
    <marker id="ah" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
      <path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" />
    </marker>
    <marker id="ah-c" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7.5" markerHeight="7.5" orient="auto-start-reverse">
      <path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" />
    </marker>
  </defs>

  <text class="d-tag" x="20" y="26">The whole pipeline</text>

  <!-- ingest lane labels -->
  <text class="d-tag" x="20" y="60" style="fill:#6b7758">① Build a map</text>
  <text class="d-tag" x="20" y="300" style="fill:#6b7758">② Bring a route</text>

  <!-- TOP LANE: OSM -> packer -> obcm -->
  <rect class="d-panel-2" x="18" y="66" width="118" height="58" rx="9" />
  <text class="d-label" x="77" y="90" text-anchor="middle">.osm.pbf</text>
  <text class="d-sub" x="77" y="108" text-anchor="middle">OSM extract</text>

  <rect class="d-panel" x="176" y="66" width="118" height="58" rx="9" />
  <text class="d-label" x="235" y="90" text-anchor="middle">obc-pack</text>
  <text class="d-sub" x="235" y="108" text-anchor="middle">Rust packer</text>

  <rect class="d-panel" x="334" y="66" width="118" height="58" rx="9" style="fill:#eef2df" />
  <text class="d-label" x="393" y="90" text-anchor="middle">.obcm</text>
  <text class="d-sub" x="393" y="108" text-anchor="middle">map · LOD pyramid</text>

  <line class="d-flow" x1="138" y1="95" x2="174" y2="95" marker-end="url(#ah)" />
  <line class="d-flow" x1="296" y1="95" x2="332" y2="95" marker-end="url(#ah)" />

  <!-- BOTTOM LANE: GPX -> route -> obcr -->
  <rect class="d-panel-2" x="18" y="306" width="118" height="58" rx="9" />
  <text class="d-label" x="77" y="330" text-anchor="middle">.gpx</text>
  <text class="d-sub" x="77" y="348" text-anchor="middle">your ride</text>

  <rect class="d-panel" x="176" y="306" width="118" height="58" rx="9" />
  <text class="d-label" x="235" y="330" text-anchor="middle">obc-route</text>
  <text class="d-sub" x="235" y="348" text-anchor="middle">gpx → obcr</text>

  <rect class="d-panel" x="334" y="306" width="118" height="58" rx="9" style="fill:#eef2df" />
  <text class="d-label" x="393" y="330" text-anchor="middle">.obcr</text>
  <text class="d-sub" x="393" y="348" text-anchor="middle">route · profile</text>

  <line class="d-flow" x1="138" y1="335" x2="174" y2="335" marker-end="url(#ah)" />
  <line class="d-flow" x1="296" y1="335" x2="332" y2="335" marker-end="url(#ah)" />

  <!-- SHARED CORE -->
  <rect class="d-hot" x="500" y="150" width="150" height="130" rx="13" style="fill:#f8efe4" />
  <text class="d-title" x="575" y="196" text-anchor="middle" style="fill:#a9501c">obc-app</text>
  <text class="d-title" x="575" y="216" text-anchor="middle" style="fill:#a9501c">obc-render</text>
  <text class="d-sub" x="575" y="240" text-anchor="middle">one render path</text>
  <text class="d-sub" x="575" y="256" text-anchor="middle">no_std · zero-alloc</text>

  <!-- formats -> core -->
  <path class="d-hot" d="M452 95 C 486 95, 478 188, 498 200" marker-end="url(#ah-c)" />
  <path class="d-hot" d="M452 335 C 486 335, 478 240, 498 232" marker-end="url(#ah-c)" />

  <!-- HOSTS -->
  <rect class="d-panel" x="690" y="126" width="138" height="60" rx="9" />
  <text class="d-label" x="759" y="150" text-anchor="middle" style="font-size:10.5px">obc-sim · web demo</text>
  <text class="d-sub" x="759" y="168" text-anchor="middle">desktop · browser</text>
  <rect x="793" y="110" width="35" height="17" rx="8" fill="#3c6b39" />
  <text x="810" y="122" text-anchor="middle" style="font-family:var(--mono);font-size:9px;fill:#fff;letter-spacing:0.06em">LIVE</text>

  <rect class="d-panel" x="690" y="244" width="138" height="60" rx="9" />
  <text class="d-label" x="759" y="268" text-anchor="middle">device firmware</text>
  <text class="d-sub" x="759" y="286" text-anchor="middle">nRF54LM20 · hardware</text>

  <path class="d-flow" d="M650 200 C 672 196, 668 158, 688 156" marker-end="url(#ah)" />
  <path class="d-flow" d="M650 232 C 672 236, 668 274, 688 276" marker-end="url(#ah)" />

  <!-- device-only: live sensors feed the app (the sim replays a GPX instead) -->
  <rect class="d-panel-2" x="690" y="336" width="138" height="58" rx="9" />
  <text class="d-label" x="759" y="360" text-anchor="middle">live sensors</text>
  <text class="d-sub" x="759" y="378" text-anchor="middle">GPS · baro · compass</text>
  <line class="d-flow" x1="759" y1="335" x2="759" y2="309" marker-end="url(#ah)" />
  <text class="d-sub" x="767" y="326" style="font-size:8px;fill:#6b7758">to the app</text>
</svg>
<figcaption>Two ingest lanes — <b>maps</b> and <b>routes</b> — are baked into compact binary formats, then fed to <b>one</b> shared application and render path. The same code runs on the desktop simulator, on this site's landing page (the <code>obc-web-demo</code> wasm host), and on the device — where live sensors feed the app the data the other hosts get from a GPX replay.</figcaption>
</figure>

The whole project is built around two ideas: **compact binary formats a microcontroller can read directly off flash** (no JSON, no reparsing, no heap churn), and **a single rendering path** the simulator and the firmware both run, so the desktop and the device can never drift apart.

## Where to find what

<div class="card-grid">
  <a class="doc-card" href="software/rendering/">
    <span class="dc-tag">Software</span>
    <h3>Rendering pipeline</h3>
    <p>How one map frame is drawn — projection, level-of-detail, the quadtree cull, the stub-select collector, and the polygon/line rasterisers.</p>
  </a>
  <a class="doc-card" href="software/architecture/">
    <span class="dc-tag">Software</span>
    <h3>System architecture</h3>
    <p>The crate graph, the per-frame loop, the "two hosts, one render path" model, and the seams that keep the device-specific bits at the edges.</p>
  </a>
  <a class="doc-card" href="software/formats/">
    <span class="dc-tag">Software</span>
    <h3>Data formats</h3>
    <p>OBCM (maps) and OBCR (routes): why they're binary, the LOD pyramid, the quadtree index, and delta-encoded geometry.</p>
  </a>
  <a class="doc-card" href="software/ui/">
    <span class="dc-tag">Software</span>
    <h3>The UI system</h3>
    <p>The screen stack, five gestures, render-on-demand, and the "adding a screen is a local edit" philosophy behind the on-device interface.</p>
  </a>
  <a class="doc-card" href="hardware/">
    <span class="dc-tag">Hardware</span>
    <h3>Hardware</h3>
    <p>The reflective memory-LCD panel, the nRF54L microcontroller, schematic and PCB. <span class="pill">coming soon</span></p>
  </a>
  <a class="doc-card" href="build/">
    <span class="dc-tag">Building one</span>
    <h3>Build guide</h3>
    <p>Bill of materials, the tools you'll need, flashing the firmware, and putting it together. <span class="pill">coming soon</span></p>
  </a>
</div>

## The stack at a glance

| Layer | Crate / file | What it does |
| :-- | :-- | :-- |
| Map packer | [`obc-pack`](src:host/obc-pack) | OSM `.osm.pbf` → `.obcm` (ingest, multipolygon assembly, quadtree build, per-edge ascent) |
| DEM rasteriser | [`obc-dem`](src:host/obc-dem) | Copernicus GLO-30 → `.obcd` terrain cells — the elevation raster carried beside a map |
| Cell assembler | [`obcm-assemble`](src:host/obcm-assemble) | Downloaded OBCA cells → one `.obcm` (or a volume set): geometry grafted, the nav graph rewritten, verified against the spec |
| Elevation | [`obc-elevation`](src:firmware/obc-elevation) | The OBCT reader, the sampling rules and the shared climb dead-band — one implementation, host and device |
| Map reader | [`obc-reader`](src:firmware/obc-reader) | Parses OBCM directly off bytes — header, styles, LOD table, quadtree, chunk decode |
| Weather reader | [`obc-weather`](src:firmware/obc-weather) | Validates OBCW and decodes one independently addressed rain tile at a time, with no provider or storage policy |
| Route reader | [`obc-route`](src:firmware/obc-route) | OBCR reading, GPX → OBCR conversion, map-matching, elevation profile |
| Renderer | [`obc-render`](src:firmware/obc-render) | The shared draw path — projection, culling, rasterising. `no_std`, zero-alloc |
| Application | [`obc-app`](src:firmware/obc-app) | Camera, screen stack, input model, ride tracking — one per-frame entry point |
| Simulator host | [`obc-sim`](src:apps/obc-sim) | Desktop shell: window, control panel, colour policy, GPX replay, headless capture |
| Web demo host | [`obc-web-demo`](src:apps/obc-web-demo) | The landing page's thin wasm host — same crates, a JS-driven frame loop, no GUI framework (shared host glue: [`obc-host-core`](src:host/obc-host-core)) |
| Conversion bridge | [`obc-web-convert`](src:apps/obc-web-convert) | The web builder's wasm shim over the same GPX ↔ OBCR routines — route conversion runs in the tab, no server |
| Assembly bridge | [`obc-web-assemble`](src:apps/obc-web-assemble) | The web builder's wasm shim over the same cell assembler — downloaded map cells become one map in the tab, verified before anything leaves it |

> **New here?** Start with **[System architecture](software/architecture/)** for the lay of the land, then the **[Rendering pipeline](software/rendering/)**. The **[data formats](software/formats/)** page is the reference the other two lean on.

## A note on these docs

This documentation is open-source and lives [in the repo](src:docs/content). It's written by hand in Markdown with bespoke diagrams; if you spot something out of date with the code, the code is the source of truth — and a correction is always welcome.
