---
title: System architecture
description: How OpenBikeComputer is organised so a desktop simulator and a microcontroller run the same application and rendering code — the crate graph, the per-frame loop, the seams, and the two-plane input model.
---

# System architecture

The whole project is shaped by one decision: **everything device-specific lives at the edges, and everything in the middle is shared.** The map reader, the route reader, the renderer, and the application logic are one body of `no_std` code that runs *byte-for-byte identically* on the desktop simulator and on the microcontroller. Only the outermost shell — where pixels land, where bytes come from, what a "fix" is — differs between them.

That's what lets the simulator you can [run in your browser](../../) be the real thing rather than a mock-up, and it's what lets the nRF54L firmware reuse the entire stack unchanged. This page is the map of that structure.

## The runtime stack

The crates form a stack with dependencies pointing **one way — downward**. The foundation parses bytes; each layer up adds capability; the two *hosts* sit on top. Nothing in the shared core ever depends on a host.

<figure class="fig">
<svg viewBox="0 0 720 410" role="img" aria-label="The crate dependency stack. At the top, two hosts — obc-sim (desktop and browser) and obc-fw-nrf54l plus obc-platform (device) — both depend on obc-app. obc-app depends on obc-render, which depends on obc-reader and obc-route. obc-route also depends on obc-reader, the foundation. Every arrow points downward, so the shared core never depends on a host.">
  <defs>
    <marker id="aA" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">The runtime stack — every arrow points down</text>

  <!-- hosts -->
  <rect class="d-panel" x="150" y="52" width="180" height="50" rx="10" />
  <text class="d-label" x="240" y="78" text-anchor="middle">obc-sim</text>
  <text class="d-sub" x="240" y="93" text-anchor="middle">desktop + browser</text>
  <rect class="d-panel" x="390" y="52" width="200" height="50" rx="10" />
  <text class="d-label" x="490" y="78" text-anchor="middle">obc-fw-nrf54l</text>
  <text class="d-sub" x="490" y="93" text-anchor="middle">+ obc-platform · device</text>
  <text class="d-tag" x="150" y="44" style="fill:#6b7758">hosts</text>

  <!-- app -->
  <rect class="d-hot" x="150" y="146" width="440" height="56" rx="12" style="fill:#f8efe4" />
  <text class="d-title" x="370" y="172" text-anchor="middle" style="fill:#a9501c">obc-app</text>
  <text class="d-sub" x="370" y="190" text-anchor="middle">camera · screen stack · input · ride tracking — the per-frame driver</text>

  <!-- render -->
  <rect class="d-panel" x="210" y="242" width="320" height="50" rx="10" />
  <text class="d-label" x="370" y="268" text-anchor="middle">obc-render</text>
  <text class="d-sub" x="370" y="283" text-anchor="middle">projection · culling · rasterising</text>

  <!-- foundation -->
  <rect class="d-panel" x="150" y="332" width="200" height="54" rx="10" />
  <text class="d-label" x="250" y="358" text-anchor="middle">obc-reader</text>
  <text class="d-sub" x="250" y="374" text-anchor="middle">OBCM · quadtree · ByteSource</text>
  <rect class="d-panel" x="390" y="332" width="200" height="54" rx="10" />
  <text class="d-label" x="490" y="358" text-anchor="middle">obc-route</text>
  <text class="d-sub" x="490" y="374" text-anchor="middle">OBCR · GPX · map-match</text>

  <!-- arrows (depends-on, downward) -->
  <line class="d-flow" x1="240" y1="102" x2="258" y2="144" marker-end="url(#aA)" />
  <line class="d-flow" x1="490" y1="102" x2="472" y2="144" marker-end="url(#aA)" />
  <line class="d-flow" x1="370" y1="202" x2="370" y2="240" marker-end="url(#aA)" />
  <line class="d-flow" x1="320" y1="292" x2="270" y2="330" marker-end="url(#aA)" />
  <line class="d-flow" x1="430" y1="292" x2="480" y2="330" marker-end="url(#aA)" />
  <line class="d-flow" x1="388" y1="356" x2="354" y2="356" marker-end="url(#aA)" />

  <text class="d-sub" x="610" y="360" style="font-size:11px">offline:</text>
  <text class="d-sub" x="610" y="374" style="font-size:11px">obc-pack</text>
  <text class="d-sub" x="610" y="386" style="font-size:11px">→ obc-reader</text>
</svg>
<figcaption>Hosts depend on <b>obc-app</b>; app on <b>obc-render</b>; render on <b>obc-reader</b> + <b>obc-route</b>; route on reader. Because every arrow points down, the shared core compiles and runs without <i>any</i> host — which is exactly how it's developed on the desktop today. (<b>obc-pack</b>, the offline map packer, shares the reader's format code but isn't part of the runtime stack.)</figcaption>
</figure>

The one-way rule is the load-bearing constraint. `obc-app` builds for the bare-metal target (`thumbv8m.main-none-eabihf`) with no host present; the simulator and the firmware are just two different things that link *against* it. Swap the host, keep the core.

## Two hosts, one core — and the seams between them

A "host" is whatever constructs an [`App`](src:firmware/obc-app/src/app.rs) and drives it. The simulator ([`obc-sim`](src:firmware/obc-sim)) is an `eframe`/`egui` desktop+wasm shell; the device firmware ([`obc-fw-nrf54l`](src:firmware/obc-fw-nrf54l), via [`obc-platform`](src:firmware/obc-platform)) is bare-metal on the nRF54L15. (The seams were first proven on an STM32F429 prototype, since removed; the nRF is what the project ships on, and the *same* core ran unchanged on both.) Each owns its window/panel, its storage, and its sensors — and hands the core four small abstractions. Those four **seams** are the entire device-specific surface area; find them and you've found every boundary that matters.

<figure class="fig">
<svg viewBox="0 0 720 372" role="img" aria-label="The shared core sits in the middle and connects through four seams to each host. DrawTarget carries pixels out (simulator: an RGB888 framebuffer; device: a resident RGB222 framebuffer pushed to the panel a band at a time). The colour function maps a 16-bit colour to a pixel (true-colour or 64-colour in the sim; native RGB222 on the panel). ByteSource brings bytes in (an in-memory slice in the sim; FatFs on the SD card on the device). The HAL traits bring the world in (the control panel, a GPX replay and the keyboard in the sim; GPS, a barometer and GPIO buttons on the device).">
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
  <text class="d-sub" x="110" y="135" text-anchor="middle">RGB888 framebuffer</text>
  <rect class="d-panel" x="520" y="112" width="180" height="38" rx="9" />
  <text class="d-sub" x="610" y="135" text-anchor="middle">RGB222 FB · banded push</text>
  <line class="d-stroke" x1="200" y1="131" x2="298" y2="131" /><line class="d-stroke" x1="422" y1="131" x2="520" y2="131" />

  <!-- 2 color_fn -->
  <rect class="d-panel-2" x="298" y="170" width="124" height="38" rx="9" />
  <text class="d-label" x="360" y="187" text-anchor="middle" style="font-size:11px">color_fn</text>
  <text class="d-sub" x="360" y="200" text-anchor="middle">u16 → pixel</text>
  <rect class="d-panel" x="20" y="170" width="180" height="38" rx="9" />
  <text class="d-sub" x="110" y="193" text-anchor="middle">true-colour / 64-colour</text>
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

  <!-- 4 HAL -->
  <rect class="d-panel-2" x="298" y="286" width="124" height="38" rx="9" />
  <text class="d-label" x="360" y="303" text-anchor="middle" style="font-size:10.5px">HAL traits</text>
  <text class="d-sub" x="360" y="316" text-anchor="middle">sensors · input</text>
  <rect class="d-panel" x="20" y="286" width="180" height="38" rx="9" />
  <text class="d-sub" x="110" y="309" text-anchor="middle">panel · GPX · keys</text>
  <rect class="d-panel" x="520" y="286" width="180" height="38" rx="9" />
  <text class="d-sub" x="610" y="309" text-anchor="middle">GPS · baro · GPIO</text>
  <line class="d-stroke" x1="200" y1="305" x2="298" y2="305" /><line class="d-stroke" x1="422" y1="305" x2="520" y2="305" />
</svg>
<figcaption>The core is generic over a <b>DrawTarget</b> (where pixels go) and takes a <b>colour function</b> (16-bit style colour → this panel's pixel); it reads every map and route through a <b>ByteSource</b> and gets the world through the <b>HAL traits</b>. Implement those four for a new board and the whole stack runs on it — no changes to the core.</figcaption>
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

**The HAL — the world, abstracted.** GPS fixes, the GPS receiver's UTC time, barometric altitude, ambient temperature, the recorded-track log, and raw button/encoder events each arrive through their own trait in [`hal.rs`](src:firmware/obc-app/src/hal.rs), bundled into a `Sensors` set the app polls once per tick. The app integrates each stream on its own asynchronous cadence and is completely oblivious to whether a fix came from a satellite or a GPX replay.

Each `poll` is a **mailbox drain**, not a bus transaction: it returns `Some` only on the tick a fresh sample arrived and `None` between. On the device this is event-driven — a high-priority task drives the I²C bus (a u-blox SAM-M10Q GPS + a Bosch BMP581 altimeter on one shared bus) only when the GPS signals a fix is ready, so there is **zero** bus traffic at the frame rate. That task also makes the sample **coherent**: it reads the barometer on each GPS fix, so position and altitude share one instant. The one tradeoff is that climb then accrues only while fixes arrive — a GPS outage (a tunnel) pauses it — but during an outage there's no position to log anyway. A cold start or a dropout simply yields `None`, so the camera never teleports onto a stale fix.

The same task also **power-manages** the receiver. Continuous tracking draws far more than an idle device should, so once it has a boot fix the GPS is put into deep sleep (a backup mode that keeps its clock and almanac on microamps) whenever a ride isn't running — drawing essentially nothing until you start one, when a poke wakes it for a fast *warm* fix. While riding it runs full-power, or, with the Power screen's power-saver on, the receiver's own low-power tracking mode.

The receiver's UTC time rides the same mailbox model but is published **independent of the position fix** — the GPS resolves time before a 3D lock — so when *Set from GPS* is on (Date & Time settings) the clock can be set during acquisition, while there's still no usable fix. There is no battery-backed RTC, so the wall clock is a stored set-point advanced by the monotonic timer; a GPS stamp simply re-establishes that set-point, and between stamps (or with the option off) it free-runs from the last value.

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
  <text class="d-sub" x="500" y="64" style="font-size:10px;fill:#a9501c">if map dirty</text>
  <text class="d-sub" x="500" y="146" style="font-size:10px;fill:#a9501c">if overlay dirty</text>

  <!-- loop back -->
  <path class="d-flow" d="M619 150 C 619 208, 300 214, 92 214 C 60 214, 60 160, 64 130" marker-end="url(#aC)" stroke-dasharray="3 4" />
  <text class="d-sub" x="340" y="208" text-anchor="middle">next frame</text>
</svg>
<figcaption><code>tick</code> folds sensor samples into the camera and ride stats; <code>handle_input</code> turns the encoder + Back into gestures that drive the screen stack; <code>take_dirty</code> reports a <code>{ map, overlay }</code> change set; the host renders only the parts that changed. On a static screen nothing dirties, so <code>render_map</code> — the expensive part — is skipped entirely.</figcaption>
</figure>

In code, the host loop is almost exactly that diagram:

```rust
loop {
    app.tick(RideClock(now), sensors, route);          // GPS + baro → camera, map-match, ride stats
    app.handle_input(InputClock(now), &mut controls);  // encoder + Back → gestures → screen stack
    let dirty = app.take_dirty();
    if dirty.map     { app.render_map(&mut display, &reader, route, w, h, color_fn); }
    if dirty.overlay { app.render_overlay(&mut display, w, h, color_fn); }
}
```

This **render-on-demand** is the headline power lever. The reflective panel holds its image without power, so the goal is to *not draw*: a parked bike on the Home screen issues no map renders frame after frame — the lone exception is one cheap chrome repaint a minute, when the wall clock ticks the displayed `HH:MM` over (no map data is read, so it costs almost nothing). The map only dirties when something the picture shows actually changes — a fresh fix on a riding screen, an applied gesture, a route load, the clock crossing into a new minute — so redraws happen exactly when the picture would change and never otherwise. (The two clocks are deliberate: ride stats use a sample-relative `RideClock` so a fast GPX replay doesn't distort moving-time, while button holds use a real-time `InputClock`.)

## Staying responsive: the two planes

There's a tension on the device. A dense map frame can take tens of milliseconds to render, but a button press must feel instant. If both lived on one thread, a press during a long render would stutter. So the device splits the work into **two cooperating planes**.

<figure class="fig">
<svg viewBox="0 0 720 290" role="img" aria-label="Two planes. The input plane runs on a high-priority executor as frequent short ticks that sample the buttons, recognise gestures and animate the overlay. The map plane runs one long base-map render of tens of milliseconds. The input plane preempts the map render at intervals, and recognised gestures flow one way down a channel into the map plane. Recognising a gesture never blocks on the render, so input stays responsive while a frame draws; the shared SPI panel is serialised by a bus mutex.">
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
  <text class="d-label" x="372" y="201" text-anchor="middle" style="fill:#a9501c">render base map · 24–51 ms</text>

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
<figcaption>The <b>input plane</b> samples buttons, recognises the five gestures, and repaints the hold-progress overlay — re-pushing just its small screen window, the bulge composited over the map read back from the framebuffer — on a high-priority executor that preempts the CPU-bound <b>map plane</b> every few milliseconds. Recognition is coupled to the map plane only by a one-way <b>gesture channel</b>, so a press lands a frame later without ever blocking on the render; the shared SPI panel + framebuffer are serialised by a <b>bus mutex</b>, so a long render can briefly hold off the overlay repaint (never the recognition). On the simulator both halves run inline; the recognition logic is the same struct either way, so behaviour is identical.</figcaption>
</figure>

The split is a behaviour-preserving relocation: the same `InputPlane` either runs inline (the simulator) or stands alone on the high-priority executor (the device). Because gesture recognition depends only on the raw events plus a clock — never on app state — buffering a gesture and applying it a moment later is identical to applying it inline. That's the property that makes the whole split safe.

## Where this lives

- The per-frame driver, the screen stack, and the dirty tracking: [`obc-app/src/app.rs`](src:firmware/obc-app/src/app.rs)
- The hardware-abstraction traits: [`obc-app/src/hal.rs`](src:firmware/obc-app/src/hal.rs)
- The two-plane input model: [`obc-app/src/input_plane.rs`](src:firmware/obc-app/src/input_plane.rs)
- The byte-streaming seam: [`obc-reader/src/byte_io.rs`](src:firmware/obc-reader/src/byte_io.rs)
- The device host (DrawTarget / ByteSource / sensors over real hardware): [`obc-platform`](src:firmware/obc-platform)

For how the renderer in the middle of this stack actually draws a frame, see the [rendering pipeline](../rendering/). For the binary formats the reader streams, see [data formats](../formats/). For the screen stack and gestures the loop drives, see [the UI system](../ui/).
