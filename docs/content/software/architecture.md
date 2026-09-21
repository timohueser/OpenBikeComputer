---
title: System architecture
description: The shared runtime, the host boundary, the frame loop, and the routing seam.
copy: ai
---

# System architecture

Hardware-specific code stays at the system boundary. The device, the simulator, the
[browser demo](../../) and the iPhone run the same `no_std` application core. Each host supplies
storage, sensors, input, and display.

## Runtime layers

Dependencies point from the hosts to the shared core. The core does not depend on a host, so new
hardware cannot reach into the application.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 486" role="img" aria-label="Hosts depend on the shared application. The application composes map, route, and rendering modules. Foundation crates define data and host interfaces. This is a layer overview, not a complete dependency graph.">
  <defs><marker id="software-architecture-1" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker></defs>
  <text class="d-tag" x="20" y="26" text-anchor="start">Runtime layers</text>
  <rect class="d-panel" x="20" y="54" width="210" height="72" rx="8" />
  <text class="d-title" x="125" y="79" text-anchor="middle">Device host</text>
  <text class="d-sub" x="125" y="99" text-anchor="middle">obc-fw-nrf54l</text>
  <rect class="d-panel" x="255" y="54" width="210" height="72" rx="8" />
  <text class="d-title" x="360" y="79" text-anchor="middle">Simulator host</text>
  <text class="d-sub" x="360" y="99" text-anchor="middle">obc-sim</text>
  <rect class="d-panel" x="490" y="54" width="210" height="72" rx="8" />
  <text class="d-title" x="595" y="79" text-anchor="middle">Browser host</text>
  <text class="d-sub" x="595" y="99" text-anchor="middle">obc-web-demo</text>
  <path class="d-flow" d="M125 126 L125 151" />
  <path class="d-flow" d="M360 126 L360 151" />
  <path class="d-flow" d="M595 126 L595 151" />
<path class="d-flow" d="M125 151 H595" />
  <path class="d-flow" d="M360 151 L360 177" marker-end="url(#software-architecture-1)" />
  <rect class="d-panel d-focus" x="20" y="180" width="680" height="74" rx="8" />
  <text class="d-title" x="360" y="205" text-anchor="middle">Application · obc-app</text>
  <text class="d-sub" x="360" y="225" text-anchor="middle">Screens · navigation · ride state · domain effects</text>
  <path class="d-flow" d="M360 254 L360 283" marker-end="url(#software-architecture-1)" />
  <rect class="d-panel" x="20" y="286" width="680" height="74" rx="8" />
  <text class="d-title" x="360" y="311" text-anchor="middle">Shared modules</text>
  <text class="d-sub" x="360" y="331" text-anchor="middle">obc-render · obc-reader · obc-route</text>
  <path class="d-flow" d="M360 360 L360 389" marker-end="url(#software-architecture-1)" />
  <rect class="d-panel" x="20" y="392" width="680" height="74" rx="8" />
  <text class="d-title" x="360" y="417" text-anchor="middle">Foundations</text>
  <text class="d-sub" x="360" y="437" text-anchor="middle">obc-map-scene · obc-formats · obc-elevation · obc-ports</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Read from top to bottom: each layer uses the shared layers below it. This overview groups responsibilities; it is not a complete Cargo dependency graph.</figcaption>
</figure>

[`App`](src:firmware/obc-app/src/app.rs) is the composition root. The
[Navigator](src:firmware/obc-app/src/navigator.rs) owns the active route, route matching, guidance,
and planner pacing. The [UI runtime](src:firmware/obc-app/src/ui_runtime.rs) owns screens, timers,
and dirty regions. The [host protocol](src:firmware/obc-app/src/host.rs) defines bounded commands
and events.

The host owns [`RenderScratch`](src:firmware/obc-render/src/lib.rs) and lends it to each render
call. The memory a frame needs is therefore not part of application state.

Four foundation crates carry no policy:

- [`obc-formats`](src:firmware/obc-formats) defines the persistent byte constants and byte I/O.
- [`obc-map-scene`](src:firmware/obc-map-scene) separates map sources from the renderer.
- [`obc-elevation`](src:firmware/obc-elevation) reads terrain and applies the shared elevation rules.
- [`obc-ports`](src:firmware/obc-ports) defines the semantic host interfaces.

## Four hosts, one core

- [`obc-fw-nrf54l`](src:firmware/obc-fw-nrf54l) is the device.
- [`obc-sim`](src:apps/obc-sim) is the desktop simulator.
- [`obc-web-demo`](src:apps/obc-web-demo) is the browser demo.
- [`obc-ios-host`](src:apps/obc-ios-host) is the iPhone host.

[`obc-host-core`](src:host/obc-host-core) holds the behavior that the three non-device hosts share.

The iPhone host runs the application over one card file. The phone supplies the position, heading,
altitude, and battery level. It is a development tool: it tests the user interface, position
tracking, route following, and ride recording against real sensors and a real rider. It cannot show
device speed, power use, the display driver, or the Bluetooth link. Only the device shows those.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
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
  <text class="d-label" x="360" y="129" text-anchor="middle" style="font-size:12px">DrawTarget</text>
  <text class="d-sub" x="360" y="142" text-anchor="middle">pixels out</text>
  <rect class="d-panel" x="20" y="112" width="180" height="38" rx="9" />
  <text class="d-sub" x="110" y="135" text-anchor="middle">RGB222 FB · self-diffed</text>
  <rect class="d-panel" x="520" y="112" width="180" height="38" rx="9" />
  <text class="d-sub" x="610" y="135" text-anchor="middle">RGB222 FB · banded push</text>
  <line class="d-stroke" x1="200" y1="131" x2="298" y2="131" /><line class="d-stroke" x1="422" y1="131" x2="520" y2="131" />

  <!-- 2 color_fn -->
  <rect class="d-panel-2" x="298" y="170" width="124" height="38" rx="9" />
  <text class="d-label" x="360" y="187" text-anchor="middle" style="font-size:12px">color_fn</text>
  <text class="d-sub" x="360" y="200" text-anchor="middle">u16 → pixel</text>
  <rect class="d-panel" x="20" y="170" width="180" height="38" rx="9" />
  <text class="d-sub" x="110" y="193" text-anchor="middle">native RGB222 (64)</text>
  <rect class="d-panel" x="520" y="170" width="180" height="38" rx="9" />
  <text class="d-sub" x="610" y="193" text-anchor="middle">native RGB222 (64)</text>
  <line class="d-stroke" x1="200" y1="189" x2="298" y2="189" /><line class="d-stroke" x1="422" y1="189" x2="520" y2="189" />

  <!-- 3 ByteSource -->
  <rect class="d-panel-2" x="298" y="228" width="124" height="38" rx="9" />
  <text class="d-label" x="360" y="245" text-anchor="middle" style="font-size:12px">ByteSource</text>
  <text class="d-sub" x="360" y="258" text-anchor="middle">bytes in</text>
  <rect class="d-panel" x="20" y="228" width="180" height="38" rx="9" />
  <text class="d-sub" x="110" y="251" text-anchor="middle">flat-store map object</text>
  <rect class="d-panel" x="520" y="228" width="180" height="38" rx="9" />
  <text class="d-sub" x="610" y="251" text-anchor="middle">flat-store object</text>
  <line class="d-stroke" x1="200" y1="247" x2="298" y2="247" /><line class="d-stroke" x1="422" y1="247" x2="520" y2="247" />

  <!-- 4 semantic ports -->
  <rect class="d-panel-2" x="298" y="286" width="124" height="38" rx="9" />
  <text class="d-label" x="360" y="303" text-anchor="middle" style="font-size:12px">obc-ports</text>
  <text class="d-sub" x="360" y="316" text-anchor="middle">semantic HAL</text>
  <rect class="d-panel" x="20" y="286" width="180" height="38" rx="9" />
  <text class="d-sub" x="110" y="309" text-anchor="middle">panel · GPX · keys</text>
  <rect class="d-panel" x="520" y="286" width="180" height="38" rx="9" />
  <text class="d-sub" x="610" y="309" text-anchor="middle">GPS · baro · mag · GPIO</text>
  <line class="d-stroke" x1="200" y1="305" x2="298" y2="305" /><line class="d-stroke" x1="422" y1="305" x2="520" y2="305" />
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>A host supplies pixels, color conversion, random-access bytes, and semantic hardware values.</figcaption>
</figure>

The host supplies bytes through [`ByteSource`](src:firmware/obc-formats/src/io.rs), which reads any
offset on request. A map therefore does not have to fit in RAM. Every host reads its objects from
the same [flat store](src:host/obc-host-core/src/flat_store.rs); only the medium differs, which is a
card on the device, a sparse file on the simulator, and memory pages in the browser.

A reader pins one object revision while it reads. A replacement object gets a new revision, and the
open reader keeps its bytes until it closes. A map or a route can therefore be replaced while the
rider looks at it.

Recording is the write path with the same rule. The
[recorder](src:host/obc-host-core/src/flat_recorder.rs) appends bounded batches, writes periodic
checkpoints, and closes a ride with a footer. An interrupted ride restarts from its last
checkpoint, because the samples before that point are already durable.

[`obc-ports`](src:firmware/obc-ports/src/lib.rs) defines the sensor, input, settings, and track
interfaces. A sensor poll drains a mailbox. It does not start a bus transaction.

## The per-frame loop

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
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
  <text class="d-sub" x="466" y="56" style="font-size:12px;fill:#a9501c">if map dirty</text>
  <text class="d-sub" x="452" y="158" style="font-size:12px;fill:#a9501c">if overlay dirty</text>

  <!-- loop back -->
  <path class="d-flow" d="M619 150 C 619 208, 300 214, 92 214 C 60 214, 60 160, 64 130" marker-end="url(#aC)" stroke-dasharray="3 4" />
  <text class="d-sub" x="340" y="208" text-anchor="middle">next frame</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The host renders only dirty regions. A static screen does not cause a map render.</figcaption>
</figure>

The application renders on demand. It repaints a region only when the facts that the region draws
have changed, so a static screen costs no processor and no panel traffic.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 760px">
<svg viewBox="0 0 760 292" role="img" aria-label="The device waits for input, sensor data, an animation deadline, or the watchdog guard. It runs one cycle after a wake.">
  <defs>
    <marker id="lpF" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
    <marker id="lpC" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">On the device: sleep until a real event</text>
  <text class="d-sub" x="26" y="58" style="font-size:12px;fill:#4d5b3c">three wake sources</text>
  <rect class="d-panel-2" x="24" y="72" width="156" height="38" rx="9" />
  <text class="d-label" x="102" y="89" text-anchor="middle" style="font-size:12px">button edge</text>
  <text class="d-sub" x="102" y="102" text-anchor="middle" style="font-size:12px">a gesture · a charging hold</text>
  <line class="d-flow" x1="180" y1="91" x2="214" y2="91" marker-end="url(#lpF)" />
  <rect class="d-panel-2" x="24" y="120" width="156" height="38" rx="9" />
  <text class="d-label" x="102" y="137" text-anchor="middle" style="font-size:12px">sensor sample</text>
  <text class="d-sub" x="102" y="150" text-anchor="middle" style="font-size:12px">GPS fix · baro · heading</text>
  <line class="d-flow" x1="180" y1="139" x2="214" y2="139" marker-end="url(#lpF)" />
  <rect class="d-panel-2" x="24" y="168" width="156" height="38" rx="9" />
  <text class="d-label" x="102" y="185" text-anchor="middle" style="font-size:12px">animation deadline</text>
  <text class="d-sub" x="102" y="198" text-anchor="middle" style="font-size:12px">next clock minute · cursor</text>
  <line class="d-flow" x1="180" y1="187" x2="214" y2="187" marker-end="url(#lpF)" />
  <path d="M218 86 C 230 86, 230 145, 242 145 C 230 145, 230 204, 218 204" fill="none" stroke="#6b7758" stroke-width="1.3" />
  <text class="d-sub" x="232" y="226" text-anchor="middle" style="font-size:12px;fill:#4d5b3c">wake on</text>
  <text class="d-sub" x="232" y="242" text-anchor="middle" style="font-size:12px;fill:#4d5b3c">first event</text>
  <rect class="d-panel" x="262" y="106" width="150" height="78" rx="14" style="fill:#f4f1e3" />
  <text class="d-title" x="337" y="140" text-anchor="middle">asleep · WFI</text>
  <text class="d-sub" x="337" y="158" text-anchor="middle" style="font-size:12px">CPU idle between events</text>
  <text x="382" y="122" style="font-family:var(--mono);font-size:12px;fill:#9aa884">z z</text>
  <line class="d-flow" x1="246" y1="145" x2="260" y2="145" marker-end="url(#lpF)" />
  <line x1="412" y1="132" x2="476" y2="132" stroke="#cf6a2a" stroke-width="2.2" marker-end="url(#lpC)" />
  <text x="444" y="124" text-anchor="middle" style="font-family:var(--mono);font-size:12px;fill:#a9501c">wake</text>
  <rect class="d-hot" x="480" y="92" width="252" height="98" rx="14" style="fill:#f8efe4" />
  <text class="d-title" x="606" y="116" text-anchor="middle" style="fill:#a9501c">run one iteration</text>
  <text class="d-sub" x="606" y="138" text-anchor="middle" style="font-size:12px">apply gestures · advance animations</text>
  <text class="d-sub" x="606" y="153" text-anchor="middle" style="font-size:12px">run_pass → render + effects</text>
  <text class="d-sub" x="606" y="168" text-anchor="middle" style="font-size:12px">→ render only what changed</text>
  <path class="d-flow" d="M540 190 C 500 224, 420 224, 360 200" marker-end="url(#lpF)" stroke-dasharray="4 4" />
  <text class="d-sub" x="452" y="234" text-anchor="middle" style="font-size:12px">arm the next wake, sleep again</text>
  <rect x="240" y="252" width="500" height="26" rx="7" style="fill:#eef2df;stroke:#9aa884;stroke-width:0.8" />
  <text x="490" y="269" text-anchor="middle" style="font-family:var(--mono);font-size:12px;fill:#3c6b39">Idle (no animation · GNSS stopped): ~10 s watchdog-feed guard</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The device sleeps between events. A hardware timer generates the display COM signal without CPU work.</figcaption>
</figure>

[`App::run_pass`](src:firmware/obc-app/src/device_core/pass.rs) takes what the platform finished,
what changed underneath it, and what the rider did. It runs every domain in a fixed order and
returns a plan: what to repaint, when to run again, and at most one bounded effect for each domain.

Each effect carries an operation token, and its answer must return that token. A late answer to a
cancelled operation is therefore refused instead of applied. Effects carry small identifiers and
results; bulk data stays in caller-owned buffers.
[`obc-host-core`](src:host/obc-host-core/src/dispatch.rs) performs the effects for the frame-stepped
hosts. The board performs the same effects with its own asynchronous execution.

## On-device routing: the router seam

Navigator owns the planning lifecycle and issues one physical operation at a time. The
[host executor](src:host/obc-host-core/src/dispatch.rs) and the
[board executor](src:firmware/obc-fw-nrf54l/src/ride.rs) do only the work Navigator asks for. They
never start the next step or publish a route on their own.

| Operation | Executor work | Result Navigator waits for |
| --- | --- | --- |
| Acquire | Claim the workspace and hold the map source, and the route source for a detour. | Sources and workspace are held. |
| Step | Run one bounded search, trim, or preview step against those sources. | Progress, a preview, or a failure. |
| Commit | Publish the completed route or the accepted detour. | The new route identifier, or a failure. |
| Release | Finish or cancel pending work and return the workspace. | The workspace is available. |

Planning stays bound to the sources it started with: the exact map object and route revision. A
changed source invalidates the result. The executor cannot substitute the current map halfway
through a plan, so a preview always describes the route the rider reviewed.

Publication writes a new route object and never overwrites the original. A detour is planned the
same way, and the rider sees its cost before it replaces the active route.

The router projects each route endpoint onto stored road geometry and accepts a road within a fixed
radius. Sparse lookup anchors make long road edges discoverable. The search is profile-weighted A*
with a widening epsilon ladder: a wider bound gives an answer where a tight one runs out of table
space. A fixed search table holds the frontier, so the practical range follows the size of that
table and not a distance limit. The constants are in
[`nav.rs`](src:firmware/obc-route/src/nav.rs).

If the map carries terrain, the planner samples it for route elevations, and the shared integrator
calculates climb and descent. A map without terrain still plans routes.

A **visit** is one complete route: a ride to a mapped approach for the selected place, a return or
forward connection, and the remaining original route. The rider reviews it before it becomes
active, and acceptance is one durable transaction, so a restart can offer to resume it. The
[shared builder](src:firmware/obc-route/src/visit.rs) composes the route, and
[Navigator](src:firmware/obc-app/src/navigator/visit.rs) owns the requests and the phase changes.

## Staying responsive: the two planes

The device runs two cooperating execution planes. The high-priority input plane samples the buttons
and recognizes gestures. The map plane applies the gestures and owns all rendering and panel
output. A bounded channel carries gestures from one to the other.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 330" role="img" aria-label="The input task recognizes gestures while the map task performs a longer render. A bounded channel delivers gestures to the map task. The map task owns rendering and panel output.">
  <defs><marker id="software-architecture-6" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker></defs>
  <text class="d-tag" x="20" y="26" text-anchor="start">Input stays responsive during rendering</text>
  <text class="d-title" x="20" y="84" text-anchor="start">Input task</text>
  <text class="d-sub" x="20" y="104" text-anchor="start">Recognize gestures</text>
<rect class="d-forest" x="220" y="60" width="22" height="32" rx="4" />
<rect class="d-forest" x="284" y="60" width="22" height="32" rx="4" />
<rect class="d-forest" x="348" y="60" width="22" height="32" rx="4" />
<rect class="d-forest" x="412" y="60" width="22" height="32" rx="4" />
<rect class="d-forest" x="476" y="60" width="22" height="32" rx="4" />
<rect class="d-forest" x="540" y="60" width="22" height="32" rx="4" />
<rect class="d-forest" x="604" y="60" width="22" height="32" rx="4" />
<rect class="d-forest" x="668" y="60" width="22" height="32" rx="4" />
  <text class="d-title" x="20" y="197" text-anchor="start">Map task</text>
  <text class="d-sub" x="20" y="217" text-anchor="start">Render and present</text>
  <rect class="d-panel d-focus" x="220" y="170" width="470" height="72" rx="8" />
  <text class="d-title" x="455" y="195" text-anchor="middle">One longer frame</text>
  <text class="d-sub" x="455" y="215" text-anchor="middle">Duration depends on the map and changed rows</text>
  <path class="d-flow" d="M359 100 L359 165" marker-end="url(#software-architecture-6)" />
  <text class="d-sub" x="375" y="132" text-anchor="start">Bounded gesture channel</text>
  <text class="d-sub" x="20" y="284" text-anchor="start">Input sampling can preempt the map task.</text>
  <text class="d-sub" x="20" y="305" text-anchor="start">Panel output remains serialized.</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The input plane recognizes gestures during a map render. The map plane owns all rendering and panel output.</figcaption>
</figure>

A map frame takes hundreds of milliseconds. Without the split, a button press during a frame would
be seen late or lost. The simulator runs the same `InputPlane` inline, because gesture recognition
depends only on raw input and time.

## Source index

- Application and dirty state: [`app.rs`](src:firmware/obc-app/src/app.rs)
- Input recognition: [`input_plane.rs`](src:firmware/obc-app/src/input_plane.rs)
- Display output: [`obc-display`](src:firmware/obc-display)
- Storage: [`obc-storage`](src:firmware/obc-storage)
- Sensor adapters: [`obc-platform`](src:firmware/obc-platform) and [`obc-sensors`](src:firmware/obc-sensors)
- Map formats: [data formats](../formats/)
- Rendering: [rendering pipeline](../rendering/)
- UI: [UI system](../ui/)
- Terrain: [terrain and elevation](../terrain/)
