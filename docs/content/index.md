---
title: Overview
description: The OpenBikeComputer system, its main data paths, and its technical references.
copy: ai
---

# OpenBikeComputer documentation

OpenBikeComputer is an open-source bikepacking computer. It provides offline maps, route
navigation, and ride recording, with no network on the ride.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 410" role="img" aria-label="Map tools prepare OBCM maps. The GPX converter prepares OBCR routes. Both feed the shared application on the device, simulator, and browser. Device sensors supply live readings.">
  <defs><marker id="index-1" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker></defs>
  <text class="d-tag" x="20" y="26" text-anchor="start">Prepare data, then use it offline</text>
  <rect class="d-panel" x="20" y="55" width="200" height="70" rx="8" />
  <text class="d-title" x="120" y="80" text-anchor="middle">Map tools</text>
  <text class="d-sub" x="120" y="100" text-anchor="middle">OSM + terrain → OBCM</text>
  <rect class="d-panel" x="260" y="55" width="200" height="70" rx="8" />
  <text class="d-title" x="360" y="80" text-anchor="middle">Route converter</text>
  <text class="d-sub" x="360" y="100" text-anchor="middle">GPX → OBCR</text>
  <rect class="d-panel" x="500" y="55" width="200" height="70" rx="8" />
  <text class="d-title" x="600" y="80" text-anchor="middle">Live readings</text>
  <text class="d-sub" x="600" y="100" text-anchor="middle">GPS · barometer · compass</text>
  <path class="d-flow" d="M120 125 L120 160" />
  <path class="d-flow" d="M360 125 L360 178" marker-end="url(#index-1)" />
  <path class="d-flow" d="M600 125 L600 160" />
<path class="d-flow" d="M120 160 H600" />
  <rect class="d-panel d-focus" x="170" y="180" width="380" height="76" rx="8" />
  <text class="d-title" x="360" y="205" text-anchor="middle">Shared application</text>
  <text class="d-sub" x="360" y="225" text-anchor="middle">obc-app + readers + obc-render</text>
  <path class="d-flow" d="M360 256 L360 288" />
<path class="d-flow" d="M120 288 H600" />
  <path class="d-flow" d="M120 288 L120 316" marker-end="url(#index-1)" />
  <path class="d-flow" d="M360 288 L360 316" marker-end="url(#index-1)" />
  <path class="d-flow" d="M600 288 L600 316" marker-end="url(#index-1)" />
  <rect class="d-panel" x="20" y="318" width="200" height="70" rx="8" />
  <text class="d-title" x="120" y="343" text-anchor="middle">Device</text>
  <text class="d-sub" x="120" y="363" text-anchor="middle">nRF54LM20</text>
  <rect class="d-panel" x="260" y="318" width="200" height="70" rx="8" />
  <text class="d-title" x="360" y="343" text-anchor="middle">Simulator</text>
  <text class="d-sub" x="360" y="363" text-anchor="middle">Desktop host</text>
  <rect class="d-panel" x="500" y="318" width="200" height="70" rx="8" />
  <text class="d-title" x="600" y="343" text-anchor="middle">Web demo</text>
  <text class="d-sub" x="600" y="363" text-anchor="middle">Browser host</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The tools convert maps and routes to compact binary files. The device, simulator, and web demo use the same application and rendering code. Device sensors supply live data.</figcaption>
</figure>

The device reads compact binary formats straight from storage. The conversion work happens on a
computer, before the ride.

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
    <p>The OBCM, OBCR, ride, terrain, catalog, and cell formats.</p>
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
| Route reader | [`obc-route`](src:firmware/obc-route) | Reads OBCR routes and provides conversion, matching, and profiles. |
| Renderer | [`obc-render`](src:firmware/obc-render) | Draws maps without allocation. |
| Application | [`obc-app`](src:firmware/obc-app) | Controls screens, input, navigation, and ride recording. |
| Simulator | [`obc-sim`](src:apps/obc-sim) | Hosts the application on a desktop. |
| Web demo | [`obc-web-demo`](src:apps/obc-web-demo) | Hosts the application in WebAssembly. |
| Conversion bridge | [`obc-web-convert`](src:apps/obc-web-convert) | Converts GPX and OBCR data in the browser. |
| Assembly bridge | [`obc-web-assemble`](src:apps/obc-web-assemble) | Assembles and verifies map cells in the browser. |

Start with [System architecture](software/architecture/). Then read
[Rendering pipeline](software/rendering/) and [Data formats](software/formats/).

These pages explain how the system works and why. The [`specs/`](src:specs) directory defines the
exact binary and wire contracts.
