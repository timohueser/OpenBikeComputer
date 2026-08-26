---
title: System architecture
description: The shared runtime, host interfaces, event loop, routing flow, and input architecture.
---

# System architecture

OpenBikeComputer puts hardware-specific code at the system boundary.
The device, simulator, and [browser demo](../../) use the same `no_std` application core.
Each host supplies storage, sensors, input, and display functions.

## Runtime layers

Dependencies point from hosts to the shared core.
The shared core does not depend on a host.

<figure class="fig">
<svg viewBox="0 0 720 520" role="img" aria-label="Main map and route dependencies. Hosts use the application. The application uses the renderer, map reader, route reader, elevation library, and ports. Foundation crates do not depend on a host.">
  <defs>
    <marker id="aA" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Main map and route dependencies</text>

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
  <text class="d-sub" x="370" y="283" text-anchor="middle">projection · culling · rasterization</text>

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
<figcaption>The arrows show the main map and route dependencies. Host and platform code depend on the shared core.</figcaption>
</figure>

The runtime uses these layers:

| Layer | Responsibility |
| --- | --- |
| Hosts | Construct and drive `App`. Provide system functions. |
| `obc-app` | Own ride state, catalogs, screens, and host messages. |
| `obc-render` | Project, select, and draw map features. |
| `obc-reader` | Read OBCM tables, indexes, and chunks. |
| `obc-route` | Read and write routes. Match positions and calculate routes. |
| `obc-weather` | Validate OBCW data and decode rain tiles. |
| Foundation crates | Define formats, map-scene interfaces, elevation rules, and ports. |

`App` is the composition root for the shared application.
The [ride engine](src:firmware/obc-app/src/ride_engine.rs) owns ride-derived state.
The [UI runtime](src:firmware/obc-app/src/ui_runtime.rs) owns screens, timers, and dirty regions.
The [catalog state](src:firmware/obc-app/src/catalog_state.rs) owns durable object identifiers.
The [host protocol](src:firmware/obc-app/src/host.rs) defines bounded commands and events.

The host owns [`RenderScratch`](src:firmware/obc-render/src/lib.rs).
The host lends this working memory to each render call.
Application state does not use this scratch area.

Foundation crates have narrow responsibilities:

- [`obc-formats`](src:firmware/obc-formats) defines persistent byte constants and byte I/O interfaces.
- [`obc-map-scene`](src:firmware/obc-map-scene) separates map sources from the renderer.
- [`obc-elevation`](src:firmware/obc-elevation) reads OBCT data and applies shared elevation rules.
- [`obc-ports`](src:firmware/obc-ports) defines dependency-free values and semantic host interfaces.

## Three hosts, one core

A host constructs [`App`](src:firmware/obc-app/src/app.rs) and drives the runtime.
The following crates are hosts:

- [`obc-sim`](src:apps/obc-sim) is the desktop simulator.
- [`obc-web-demo`](src:apps/obc-web-demo) is the browser demo.
- [`obc-fw-nrf54l`](src:firmware/obc-fw-nrf54l) is the device host.

[`obc-host-core`](src:host/obc-host-core) contains host behavior that the simulator and browser share.
The conversion and assembly WebAssembly crates are tools.
They do not construct `App`.

<figure class="fig">
<svg viewBox="0 0 720 372" role="img" aria-label="Four main host interfaces connect the shared core to a host. They provide pixels, color conversion, random-access bytes, and semantic hardware values.">
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
  <text class="d-sub" x="110" y="193" text-anchor="middle">native RGB222 (64)</text>
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
  <text class="d-sub" x="610" y="251" text-anchor="middle">flat-store object</text>
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
<figcaption>A host supplies pixels, color conversion, random-access bytes, and semantic hardware values.</figcaption>
</figure>

### Random-access data

All large objects use [`ByteSource`](src:firmware/obc-formats/src/io.rs):

```rust
pub trait ByteSource {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error>;
    fn len(&self) -> u64;
}
```

The reader requests only the required tables and chunks.
The device reads these bytes from a flat-store object.
Host implementations can read from memory or a file.

### Semantic ports

[`obc-ports`](src:firmware/obc-ports/src/lib.rs) defines interfaces for sensors, input, settings, and tracks.
A sensor poll drains a mailbox.
It does not start a bus transaction.
The device sensor task publishes coherent position and altitude samples.

## The per-frame loop

Each host processes sensor data, input, dirty regions, and host messages.

<figure class="fig">
<svg viewBox="0 0 720 264" role="img" aria-label="One runtime cycle processes sensors and input. Dirty flags control map and overlay rendering.">
  <defs>
    <marker id="aC" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
    <marker id="aCm" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6.5" markerHeight="6.5" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">One frame — then redraw only what changed</text>

  <rect class="d-panel" x="28" y="74" width="128" height="52" rx="10" />
  <text class="d-label" x="92" y="98" text-anchor="middle">stage_input</text>
  <text class="d-sub" x="92" y="113" text-anchor="middle">sensors · gestures</text>

  <rect class="d-panel" x="186" y="74" width="150" height="52" rx="10" />
  <text class="d-label" x="261" y="98" text-anchor="middle">the domains</text>
  <text class="d-sub" x="261" y="113" text-anchor="middle">one bounded effect each</text>

  <rect class="d-panel" x="366" y="74" width="126" height="52" rx="10" />
  <text class="d-label" x="429" y="98" text-anchor="middle">stage_plan</text>
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
<figcaption>The host renders only dirty regions. A static screen does not cause a map render.</figcaption>
</figure>

Dirty regions reduce processor and display work.
The application reports a wake deadline for visible animations.
The device also wakes for input, sensor data, and the watchdog guard.

<figure class="fig">
<svg viewBox="0 0 760 292" role="img" aria-label="The device waits for input, sensor data, an animation deadline, or the watchdog guard. It runs one cycle after a wake.">
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
  <text class="d-sub" x="606" y="153" text-anchor="middle" style="font-size:9.5px">run_pass → render + effects</text>
  <text class="d-sub" x="606" y="168" text-anchor="middle" style="font-size:9.5px">→ render only what changed</text>
  <path class="d-flow" d="M540 190 C 500 224, 420 224, 360 200" marker-end="url(#lpF)" stroke-dasharray="4 4" />
  <text class="d-sub" x="452" y="234" text-anchor="middle" style="font-size:9.5px">arm the next wake, sleep again</text>
  <rect x="250" y="252" width="420" height="26" rx="7" style="fill:#eef2df;stroke:#9aa884;stroke-width:0.8" />
  <text x="460" y="269" text-anchor="middle" style="font-family:var(--mono);font-size:9.5px;fill:#3c6b39">idle (nothing animating · GPS asleep): just the ~10 s watchdog-feed guard tick</text>
</svg>
<figcaption>The device sleeps between events. A hardware timer generates the display COM signal without CPU work.</figcaption>
</figure>

The application runs one pass per iteration.
[`App::run_pass`](src:firmware/obc-app/src/device_core/pass.rs) takes what the platform finished, what changed underneath it, and what the rider did.
It runs every domain in a fixed order.
It returns a plan: what to repaint, when to run again, and one bounded effect for each domain.
Each effect carries an operation token.
The answer must return that token.
A domain refuses an answer for an operation it cancelled or replaced.
Effects and answers carry bounded identifiers and small results.
Bulk data stays in caller-owned buffers.
[`obc-host-core`](src:host/obc-host-core/src/dispatch.rs) performs the effects for every frame-stepped host.
The board performs the same effects with its own asynchronous execution.

Two requests still use the older mailbox: close the ride log and forget the paired phone.
No domain can yet validate their completion.
[`device_core/residual.rs`](src:firmware/obc-app/src/device_core/residual.rs) lists the two and the issue that removes each one.

## On-device routing: the router seam

The application hands the host one bounded planning operation.
The host runs [`NavPlanner`](src:firmware/obc-route/src/nav.rs) in bounded steps.
It answers with the operation's own token.
The planner reads the navigation graph from the selected map.
It writes a normal OBCR object to the reserved navigation slot.

<figure class="fig">
<svg viewBox="0 0 720 320" role="img" aria-label="The application requests a route. The host runs the route planner and stores an OBCR object. The host then reports the new durable object identifier.">
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
  <text class="d-label" x="40" y="143" style="fill:#a9501c;font-size:10.5px">NavRequest (one operation)</text>
  <text class="d-sub" x="40" y="158" style="font-size:9px">from = rider fix · to = POI coord · name</text>

  <!-- request arrow to host -->
  <line class="d-flow" x1="276" y1="146" x2="404" y2="146" marker-end="url(#aR1)" />
  <text class="d-sub" x="340" y="138" text-anchor="middle" style="font-size:8.5px">Acquire (carries the token)</text>

  <!-- host side -->
  <rect class="d-panel" x="404" y="70" width="292" height="120" rx="10" />
  <text class="d-tag" x="420" y="90">plan against the resident map</text>
  <text class="d-sub" x="420" y="110" style="font-size:9.5px">obc-route::NavPlanner — exact road projection,</text>
  <text class="d-sub" x="420" y="126" style="font-size:9.5px">profile-weighted A* (ε-ladder) over §8 graph</text>
  <text class="d-sub" x="420" y="146" style="font-size:9.5px">→ stream OBCR into <tspan font-family="var(--mono)">the reserved route object</tspan></text>
  <text class="d-sub" x="420" y="162" style="font-size:9.5px">→ rescan catalog, resolve durable id</text>
  <text class="d-sub" x="420" y="180" style="font-size:8.5px;fill:#a9501c">stepped once per pass — the loop's watchdog covers it</text>

  <!-- answer arrow back -->
  <line class="d-flow" x1="404" y1="210" x2="276" y2="210" marker-end="url(#aR2)" stroke="#cf6a2a" stroke-width="2" />
  <text class="d-sub" x="340" y="202" text-anchor="middle" style="font-size:8.5px;fill:#a9501c">PlanFinished / Failed (same token)</text>

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
  <text class="d-sub" x="420" y="264" style="font-size:9.5px">the reserved object is just another route</text>
  <text class="d-sub" x="420" y="280" style="font-size:9.5px">in the catalog — same RouteReader,</text>
  <text class="d-sub" x="420" y="296" style="font-size:9.5px">matcher, profile as a loaded GPX</text>
</svg>
<figcaption>The planner returns a normal OBCR object. The standard route load path handles this object.</figcaption>
</figure>

The router projects each endpoint onto stored road geometry.
It accepts roads within 100 m.
Sparse lookup anchors make long road edges discoverable.

The search uses profile-weighted A*.
Its epsilon sequence is 1.3, 2.0, and 3.0.
The fixed search table contains 1,536 nodes and uses less than 40 KiB.
The table limit controls range.
Route range is not a fixed distance.

If the map contains terrain, the planner samples it for route elevations.
The shared ascent integrator calculates climb and descent.
A map without terrain still supports route planning.

## Staying responsive: the two planes

The device uses two cooperating execution planes.
The high-priority input plane samples buttons and recognizes gestures.
The map plane applies gestures and owns all rendering.
A bounded channel sends gestures from the input plane to the map plane.

<figure class="fig">
<svg viewBox="0 0 720 290" role="img" aria-label="A high-priority input plane recognizes gestures. The map plane renders the base map and overlay. A channel sends gestures to the map plane.">
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
  <text class="d-sub" x="172" y="108">sample buttons · recognize gesture · animate overlay — every few ms</text>

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

  <text class="d-sub" x="20" y="258" style="font-size:11px">Gestures flow one way; the shared panel + framebuffer are serialized by a bus mutex. On the simulator both halves run inline.</text>
</svg>
<figcaption>The input plane recognizes gestures during a map render. The map plane owns all rendering and panel output.</figcaption>
</figure>

The simulator runs the same `InputPlane` inline.
Gesture recognition depends only on raw input and time.
It does not depend on application state.

## Source index

- Application and dirty state: [`obc-app/src/app.rs`](src:firmware/obc-app/src/app.rs)
- Input recognition: [`obc-app/src/input_plane.rs`](src:firmware/obc-app/src/input_plane.rs)
- Device plane integration: [`obc-fw-nrf54l/src/input_plane.rs`](src:firmware/obc-fw-nrf54l/src/input_plane.rs)
- Display output: [`obc-display`](src:firmware/obc-display)
- Storage: [`obc-storage`](src:firmware/obc-storage)
- Sensor adapters: [`obc-platform`](src:firmware/obc-platform) and [`obc-sensors`](src:firmware/obc-sensors)
- Map formats: [data formats](../formats/)
- Rendering: [rendering pipeline](../rendering/)
- UI: [UI system](../ui/)
- Terrain: [terrain and elevation](../terrain/)
