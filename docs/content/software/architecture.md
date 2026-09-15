---
title: System architecture
description: The shared runtime, host interfaces, event loop, routing flow, and input architecture.
copy: ai
---

# System architecture

OpenBikeComputer puts hardware-specific code at the system boundary.
The device, simulator, and [browser demo](../../) use the same `no_std` application core.
Each host supplies storage, sensors, input, and display functions.

## Runtime layers

Dependencies point from hosts to the shared core.
The shared core does not depend on a host.

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

The runtime uses these layers:

| Layer | Responsibility |
| --- | --- |
| Hosts | Construct and drive `App`. Provide system functions. |
| `obc-app` | Own ride state, catalogs, screens, and host messages. |
| `obc-render` | Project, select, and draw map features. |
| `obc-reader` | Read OBCM tables, indexes, and chunks. |
| `obc-route` | Read and write routes. Match positions and calculate routes. |
| Foundation crates | Define formats, map-scene interfaces, elevation rules, and ports. |

`App` is the composition root for the shared application.
The [Navigator](src:firmware/obc-app/src/navigator.rs) owns the active route, route matching,
guidance state, route caches, and planner pacing. `App` keeps only tick cadence and one-shot
sensor sampling state.
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

### Random-access data

All large objects use [`ByteSource`](src:firmware/obc-formats/src/io.rs):

```rust
pub trait ByteSource {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error>;
    fn len(&self) -> u64;
}
```

Map cells carry the canonical style ids. The assembler checks all cells and the selected skin
before writing output, including local CLI assemblies. A skin can change drawing order while
preserving those ids. This uses the existing cell bytes and needs no catalog update.

The reader requests only the required tables and chunks.
The device reads these bytes from a flat-store object.
The simulator and browser demo also read their maps through the shared flat store.
At startup, a temporary simulator session imports the map, route and trip inputs into one sparse card.
Map import uses a 16 KiB buffer. Explicit Unix sessions can create or reopen a persistent card.
The browser imports its embedded OBCM into sparse memory pages.
Both hosts then read one pinned object revision through an owned source.
The simulator and its background terrain worker share that source; the last reader releases it and removes the temporary card.
See the [shared host store](src:host/obc-host-core/src/flat_store.rs) and
[map reader](src:host/obc-host-core/src/flat_map.rs).

The host library also provides an explicit persistent card owner on Unix systems.
Simulator maps, routes and trips share this owner. A new card gets a new store identity.
Opening an existing card preserves its store, object, and revision identities.
Creation never overwrites an existing path. Opening never formats an invalid card.
The shared store applies its normal recording recovery during mount.
The owner holds an exclusive file lock until the last object reader closes.
A failed commit can have reached the file. In that case, the owner stops further changes and
requires a fresh mount to select the durable catalog. Existing readers keep their pinned bytes.
Reopening does not import the input files again and does not reset the card.
Persistent Windows cards remain unsupported. Simulator planning and map-referenced altitude use
the terrain inside the same retained map object in both import and reopen sessions. A small tile
cache stays with that exact card, object and revision. A replacement gets a new cache; readers
of the previous revision keep their original bytes until they close. Missing or unreadable
terrain leaves elevation unavailable and keeps the map usable. External terrain sidecars are
not runtime inputs. Peak View retains its separate worker and cache over the selected map.

The browser imports its routes into the same session card as the map. Its
[route repository](src:host/obc-host-core/src/flat_routes.rs) reads committed catalog metadata
and binds active readers to an exact object revision. Computed routes use fresh object IDs.
An explicit route replacement keeps its object ID and advances its revision.
Old readers remain valid until their last lease drops.
Settled frames neither reopen the source nor scan the catalog.

The browser card remains volatile. It allocates memory in 16 KiB pages; released pages remain
available for reuse, so memory use follows the session's high-water mark. The bundled 3,752-byte
route uses one page instead of a retained byte vector.

Catalog deletion retains the selected object kind, so equal numeric IDs in separate repositories
cannot redirect a removal. The domain removes a trip's member routes before the trip object.
A failed member stops that cascade. The trip and remaining members stay stored; an explicit retry
can pass members that were already removed. Catalog scope is published only after complete
route, metadata, trip and ride reads from one unchanged card and catalog sequence.

The shared host dispatcher retries a recording open until the repository confirms that the object exists.
While an open is still owed, append and checkpoint operations report a write failure and keep their samples pending.
Save stops new sample and total accumulation. Recorder first repairs an owed checkpoint and drains
staged samples through acknowledged append results. A short append keeps the remaining samples
and requests a checkpoint before retry. Only an empty staging buffer can proceed to finalization;
the board and host executors do not append samples during that close. A failed close retries only
the close. Discard bypasses the drain and clears staging after confirmed removal.
If opening keeps failing while Save has staged samples, the samples and Save request stay pending.
An empty ride with no object can still end without a saved ride.

The board and shared host card recorder accept a complete staged batch together with its precise App totals. If the remaining
buffer cannot hold the batch, it accepts none of it and checkpoints the previous accepted boundary.
A periodic checkpoint also uses that boundary while samples remain staged. With no staged samples,
a checkpoint can capture newer barometric totals without adding a GPS point. A failed checkpoint
replays its original bytes, totals and start time before it can accept any newer context.

Host adapters can still report a partial append. A medium without durable recovery reports an
unsupported checkpoint. Recorder then continues the pending work without a persistent recovery
claim. The browser memory card first completes the real journal operation; a file card reports a
durable checkpoint only after its storage barrier succeeds.

Native recording and the saved-ride catalog share the same card owner as map, routes, trips and
rides. The [physical recorder](src:host/obc-host-core/src/flat_recorder.rs) reserves one recording
object and keeps only the bounded sample delta, CRC, continuation and final footer in memory.
Checkpoints write the real tail journal. Save journals one footer, then clears the recording flag
on that exact object. GPX export follows the successful card commit; an export failure does not
reverse Save. Folder ride files are import inputs for a new session, not live storage or sync proof.

Before native startup offers recovery, it validates the recording key, sample boundary and
[shared continuation bytes](src:firmware/obc-app/src/recorder/continuation.rs), then confirms the
observed card state with a durability barrier. Continue attaches to the same object and rebases
new sample times from the current pass clock. It retains the original trusted UTC start and the
accepted sensor and barometric totals. Recovery uses bounded journal reads and takes no normal
object-reader slot.

A recovered footer is a pending Save, not a resumable ride. Startup completes its exact catalog
amendment before feeding the App or saved-ride catalog. Failure stops startup; it cannot offer
Continue or append another footer. An uncertain catalog write or failed recovery confirmation
fences every writer on the shared owner until close and reopen. Damaged-ride removal checks the
captured card, object, revision and recording flag under that owner before mutation. Only a real
archive receipt can grant sync proof; the simulator has no direct synced-state control.
The browser uses the same physical recorder and saved-ride reader on its memory card. Each saved
ride ID names a finalized object with recorded samples and totals. The catalog starts empty; it
contains no example sync proof. A new page creates a new card identity and loses the previous
page's objects.

A demo baseline reset preserves committed objects. It first asks Recorder to discard the open
ride and consumes the exact acknowledgment in the old App. Only then can it replace App and the
host loop, seek playback or run the guided pre-roll. Pending or failed cleanup cannot acknowledge
a reset through an old matching screen. Failure keeps the old session available and stops commands
that depend on that reset. A reset also refuses while Navigator owns work or sources that still
need release. Pausing playback or saving a ride does not start another recording.

### Semantic ports

[`obc-ports`](src:firmware/obc-ports/src/lib.rs) defines interfaces for sensors, input, settings, and tracks.
A sensor poll drains a mailbox.
It does not start a bus transaction.
The device sensor task publishes coherent position and altitude samples.

## The per-frame loop

Each host processes sensor data, input, dirty regions, and host messages.

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

Dirty regions reduce processor and display work.
The application reports a wake deadline for visible animations.
The device also wakes for input, sensor data, and the watchdog guard.

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

All platform requests use typed effects and results.
The ride recorder validates its close result.
The [phone-key removal state](src:firmware/obc-app/src/ble.rs) validates one result for each admitted request.
A link disconnect does not prove that stored keys were removed.
The board reports durable-key and host-key removal separately from unconfirmed controller cleanup.

## On-device routing: the router seam

Navigator owns the planning lifecycle. It issues one physical operation and waits for its result.
Each operation has a fresh token. Navigator accepts a reply only when both its token and its phase
match the expected operation. A duplicate step result cannot advance the next step.
The [host executor](src:host/obc-host-core/src/dispatch.rs) and
[board executor](src:firmware/obc-fw-nrf54l/src/ride.rs) perform only the work requested by Navigator.
They do not start another step or publish a completed plan on their own.

| Operation | Executor work | Result Navigator waits for |
| --- | --- | --- |
| Acquire | Claim the workspace and retain the admitted map source. A detour also retains its original route source. | Sources and workspace are held. |
| Step | Run one bounded search, trim, or preview step against those sources. | Search or output progress, a detour preview, or failure. |
| Commit | Publish the completed route, or the detour the rider accepted. | The new durable route identifier, or failure. |
| Release | Finish pending storage work, cancel unused allocations or remove a cancelled publication, then return the workspace. | Cleanup is complete and the arena is available. |

Assistant planning adds an immutable review step. Navigator freezes the request's exact
map and original-route identities, profile, progress occurrence, required anchors, and facts
policy. The executor publishes one complete candidate, releases planner memory, and keeps
the admitted source leases. Preview does not change the active route or Recorder.

Accept checks the current origin and sources again. Navigator's small Metadata handshake
asks the existing serialized writer to update the optional checkpoint. The same transaction
marks the exact route catalog entry as accepted. The Metadata image preserves all 128 ride
archive proof rows. Only a verified durable acknowledgment activates the candidate. A
known failure keeps the preview available for retry. An uncertain publication keeps a
fence until recovery; cancellation does not assume that the publication failed. A complete
read after remount of the same card can resolve the pending edit against either its old
or proposed checkpoint. The shared writer resumes only after that read and its durability
barrier succeed. A queued cancellation clears a recovered accepted checkpoint before
candidate retirement. Reopened host sources must match the frozen map and route identities,
including stored payload length and CRC. A different host owner does not make unchanged
bytes stale, and a changed source cannot authorize the preview.

A candidate remains marked in its immutable route bytes. Its accepted catalog flag makes
it available as an ordinary route. Without that flag, the route list labels it as an
unaccepted preview and does not activate it. Acceptance binds the current revision, length,
and CRC. A replacement payload cannot inherit the flag. Clearing the checkpoint preserves
accepted route eligibility.

At boot, a verified checkpoint only offers **Resume**. It does not restore ordinary
navigation or start Recorder. Explicit Resume requires a matching phase occurrence,
current map binding, and re-verified route bytes. An accepted visit also protects its
original route from explicit cleanup, deletion, and replacement until that dependency is released
by a durable phase update. New fixes received during a phase write remain available to
the phase owner after acknowledgment.

A replacement plan cannot acquire the workspace before the previous release is acknowledged.
Release after successful planning keeps the accepted route or detour preview. Releasing working
memory is not a cancellation result. Workspace refusal, planner failure and storage failure remain distinct.

Planning stays bound to its admitted sources. The board checks its boot-long map lease against
the exact card, map object and revision. Both executors retain the exact original route through detour preview and commit.
The host also retains its admitted map lease. A changed current source invalidates the
result; the executor cannot substitute the latest map or route halfway through the operation.

Publication creates a fresh OBCR object. It does not overwrite the original
route. The standard catalog and route-load path handles the completed object. Flat-store
publication requires capacity for both the publication commit and a cleanup commit if cancellation
arrives after publication. Cleanup removes only the result of that cancelled operation.

The [board detour executor](src:firmware/obc-fw-nrf54l/src/detour.rs) writes a temporary leg into
one of the card store's two reservation slots. It seals the leg before reading it. A sealed leg has
one owner and cannot be changed by an old write token. It is not a catalog object.

Trim and splice run in bounded phases. If trim finds sustained contact with the route tail, it
writes the shorter leg into the other slot, seals it, and releases the first leg. An optional trim
failure keeps the original leg and its preview figures. Preview retains only the sealed leg, the
original route lease, and small frozen figures. It releases the arena so rendering can resume.

When the rider accepts the preview, commit takes the same arena again. It reads the original and
sealed leg while it writes the spliced route into the other reservation. A failed commit keeps the
preview for retry. Cancellation drains any pending writer request before it releases the arena.
The same 128 KiB arena serves planning, transforms, rendering, and cable transfer; the detour path
adds no permanent route index or output buffer.

The [resource guard](src:firmware/tools/resource_guard.py) reads the linked Cortex-M33 instructions.
Its fixed stack-entry measurements include split local allocations and saved integer and
floating-point registers. Each selected direct-call chain counts those entry costs once, including
the task entry. Parsing stops when the function body begins. These checks do not measure body
stack adjustments, indirect calls, interrupt preemption, or physical stack high-water. A passing
single-entry check does not prove that a nested call path fits the available stack.

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
The map plane applies gestures and owns all rendering and panel output.
A bounded channel sends gestures from the input plane to the map plane.

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
