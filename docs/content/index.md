---
title: Overview
description: The OpenBikeComputer system, its main data paths, and its technical references.
copy: ai
---

# OpenBikeComputer documentation

OpenBikeComputer is an open-source bikepacking computer. It provides offline maps, route navigation, and ride recording.

The device, simulator, and web demo use the same application and rendering code. These pages describe the system boundaries and data paths.

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
<figcaption>The tools convert maps and routes to compact binary files. The device, simulator, and web demo use the same application and rendering code. Device sensors supply live data.</figcaption>
</figure>

The device reads compact binary formats directly from storage. It does not convert JSON or XML while it operates.

## Where to find what

<div class="card-grid">
  <a class="doc-card" href="software/rendering/">
    <span class="dc-tag">Software</span>
    <h3>Rendering pipeline</h3>
    <p>Projection, level of detail, quadtree culling, rasterization, and display output.</p>
  </a>
  <a class="doc-card" href="software/architecture/">
    <span class="dc-tag">Software</span>
    <h3>System architecture</h3>
    <p>Runtime layers, host boundaries, the frame loop, and the routing seam.</p>
  </a>
  <a class="doc-card" href="software/formats/">
    <span class="dc-tag">Software</span>
    <h3>Data formats</h3>
    <p>The OBCM, OBCR, ride, terrain, weather, catalog, and cell formats.</p>
  </a>
  <a class="doc-card" href="software/ui/">
    <span class="dc-tag">Software</span>
    <h3>The UI system</h3>
    <p>Screens, input gestures, settings, overlays, and render-on-demand behavior.</p>
  </a>
  <a class="doc-card" href="hardware/">
    <span class="dc-tag">Hardware</span>
    <h3>Hardware</h3>
    <p>The nRF54LM20 development platform and the reflective memory LCD.</p>
  </a>
</div>

## The stack at a glance

| Layer | Crate / file | What it does |
| :-- | :-- | :-- |
| Map packer | [`obc-pack`](src:host/obc-pack) | Converts OSM PBF data to OBCM cells. |
| DEM baker | [`obc-dem`](src:host/obc-dem) | Converts Copernicus GLO-30 data to OBCD terrain cells. |
| Cell assembler | [`obcm-assemble`](src:host/obcm-assemble) | Combines OBCM and OBCD cells into one verified OBCM map. |
| Elevation | [`obc-elevation`](src:firmware/obc-elevation) | Reads OBCT data and supplies shared elevation calculations. |
| Map reader | [`obc-reader`](src:firmware/obc-reader) | Reads OBCM indexes, styles, features, POIs, and navigation data. |
| Weather reader | [`obc-weather`](src:firmware/obc-weather) | Validates OBCW data and reads rain tiles. |
| Route reader | [`obc-route`](src:firmware/obc-route) | Reads OBCR routes and provides conversion, matching, and profiles. |
| Renderer | [`obc-render`](src:firmware/obc-render) | Draws maps without allocation. |
| Application | [`obc-app`](src:firmware/obc-app) | Controls screens, input, navigation, and ride recording. |
| Simulator | [`obc-sim`](src:apps/obc-sim) | Hosts the application on a desktop. |
| Web demo | [`obc-web-demo`](src:apps/obc-web-demo) | Hosts the application in WebAssembly. |
| Conversion bridge | [`obc-web-convert`](src:apps/obc-web-convert) | Converts GPX and OBCR data in the browser. |
| Assembly bridge | [`obc-web-assemble`](src:apps/obc-web-assemble) | Assembles and verifies map cells in the browser. |

Start with [System architecture](software/architecture/). Then read [Rendering pipeline](software/rendering/) and [Data formats](software/formats/).

## Scope

These pages explain architecture and behavior. The [`specs/`](src:specs) directory defines exact binary and wire contracts.
