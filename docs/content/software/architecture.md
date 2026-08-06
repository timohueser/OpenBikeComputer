---
title: System architecture
description: How OpenBikeComputer is organised so a desktop simulator and a microcontroller run the same application and rendering code — the crate graph, the per-frame loop, the seams, and the two-plane input model.
---

# System architecture

The whole project is shaped by one decision: **everything device-specific lives at the edges, and everything in the middle is shared.** The map reader, the route reader, the renderer, and the application logic are one body of `no_std` code that runs *byte-for-byte identically* on the desktop simulator and on the microcontroller. Only the outermost shell — where pixels land, where bytes come from, what a "fix" is — differs between them.

That's why the landing page's [browser demo](../../) (the `obc-web-demo` wasm host) runs the same code as the device, and why the nRF54L firmware reuses the entire stack unchanged. This page is the map of that structure.

## The runtime stack

The crates form a stack with dependencies pointing **one way — downward**. The foundation fixes byte contracts and semantic boundaries; each layer up adds capability; the *hosts* sit on top. Nothing in the shared core ever depends on a host.

<figure class="fig">
<svg viewBox="0 0 720 520" role="img" aria-label="The crate dependency stack. At the top, obc-sim and obc-web-demo share host glue in obc-host-core, while obc-fw-nrf54l composes obc-platform adapters; the hosts depend on obc-app and their port implementations bind directly to obc-ports. The app depends on obc-render, obc-reader, obc-route, obc-elevation and obc-ports. obc-render and obc-reader independently depend on the allocation-free obc-map-scene foundation, so rendering does not depend on the concrete OBCM reader. obc-reader and obc-route depend on obc-formats; obc-route and obc-app also depend on obc-elevation, a fourth foundation crate holding the OBCT reader, the sampling rules and the shared dead-band, whose own only dependency is obc-formats. Every arrow points downward, so the shared core never depends on a host.">
  <defs>
    <marker id="aA" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">The runtime stack — every arrow points down</text>

  <!-- hosts -->
  <rect class="d-panel" x="150" y="52" width="180" height="50" rx="10" />
  <text class="d-label" x="240" y="78" text-anchor="middle" style="font-size:11px">obc-sim · obc-web-demo</text>
  <text class="d-sub" x="240" y="93" text-anchor="middle">desktop sim · browser demo</text>
  <rect class="d-panel" x="390" y="52" width="200" height="50" rx="10" />
  <text class="d-label" x="490" y="78" text-anchor="middle">obc-fw-nrf54l</text>
  <text class="d-sub" x="490" y="93" text-anchor="middle">device · obc-platform adapters</text>
  <text class="d-tag" x="150" y="44" style="fill:#6b7758">hosts</text>

  <!-- app -->
  <rect class="d-hot" x="150" y="146" width="440" height="56" rx="12" style="fill:#f8efe4" />
  <text class="d-title" x="370" y="172" text-anchor="middle" style="fill:#a9501c">obc-app</text>
  <text class="d-sub" x="370" y="190" text-anchor="middle">camera · screen stack · input · ride tracking — the per-frame driver</text>

  <!-- render -->
  <rect class="d-panel" x="210" y="242" width="320" height="50" rx="10" />
  <text class="d-label" x="370" y="268" text-anchor="middle">obc-render</text>
  <text class="d-sub" x="370" y="283" text-anchor="middle">projection · culling · rasterising</text>

  <!-- core sources -->
  <rect class="d-panel" x="150" y="332" width="200" height="54" rx="10" />
  <text class="d-label" x="250" y="358" text-anchor="middle">obc-reader</text>
  <text class="d-sub" x="250" y="374" text-anchor="middle">OBCM · quadtree · chunk decode</text>
  <rect class="d-panel" x="390" y="332" width="200" height="54" rx="10" />
  <text class="d-label" x="490" y="358" text-anchor="middle">obc-route</text>
  <text class="d-sub" x="490" y="374" text-anchor="middle">OBCR · GPX · map-match</text>
  <!-- foundation -->
  <rect class="d-panel-2" x="30" y="432" width="150" height="54" rx="10" />
  <text class="d-label" x="105" y="454" text-anchor="middle" style="font-size:11px">obc-map-scene</text>
  <text class="d-sub" x="105" y="470" text-anchor="middle" style="font-size:9px">styles · candidate visits</text>
  <rect class="d-panel-2" x="200" y="432" width="150" height="54" rx="10" />
  <text class="d-label" x="275" y="454" text-anchor="middle" style="font-size:11px">obc-formats</text>
  <text class="d-sub" x="275" y="470" text-anchor="middle" style="font-size:9px">layouts · codecs · bytes</text>
  <rect class="d-panel-2" x="370" y="432" width="150" height="54" rx="10" />
  <text class="d-label" x="445" y="454" text-anchor="middle" style="font-size:11px">obc-elevation</text>
  <text class="d-sub" x="445" y="470" text-anchor="middle" style="font-size:9px">OBCT · sampling · dead-band</text>
  <rect class="d-panel-2" x="540" y="432" width="150" height="54" rx="10" />
  <text class="d-label" x="615" y="454" text-anchor="middle" style="font-size:11px">obc-ports</text>
  <text class="d-sub" x="615" y="470" text-anchor="middle" style="font-size:9px">semantic traits · no deps</text>

  <!-- arrows (depends-on, downward) -->
  <line class="d-flow" x1="240" y1="102" x2="258" y2="144" marker-end="url(#aA)" />
  <line class="d-flow" x1="490" y1="102" x2="472" y2="144" marker-end="url(#aA)" />
  <line class="d-flow" x1="370" y1="202" x2="370" y2="240" marker-end="url(#aA)" />
  <path class="d-flow" d="M240 292 C 150 320, 88 380, 100 430" marker-end="url(#aA)" />
  <line class="d-flow" x1="388" y1="356" x2="354" y2="356" marker-end="url(#aA)" />
  <line class="d-flow" x1="215" y1="386" x2="135" y2="430" marker-end="url(#aA)" />
  <line class="d-flow" x1="265" y1="386" x2="262" y2="430" marker-end="url(#aA)" />
  <line class="d-flow" x1="430" y1="386" x2="300" y2="430" marker-end="url(#aA)" />
  <line class="d-flow" x1="478" y1="386" x2="440" y2="430" marker-end="url(#aA)" />
  <line class="d-flow" x1="530" y1="386" x2="600" y2="430" marker-end="url(#aA)" />

  <!-- app also reaches past render straight to the foundation crates -->
  <path class="d-flow" d="M186 202 C 170 252, 176 300, 206 330" marker-end="url(#aA)" opacity="0.8" />
  <path class="d-flow" d="M554 202 C 570 252, 564 300, 534 330" marker-end="url(#aA)" opacity="0.8" />
  <path class="d-flow" d="M590 174 C 700 240, 704 398, 640 430" marker-end="url(#aA)" opacity="0.8" />
</svg>
<figcaption>Hosts depend on <b>obc-app</b>; the app on <b>obc-render</b> — and directly on <b>obc-reader</b> + <b>obc-route</b>. Render and reader meet only through <b>obc-map-scene</b>, so the renderer never learns OBCM offsets, quadtrees, or cache policy. Beneath them, <b>obc-formats</b> owns persistent byte facts, <b>obc-elevation</b> owns the terrain raster's reader and its sampling arithmetic (<b>obc-route</b> reaches it at route emit, and <b>obc-app</b> at every GPS fix — that second edge is left undrawn only to keep the diagram legible), and <b>obc-ports</b> owns the semantic sensor/input/track/settings traits; host and platform implementations bind to those directly, never upward to app policy. Because every arrow points down, the shared core compiles and runs without any host. (A whole tier sits outside this stack — the map producers <b>obc-pack</b> and <b>obc-dem</b>, the cell assembler <b>obcm-assemble</b>, the wasm shims <b>obc-web-convert</b> / <b>obc-web-assemble</b> over them, and the host tools and oracles beside them — none constructs an <code>App</code>, and the producers consume <b>obc-elevation</b> from the host side, which is what makes the packer's climb arithmetic the device's.)</figcaption>
</figure>

The one-way rule is the load-bearing constraint. `obc-app` builds for the bare-metal target (`thumbv8m.main-none-eabihf`) with no host present; the simulator and the firmware are just two different things that link *against* it. Swap the host, keep the core.

Inside the crate, the `App` a host constructs is a **composition root**, not the implementation site: one plain struct per responsibility, composed by value and driven through the same façade methods hosts have always called.

- The [ride engine](src:firmware/obc-app/src/ride_engine.rs) owns everything derived from the sensors and the active route — the matcher, the once-per-load elevation/climb/waypoint caches, the breadcrumb.
- The [UI runtime](src:firmware/obc-app/src/ui_runtime.rs) owns the screen stack, timers, repaint accumulation, and the delivery discipline for host-pushed cards.
- The [catalog state](src:firmware/obc-app/src/catalog_state.rs) owns the route/ride/trip catalogs and their durable ids; the id ↔ summary pairing and the rescan remap live in one place, so an inserted or deleted file can never silently retarget what's navigated.
- The [host module](src:firmware/obc-app/src/host.rs) owns the typed command/event vocabulary and its pending state.
- The render-only scratch — the dominant slice of what a frame needs resident — is *not* an `App` field at all: the host owns a [`RenderScratch`](src:firmware/obc-render/src/lib.rs) and lends it to each render call. It is pure per-frame working memory, so nothing that decides what a frame looks like may live in it; presentation switches travel beside it as a `RenderConfig` the caller restates every frame. On the device that scratch is placement-initialized straight into reserved RAM (an empty scratch *is* the all-zero bit pattern), like every other KB-scale component here — building one on the ~36 KB stack is exactly the overflow this project has already paid for once.

Components talk through parameters, not back-references — a delivery rule that needs to know "is a ride being tracked?" is handed that fact — so the dependency direction inside the crate is as one-way as the crate graph around it.

At the bottom, [`obc-formats`](src:firmware/obc-formats) is deliberately smaller than a reader: no allocator, storage adapter, cache, converter, or rendering policy. The format specs stay the normative byte contracts; this crate is their code authority for versions, fixed lengths, flags, sentinels, endian primitives, and the neutral byte-source/sink traits. Every persistent-format producer and consumer — including `obc-pack` and the deliberately independent `obcm-testkit` byte builder — imports those facts from here, so each byte fact has exactly one import path; `obc-reader` and `obc-route` keep the parsing and streaming algorithms.

[`obc-map-scene`](src:firmware/obc-map-scene) is the equally small boundary between a map source and rasterisation: neutral bounds, geometry/style metadata, LOD selection, candidate visitation, and selected-feature decode into caller-owned buffers. The normal [`obc-reader` adapter](src:firmware/obc-reader/src/scene.rs) is monomorphised into the renderer's hot path; a static test scene can implement the same contract with no map file at all. Opaque six-byte candidate tokens let a source find winners again without leaking byte offsets.

[`obc-elevation`](src:firmware/obc-elevation) is the third crate of that size and the newest: the OBCT reader, the normative bilinear sampler, a fixed-slot tile cache, and the dead-banded ascent integrator, with `obc-formats` as its only dependency and no knowledge of maps, routes or screens. Its seam is a single method — `ElevationSource::sample(lat, lon) -> Option<i16>` — and the crate ships the null implementation that answers `None` for everything, which is what makes a card with no terrain file provably identical to the system before terrain existed. The dependency direction is strictly *consumers → elevation*: `obc-route` samples it at route emit, `obc-app` at each GPS fix, and the host-side `obc-pack` samples the same code over the same bytes when it bakes per-edge ascent — the mechanism behind [one sampling truth](../terrain/#one-sampling-truth).

Beside it, [`obc-ports`](src:firmware/obc-ports) owns the dependency-free values and traits that cross the app/host boundary. The app and every implementation — platform, board, simulator, replay, browser host, USB — bind to it directly. `SettingsStore` uses an associated value type so the foundation never depends upward on the app's `Settings` model.

## Two hosts, one core — and the seams between them

A "host" is whatever constructs an [`App`](src:firmware/obc-app/src/app.rs) and drives it. The simulator ([`obc-sim`](src:apps/obc-sim)) is an `eframe`/`egui` desktop shell; the device firmware ([`obc-fw-nrf54l`](src:firmware/obc-fw-nrf54l), via [`obc-platform`](src:firmware/obc-platform)) is bare-metal on the nRF54LM20. A third host, [`obc-web-demo`](src:apps/obc-web-demo), puts the same core behind a small wasm API for the landing page's demo — no GUI framework; the page's JS owns the canvas and the frame loop. Seam-wise it's a minimal sibling of the simulator, with in-memory stores and a GPX replay for sensors; the host logic the two share — replay stepping, the frame-interleaved route planner, the stores, and the command/event orchestration below — lives in [`obc-host-core`](src:host/obc-host-core). Two further wasm crates are deliberately *not* hosts: [`obc-web-convert`](src:apps/obc-web-convert) exposes `obc-route`'s GPX ↔ OBCR converters, while [`obc-web-assemble`](src:apps/obc-web-assemble) exposes the shared cell assembler. Each takes exactly the layer it needs, with no screen, replay, or planner attached.

Each host owns its window/panel, its storage, and its sensors — and hands the core four small abstractions. Those four **seams** are the entire device-specific surface area; find them and you've found every boundary that matters.

<figure class="fig">
<svg viewBox="0 0 720 372" role="img" aria-label="The shared core sits in the middle and connects through four seams to each host. DrawTarget carries pixels out: both hosts render into a resident RGB222 framebuffer and present it through the shared display contracts — a native-frame format paired with a presenter (the device's FLPR scans the frame to the panel; the simulator self-diffs it and uploads the changed rows to a texture). The colour function maps a 16-bit colour to a pixel — native RGB222 (64-colour) on both; the simulator's un-quantized true-colour reference stays on the headless PNG path. ByteSource brings bytes in (an in-memory slice in the sim; FatFs on the SD card on the device). The HAL traits bring the world in (the control panel, a GPX replay and the keyboard in the sim; GPS, a barometer and GPIO buttons on the device).">
  <text class="d-tag" x="20" y="22">Everything device-specific lives at four seams</text>

  <!-- column headers -->
  <text class="d-title" x="110" y="44" text-anchor="middle" style="font-size:12px">obc-sim host</text>
  <text class="d-title" x="610" y="44" text-anchor="middle" style="font-size:12px">device host</text>

  <!-- core -->
  <rect class="d-hot" x="282" y="40" width="156" height="46" rx="11" style="fill:#f8efe4" />
  <text class="d-title" x="360" y="62" text-anchor="middle" style="fill:#a9501c">shared core</text>
  <text class="d-sub" x="360" y="77" text-anchor="middle">reader·route·render·app</text>
  <!-- spine -->
  <line x1="360" y1="86" x2="360" y2="320" stroke="#9aa884" stroke-width="1.4" />

  <!-- seam rows: y-centers 130, 188, 246, 304 -->
  <!-- 1 DrawTarget -->
  <rect class="d-panel-2" x="298" y="112" width="124" height="38" rx="9" />
  <text class="d-label" x="360" y="129" text-anchor="middle" style="font-size:11px">DrawTarget</text>
  <text class="d-sub" x="360" y="142" text-anchor="middle">pixels out</text>
  <rect class="d-panel" x="20" y="112" width="180" height="38" rx="9" />
  <text class="d-sub" x="110" y="135" text-anchor="middle">RGB222 FB · self-diffed</text>
  <rect class="d-panel" x="520" y="112" width="180" height="38" rx="9" />
  <text class="d-sub" x="610" y="135" text-anchor="middle">RGB222 FB · banded push</text>
  <line class="d-stroke" x1="200" y1="131" x2="298" y2="131" /><line class="d-stroke" x1="422" y1="131" x2="520" y2="131" />

  <!-- 2 color_fn -->
  <rect class="d-panel-2" x="298" y="170" width="124" height="38" rx="9" />
  <text class="d-label" x="360" y="187" text-anchor="middle" style="font-size:11px">color_fn</text>
  <text class="d-sub" x="360" y="200" text-anchor="middle">u16 → pixel</text>
  <rect class="d-panel" x="20" y="170" width="180" height="38" rx="9" />
  <text class="d-sub" x="110" y="193" text-anchor="middle">64-colour (PNG: true-colour)</text>
  <rect class="d-panel" x="520" y="170" width="180" height="38" rx="9" />
  <text class="d-sub" x="610" y="193" text-anchor="middle">native RGB222 (64)</text>
  <line class="d-stroke" x1="200" y1="189" x2="298" y2="189" /><line class="d-stroke" x1="422" y1="189" x2="520" y2="189" />

  <!-- 3 ByteSource -->
  <rect class="d-panel-2" x="298" y="228" width="124" height="38" rx="9" />
  <text class="d-label" x="360" y="245" text-anchor="middle" style="font-size:11px">ByteSource</text>
  <text class="d-sub" x="360" y="258" text-anchor="middle">bytes in</text>
  <rect class="d-panel" x="20" y="228" width="180" height="38" rx="9" />
  <text class="d-sub" x="110" y="251" text-anchor="middle">in-memory slice</text>
  <rect class="d-panel" x="520" y="228" width="180" height="38" rx="9" />
  <text class="d-sub" x="610" y="251" text-anchor="middle">FatFs on the SD card</text>
  <line class="d-stroke" x1="200" y1="247" x2="298" y2="247" /><line class="d-stroke" x1="422" y1="247" x2="520" y2="247" />

  <!-- 4 semantic ports -->
  <rect class="d-panel-2" x="298" y="286" width="124" height="38" rx="9" />
  <text class="d-label" x="360" y="303" text-anchor="middle" style="font-size:10.5px">obc-ports</text>
  <text class="d-sub" x="360" y="316" text-anchor="middle">semantic HAL</text>
  <rect class="d-panel" x="20" y="286" width="180" height="38" rx="9" />
  <text class="d-sub" x="110" y="309" text-anchor="middle">panel · GPX · keys</text>
  <rect class="d-panel" x="520" y="286" width="180" height="38" rx="9" />
  <text class="d-sub" x="610" y="309" text-anchor="middle">GPS · baro · mag · GPIO</text>
  <line class="d-stroke" x1="200" y1="305" x2="298" y2="305" /><line class="d-stroke" x1="422" y1="305" x2="520" y2="305" />
</svg>
<figcaption>The core is generic over a <b>DrawTarget</b> (where pixels go) and takes a <b>colour function</b> (16-bit style colour → this panel's pixel); it reads every map and route through a <b>ByteSource</b> and gets the world through the semantic traits owned by <b>obc-ports</b>. Implement those four seams for a new board and the whole stack runs on it — no changes to the core.</figcaption>
</figure>

Two of these deserve a closer look.

**`ByteSource` — reading bytes you don't have room for.** A full map is far larger than the device's RAM, so the reader never holds the whole file. It reads through a tiny random-access trait, a chunk at a time, backed by an in-memory slice on the host and by a FatFs handle on the SD card on the device:

```rust
pub trait ByteSource {
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), Error>;
    fn len(&self) -> u32;
}
```

`read_at` takes `&self` (not `&mut self`) so the reader can hold one shared `&dyn ByteSource` and stay simple; a seeking medium uses interior mutability behind it. The same seam serves the route reader, and a small resident cache keeps recently-read chunks hot across frames so streaming doesn't re-hit the card.

The seam is also where the device buys back the cost of random access on a FAT filesystem. FAT stores a file's location as a singly-linked cluster chain, so seeking *backward* in a 300 MB map means re-walking the chain from the front — over a hundred extra sector reads per 2 KB chunk, which once made the on-device router two orders of magnitude slower than the search itself. Since the map is opened read-only and never moves, the firmware resolves its chain **once** at open into a small table of contiguous extents; every later `read_at` is then plain arithmetic plus the data blocks. The reader above never learns any of this — it still just calls `read_at`.

**The HAL — the world, abstracted.** GPS fixes, GPS time, barometric altitude, temperature, the compass heading, the heart rate / power / cadence from BLE sensors, the track log, settings persistence, and raw button events each arrive through their own trait in [`obc-ports`](src:firmware/obc-ports/src/lib.rs), with live sensors bundled into a `Sensors` set the app polls once per tick. The implementations live where they belong: board-agnostic adapters in [`obc-platform`](src:firmware/obc-platform/src), the pure sensor-chip decoders in [`obc-sensors`](src:firmware/obc-sensors/src), the SD storage adapters in [`obc-storage`](src:firmware/obc-storage/src), recorded-ride sources in [`obc-replay`](src:host/obc-replay/src), and shared desktop/browser stepping in [`obc-host-core`](src:host/obc-host-core/src/replay.rs). The app is oblivious to whether a fix came from a satellite or a GPX replay — or a heart rate from a real strap or an injected test line. The three BLE-sensor sources are fed by the radio's [central-role manager](../companion-link/#sensors-the-device-as-ble-central) (or the simulator's sliders), and a value older than 5 s drains as `None` — a dropped strap reads `--` and records as absent rather than freezing its last number into the log.

Each `poll` is a **mailbox drain**, not a bus transaction: it returns `Some` only on the tick a fresh sample arrived and `None` between. On the device this is event-driven — a high-priority task drives the I²C bus (a u-blox SAM-M10Q GPS + a Bosch BMP581 altimeter + an electronic compass on one shared bus) only when the GPS signals a fix is ready, so there is **zero** bus traffic at the frame rate. That task also makes position and altitude **coherent**: it reads the barometer on each GPS fix, so the two share one instant — they're written together into the track log. The one tradeoff is that climb then accrues only while fixes arrive — a GPS outage (a tunnel) pauses it — but during an outage there's no position to log anyway. A cold start or a dropout simply yields `None`, so the camera never teleports onto a stale fix.

**The altimeter and the map calibrate each other.** A barometer measures pressure, not height, and the only way to turn one into the other is to assume a sea-level pressure — so the device's altitude reading is offset by whatever the weather is doing, drifting metres per hour as a front passes. Only *differences* were ever trustworthy, which is why climb, the recorded track and the exported GPX have always been built out of them. The terrain file solves the other half: sampled at each GPS fix it gives an absolute, weather-immune ground height — but a static, coarsely-posted one that knows nothing about the bridge you are on. Each is precisely the other's calibration, so the app subtracts them at every fix and low-passes the difference over a few minutes. That slow average *is* the barometer's unknown offset; adding it back gives an absolute elevation with the barometer's own metre-by-metre responsiveness, and the **Current Elevation** tile shows it once the estimate has settled. Large steps in the difference — a bridge over a gorge, a cutting, a tunnel with a mountain faithfully reported overhead — are rejected rather than averaged, so the estimate follows the trend and not the excursions; a *sustained, self-consistent* run of them instead re-seeds it, so a reference that genuinely moved can never wedge the filter. Two deliberate non-changes: with no terrain file the estimate never settles and the tile reads exactly what it read before, and the **recorded ride is never fused** — its elevations stay the rider's own measurement, since folding the map into them would double-count it. The same offset is, as a side effect, a sea-level-reduced pressure trend with the ride's own climbing subtracted out — the honest signal an offline weather warning would need.

The compass earns its place by covering for the GPS: a receiver only reports a course while you're actually moving, so a stopped rider's heading-up map would otherwise snap to north. A magnetometer gives the heading independent of motion, so the orientation holds while you're stopped; once moving, the GPS course takes over again. Because the heading is *never logged* — it only orients the live map — it isn't tied to the fix like the altimeter is: the task reads it on its own faster cadence **while stopped** (so the map stays lively as you turn the device by hand, even when the fix rate is slow for power saving), and not at all while moving or idle. Only the magnetometer's three axes are used (a flat heading — no tilt compensation), so the source is a plain 3-axis compass even though the bring-up board reads it from a 9-axis IMU's magnetometer; the heading geometry is kept separate from the chip's register map, so the expected swap to a dedicated magnetometer touches only the chip half.

The same task also **power-manages** the receiver. Continuous tracking draws far more than an idle device should, so once it has a boot fix the GPS is put into deep sleep (a backup mode that keeps its clock and almanac on microamps) whenever a ride isn't running — drawing essentially nothing until you start one, when a poke wakes it for a fast *warm* fix. While riding it runs full-power, or, with the Power screen's power-saver on, the receiver's own low-power tracking mode.

The receiver's UTC time rides the same mailbox model but is published **independent of the position fix** — the GPS resolves time before a 3D lock, so the clock is set during acquisition. There is no battery-backed RTC: the wall clock is a stored set-point advanced by the monotonic timer, and a GPS stamp re-establishes that set-point. A fresh boot's set-point is stale by however long the device was powered down, so it is **display-only until a real source re-establishes it this boot** — a GPS fix, or the phone's [`setClock`](../companion-link/#the-trusted-clock-and-route-retention) on connect. That trusted-this-boot distinction is the safety gate the [storage auto-expiry](../ui/#self-cleaning-storage-routes-and-rides-expire) sweep won't delete without; manual date/time editing was removed so nothing else can establish trust, and the remaining UTC-offset stepper shifts only the displayed hour.

## The per-frame loop

Every host runs the same four-step loop. The crucial part is the last step: the app tells the host **what actually changed**, and the host redraws only that.

<figure class="fig">
<svg viewBox="0 0 720 264" role="img" aria-label="The per-frame loop. tick reads the sensors into the camera and ride state; handle_input turns controls into gestures that drive the screen stack; take_dirty reports what changed; then render_map runs only if the map is dirty and render_overlay only if the overlay is dirty. An arc loops back to the next frame. A static screen dirties nothing, so no map redraw happens at all.">
  <defs>
    <marker id="aC" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
    <marker id="aCm" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6.5" markerHeight="6.5" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">One frame — then redraw only what changed</text>

  <rect class="d-panel" x="28" y="74" width="128" height="52" rx="10" />
  <text class="d-label" x="92" y="98" text-anchor="middle">tick</text>
  <text class="d-sub" x="92" y="113" text-anchor="middle">sensors → camera</text>

  <rect class="d-panel" x="186" y="74" width="150" height="52" rx="10" />
  <text class="d-label" x="261" y="98" text-anchor="middle">handle_input</text>
  <text class="d-sub" x="261" y="113" text-anchor="middle">controls → gestures</text>

  <rect class="d-panel" x="366" y="74" width="126" height="52" rx="10" />
  <text class="d-label" x="429" y="98" text-anchor="middle">take_dirty</text>
  <text class="d-sub" x="429" y="113" text-anchor="middle">what changed?</text>

  <rect class="d-hot" x="544" y="48" width="150" height="40" rx="9" style="fill:#f8efe4" />
  <text class="d-label" x="619" y="73" text-anchor="middle" style="fill:#a9501c">render_map</text>
  <rect class="d-panel" x="544" y="110" width="150" height="40" rx="9" />
  <text class="d-label" x="619" y="135" text-anchor="middle">render_overlay</text>

  <line class="d-flow" x1="156" y1="100" x2="184" y2="100" marker-end="url(#aC)" />
  <line class="d-flow" x1="336" y1="100" x2="364" y2="100" marker-end="url(#aC)" />
  <path class="d-hot" d="M492 92 C 516 80, 520 70, 542 68" marker-end="url(#aCm)" />
  <path class="d-hot" d="M492 108 C 516 120, 520 128, 542 130" marker-end="url(#aCm)" />
  <text class="d-sub" x="466" y="56" style="font-size:10px;fill:#a9501c">if map dirty</text>
  <text class="d-sub" x="452" y="158" style="font-size:10px;fill:#a9501c">if overlay dirty</text>

  <!-- loop back -->
  <path class="d-flow" d="M619 150 C 619 208, 300 214, 92 214 C 60 214, 60 160, 64 130" marker-end="url(#aC)" stroke-dasharray="3 4" />
  <text class="d-sub" x="340" y="208" text-anchor="middle">next frame</text>
</svg>
<figcaption><code>tick</code> folds sensor samples into the camera and ride stats; <code>handle_input</code> turns the four buttons into gestures that drive the screen stack; <code>take_dirty</code> reports a <code>{ map, overlay }</code> change set; the host renders only the parts that changed. On a static screen nothing dirties, so <code>render_map</code> — the expensive part — is skipped entirely.</figcaption>
</figure>

In code, the host loop is almost exactly that diagram:

```rust
loop {
    app.tick(RideClock(now), sensors, route);          // GPS + baro → camera, map-match, ride stats
    app.handle_input(InputClock(now), &mut controls);  // four buttons → gestures → screen stack
    let dirty = app.take_dirty();
    if dirty.map     { app.render_map(&mut display, &reader, route, w, h, color_fn); }
    if dirty.overlay { app.render_overlay(&mut display, w, h, color_fn); }
}
```

This **render-on-demand** is the main power lever. The reflective panel holds its image without power, so the goal is to *not draw*: a parked bike on the Home screen issues no map renders frame after frame — the lone exception is one cheap chrome repaint a minute, when the displayed `HH:MM` ticks over. The map only dirties when something the picture shows actually changes — a fresh fix on a riding screen, an applied gesture, a route load, a new minute — so redraws happen exactly when the picture would change and never otherwise. (The two clocks are deliberate: ride stats use a sample-relative `RideClock` so a fast GPX replay doesn't distort moving-time, while button holds use a real-time `InputClock`.)

On the device the loop goes one step further: it doesn't just skip the *drawing* when nothing changed, it skips the *waking*. Rather than tick on a fixed timer, the loop **sleeps until a real event** — a button edge, a fresh sensor sample, or the next animation deadline — and the processor idles (WFI) in between. The app reports that deadline as a single "next wake time" (the soonest timed repaint a visible screen needs: the Home clock's next minute, a settling cursor, the idle-return timeout), so the host arms exactly one timer and selects it against the input and sensor wakes. When nothing is animating and the GPS is asleep, the only timer left is a ~10 s guard tick that feeds the hardware **watchdog** — the last-resort net that reboots the device if a plane wedges. A parked computer wakes a handful of times a *minute*, not a hundred times a *second*. (One subtlety: a press-and-hold emits no gesture until it completes, so the input plane also nudges the loop the moment a hold starts charging — otherwise the hold animation would sleep through its own charge.) The one unavoidable heartbeat is the panel's **COM electrode** wave — the memory-in-pixel cells must never see a DC bias, so a ~60 Hz square wave runs the whole time the panel is powered. On the production board it is handed to a hardware timer (a TIMER→DPPI→GPIOTE toggle chain) so it free-runs with no CPU at all.

<figure class="fig">
<svg viewBox="0 0 760 292" role="img" aria-label="The device loop sleeps until a real event. On the left, three wake sources — a button edge (a recognised gesture, or a hold starting to charge), a fresh sensor sample (a GPS fix, barometer reading or compass heading), and the soonest animation deadline (the next clock minute or a settling cursor) — are awaited together by a single select. Whichever fires first wakes the processor, which is otherwise idle (WFI). On waking it runs exactly one iteration — apply gestures, advance animations, tick, take_dirty, render only what changed — then arms the next wake and sleeps again. When nothing on screen is animating and the GPS is asleep there is no timer at all: it simply waits for a press.">
  <defs>
    <marker id="lpF" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
    <marker id="lpC" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">On the device: sleep until a real event</text>
  <text class="d-sub" x="26" y="58" style="font-size:9px;fill:#6b7758">three wake sources</text>
  <rect class="d-panel-2" x="24" y="72" width="156" height="38" rx="9" />
  <text class="d-label" x="102" y="89" text-anchor="middle" style="font-size:10.5px">button edge</text>
  <text class="d-sub" x="102" y="102" text-anchor="middle" style="font-size:8.5px">a gesture · a charging hold</text>
  <line class="d-flow" x1="180" y1="91" x2="214" y2="91" marker-end="url(#lpF)" />
  <rect class="d-panel-2" x="24" y="120" width="156" height="38" rx="9" />
  <text class="d-label" x="102" y="137" text-anchor="middle" style="font-size:10.5px">sensor sample</text>
  <text class="d-sub" x="102" y="150" text-anchor="middle" style="font-size:8.5px">GPS fix · baro · heading</text>
  <line class="d-flow" x1="180" y1="139" x2="214" y2="139" marker-end="url(#lpF)" />
  <rect class="d-panel-2" x="24" y="168" width="156" height="38" rx="9" />
  <text class="d-label" x="102" y="185" text-anchor="middle" style="font-size:10.5px">animation deadline</text>
  <text class="d-sub" x="102" y="198" text-anchor="middle" style="font-size:8.5px">next clock minute · cursor</text>
  <line class="d-flow" x1="180" y1="187" x2="214" y2="187" marker-end="url(#lpF)" />
  <path d="M218 86 C 230 86, 230 145, 242 145 C 230 145, 230 204, 218 204" fill="none" stroke="#6b7758" stroke-width="1.3" />
  <text class="d-sub" x="232" y="226" text-anchor="middle" style="font-size:9px;fill:#6b7758">select —</text>
  <text class="d-sub" x="232" y="237" text-anchor="middle" style="font-size:9px;fill:#6b7758">first to fire</text>
  <rect class="d-panel" x="262" y="106" width="150" height="78" rx="14" style="fill:#f4f1e3" />
  <text class="d-title" x="337" y="140" text-anchor="middle">asleep · WFI</text>
  <text class="d-sub" x="337" y="158" text-anchor="middle" style="font-size:9.5px">CPU idle between events</text>
  <text x="382" y="122" style="font-family:var(--mono);font-size:9px;fill:#9aa884">z z z</text>
  <line class="d-flow" x1="246" y1="145" x2="260" y2="145" marker-end="url(#lpF)" />
  <line x1="412" y1="132" x2="476" y2="132" stroke="#cf6a2a" stroke-width="2.2" marker-end="url(#lpC)" />
  <text x="444" y="124" text-anchor="middle" style="font-family:var(--mono);font-size:9.5px;fill:#a9501c">wake</text>
  <rect class="d-hot" x="480" y="92" width="252" height="98" rx="14" style="fill:#f8efe4" />
  <text class="d-title" x="606" y="116" text-anchor="middle" style="fill:#a9501c">run one iteration</text>
  <text class="d-sub" x="606" y="138" text-anchor="middle" style="font-size:9.5px">apply gestures · advance animations</text>
  <text class="d-sub" x="606" y="153" text-anchor="middle" style="font-size:9.5px">tick → take_dirty</text>
  <text class="d-sub" x="606" y="168" text-anchor="middle" style="font-size:9.5px">→ render only what changed</text>
  <path class="d-flow" d="M540 190 C 500 224, 420 224, 360 200" marker-end="url(#lpF)" stroke-dasharray="4 4" />
  <text class="d-sub" x="452" y="234" text-anchor="middle" style="font-size:9.5px">arm the next wake, sleep again</text>
  <rect x="250" y="252" width="420" height="26" rx="7" style="fill:#eef2df;stroke:#9aa884;stroke-width:0.8" />
  <text x="460" y="269" text-anchor="middle" style="font-family:var(--mono);font-size:9.5px;fill:#3c6b39">idle (nothing animating · GPS asleep): just the ~10 s watchdog-feed guard tick</text>
</svg>
<figcaption>The device loop <code>select</code>s over three wake sources — input, sensor sample, and the soonest animation deadline the app reports — and the processor sleeps (WFI) until one fires; it then runs one iteration and arms the next wake. Idle, the only timer left is the watchdog-feed guard tick, so a parked Home screen wakes a handful of times a minute instead of 125 times a second. Only the panel's COM wave never stops — and on the production board that's a hardware timer, not the CPU.</figcaption>
</figure>

Beyond rendering, each loop pass also carries the app's **requests** to the host and the host's **answers** back. That traffic speaks one typed vocabulary ([`host.rs`](src:firmware/obc-app/src/host.rs)): everything the app can ask — delete this route, run this plan, persist the settings, rescan the store — is a `HostCommand` the host drains once per pass, and every answer or fact is a `HostEvent` it applies back. The protocol encodes each request's semantics rather than leaving them to convention: one-shots drain exactly once, store-change bursts ride as a count that can't be lost, cancellation drains before new work, and a full mailbox back-pressures instead of dropping anything. Commands and events carry only bounded ids and small results — bulk data such as catalogs and elevation profiles stays in app-owned buffers the host fills directly — and the input/overlay plane deliberately stays *off* this channel, so a button edge never queues behind store work.

The *sequencing* of that traffic — drain the mailbox, delete a route and re-feed the catalog, step the resumable planner one bounded pass, reconcile the ride log — is written once, for every frame-stepped host, in [`obc-host-core`](src:host/obc-host-core/src/dispatch.rs). A single dispatcher drives the drain against narrow repository interfaces that the simulator's folder-backed stores and the web demo's in-memory stores both satisfy, with one shared conformance suite proving they behave alike. The board keeps its own asynchronous execution but consumes the identical `HostCommand`/`HostEvent` vocabulary, and shared protocol tests pin that its ordering matches the dispatcher's.

## On-device routing: the router seam

The device can compute its own route to a POI — no phone, no pre-planned GPX. That capability arrives as a **fifth seam** in the same shape as the others: the shared core *asks*, the host *does*, and the answer flows back through one narrow call. The core never learns whether the route came off the SD card, over Bluetooth, or out of the on-device router — by the time it navigates, all three are the same OBCR bytes.

The trick that keeps it simple is that the router's output is **a route file like any other**. It writes a normal OBCR to a *reserved* path — `/routes/_nav.obcr` (the 8.3 face `_NAV.OBR` on the card), overwritten in place on every plan — which the existing catalog scan picks up and the existing stream path loads. From the [per-frame loop's](#the-per-frame-loop) point of view, a computed route and an uploaded GPX are indistinguishable: same matcher, same elevation profile, same "distance to go." The mid-ride [Detour](../ui/#detouring-around-a-blocked-stretch) commits through the same slot: the spliced route it produces is an ordinary OBCR written to the reserved path, rescanned and re-adopted like any plan — even when the route being detoured *is* the previous occupant of that slot.

**Where a computed route's elevation comes from.** An OBCR carries a height per point, and an imported GPX brings its own; a route the *device* planned has to get them from somewhere. That somewhere is a **terrain file carried beside the map** — a raster of ground heights on the same grid, mounted at boot from a sidecar next to the `.obcm` — which the emit step samples at each vertex it writes, inserting a few interpolated points where a road segment runs long enough to hide a crest between its ends. The route's climb and descent totals then come out of the *same* dead-band integrator an imported track's do, so a planned route and its exported GPX agree. Terrain is strictly an enhancement: the sampler sits behind a one-method seam whose null implementation answers "no height here" for everything, so a card with no terrain file plans, rides and renders exactly as before, with flat profiles — and no screen, stat or export needed a line of code to know the difference.

<figure class="fig">
<svg viewBox="0 0 720 320" role="img" aria-label="The router seam as a request-and-answer loop. From a POI detail screen, a press opens a Create a route confirm showing the destination's category glyph and its straight-line distance. Accepting records a one-shot NavRequest — the rider's fix as the start, the POI coordinate as the goal. On accept the confirm swaps to a planning screen — a spinning compass needle, cancellable with Back. The host drains the request and steps obc-route's NavPlanner — one bounded step per loop pass — against the resident map on the SD card, streaming the emitted OBCR into the reserved nav file. It then rescans the route catalog and answers back with the NavPlanned event carrying the new route's durable id — the same rescan-then-resolve contract BLE uploads use. On success the planning screen is replaced by the NEW ROUTE overview — distance, bike type, and a decimated route-shape preview — and the route activates, re-entering the normal load and navigation pipeline. On failure a two-tier card shows either too far to route here — the router's fixed search table filled before the goal was reached, which is the device's honest range limit — or couldn't find a route for everything else.">
  <defs>
    <marker id="aR1" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
    <marker id="aR2" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">The core asks, the host routes, the answer re-enters the load path</text>

  <!-- core column header -->
  <text class="d-title" x="150" y="52" text-anchor="middle" style="font-size:12px">shared core (obc-app)</text>
  <text class="d-title" x="560" y="52" text-anchor="middle" style="font-size:12px">host</text>
  <line x1="360" y1="60" x2="360" y2="292" stroke="#9aa884" stroke-width="1.3" stroke-dasharray="3 4" />

  <!-- core side -->
  <rect class="d-panel-2" x="24" y="70" width="252" height="40" rx="9" />
  <text class="d-label" x="40" y="88" style="font-size:10.5px">POI detail → press</text>
  <text class="d-sub" x="40" y="102" style="font-size:9px">"Create a route?" confirm</text>

  <rect class="d-hot" x="24" y="124" width="252" height="44" rx="10" style="fill:#f8efe4" />
  <text class="d-label" x="40" y="143" style="fill:#a9501c;font-size:10.5px">NavRequest (one-shot)</text>
  <text class="d-sub" x="40" y="158" style="font-size:9px">from = rider fix · to = POI coord · name</text>

  <!-- request arrow to host -->
  <line class="d-flow" x1="276" y1="146" x2="404" y2="146" marker-end="url(#aR1)" />
  <text class="d-sub" x="340" y="138" text-anchor="middle" style="font-size:8.5px">PlanRoute command</text>

  <!-- host side -->
  <rect class="d-panel" x="404" y="70" width="292" height="120" rx="10" />
  <text class="d-tag" x="420" y="90">plan against the resident map</text>
  <text class="d-sub" x="420" y="110" style="font-size:9.5px">obc-route::NavPlanner — snap 250 m,</text>
  <text class="d-sub" x="420" y="126" style="font-size:9.5px">profile-weighted A* (ε-ladder) over §8 graph</text>
  <text class="d-sub" x="420" y="146" style="font-size:9.5px">→ stream OBCR into <tspan font-family="var(--mono)">/routes/_nav.obcr</tspan></text>
  <text class="d-sub" x="420" y="162" style="font-size:9.5px">→ rescan catalog, resolve durable id</text>
  <text class="d-sub" x="420" y="180" style="font-size:8.5px;fill:#a9501c">stepped once per pass — the loop's watchdog covers it</text>

  <!-- answer arrow back -->
  <line class="d-flow" x1="404" y1="210" x2="276" y2="210" marker-end="url(#aR2)" stroke="#cf6a2a" stroke-width="2" />
  <text class="d-sub" x="340" y="202" text-anchor="middle" style="font-size:8.5px;fill:#a9501c">NavPlanned(Result) event</text>

  <!-- ok / err -->
  <rect class="d-panel-2" x="24" y="224" width="252" height="40" rx="9" />
  <text class="d-sub" x="40" y="242" style="font-size:9.5px;fill:#2c5230"><tspan style="font-weight:700">Ok(id)</tspan> → NEW ROUTE overview + preview,</text>
  <text class="d-sub" x="40" y="256" style="font-size:9px;fill:#2c5230">route activates → normal load/nav path</text>

  <rect class="d-panel-2" x="24" y="272" width="252" height="40" rx="9" />
  <text class="d-sub" x="40" y="290" style="font-size:9.5px;fill:#c0492e"><tspan style="font-weight:700">Err</tspan> → two-tier card:</text>
  <text class="d-sub" x="40" y="304" style="font-size:9px;fill:#c0492e">Exhausted → "Too far…" · else "Couldn't find…"</text>

  <!-- resident map note -->
  <rect class="d-panel" x="404" y="224" width="292" height="88" rx="10" />
  <text class="d-tag" x="420" y="244">re-enters the load path</text>
  <text class="d-sub" x="420" y="264" style="font-size:9.5px">the reserved file is just another route</text>
  <text class="d-sub" x="420" y="280" style="font-size:9.5px">in the catalog — same RouteReader,</text>
  <text class="d-sub" x="420" y="296" style="font-size:9.5px">matcher, profile as a loaded GPX</text>
</svg>
<figcaption>The flow mirrors a <a href="../companion-link/">BLE route upload</a>: the core records a <b>one-shot request</b>, the host does the slow work and <b>rescans-then-resolves</b> before answering with a durable id, and the answer opens either an overview or a failure card. The overview's small route-shape preview comes through the same seam — the host decimates the planned polyline to at most 64 points, so the sketch costs a fixed ~512 B. Everything downstream is the unchanged navigation pipeline: a computed route rides through the same matcher and profile as any other.</figcaption>
</figure>

A plan takes seconds on the SD-bound device, so the flow visibly *breathes* instead of blocking: accepting the confirm opens a **planning screen** — a spinning compass needle over plain copy — while the host steps the planner **one bounded slice per loop pass** (snap, then a miss-budgeted burst of A\* settles a step — the step ends after a small fixed number of SD chunk reads, so a warm cache does far more useful search per pass — then a few emit hops a step). Render, input and the watchdog all run normally *between* steps, and **Back cancels**: the screen pops back to the POI detail and the host simply stops stepping — nothing reaches the reserved file until the emit phase, and a cancelled plan's partial file is discarded.

The miss budget is sized from measurement. The router reads the §8 graph through a small **eight-slot, 512-byte tile cache** — about 4 kB of static RAM. An earlier design used two larger slots, on the theory that A\* settles nodes in spatial runs; profiling showed the opposite — the frontier pops the *globally* best-`f` node and hops between several simultaneously-active leaves, so two slots thrashed. Eight slots in the same RAM lifted the hit rate on a representative 8 km plan from ~33 % to ~56 %, and because a search step is bounded by **cache misses** (roughly one SD read each) rather than a fixed settle count, a warmer cache does more useful search per pass — that plan's search dropped from 101 steps to 58.

Three bounds are worth stating, because the router trades exactness and range for fitting in fixed RAM — and not even RAM of its own: the search table and the render scratch are two arms of one reserved block, so the router's ceiling is what the *largest* consumer already costs, not an allocation added beside it.

- **No distance cap — running out of memory *is* the range limit.** There is no crow-flies pre-check; the limiter is the fixed A\* table. A full table doesn't abort on sight: the first failed insert latches a `table_full` flag and the search keeps going without adding new nodes, so a goal discovered before the table filled is still returned. Only once the frontier drains does the plan fail — surfaced as `Exhausted` → "Too far to route here.", while every other failure (no routable node within the 250 m snap radius, an empty frontier, a failed read) is the generic "Couldn't find a route." tier. Range is therefore **terrain-dependent**, not a radius: a sparse alpine network plans into the double-digit kilometres, a dense urban grid exhausts far sooner — the table holds a node *count*, and terrain decides how much ground that count covers.
- **Profile-weighted A\* with an escalating ε — not shortest path.** The priority is `f = g + ε·h`. `g` is distance **scaled by the chosen bike profile's weight** for each edge's way-kind, **plus that edge's climb charged at the profile's climb weight**, so the search optimises "shortest under your bike type, counting the climbing" (profiles, climb weights and a worked example: the [packer page](../packer-routing/#weighting-the-graph-bike-profiles)). The climb term is only ever *added* — a descent never buys a discount — which is what leaves the great-circle heuristic a lower bound and the ε guarantee intact. ε inflates the heuristic to make the search goal-greedy — a narrow corridor toward the POI instead of plain A\*'s outward exploration — which is what lets the fixed scratch reach multi-kilometre routes. It starts at **1.3×** and, only if that bound exhausts the table, escalates 1.3 → 2.0 → 3.0, retrying the same snapped endpoints over a still-warm cache. The returned path is at most the successful rung's ε times the best climb-aware route under the profile.
- **A ~40 kB nav budget, and the simulator sits inside it.** The A\* scratch is **one** fixed table — 1 536 nodes, ~40 kB, the LM20's nav-budget cap — and it is the same size everywhere: device, host and simulator all compile the same constant, so the simulator's plannable range *is* the shipping device's range by construction rather than by resemblance. The scratch lives in static memory — an arm of the shared block, claimed for the length of a search and handed back to the renderer after — never a stack frame, so a long plan can't blow the device's tight stack; on glass, planning is SD-bound at roughly 100 ms per chunk read.

The router itself — snap, the weighted-A\* settle loop, and the OBCR emit — is [`obc-route/src/nav.rs`](src:firmware/obc-route/src/nav.rs); the §8 graph it reads is described on the [data formats](../formats/#the-navigation-graph-a-routable-network) page and built by the [packer](../packer-routing/#building-the-navigation-graph); the app-side request/answer seam is the typed host protocol — the host drains a `PlanRoute` command and answers with a `NavPlanned` event through [`apply_event`](src:firmware/obc-app/src/app.rs), alongside the BLE-upload events it deliberately mirrors.

## Staying responsive: the two planes

There's a tension on the device. A dense map frame can take the better part of a tenth of a second to render and push to glass, but a button press must feel instant. If both lived on one thread, a press during a long render would stutter. So the device splits the work into **two cooperating planes**.

<figure class="fig">
<svg viewBox="0 0 720 290" role="img" aria-label="Two planes. The input plane runs on a high-priority executor as frequent short ticks that sample the buttons, recognise gestures and animate the overlay. The map plane runs one long base-map render and panel push of about 44 milliseconds. The input plane preempts the map render at intervals, and recognised gestures flow one way down a channel into the map plane. Recognising a gesture never blocks on the render, so input stays responsive while a frame draws; the panel is scanned by the FLPR coprocessor straight out of the framebuffer.">
  <defs>
    <marker id="aD" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Input never waits on the map render</text>

  <!-- input plane lane -->
  <text class="d-label" x="20" y="74">input plane</text>
  <text class="d-sub" x="20" y="88">high-priority executor</text>
  <g>
    <rect x="172" y="58" width="26" height="34" rx="5" class="d-forest" />
    <rect x="246" y="58" width="26" height="34" rx="5" class="d-forest" />
    <rect x="320" y="58" width="26" height="34" rx="5" class="d-forest" />
    <rect x="394" y="58" width="26" height="34" rx="5" class="d-forest" />
    <rect x="468" y="58" width="26" height="34" rx="5" class="d-forest" />
    <rect x="542" y="58" width="26" height="34" rx="5" class="d-forest" />
    <rect x="616" y="58" width="26" height="34" rx="5" class="d-forest" />
  </g>
  <text class="d-sub" x="172" y="108">sample buttons · recognise gesture · animate overlay — every few ms</text>

  <!-- map plane lane -->
  <text class="d-label" x="20" y="194">map plane</text>
  <text class="d-sub" x="20" y="208">the expensive render</text>
  <rect class="d-hot" x="172" y="176" width="400" height="40" rx="8" style="fill:#f8efe4" />
  <text class="d-label" x="372" y="201" text-anchor="middle" style="fill:#a9501c">render base map + push · ~44 ms</text>

  <!-- preempt marks -->
  <g stroke="#cf6a2a" stroke-width="1.3" stroke-dasharray="3 3" opacity="0.8">
    <line x1="259" y1="92" x2="259" y2="176" />
    <line x1="407" y1="92" x2="407" y2="176" />
    <line x1="481" y1="92" x2="481" y2="176" />
  </g>
  <text class="d-sub" x="600" y="150" text-anchor="middle" style="font-size:10px">preempts the render</text>

  <!-- gesture channel -->
  <line x1="333" y1="92" x2="333" y2="174" stroke="#cf6a2a" stroke-width="2" marker-end="url(#aD)" />
  <rect x="300" y="126" width="120" height="22" rx="6" style="fill:#f8efe4;stroke:#cf6a2a;stroke-width:1.2" />
  <text class="d-sub" x="360" y="141" text-anchor="middle" style="fill:#a9501c">gesture channel →</text>

  <text class="d-sub" x="20" y="258" style="font-size:11px">Gestures flow one way; the shared panel + framebuffer are serialised by a bus mutex. On the simulator both halves run inline.</text>
</svg>
<figcaption>The <b>input plane</b> samples buttons, recognises the five gestures, and repaints the hold-progress overlay — re-pushing just its small screen window, the bulge composited over the map read back from the framebuffer — on a high-priority executor that preempts the CPU-bound <b>map plane</b> every few milliseconds. Recognition is coupled to the map plane only by a one-way <b>gesture channel</b>, so a press lands a frame later without ever blocking on the render; the panel is scanned by the <b>FLPR coprocessor</b> straight out of the framebuffer, so a long render can briefly hold off the overlay repaint (never the recognition). On the simulator both halves run inline; the recognition logic is the same struct either way, so behaviour is identical.</figcaption>
</figure>

The split is a behaviour-preserving relocation: the same `InputPlane` either runs inline (the simulator) or stands alone on the high-priority executor (the device). Because gesture recognition depends only on the raw events plus a clock — never on app state — buffering a gesture and applying it a moment later is identical to applying it inline. That's the property that makes the whole split safe.

## Where this lives

- The per-frame driver, the screen stack, and the dirty tracking: [`obc-app/src/app.rs`](src:firmware/obc-app/src/app.rs)
- The semantic hardware/host ports: [`obc-ports/src/lib.rs`](src:firmware/obc-ports/src/lib.rs); direct platform implementations: [`obc-platform/src/`](src:firmware/obc-platform/src)
- The two-plane input model: [`obc-app/src/input_plane.rs`](src:firmware/obc-app/src/input_plane.rs)
- The on-device router (snap · weighted A\* · OBCR emit): [`obc-route/src/nav.rs`](src:firmware/obc-route/src/nav.rs)
- The elevation seam, the OBCT sampler and the terrain tile cache: [`obc-elevation`](src:firmware/obc-elevation); the altimeter-fusion filter that rides on it: [`obc-app/src/altitude.rs`](src:firmware/obc-app/src/altitude.rs)
- Persistent-format constants, primitive codecs, and the byte-streaming seam: [`obc-formats`](src:firmware/obc-formats)
- The device host adapters over real hardware: the DrawTarget / present seam in [`obc-display`](src:firmware/obc-display), the SD `ByteSource`/`ByteSink` in [`obc-storage`](src:firmware/obc-storage), the sensor chip decoders in [`obc-sensors`](src:firmware/obc-sensors), and the source/handoff bridges in [`obc-platform`](src:firmware/obc-platform)

For how the renderer in the middle of this stack actually draws a frame, see the [rendering pipeline](../rendering/). For the binary formats the reader streams, see [data formats](../formats/). For the screen stack and gestures the loop drives, see [the UI system](../ui/). For the raster behind the elevation seam — where it comes from and what it costs — see [terrain & elevation](../terrain/).
