---
title: The UI system
description: How OpenBikeComputer's on-device interface works — screens as plain values, navigation as a return value, five gestures from two buttons, in-place settings editing that persists to RRAM, render-on-demand, and the "field map" look — all no_std and zero-allocation.
---

# The UI system

The device has two buttons, a 240×320 reflective panel, and a microcontroller with no room to waste. The interface that runs on it is deliberately small: `no_std`, **zero-allocation**, and built with **no retained widget tree** — no DOM, no `Box<dyn Widget>`, no per-frame layout pass. A screen is just a value; navigation is just a return value; and the whole thing runs identically in the browser simulator and on the device.

This page is about *how that works* — the handful of abstractions that make a real navigable UI out of almost nothing.

## A screen is a value, not a widget tree

The core idea: each screen is an enum variant wrapping a little struct of typed state, and the set of screens is one `enum Screen` dispatched by `match`. The enum, its `handle`/`draw` delegation matches, and each screen's classification are all generated from a single declarative `screens!` table — one row per screen — so there are no trait objects, no heap, and no second list to keep in sync: adding a screen is **one table row plus its module**.

<figure class="fig">
<svg viewBox="0 0 720 322" role="img" aria-label="On the left, the Screen enum lists representative variants: Home, Map, Statistics, RideControl, Menu, RideMenu, RouteMenu, RouteOverview, and RouteSwap. The Map variant points to its module on the right, which holds typed state, a handle method returning a Transition, and a draw method emitting pixels. A tag notes static match dispatch, no dyn and no allocation.">
  <defs>
    <marker id="aU1" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">A screen is a value — no retained widget tree</text>

  <!-- enum Screen -->
  <rect class="d-panel" x="36" y="44" width="210" height="254" rx="11" />
  <text class="d-label" x="56" y="66">enum Screen</text>
  <g font-family="var(--mono)">
    <rect x="52" y="78"  width="178" height="22" rx="5" class="d-hot-fill" /><text class="d-sub" x="62" y="93" style="fill:#fff">Map(MapScreen)</text>
    <rect x="52" y="104" width="178" height="20" rx="5" class="d-muted" /><text class="d-sub" x="62" y="118">Home(HomeScreen)</text>
    <rect x="52" y="126" width="178" height="20" rx="5" class="d-muted" /><text class="d-sub" x="62" y="140">Statistics(…)</text>
    <rect x="52" y="148" width="178" height="20" rx="5" class="d-muted" /><text class="d-sub" x="62" y="162">RideControl(…)</text>
    <rect x="52" y="170" width="178" height="20" rx="5" class="d-muted" /><text class="d-sub" x="62" y="184">Menu(…)</text>
    <rect x="52" y="192" width="178" height="20" rx="5" class="d-muted" /><text class="d-sub" x="62" y="206">RideMenu(…)</text>
    <rect x="52" y="214" width="178" height="20" rx="5" class="d-muted" /><text class="d-sub" x="62" y="228">RouteMenu(…)</text>
    <rect x="52" y="236" width="178" height="20" rx="5" class="d-muted" /><text class="d-sub" x="62" y="250">RouteOverview(…)</text>
    <rect x="52" y="258" width="178" height="20" rx="5" class="d-muted" /><text class="d-sub" x="62" y="272">RouteSwap(…)</text>
  </g>

  <!-- arrow to module -->
  <line class="d-flow" x1="232" y1="89" x2="320" y2="120" marker-end="url(#aU1)" />

  <!-- module -->
  <rect class="d-panel-2" x="324" y="62" width="360" height="150" rx="11" />
  <text class="d-label" x="344" y="84">screen/map.rs</text>
  <g font-family="var(--mono)">
    <rect x="340" y="96"  width="328" height="32" rx="6" style="fill:#eef2df;stroke:#9aa884;stroke-width:1" />
    <text class="d-sub" x="352" y="110">struct MapScreen { … }</text>
    <text class="d-sub" x="352" y="123" style="fill:#a9501c;font-size:9px">typed state — owned, inline, no alloc</text>
    <rect x="340" y="134" width="328" height="32" rx="6" style="fill:#eef2df;stroke:#9aa884;stroke-width:1" />
    <text class="d-sub" x="352" y="148">fn handle(g, &amp;mut Ctx) → Transition</text>
    <text class="d-sub" x="352" y="161" style="fill:#a9501c;font-size:9px">logic — react to a gesture, ask to navigate</text>
    <rect x="340" y="172" width="328" height="32" rx="6" style="fill:#eef2df;stroke:#9aa884;stroke-width:1" />
    <text class="d-sub" x="352" y="186">fn draw(target, &amp;Render)</text>
    <text class="d-sub" x="352" y="199" style="fill:#a9501c;font-size:9px">pixels — read state, paint the panel</text>
  </g>

  <text class="d-tag" x="324" y="240">dispatched by match — static, zero-alloc</text>
  <text class="d-sub" x="324" y="258" style="font-size:11px">add a screen = 1 module + 1 row in the screens! table</text>
</svg>
<figcaption>Every screen owns its state by value and answers two calls: <b>handle</b> (react to a gesture) and <b>draw</b> (paint). The <code>Screen</code> enum forwards both through a <code>match</code> — static dispatch, no <code>dyn</code>, no allocation. The enum and its delegation matches expand from one <code>screens!</code> table, and there's no widget tree to retain between frames.</figcaption>
</figure>

```rust
// The one screen table. Each row declares a variant, its state type, and its capabilities (Caps);
// a dumb local macro expands it into the Screen enum, the handle/draw/prepare delegation matches,
// and the per-screen Caps table that every cross-cutting UI policy reads.
screens! {
    Home(HomeScreen) => Caps::nav().timed(),         // screensaver clock ticks each minute → timed
    Map(MapScreen) => Caps::map().timed(),           // reads the map Reader; a ride view + browse-exempt
    Statistics(StatisticsScreen) => Caps::riding().timed(),
    RideControl(RideControl) => Caps::nav().ride_view().hold_fill(), // the Paused page; guarded Finish/Discard
    RideStart(RideStartScreen) => Caps::nav(),       // the browse map's start card (route-less ride)
    Menu(MenuScreen) => Caps::nav().timed(),          // the compass dial sweeps its needle → timed
    RideMenu(RideMenuScreen) => Caps::nav().timed(),  // the same dial chrome, with ride-scoped stations
    RideWaypoints(RideWaypointsScreen) => Caps::nav(),
    Detour(DetourScreen) => Caps::map().remap(RemapKind::Route),        // live route index follows rescans
    DetourPreview(DetourPreviewScreen) => Caps::map().remap(RemapKind::Route), // the planned detour + cost line
    PoiMenu(PoiMenuScreen) => Caps::nav(),           // POIs browser: the category list
    PoiList(PoiListScreen) => Caps::nav().reader(ReaderNeed::PoiSnapshot),  // one-shot nearest-16 query
    PoiDetail(PoiDetailScreen) => Caps::nav().reader(ReaderNeed::PoiHours), // one-shot opening-hours read
    RouteMenu(RouteMenuScreen) => Caps::nav().remap(RemapKind::Route),      // holds a route index (rescan remap)
    Rides(RidesScreen) => Caps::nav().remap(RemapKind::Ride),
    RideDetail(RideDetailScreen) => Caps::nav().timed().hold_fill().remap(RemapKind::Ride),
    RouteOverview(RouteOverviewScreen) => Caps::nav().timed().hold_fill().remap(RemapKind::Route),
    RouteSwap(RouteSwapScreen) => Caps::nav().exempt().timed().hold_fill().remap(RemapKind::Route),
    // Event-opened cards — raised by something happening, not a gesture: the BLE seam (route uploads
    // + pairing, see "Screens the companion link pushes") or the storage/sensor path (the warning
    // card). `modal()` = idle-return exempt: the timeout never yanks one away until it's dismissed.
    RouteReceived(RouteReceivedScreen) => Caps::modal().timed().remap(RemapKind::Route),
    Passkey(PasskeyScreen) => Caps::modal(),          // the 6-digit pairing code, modal + non-dismissible
    Warning(WarningScreen) => Caps::modal(),          // advisory: missing sensor / slow map / write error
    // The Settings tree. `settings()` is what holds the debounced settings save while one is on top;
    // a guarded row adds `.hold_fill()` (the factory Reset, Forget phone, Fields delete, forget sensor).
    Settings(SettingsScreen) => Caps::settings(),      Ride(RideScreen) => Caps::settings(),
    Reset(ResetScreen) => Caps::settings().hold_fill(), Bluetooth(BluetoothScreen) => Caps::settings().hold_fill(),
    // Five groups: Ride, Display, Connections (→ Bluetooth / Sensors), Power, System (→ Units,
    // Date&Time, Language, Firmware, Reset), plus Ride's Fields → AddField editor. All Caps::settings().
}

// Each variant is a module with typed state and two methods (plus an optional third for the two POI
// screens — `prepare`, which resolves a Reader-backed one-shot before drawing, see "the POIs browser").
impl MenuScreen {
    fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition { /* logic  */ }
    fn draw(&self, cv: &mut impl Surface, rx: &mut Render)       { /* pixels */ }
}
```

Every cross-cutting behavior — which screen counts as live data, which is idle-return exempt, which
needs the map `Reader`, which has a timer or a guarded hold, which remaps catalog indices after a
rescan — is a **capability on the row**, not a `matches!` scattered across the app. Adding a
cross-cutting policy is an explicit new field on `Caps`, exhaustively matched, so a screen can't be
silently forgotten; the invariant tests enumerate the whole table and check the combinations are
consistent (a Reader-needing map screen, a modal that isn't a ride view, and so on).

## Navigation is a return value

A screen never reaches out and changes the UI. It *returns* what it wants — a `Transition` — and a tiny `apply` function runs that against the screen **stack** (a `heapless::Vec<Screen, 10>`). The bottom of the stack is always Home, which is never popped, so `back` always has somewhere to go and the stack can never empty.

<figure class="fig">
<svg viewBox="0 0 720 330" role="img" aria-label="A pipeline across the top: a gesture goes into the top screen's handle method, which returns a Transition, which apply runs against the stack. Below, the screen stack with Home locked at the bottom, then Map, then Ride menu on top. To the right, the six transitions are listed as stack operations: None stays, Push grows, Pop shrinks, Replace swaps the top, Root truncates to Home then pushes, and Home truncates to the root.">
  <defs>
    <marker id="aU2" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Navigation is a return value</text>

  <!-- pipeline -->
  <g>
    <rect class="d-panel-2" x="24" y="40" width="120" height="34" rx="8" /><text class="d-label" x="84" y="61" text-anchor="middle">gesture</text>
    <line class="d-flow" x1="146" y1="57" x2="178" y2="57" marker-end="url(#aU2)" />
    <rect class="d-panel-2" x="180" y="40" width="170" height="34" rx="8" /><text class="d-sub" x="265" y="61" text-anchor="middle">top.handle(g, Ctx)</text>
    <line class="d-flow" x1="352" y1="57" x2="384" y2="57" marker-end="url(#aU2)" />
    <rect class="d-hot" x="386" y="40" width="120" height="34" rx="8" style="fill:#f8efe4" /><text class="d-label" x="446" y="61" text-anchor="middle" style="fill:#a9501c">Transition</text>
    <line class="d-flow" x1="508" y1="57" x2="540" y2="57" marker-end="url(#aU2)" />
    <rect class="d-panel-2" x="542" y="40" width="154" height="34" rx="8" /><text class="d-sub" x="619" y="61" text-anchor="middle">apply(&amp;mut stack)</text>
  </g>

  <!-- stack -->
  <text class="d-tag" x="24" y="104">the stack</text>
  <g>
    <rect x="40" y="200" width="150" height="34" rx="6" class="d-water" /><text class="d-label" x="115" y="222" text-anchor="middle" style="fill:#fff">Ride menu</text>
    <text class="d-sub" x="200" y="221" style="font-size:9px">← top (gets input)</text>
    <rect x="40" y="166" width="150" height="34" rx="6" class="d-forest" /><text class="d-label" x="115" y="188" text-anchor="middle" style="fill:#fff">Map</text>
    <rect x="40" y="132" width="150" height="34" rx="6" class="d-muted" /><text class="d-label" x="115" y="154" text-anchor="middle">Home</text>
    <text class="d-sub" x="200" y="153" style="font-size:9px">← root · never popped</text>
  </g>
  <text class="d-sub" x="40" y="256" style="font-size:10px">back from the root is the guaranteed escape</text>

  <!-- transition legend -->
  <g font-family="var(--mono)">
    <text class="d-label" x="360" y="116" style="font-size:11px">None</text>    <text class="d-sub" x="470" y="116">stay — handled in place</text>
    <text class="d-label" x="360" y="146" style="font-size:11px">Push(s)</text> <text class="d-sub" x="470" y="146">grow — open an overlay / go forward</text>
    <text class="d-label" x="360" y="176" style="font-size:11px">Pop</text>     <text class="d-sub" x="470" y="176">shrink — back to the caller</text>
    <text class="d-label" x="360" y="206" style="font-size:11px">Replace(s)</text><text class="d-sub" x="470" y="206">swap the top — sibling move</text>
    <text class="d-label" x="360" y="236" style="font-size:11px">Root(s)</text> <text class="d-sub" x="470" y="236">truncate to Home, then push</text>
    <text class="d-label" x="360" y="266" style="font-size:11px">Home</text>    <text class="d-sub" x="470" y="266">clear all overlays to the root</text>
  </g>
</svg>
<figcaption>The top screen handles a gesture and returns a <code>Transition</code>; <code>apply</code> is the one place the stack mutates. Because the whole vocabulary of navigation is this six-variant enum, every flow in the UI — overlays, back, sibling swaps, "load a route and ride" — is expressible without any screen knowing what's above or below it.</figcaption>
</figure>

```rust
pub enum Transition {
    None,            // gesture handled in place
    Push(Screen),    // open an overlay / navigate forward
    Pop,             // back — return to the screen that opened this one
    Replace(Screen), // swap the top without growing the stack (Map ↔ Elevation)
    Root(Screen),    // truncate to the Home root, then push — "load a route and ride"
    Home,            // clear every overlay back to Home (Finish / Discard)
}

pub fn apply(stack: &mut Stack, t: Transition) {
    match t {
        Transition::Push(s)    => { let _ = stack.push(s); }
        Transition::Pop        => { if stack.len() > 1 { stack.pop(); } } // root is never popped
        Transition::Replace(s) => { if let Some(top) = stack.last_mut() { *top = s; } }
        Transition::Root(s)    => { stack.truncate(1); let _ = stack.push(s); }
        Transition::Home       => stack.truncate(1),
        Transition::None       => {}
    }
}
```

Here's a whole screen's logic — the main Menu, a compass dial whose amber needle sweeps to the
selected station — to show how little a screen has to say. Even with an animation, the logic is
three lines per gesture: the sweep itself runs through the same timer-poll contract the Home
clock uses (see *Render on demand* below), so it costs nothing once the needle has landed:

```rust
fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
    match g {
        Gesture::Turn(n) => self.dial.turn(n), // shared selection wrap + eased needle target
        Gesture::Press   => match self.dial.selected() {
            0 => Transition::Push(Screen::RouteMenu(RouteMenuScreen::new())), // Routes
            1 => Transition::Push(Screen::Rides(RidesScreen::new())),         // Rides
            2 => Transition::Push(Screen::PoiMenu(PoiMenuScreen::new())),     // POIs
            3 => open_map(cx),                                                // Map — browse map / ride base
            _ => Transition::Push(Screen::Settings(SettingsScreen::new())),   // Settings
        },
        Gesture::Back    => Transition::Pop, // return to whoever opened the Menu
        _ => Transition::None,
    }
}
```

Four of the five stations open a menu; the **Map** station opens the riding map directly. Which map depends on whether a ride is already being tracked. Mid-ride it lands you back on the live riding map — the ride base — by rooting the stack to a clean `[Home, Map]`, the same normalization the [idle return](#the-whole-flow) does, so it never stacks a second Map or leaves stale menus buried underneath. With **no** ride running it opens a route-less **browse map**: the identical Map screen — GPS-follow camera, zoom on turn, `hold` to Pan — reached without a route or a session, for reading the map while riding without recording. On entry the browse map briefly shows a one-shot bottom-centre *Press to start a ride* hint (auto-hiding after a few seconds — the lowest-priority tenant of the bottom chip slot). On the browse map `back` pops back to the Menu (there's no Statistics sibling without a ride) and `press` opens the **start card** — a small pre-ride launchpad: the selected bike's pixel sprite and profile name (the same sprite the Bike type settings screen draws), a two-row *GPS* / *Battery* checklist (the live fix state and the battery percent), then *Start ride* / *Back* rows. *Start ride* begins a route-less tracking session (the same session-begin the Route overview's START runs, minus the route) and roots to `[Home, Map]`. A route-less ride records and saves exactly like a guided one; only the route-relative stat tiles (*to go*, *to climb*, grade) read `--`, and the Statistics band shows a "No route loaded" note over an otherwise-live grid. The browse map is a *deliberate* view, so — unlike a menu left open — the idle-return timeout leaves it be.

Once a ride is running, `back-hold` opens a second compass with the **same five-detent bezel, amber needle sweep, and label strip**, but ride-scoped stations: **Waypoints**, **Detour**, **POIs**, **Routes**, and **Main menu**. Waypoints starts at north; one counter-clockwise detent reaches Main menu. **Waypoints** opens the route's whole-plan list — names, distance-to-go and climb-to-go — on the next stop; Detour opens the rejoin chooser below; POIs and Routes open their existing browsers. A route-less recording keeps the same five positions: Waypoints and Detour stay dimmed and Waypoints opens *No waypoints / No route loaded*, so the ring never changes under the rider's hand. Detour additionally dims on a map without a routing graph and while the rider is off the route — the station is actionable exactly when a detour could actually be planned.

### Detouring around a blocked stretch

**Detour** routes the rider *around* a closed or unpleasant stretch of the active route: pick how far ahead to rejoin, let the device plan a real path to that point on the map's routing graph, preview its shape and distance cost, and commit — after which the detour simply **is** the route and normal guidance continues through it. Opening the station replaces the transient ride compass with a north-up map chooser. Each encoder detent adds or removes **100 m** of along-route rejoin distance, starting from a 600 m minimum (below it, the planner's take-off and landing neighbourhoods overlap the whole stretch and a "detour" would just re-follow the route); an orange inner stroke marks the entire skipped stretch without hiding the normal magenta route, a ring marks the candidate rejoin point, and the floating bottom panel shows the *actual* clamped distance. The rider's distance choice does the semantic heavy lifting: the device cannot know how far a real-world closure extends, so escalating the rejoin distance — and replanning — is the escape hatch when the first detour is still blocked. The camera continuously fits the rider, the whole selected stretch, and a margin into the map area above that panel, so extending the selection never pushes either end out of view.

An encoder **hold** toggles a second, candidate-centred inspection view. The selected rejoin point does not move; `turn` now zooms out/in around its ring across a bounded range that extends slightly wider than the fitted overview. The panel keeps showing the selected distance without exposing an implementation-level zoom number. Hold again returns to the fitted overview. `Press` commits from either view and `back` cancels the chooser, so inspection adds precision without adding another confirmation step or changing the existing escape path.

`Press` freezes the request — the rider's along-route position, the chosen rejoin distance, and the current fix — and hands it to the host, which plans on the map's **§8 navigation graph** while the shared spinning-needle wait shows detour copy (`back` cancels it, detents intact). The skipped stretch is not resolved to graph edges by id — a route polyline is planner GPX, not guaranteed graph-aligned — but blacklisted **geometrically**: the span is downsampled into a small corridor, and the search skips any edge *both* of whose endpoints hug it within ~40 m. Both-endpoints proximity doubles as the parallelism test: the blocked road and a street hugging it are skipped, while a grade-separated bridge *crossing* the corridor (its endpoints off to either side) and the junction edges that leave the road stay usable. Discs around the snapped start and rejoin nodes stay exempt so the search can always take off and land on the route itself.

A successful plan lands on the **preview**: the detour's decimated polyline in **blue** — the replanned portion reads apart from the magenta line it will replace — over the warning-coloured skipped stretch, and the panel's one honest figure — the signed distance cost (*+434 m*: detour length minus the stretch it replaces). It is deliberately distance-only: the routing graph carries no elevation, and a made-up climb figure would be worse than none. `Press` commits; `back` returns to the chooser. A failed plan (no path, or the search exhausting its fixed table) shows the routing-failure card with the one remedy that actually helps either way — *try a farther rejoin*.

Committing **splices**: the device streams a new derived route — the ridden part up to the rider, the detour, then the original route from the rejoin point — into the reserved computed-route slot, re-adopts it as the active route (the recording session, breadcrumb, and ride totals are untouched), and drops the rider back on the exact riding view that opened the compass. Waypoints on the avoided stretch are dropped, the rest keep their positions on the new distance axis; climbs and the elevation profile rebuild from the spliced geometry (the detour's own elevation is interpolated between its endpoints — the graph has none to offer). The matcher re-anchors at the splice seam with a forward-only floor, so GPS jitter can never snap navigation back into a stretch that no longer exists. There is no separate "pure skip" mode and no swap-back dance: the spliced route is an ordinary route file, and it survives a power cycle like any other.

The chooser follows the live progress anchor even while it is open, and `Turn` followed immediately by `Press` cannot plan against a stale rendered candidate. A live catalog reorder rekeys the open chooser, a queued plan request, and the preview by the route's durable id; if that route vanished — or the rider passes the rejoin point while deciding — the flow cancels back rather than committing a stale plan. It is also guarded while the rider is already off-route and inside the final stretch where no non-degenerate rejoin remains. Commit and cancel both pop back to the exact riding view that opened the compass — Map, Statistics, Climb, or the Paused page.

## Two buttons, five gestures

The physical input model is tiny: a **rotary encoder** (which turns *and* pushes) and a dedicated **Back** button. A single shared recognizer — the same code on the simulator and the MCU — turns the raw stream of detents and button edges, plus a millisecond clock, into exactly **five gestures**. A screen's `handle` only ever sees these five.

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="Two controls on the left — a rotary encoder that turns and pushes, and a Back button — feed a shared Gestures recognizer in the middle, which also takes a millisecond clock. It emits five gestures on the right: Turn of n, Press, Hold, Back, and BackHold.">
  <defs>
    <marker id="aU3" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Two buttons → one recognizer → five gestures</text>

  <!-- encoder -->
  <circle cx="92" cy="86" r="30" style="fill:#eae4cb;stroke:#5f7d3d;stroke-width:1.6" />
  <circle cx="92" cy="86" r="11" class="d-forest" />
  <path d="M62 70 A34 34 0 0 1 78 56" fill="none" stroke="#3c6b39" stroke-width="2" marker-end="url(#aU3)" />
  <path d="M122 102 A34 34 0 0 1 106 116" fill="none" stroke="#3c6b39" stroke-width="2" marker-end="url(#aU3)" />
  <text class="d-label" x="92" y="140" text-anchor="middle">Encoder</text>
  <text class="d-sub" x="92" y="154" text-anchor="middle">turn + push</text>

  <!-- back -->
  <rect x="60" y="176" width="64" height="34" rx="8" class="d-panel-2" />
  <text class="d-label" x="92" y="198" text-anchor="middle">Back</text>

  <!-- recognizer -->
  <line class="d-flow" x1="140" y1="86" x2="246" y2="120" marker-end="url(#aU3)" />
  <line class="d-flow" x1="128" y1="193" x2="246" y2="150" marker-end="url(#aU3)" />
  <rect class="d-hot" x="250" y="96" width="170" height="74" rx="12" style="fill:#f8efe4" />
  <text class="d-title" x="335" y="126" text-anchor="middle" style="fill:#a9501c">Gestures</text>
  <text class="d-sub" x="335" y="144" text-anchor="middle">raw events + ms clock</text>
  <text class="d-sub" x="335" y="158" text-anchor="middle">shared: sim = MCU</text>

  <!-- gestures out -->
  <line class="d-flow" x1="420" y1="133" x2="486" y2="133" marker-end="url(#aU3)" />
  <g font-family="var(--mono)">
    <rect x="496" y="50"  width="180" height="26" rx="6" class="d-panel-2" /><text class="d-sub" x="506" y="67">Turn(n) — detents</text>
    <rect x="496" y="82"  width="180" height="26" rx="6" class="d-panel-2" /><text class="d-sub" x="506" y="99">Press — short encoder</text>
    <rect x="496" y="114" width="180" height="26" rx="6" class="d-panel-2" /><text class="d-sub" x="506" y="131">Hold — long encoder</text>
    <rect x="496" y="146" width="180" height="26" rx="6" class="d-panel-2" /><text class="d-sub" x="506" y="163">Back — short back</text>
    <rect x="496" y="178" width="180" height="26" rx="6" class="d-panel-2" /><text class="d-sub" x="506" y="195">BackHold — long back</text>
  </g>
</svg>
<figcaption>Recognition depends only on the raw events and the clock — never on app state — so the gestures can be recognised on a high-priority executor and handed to the screens later without any difference in behaviour. <b>Press</b> fires on release before the long-press threshold; <b>Hold</b> fires the instant the threshold is crossed <i>while still held</i>, so a held action commits without waiting for release.</figcaption>
</figure>

```rust
pub enum Gesture { Turn(i32), Press, Hold, Back, BackHold }
```

That the recognizer is fed by an injected `InputSource` is the key boundary: on the device [`ButtonInput`](src:firmware/obc-platform/src/button_input.rs) turns GPIO levels into the port's raw events; in the simulator [`DeviceInput`](src:firmware/obc-sim/src/device_input.rs) does the same for the control-panel knob and keyboard. Both implement [`obc-ports::InputSource`](src:firmware/obc-ports/src/lib.rs) directly, so neither the recognizer nor any screen knows which host supplied it. (The location, altimeter and track sinks cross the same kind of [HAL seam](../architecture/#two-hosts-one-core-and-the-seams-between-them).)

## Hold to confirm

Some actions are irreversible — finishing or discarding a ride. Rather than a modal "are you sure?", the UI uses a **guarded-action** pattern that's reusable across screens: a guarded option fires only on a *completed* `Hold`, and its row fills with a warning bar tracking the live hold-progress. Let go early and nothing happens — the recognizer makes that clean at the gesture level: a press is a `Press` only if released within a brief tap window (~200 ms); released *after* the window but *before* the hold completes, it's a **cancelled long-press** that yields nothing, never a surprise tap.

A hold is also cancelled if the **screen stack changes** while it charges — the two buttons recognise independently, so a Back tap can dismiss a popup mid-hold, and a long-press that started over one screen must never complete onto whatever replaced it (a hold aimed at a prompt's "Finish & new" landing on another screen's guarded row could fire an action the rider never aimed at). The transition cancels the in-flight hold; the bar retracts, the release stays silent, and the next press starts clean.

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="Top: a timeline showing the encoder pressed down. A release within the 200ms tap window yields a Press; a release after the window but before the 500ms hold threshold is a cancelled long-press and yields nothing; holding past 500ms yields a Hold the instant it crosses. Bottom: a Discard row filling left to right with a warning bar at 0 percent, 60 percent holding, and 100 percent commit.">
  <text class="d-tag" x="20" y="24">Hold to confirm — the guarded-action pattern</text>

  <!-- timeline -->
  <line x1="40" y1="70" x2="540" y2="70" stroke="#9aa884" stroke-width="1.5" />
  <circle cx="40" cy="70" r="5" class="d-forest" /><text class="d-sub" x="40" y="58" text-anchor="middle" style="font-size:9px">down</text>
  <!-- thresholds: the tap window and the hold threshold -->
  <line x1="168" y1="56" x2="168" y2="84" stroke="#9aa884" stroke-width="1.4" stroke-dasharray="3 3" />
  <text class="d-sub" x="168" y="98" text-anchor="middle" style="font-size:9px">tap · 200 ms</text>
  <line x1="360" y1="56" x2="360" y2="84" stroke="#c0492e" stroke-width="1.6" stroke-dasharray="3 3" />
  <text class="d-sub" x="360" y="98" text-anchor="middle" style="fill:#c0492e;font-size:9px">hold · 500 ms</text>
  <!-- press branch (release within the tap window) -->
  <circle cx="108" cy="70" r="5" class="d-amber" />
  <text class="d-sub" x="108" y="58" text-anchor="middle" style="font-size:9px">release</text>
  <text class="d-label" x="108" y="36" text-anchor="middle" style="font-size:11px">→ Press</text>
  <!-- cancelled branch (release between the two thresholds) -->
  <circle cx="264" cy="70" r="5" class="d-muted" />
  <text class="d-sub" x="264" y="58" text-anchor="middle" style="font-size:9px">release</text>
  <text class="d-sub" x="264" y="36" text-anchor="middle" style="font-size:11px">→ nothing</text>
  <!-- hold branch -->
  <circle cx="430" cy="70" r="5" class="d-hot-fill" />
  <text class="d-label" x="470" y="62" style="font-size:11px;fill:#a9501c">→ Hold fires</text>
  <text class="d-sub" x="470" y="78" style="font-size:9px">(commits, still held)</text>

  <!-- confirm fill states -->
  <text class="d-tag" x="20" y="138">the selected row, as you hold</text>
  <g>
    <!-- 0% -->
    <rect x="40" y="152" width="200" height="34" rx="6" class="d-muted" />
    <text class="d-label" x="56" y="174">Discard</text>
    <text class="d-sub" x="140" y="202" text-anchor="middle" style="font-size:9px">0% — idle</text>
    <!-- 60% -->
    <rect x="260" y="152" width="200" height="34" rx="6" class="d-muted" />
    <rect x="260" y="152" width="120" height="34" rx="6" style="fill:#c0492e" />
    <text class="d-label" x="276" y="174" style="fill:#fff">Discard</text>
    <text class="d-sub" x="360" y="202" text-anchor="middle" style="font-size:9px">holding — release = cancel</text>
    <!-- 100% -->
    <rect x="480" y="152" width="200" height="34" rx="6" style="fill:#c0492e" />
    <text class="d-label" x="496" y="174" style="fill:#fff">Discard ✓</text>
    <text class="d-sub" x="580" y="202" text-anchor="middle" style="font-size:9px">100% — committed</text>
  </g>
</svg>
<figcaption>The recognizer emits <code>Hold</code> <i>exactly</i> when the press completes, so a screen's <code>Hold</code> arm <b>is</b> the confirmation — there's no separate "did they really hold long enough?" check. The fill is driven by the live hold-progress (0–1) the input plane exposes, so the bar and the commit are always in sync.</figcaption>
</figure>

```rust
Gesture::Hold => match self.selected {     // reaching this arm IS the confirmation
    FINISH  => self.end_ride(cx, TrackAction::Save),
    DISCARD => self.end_ride(cx, TrackAction::Discard),
    _ => Transition::None,                 // Resume isn't guarded — a hold does nothing
},
```

The factory **Reset** screen is the one place a hold guards a *destructive* action. The hold threshold is a fixed ~500 ms — too short to feel safe alone — so reset is **two deliberate steps**: a press *arms* the screen, then a hold *erases*. A stray hold on an un-armed screen does nothing; only an armed, completed hold clears the settings (with a bar filling on the live progress).

### Deleting things — the hold-to-delete footer

The same guarded hold does duty as a **delete** control. Rather than a modal "are you sure?", a screen that can delete an item fills a *"hold to delete"* bar with the live hold-progress, exactly like a guarded confirm row — the completed hold *is* the confirmation, there is no second popup. This began on the Stats **Fields** editor (remove a panel) and now drives deletion with one shared idiom in two shapes: a reserved **footer band** below the **Fields** grid, and — because routes and rides are each deleted from their *detail* page, not the list — a guarded delete row: **Delete route** on the **Route overview**, the bottommost element just below START RIDE, and **Delete ride** at the bottom of the **Ride detail** (the overview's recorded sibling, opened by pressing a Rides-list row). Both detail pages lead with a **content-paired pager**: every five seconds the media band and its stats flip together — the track's **shape sketch** (aspect-fit polyline, start disc, destination diamond) over the distance figures, then the **elevation band** over the climb figures — so the page reads as two matched cards rather than a chart with rotating captions. The action rows below are the Paused page's row family: unselected rows are plain labels, the selected one wears the standard amber fill. On the overview the hold is also **aimed**: a turn moves the selection between START RIDE and Delete route, press starts only from START, and the hold charges the delete only while the Delete row is selected (a hold anywhere else does nothing) — the guarded row showing its shaded base and warning fill exactly like a Paused-page Finish. One hold, one muscle memory — a rider who learns it once knows it everywhere.

Two behaviours make it safe to press without thinking:

- **Hidden when the item is in use.** The delete simply isn't offered when it would break live state: the Route overview draws no Delete row for the route you're *actively navigating* (deleting the file under an open geometry handle mid-ride would break navigation — but a route merely *previewed* from idle is still deletable), and the Ride detail draws none while a ride is being recorded (its file isn't even written until Finish, and the filesystem refuses to delete an open handle). A state that can't act doesn't show — and the guard behind it still makes a stray hold a no-op.
- **Sync state is visible before you delete.** A tracked ride the phone hasn't downloaded yet is unrecoverable if deleted, so the Rides list marks every **downloaded** ride with a small **check mark** — the device's success idiom; a ride the phone doesn't hold shows nothing there — and the Ride detail's title bar says it in words either way (*synced* / *not synced*). Still deletable, just *informed*. (Routes get no such cue: the phone can always re-upload one.)

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="The guarded hold-to-delete. On the left, the Fields grid with a delete band that fills with a progress bar as you hold. On the right, its two states stacked: normal — hold to delete, and absent — an in-use route or recording ride simply shows no delete row.">
  <text class="d-tag" x="20" y="24">One guarded hold — the Fields footer, the Route overview + Ride detail rows</text>

  <!-- the one remaining footer screen: the Fields grid -->
  <rect class="d-panel" x="24" y="42" width="228" height="188" rx="11" />
  <rect x="32" y="50" width="212" height="18" rx="3" style="fill:#aa5500" /><text class="d-sub" x="42" y="63" style="fill:#fff;font-size:9px">FIELDS</text>
  <rect x="32" y="74" width="102" height="44" rx="4" class="d-amber" />
  <text class="d-sub" x="40" y="86" style="fill:#000;font-size:8px">SPEED</text><text class="d-sub" x="40" y="102" style="fill:#000;font-size:10px">24.3</text>
  <rect x="142" y="74" width="102" height="44" rx="4" class="d-muted" />
  <text class="d-sub" x="150" y="86" style="font-size:8px">AVG KPH</text><text class="d-sub" x="150" y="102" style="font-size:10px">17.0</text>
  <rect x="32" y="126" width="102" height="44" rx="4" class="d-muted" />
  <text class="d-sub" x="40" y="138" style="font-size:8px">KM DONE</text><text class="d-sub" x="40" y="154" style="font-size:10px">42.5</text>
  <rect x="142" y="126" width="102" height="44" rx="4" class="d-muted" />
  <text class="d-sub" x="150" y="138" style="font-size:8px">CLIMBED</text><text class="d-sub" x="150" y="154" style="font-size:10px">▲810</text>
  <!-- footer band -->
  <line x1="36" y1="182" x2="240" y2="182" stroke="#aaaa55" stroke-width="1" />
  <rect x="36" y="190" width="120" height="30" rx="6" style="fill:#c0492e" />
  <rect x="156" y="190" width="84" height="30" rx="6" class="d-muted" />
  <text class="d-sub" x="138" y="209" text-anchor="middle" style="fill:#fff;font-size:9px">hold to delete</text>
  <text class="d-sub" x="138" y="238" text-anchor="middle" style="font-size:8.5px">bar fills on the live hold</text>

  <!-- the guarded hold's two states -->
  <text class="d-tag" x="292" y="60">the guarded hold, two states</text>
  <rect x="292" y="72" width="404" height="30" rx="6" class="d-muted" />
  <text class="d-sub" x="308" y="91" style="font-size:10px">hold to delete</text>
  <text class="d-sub" x="470" y="91" style="font-size:9px;fill:#6b7758">— normal · a completed hold deletes</text>

  <rect x="292" y="110" width="404" height="30" rx="6" fill="none" stroke="#c9c7b8" stroke-width="1" stroke-dasharray="4 4" />
  <text class="d-sub" x="470" y="129" style="font-size:9px;fill:#6b7758">— hidden · item is active / recording</text>

</svg>
<figcaption>The footer reuses the guarded-hold machinery wholesale — the same <code>confirm_row</code> fill, driven by the same live <code>hold_progress</code> — so there's no new gesture and no new confirmation dialog. The Route overview's Delete-route and the Ride detail's Delete-ride rows are the same machinery in confirm-row clothes, with the same guards (the row is hidden outright for the actively-navigated route, and while a ride records). A device-side delete then flows through the object store, so the phone reconciles it from the live change signal, with a connected catalog audit as the fallback for a lost notification (see the <a href="../companion-link/#staying-in-sync-the-change-signal">companion link</a>).</figcaption>
</figure>

## Trips — folders in the Route menu

A **trip** groups routes into a multi-day plan — the Alps in five stages — and on the device it is exactly that: a **folder** in the Route menu. A trip is a tiny metadata object (a name plus the object ids of its member routes, in ride order); the routes themselves stay ordinary, unchanged route files that a trip merely *references*. So the folder is a grouping, never a copy: a route lives on the card once, filed into at most one trip or loose at the top level.

The Route menu's **top level** lists the trip folders **first**, then the unfiled routes — each group in the catalog's order. A folder row is **visually distinct** from a route row — the trip's name wearing a rounded **count badge** (how many routes it resolves) on the name line, the summed distance and climb beneath in the same two columns a route row uses — but it keeps the same list chrome as everything else: uniform rows, names over metadata. Pressing a folder opens its **stage list**: the member routes as *completely standard* route rows, under the trip's own name as the title. Picking one there loads it **identically** to picking a loose route — same Route overview, same START, same ride loop; nothing downstream is trip-aware, because the device draws one route at a time and never needs to know it came from a folder. The hierarchy is exactly **one level deep**: a stage list never nests, and `back` pops it to the top level.

Under the hood this is one screen, not two — the [`screens!`](#a-screen-is-a-value-not-a-widget-tree) Route-menu screen carries a *scope* (the whole catalog, or one trip's members), so the stage list is a thin variant of the top-level list rather than a fork. The folders are resolved from the trip catalog each frame: a member route deleted on its own simply drops out of its folder (a **dangling** reference the trip tolerates), and a folder whose every reference has dangled still lists — wearing a `0` badge and showing the empty-list state when opened — so it can always be cleaned up.

That cleanup is a **long-press** on the folder. Unlike the in-place [hold-to-delete](#deleting-things-the-hold-to-delete-footer) of a single route or ride, deleting a trip removes *several* files at once — the trip **and every route inside it** (on-device delete is post-trip cleanup, so it cascades) — so it earns a deliberate **confirm dialog**: a card naming the trip, a warning-red hold-guarded *Delete all* row (the same guarded-hold idiom, entry resting on *Cancel* so nothing is armed on the way in), and *Cancel*. Confirming hands the host the trip's durable id; the host deletes the trip file and each member route file, then rescans — the folder is gone, its routes with it, and the menu regroups. The phone reconciles the removals from the live [store-change signal](../companion-link/#staying-in-sync-the-change-signal), with its connected catalog audit as the fallback for a lost notification.

## The POIs browser

*POIs* is one of the compass Menu's stations, and it answers a bikepacker's question directly: *where's the nearest water / campsite / bakery?* The flow is two screens — a **category** list, then that category's **nearest-16** list — both built from the same [`screens!` table](#a-screen-is-a-value-not-a-widget-tree) rows and the shared list widget as every other menu, so there's almost nothing new in the plumbing. What's new is where the data comes from and how one element stays live.

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="The POIs browser flow. The compass Menu's POIs station opens the category screen: a six-row list of Water, Campsite, Lodging, Resupply, Pharmacy and Bike shop, each with a small icon. Pressing a category opens the list screen: that category's nearest sixteen POIs sorted by distance, each row a name, a bearing arrow and a distance. Back climbs one step; selecting a POI opens its detail view.">
  <defs>
    <marker id="aU9" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Menu → categories → the nearest-16 of one category</text>

  <!-- Menu station -->
  <rect class="d-panel-2" x="24" y="88" width="120" height="44" rx="10" />
  <text class="d-label" x="84" y="108" text-anchor="middle">Menu</text>
  <text class="d-sub" x="84" y="123" text-anchor="middle" style="font-size:9px">POIs station</text>

  <!-- category screen -->
  <line class="d-flow" x1="146" y1="110" x2="196" y2="110" marker-end="url(#aU9)" />
  <text class="d-sub" x="171" y="102" text-anchor="middle" style="font-size:9px">press</text>
  <rect class="d-panel" x="202" y="44" width="188" height="168" rx="11" />
  <rect x="210" y="52" width="172" height="20" rx="4" style="fill:#aa5500" /><text class="d-sub" x="220" y="66" style="fill:#fff;font-size:9px">POIS</text>
  <g font-family="var(--mono)">
    <rect x="210" y="78" width="172" height="20" rx="4" class="d-amber" />
    <circle cx="222" cy="88" r="4" fill="#000" /><text class="d-sub" x="234" y="92" style="fill:#000;font-size:9.5px">Water</text>
    <circle cx="222" cy="110" r="4" fill="#24331c" /><text class="d-sub" x="234" y="114" style="font-size:9.5px">Campsite</text>
    <circle cx="222" cy="130" r="4" fill="#24331c" /><text class="d-sub" x="234" y="134" style="font-size:9.5px">Lodging</text>
    <circle cx="222" cy="150" r="4" fill="#24331c" /><text class="d-sub" x="234" y="154" style="font-size:9.5px">Resupply</text>
    <circle cx="222" cy="170" r="4" fill="#24331c" /><text class="d-sub" x="234" y="174" style="font-size:9.5px">Pharmacy</text>
    <circle cx="222" cy="190" r="4" fill="#24331c" /><text class="d-sub" x="234" y="194" style="font-size:9.5px">Bike shop</text>
  </g>

  <!-- list screen -->
  <line class="d-flow" x1="392" y1="110" x2="442" y2="110" marker-end="url(#aU9)" />
  <text class="d-sub" x="417" y="102" text-anchor="middle" style="font-size:9px">press</text>
  <rect class="d-panel" x="448" y="44" width="248" height="168" rx="11" />
  <rect x="456" y="52" width="232" height="20" rx="4" style="fill:#aa5500" /><text class="d-sub" x="466" y="66" style="fill:#fff;font-size:9px">Water · 1/16</text>
  <g font-family="var(--mono)">
    <!-- selected row -->
    <rect x="456" y="78" width="232" height="24" rx="4" class="d-amber" />
    <text class="d-sub" x="466" y="94" style="fill:#000;font-size:9.5px">Stadtbrunnen</text>
    <path d="M636 84 l6 -6 l6 6 l-4 0 l0 8 l-4 0 l0 -8 z" fill="#000" />
    <text class="d-sub" x="656" y="94" style="fill:#000;font-size:9px">210m</text>
    <!-- more rows -->
    <text class="d-sub" x="466" y="118" style="font-size:9.5px">Spring</text>
    <path d="M638 110 l8 4 l-8 4 l3 -4 z" fill="#24331c" /><text class="d-sub" x="656" y="118" style="font-size:9px">410m</text>
    <text class="d-sub" x="466" y="140" style="font-size:9.5px">Brunnen Nord</text>
    <path d="M642 132 l0 8 l-3 -3 M642 140 l3 -3" fill="none" stroke="#24331c" stroke-width="1.4" /><text class="d-sub" x="656" y="140" style="font-size:9px">820m</text>
    <text class="d-sub" x="466" y="162" style="font-size:9.5px">Drinking water</text>
    <text class="d-sub" x="656" y="162" style="font-size:9px">1km</text>
    <text class="d-sub" x="466" y="188" style="font-size:9px;fill:#a9501c">name · arrow · distance</text>
  </g>
</svg>
<figcaption>Six categories in fixed id order, each a hand-drawn icon in the main-menu style; selecting one opens its list — the <b>nearest 16</b> POIs within the loaded map, sorted by distance to the GPS fix (fewer than 16 is normal). A row is a <b>name, a bearing arrow, and a distance</b>. The name falls back to the subtype label ("Spring", "Drinking water") when OSM has none. <code>back</code> climbs one step; pressing a row opens that POI's <a href="#the-poi-detail-view">detail view</a>.</figcaption>
</figure>

### A static list with one live element

The list is a **static snapshot**, frozen the moment you enter. Membership, order and distances don't move — rows never reshuffle under the cursor as you turn, and the SD card isn't re-scanned every frame. Re-enter the category to refresh it against your current position. Under the hood the [nearest-16 query](../formats/#pois-a-nearest-list-not-a-map-layer) needs the streaming map `Reader`, which the host only builds for the frame that needs it — so the snapshot is taken in a small **pre-draw `prepare` pass** (the one place both the `Reader` and the fix are in hand), the first time both are present, into a single buffer the app owns (holding it per-screen would inflate every slot of the screen stack). Drawing then just *reads* that frozen buffer; the query is a side effect, and side effects don't belong in `draw` (see [Logic and drawing get different views](#logic-and-drawing-get-different-views-of-the-world)). Opening a list invalidates the buffer, so the next `prepare` re-queries.

The one thing that *is* live is the **bearing arrow** — recomputed every frame from the POI's stored coordinates and the rider's current heading, pure trig with zero SD access. It points from you toward the POI **relative to your heading**, so "straight up" means "dead ahead." The drawn glyph **snaps to eight compass directions** (45° steps) and is a full arrow — shaft plus barbs, double-stroked to a 2 px line: at ~11 px a degree-true arrow just smudges, while the eight snapped shapes read without focusing. That heading has two sources, and which one is used depends on whether you're moving:

<figure class="fig">
<svg viewBox="0 0 720 210" role="img" aria-label="One POI list row, dissected. The row holds a name on the left, a bearing arrow, and a right-aligned distance. Below, the arrow's heading reference has two sources: while moving, the GPS course; while stationary, the electronic compass heading from the ICM-20948; when neither is known, the arrow is hidden rather than pointing wrong.">
  <text class="d-tag" x="20" y="24">The row, and where the arrow's "up" comes from</text>

  <!-- the row -->
  <rect class="d-panel" x="24" y="42" width="672" height="40" rx="8" />
  <text class="d-sub" x="44" y="66" font-family="var(--mono)" style="font-size:12px">Stadtbrunnen</text>
  <text class="d-sub" x="240" y="60" style="font-size:9px;fill:#a9501c">name (or subtype label if unnamed)</text>
  <!-- arrow -->
  <path d="M556 54 l8 -8 l8 8 l-5 0 l0 10 l-6 0 l0 -10 z" fill="#cf6a2a" />
  <text class="d-sub" x="540" y="78" style="font-size:8.5px;fill:#a9501c">bearing</text>
  <!-- distance -->
  <text class="d-sub" x="672" y="66" text-anchor="end" font-family="var(--mono)" style="font-size:12px">210m</text>
  <text class="d-sub" x="672" y="78" text-anchor="end" style="font-size:8.5px">distance</text>

  <!-- heading sources -->
  <text class="d-tag" x="20" y="118">the arrow's heading reference</text>
  <rect class="d-panel-2" x="24" y="128" width="216" height="66" rx="10" />
  <text class="d-label" x="40" y="150" style="font-size:11px">moving</text>
  <text class="d-sub" x="40" y="170" style="font-size:10px">GPS course over ground</text>
  <text class="d-sub" x="40" y="185" style="font-size:9px">— the direction you're going</text>

  <rect class="d-panel-2" x="252" y="128" width="216" height="66" rx="10" />
  <text class="d-label" x="268" y="150" style="font-size:11px">stationary</text>
  <text class="d-sub" x="268" y="170" style="font-size:10px">ICM-20948 compass</text>
  <text class="d-sub" x="268" y="185" style="font-size:9px">— which way you're facing</text>

  <rect class="d-hot" x="480" y="128" width="216" height="66" rx="10" style="fill:#f8efe4" />
  <text class="d-label" x="496" y="150" style="fill:#a9501c;font-size:11px">neither known</text>
  <text class="d-sub" x="496" y="170" style="font-size:10px">arrow hidden</text>
  <text class="d-sub" x="496" y="185" style="font-size:9px">— don't point wrong</text>
</svg>
<figcaption>A stopped GPS reports no course, so the arrow would freeze pointing wherever you last moved — useless when you're standing at a junction deciding which way to turn. So the row uses the GPS <b>course while moving</b> and the <b>electronic compass while stopped</b> (the same #231 heading seam the heading-up map uses). If <i>neither</i> exists — no course and no compass — the arrow is simply <b>hidden</b> rather than shown pointing the wrong way. The rest of the row (name, distance) is part of the frozen snapshot; only the arrow re-rotates.</figcaption>
</figure>

### The POI detail view

Pressing a list row opens the **detail view** for that POI — one more `Nav` screen, carrying the selected POI out of the frozen snapshot. It shows the same thing the row does, but unabridged: the **full stored name** with the **category's pixel icon** beside it (the row ellipsizes the name to fit its width; the detail wraps it to a second line instead of truncating), the **subtype label** as a muted subtitle, and — promoted to its own row directly under it — the **distance and the same live 8-way bearing arrow** at body size: the two numbers that decide *do I go*, with the identical heading seam as the row (arrow hidden when neither course nor compass is known). What the row can't fit is the reason the detail exists: **today's opening hours** and whether the place is **open right now** — a green **OPEN** pill, or a warning-red **CLOSED** one. At the bottom, a full-width **▶ Route here** bar — the Route overview's START RIDE bar, reused — makes the screen's press action visible: it opens the [create-route confirm](../architecture/#on-device-routing-the-router-seam).

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="The POI detail view. On the left, the screen: the POI name with its category icon at the top, a muted subtype subtitle beneath it, then a promoted distance row with the 8-way bearing arrow, a Today heading with an opening-hours range below, a green OPEN pill, and a full-width amber Route here bar at the bottom. On the right, the three heading states for the hours block: Today with time ranges when open some hours today, Closed today when the schedule has no interval for this weekday, and Hours not listed when the POI has no schedule at all. Below, the open-now pill is derived from the live local clock.">
  <defs>
    <marker id="aPD" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">The detail screen, and where "open now" comes from</text>

  <!-- the screen mock -->
  <rect class="d-panel" x="24" y="40" width="232" height="196" rx="10" />
  <rect x="42" y="54" width="196" height="18" rx="3" style="fill:#aa5500" /><text class="d-sub" x="52" y="67" style="fill:#fff;font-size:9px">POI</text>
  <!-- name row: category icon + name -->
  <circle cx="49" cy="93" r="6" fill="#3d3427" /><path d="M43 90 l6 -8 l6 8 z" fill="#3d3427" />
  <text class="d-sub" x="64" y="98" font-family="var(--mono)" style="font-size:12px">Stadtbaeckerei</text>
  <text class="d-sub" x="42" y="114" style="font-size:9px">Bakery</text>
  <!-- promoted distance + 8-way arrow row -->
  <path d="M52 136 l7 -7 l7 7 l-4 0 l0 9 l-6 0 l0 -9 z" fill="#cf6a2a" />
  <text class="d-sub" x="74" y="144" font-family="var(--mono)" style="font-size:11px">850m</text>
  <!-- hours -->
  <text class="d-sub" x="42" y="164" style="font-size:9px">Today</text>
  <text class="d-sub" x="42" y="182" font-family="var(--mono)" style="font-size:11px">08:00-18:00</text>
  <!-- badge pill -->
  <rect x="42" y="192" width="52" height="15" rx="3" style="fill:#3c6b39" />
  <text class="d-sub" x="68" y="203" text-anchor="middle" style="fill:#fff;font-size:9px">OPEN</text>
  <!-- footer action bar -->
  <rect x="38" y="214" width="204" height="16" rx="5" style="fill:#e3a52b" />
  <text class="d-sub" x="140" y="225" text-anchor="middle" style="fill:#3d3427;font-size:9px">&#9654; Route here</text>

  <!-- the three heading states -->
  <text class="d-tag" x="292" y="60">the hours heading — three states</text>
  <rect class="d-panel-2" x="292" y="72" width="404" height="24" rx="6" />
  <text class="d-sub" x="304" y="88" font-family="var(--mono)" style="font-size:10px">Today</text>
  <text class="d-sub" x="380" y="88" style="font-size:9px">— open some hours today; ranges stacked below</text>
  <rect class="d-panel-2" x="292" y="100" width="404" height="24" rx="6" />
  <text class="d-sub" x="304" y="116" font-family="var(--mono)" style="font-size:10px">Closed today</text>
  <text class="d-sub" x="420" y="116" style="font-size:9px">— has hours, but none this weekday</text>
  <rect class="d-panel-2" x="292" y="128" width="404" height="24" rx="6" />
  <text class="d-sub" x="304" y="144" font-family="var(--mono)" style="font-size:10px">Hours not listed</text>
  <text class="d-sub" x="440" y="144" style="font-size:9px">— HoursRef was 0xFFFF, no badge</text>

  <!-- open-now derivation -->
  <text class="d-tag" x="292" y="178">the OPEN / CLOSED badge</text>
  <rect class="d-hot" x="292" y="188" width="404" height="48" rx="10" style="fill:#f8efe4" />
  <text class="d-sub" x="308" y="208" style="font-size:9.5px">local clock → weekday (Zeller) + minute-of-day</text>
  <line class="d-flow" x1="560" y1="204" x2="600" y2="204" marker-end="url(#aPD)" />
  <text class="d-sub" x="610" y="208" style="font-size:9.5px;fill:#a9501c">is_open?</text>
  <text class="d-sub" x="308" y="226" style="font-size:9px">read live every frame — the one part that isn't frozen</text>
</svg>
<figcaption>The hours block reads the POI's [pooled schedule](../formats/#opening-hours-a-pooled-weekly-schedule) once (in the same pre-draw <code>prepare</code> pass the list snapshot uses — the one place the map <code>Reader</code> is in hand — then cached), then picks <b>today's</b> intervals. Three states: <b>Today</b> with the day's one or two ranges stacked (<code>08:00-12:00</code> / <code>14:00-18:00</code> — stacked because a two-range line won't fit the 240 px panel); <b>Closed today</b> when the schedule has no interval for this weekday; and <b>Hours not listed</b> when the POI had no parseable hours at all. The <b>OPEN / CLOSED</b> pill (green fill open, warning-red closed — a state, not just quieter text) is the only live piece — recomputed every frame from the device's local wall-clock. That local time comes from the user's <b>UTC offset</b> plus the <b>GPS clock</b> (the same clock that sets the ride time), and <code>weekday_from_ymd</code> (Zeller's congruence, Mon = 0) picks today's row; the minute-of-day then decides open vs. closed, including overnight intervals that opened last evening.</figcaption>
</figure>

## Settings: a second level of focus

Most screens have one focus: the row cursor. **Settings is five themed groups** rather than one long list — **Ride**, **Display**, **Connections**, **Power**, and **System**. Two of them are plain menus whose rows just open their own pages (**Connections** → Phone / Sensors; **System** → Units / Date & Time / Language / Firmware / Reset); the rest hold their controls inline, and **Ride** does both — it opens Bike type and Data fields as their own pages while cycling Climb, Waypoints, page-cycle and auto-delete in place. The rule is simple: a real interaction (a picker, an editor, a guarded action) keeps its own page; a lone toggle or value that belongs with a couple of siblings is just a row.

Those control screens add a *second* level of focus. A value isn't a separate sub-screen; it's edited **in place**. Rotating still moves the amber row cursor, but once you press a value row, focus drops *into* a field: a `▲▼` box marks the live one, rotating now changes *its* value, pressing steps to the next field, and back steps out. The same two-level model drives every editor — a UTC offset, a fix interval, the stats page-cycle period — and the same toggle row flips a switch like the power saver. No new gestures; the existing five just mean different things at each level.

The **Display** screen governs the Map's chrome and the auto-return: two toggles for the Map overlays — small floating top-centre **clock** digits (`HH:MM`, bare ink with a halo — no pill) and a bottom-left **scale bar** (the largest round 1/2/5 distance that fits the current zoom, in the units system) — plus the **idle-return** timeout (15 s / 30 s / 1 min / 5 min / Never, default 30 s) that decides how long an untouched UI waits before returning itself to Home (or, mid-ride, the Map). The Map's other chrome isn't a setting: a bottom-centre **one-slot warning chip** ("No GPS Fix" outranks "off route NNNm"), suppressed while panning (the pan bottom chevron owns that slot); that same bottom slot also carries the calmer waypoint chip mid-ride and, on the route-less browse map only, the one-shot *Press to start a ride* hint on entry — both at lower priority than the warning chip, so it always wins the slot. The scale bar sits right in the corner and steps up above whichever chip is up (a taller step for the two-line hint), and a warning-red **low-battery** glyph sits in the top-left corner below 10 %.

The **Ride** screen gathers everything you tune for a ride. Two rows open their own pages — **Bike type** (the routing-profile hero picker) and **Data fields** — and the rest edit in place. *Page cycle* sets how fast the grid auto-flips between pages; *Data fields* opens the grid **editor — which simply *is* the grid**: the same 3×2 tile pages the riding view shows, placed by the same layout walk and painted by the same tile renderer — in the editor the tiles carry fixed **sample values** (in a dimmed olive) so a layout is judged against realistic content rather than the dashes a route-less editor would otherwise draw. The grid draws from a predefined, in-code catalogue of fields (speed, distance, climb, grade, elevation, clock, heart rate, power, cadence, …) — the rider picks which to show and in what order, and a field takes one of three shapes: a single column, a full-width two-column tile, or the page-sized [waypoint list panel](#waypoints-on-the-route) (two columns tall enough to fill a page). The cursor is the amber tile (walking past a page's last tile flips pages), and reordering reuses the grab idiom: press *lifts* the tile (move arrows appear), rotating moves it through the order — the grid reflows live, so a two-column panel's row-aligned hops are something you watch, not infer — and press drops it. A ghost `+` tile in the first free slot opens the field picker, so a new panel visibly lands where the ghost sits; a panel is removed by a **hold-to-delete** bar, the same guarded hold as Reset's. The chosen panels lay out six to a page (3×2) and auto-cycle on the timer, so a long list stays glanceable. The selection and period live in the same persisted `Settings` value, so they survive a reboot like every other setting. The same screen also carries two press-to-cycle mode rows: the **Climb** toggle — Off / Manual / Auto — that decides whether the [climb panel](#climbs-get-their-own-panel) appears on its own when a climb begins, beneath it the **Waypoints** toggle — Off / Approach / Always — that governs the [waypoint chip](#waypoints-on-the-route); and a final **Auto-delete** row cycling how long a synced ride is kept before the device removes it (Never / 1 day / 1 week / 1 month — see [What the rider sees](#what-the-rider-sees)).

<figure class="fig">
<svg viewBox="0 0 720 232" role="img" aria-label="Settings screens have two focus levels. In row focus, rotate moves the amber row cursor, press flips a toggle or opens a value row's stepper, and back climbs one screen. Pressing a value row enters field focus, where rotate changes the live field's value shown in an up-down arrow box, press advances to the next field, and back — or pressing past the last field — steps back out to row focus.">
  <defs>
    <marker id="aU8" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Two levels of focus — rows, then fields</text>

  <!-- Row focus -->
  <rect class="d-panel" x="40" y="46" width="262" height="160" rx="12" />
  <text class="d-label" x="60" y="70">Row focus</text>
  <rect x="58" y="80" width="226" height="24" rx="5" class="d-amber" />
  <text class="d-sub" x="68" y="96" style="fill:#000;font-size:10px">amber bar = the cursor</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="60" y="130" style="font-size:10.5px">rotate — move the cursor</text>
    <text class="d-sub" x="60" y="152" style="font-size:10.5px">press &nbsp;— toggle / open a value</text>
    <text class="d-sub" x="60" y="174" style="font-size:10.5px">back &nbsp;— climb one screen up</text>
  </g>

  <!-- transitions -->
  <line class="d-flow" x1="304" y1="104" x2="416" y2="104" marker-end="url(#aU8)" />
  <text class="d-sub" x="360" y="96" text-anchor="middle" style="font-size:9px">press a value row</text>
  <line class="d-flow" x1="416" y1="150" x2="304" y2="150" marker-end="url(#aU8)" />
  <text class="d-sub" x="360" y="166" text-anchor="middle" style="font-size:9px">back / past last field</text>

  <!-- Field focus -->
  <rect class="d-panel-2" x="418" y="46" width="262" height="160" rx="12" />
  <text class="d-label" x="438" y="70">Field focus</text>
  <path d="M452 80 l7 -9 l7 9 z" fill="#ffaa00" />
  <rect x="445" y="84" width="42" height="22" rx="4" class="d-muted" style="stroke:#ffaa00;stroke-width:1.5" />
  <text class="d-sub" x="466" y="99" text-anchor="middle" style="font-size:10px">2025</text>
  <path d="M452 110 l7 9 l7 -9 z" fill="#ffaa00" />
  <text class="d-sub" x="500" y="99" style="font-size:10px">box = the live field</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="438" y="130" style="font-size:10.5px">rotate — change the value</text>
    <text class="d-sub" x="438" y="152" style="font-size:10.5px">press &nbsp;— step to the next field</text>
    <text class="d-sub" x="438" y="174" style="font-size:10.5px">back &nbsp;— step out of the field</text>
  </g>
</svg>
<figcaption>The settings editors reuse the five gestures at two levels: the row cursor, then a live field. Pressing a value row drops focus in; pressing past the last field (or <code>back</code>) lifts it back out. Edits apply <i>live</i> as you turn — there's no save button and no staging buffer: <code>back</code> just exits, and the change was already made.</figcaption>
</figure>

### Settings survive a reboot — independent of the SD card

A setting is worthless if it's forgotten on power-off, so settings persist. The values live in a small `Settings` value (`Copy`, no floats) that the screens edit; the *medium* is a host concern behind one more trait, exactly like the sensor seams. The app seeds itself from `load()` at boot and asks the host to `save()` only when something actually changed — detected by the same one-`==` before/after compare the camera uses to decide it's dirty. The write is **debounced to leaving the settings subtree**: a stepper sweep edits live in RAM but never drives a write per detent, and nothing is written while any settings screen is still on top — the store sees exactly one write when you back out.

Persistence is **acknowledged**, not fire-and-forget. `save()` reports whether the write reached durable storage; the app keeps the change marked *pending* until the host confirms it, so a failed RRAM/file write is **retried** (with a small backoff) instead of being silently lost, and a fresh edit safely supersedes an older pending one. A persistent failure also raises the shared advisory card ("Settings not saved") so it's visible, not just logged — the edit stays live in RAM the whole time.

```rust
pub trait SettingsStore {
    type Value;
    fn load(&mut self) -> Option<Self::Value>;                     // None (blank/corrupt) → app default
    fn save(&mut self, value: &Self::Value)
        -> Result<(), SettingsSaveError>;                          // Ok = durable; Err = retried
}

// Both shipped adapters bind `type Value = Settings`.
```

The nominal trait lives in dependency-free `obc-ports`; its associated value keeps that foundation from learning the app's `Settings` model. The simulator's [`FileSettingsStore`](src:firmware/obc-sim/src/settings_store.rs) and the board's [`RramSettingsStore`](src:firmware/obc-fw-nrf54l/src/settings.rs) implement that port directly. The simulator writes the blob to a file; the device writes it to a reserved slice of the nRF54L's on-chip **RRAM** — its program memory is RRAM, which is byte-writable with no flash-style erase cycle, so a tiny key-value store is cheap and needs no SD card present. Both sides share one versioned, CRC-checked byte codec, so a blank or corrupted read cleanly falls back to defaults rather than loading garbage — and the factory Reset is just writing the default blob back.

## Self-cleaning storage: routes and rides expire

Uploading a route takes ten seconds; deleting one takes a discipline nobody has. After a season the Route menu is thirty stale routes deep, and rides you long since pulled to the phone still sit on the card. So stored objects **clean themselves up** — but only ever when it is provably safe to.

The rule is anchored to *use*, not upload. A route is deleted once it has gone **unused** for its **retention** window — a per-route "keep this for…" the app picks at upload time (default two weeks; from *Never* up to two months). "Used" means *becoming the active navigation route*, so a weekly commute loop renews itself forever and a route can never expire mid-tour underneath you: when the housekeeping pass finds the route you're navigating about to expire, it re-stamps it instead of deleting it. Rides are simpler and device-side — a ride is deleted a set time **after it was verifiably synced** to the phone (the [`ackRides` reconcile](../companion-link/#synced-rides-reconciled-state-not-event-inference) is the proof it's safely off the device), and an unsynced ride is never touched, at any age.

### The device has no clock — so deletion waits for a trusted one

None of that can run on a guess about the date. The device has **no RTC**: at boot the wall clock resumes from a persisted set-point that is stale by however long the device was powered off — fine for showing `HH:MM`, useless for deciding whether a route has sat idle for two weeks. So the whole feature rests on a single gate — **nothing is deleted, and no expiry timestamp is written, unless the clock was freshly established *this boot*** by one of exactly two real sources: a **GPS fix** (whose payload carries full UTC) or the phone's **`setClock`** on connect. Until one of them stamps, the boot set-point is display-only and the housekeeping pass does nothing at all.

<figure class="fig">
<svg viewBox="0 0 720 232" role="img" aria-label="Auto-expiry as a left-to-right flow. On the left are the only two trusted time sources: a GPS fix, which carries full UTC, and the phone's setClock command, sent on every connect with UTC and a local offset. Both feed a central gate — is the clock trusted this boot? A note below the gate reads: untrusted means nothing is stamped and nothing is deleted, because the persisted boot set-point is display-only. When trusted, the gate enables the roughly-hourly housekeeping sweep, which runs only while no ride is recording. The sweep's outcomes are listed on the right: a route unused too long is deleted; the active or not-yet-started route is re-stamped instead of deleted; and a ride synced to the phone and aged past the Auto-delete setting is deleted.">
  <defs>
    <marker id="ax-f" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="20">Auto-expiry — a trusted clock gates every delete</text>

  <!-- the only two time sources -->
  <text class="d-sub" x="16" y="46" style="fill:#6b7758">the only two trusted sources</text>
  <rect class="d-panel" x="16" y="56" width="180" height="50" rx="10" />
  <text class="d-label" x="106" y="78" text-anchor="middle">GPS fix</text>
  <text class="d-sub" x="106" y="95" text-anchor="middle">carries full UTC</text>
  <rect class="d-panel" x="16" y="120" width="180" height="50" rx="10" />
  <text class="d-label" x="106" y="142" text-anchor="middle">phone setClock</text>
  <text class="d-sub" x="106" y="159" text-anchor="middle">UTC + offset · every connect</text>

  <!-- arrows into the gate -->
  <line class="d-flow" x1="196" y1="82" x2="252" y2="108" marker-end="url(#ax-f)" />
  <line class="d-flow" x1="196" y1="144" x2="252" y2="118" marker-end="url(#ax-f)" />

  <!-- the trust gate (coral = safety-critical) -->
  <rect class="d-hot" x="254" y="78" width="166" height="70" rx="12" fill="#f8efe4" />
  <text class="d-label" x="337" y="107" text-anchor="middle" style="fill:#a9501c">clock trusted</text>
  <text class="d-sub" x="337" y="127" text-anchor="middle">this boot?</text>

  <!-- untrusted note -->
  <text class="d-sub" x="337" y="178" text-anchor="middle" style="fill:#a9501c">untrusted → nothing stamped, nothing deleted</text>
  <text class="d-sub" x="337" y="194" text-anchor="middle">(the boot set-point is display-only)</text>

  <!-- arrow gate → sweep -->
  <line class="d-flow" x1="420" y1="113" x2="484" y2="92" marker-end="url(#ax-f)" />
  <text class="d-sub" x="452" y="86" text-anchor="middle" style="font-size:9px">trusted</text>

  <!-- the sweep + its outcomes -->
  <rect class="d-panel-2" x="486" y="72" width="218" height="40" rx="9" />
  <text class="d-label" x="595" y="90" text-anchor="middle">roughly-hourly sweep</text>
  <text class="d-sub" x="595" y="105" text-anchor="middle">only while not recording</text>

  <line class="d-flow" x1="595" y1="112" x2="595" y2="122" marker-end="url(#ax-f)" />

  <rect class="d-panel" x="486" y="124" width="218" height="90" rx="10" />
  <text class="d-sub" x="502" y="147" style="font-size:9.5px">route unused too long &nbsp;→&nbsp; delete</text>
  <text class="d-sub" x="502" y="171" style="font-size:9.5px">active / unstarted route &nbsp;→&nbsp; re-stamp</text>
  <text class="d-sub" x="502" y="195" style="font-size:9.5px">synced ride, aged out &nbsp;→&nbsp; delete</text>
</svg>
<figcaption>The safety core in one picture: a <b>GPS fix</b> or the phone's <b>setClock</b> are the only two sources that can establish a <em>trusted</em> clock for the boot, and the roughly-hourly housekeeping sweep refuses to stamp or delete anything until one of them has — and never runs while a ride is recording. Once trusted, it deletes routes idle past their retention and synced rides aged past the Auto-delete setting, but <b>re-stamps</b> rather than deletes the active route and any route whose usage clock was never started. A boot with neither source stays entirely hands-off.</figcaption>
</figure>

That gate is also why the **Date & Time** screen (under Settings ▸ System) no longer lets you hand-set the clock — a fat-fingered year must never be able to trigger a mass delete. It's now three rows: a read-only *GPS fix* (the UTC anchor), a read-only *Local time*, and the one **UTC offset** stepper, which shifts only the *displayed* hour. Expiry arithmetic is pure UTC, so a wrong offset is purely cosmetic — after a flight you nudge the stepper once, or the next phone connect refreshes it silently.

### What the rider sees

Almost all of this is invisible, which is the point. Two surfaces show it:

- The **Auto-delete** row on the **Ride** screen — *Synced rides*, choosing how long a synced ride survives before the device removes it (Never / 1 day / 1 week / 1 month, default 1 week). A **press** cycles the four values in place — the same press-to-cycle idiom as the Climb and Waypoints rows beside it — and `back` is the save. Route retention has **no** device editor — it's per-route and set on the phone; the device only ever displays it.
- The **Route overview**'s expiry line — a small caption above the route's stats: a muted `Auto-delete` label beside the ink value (`in 3 d`), two-tone and space-separated (no separator glyph — the device font has no middot). It's deliberately *not* an always-on countdown: it appears **only once the route is within five days of deletion** (a deadline already past, before the hourly pass collects it, reads `soon`), so it reads as a *heads-up that this route is about to go*, not standing chrome. A route that never expires, or whose usage clock hasn't started, shows no line at all.

Where that per-route state actually lives — an SD sidecar beside the catalog, never inside the byte-pinned route file, reached over BLE as a command rather than a re-upload — is a [companion-link](../companion-link/#the-trusted-clock-and-route-retention) design note.

## The UI speaks four languages

Every user-facing word — English, German, French, Spanish — is a *lookup*, not a literal. The **language** is a runtime setting, not a build flag: it lives in the same persisted `Settings` value as Units, RRAM-backed and switchable on-glass from the **Language** screen under Settings ▸ System (a one-row value picker showing each language's own name — `English` / `Deutsch` / `Français` / `Español`, so the row reads to someone who can't yet read the current UI). Language and Units are orthogonal — English + Metric is a perfectly good combo.

Because the render path is **stateless** (a screen is [a value, not a widget tree](#a-screen-is-a-value-not-a-widget-tree)), there is no ambient "current language" to set. Translation is a pure function of the message and the language, and the language rides along on the context every draw already receives:

```rust
// A message key + the language → the &'static str, a plain double index into a flash table.
pub const fn t(msg: Msg, lang: Language) -> &'static str { TABLE[msg as usize][lang as usize] }

// Draw-time convenience: `rx.t(Msg::…)` reads settings.language off the context the screen holds.
draw_text(target, rx.t(Msg::MenuRoutes), at, Font::Body, TextAlign::Center, ink);
```

The `Msg` enum and the `TABLE` it indexes are **generated at compile time**. Four per-language catalogs — `obc-app/i18n/{en,de,fr,es}.toml` — hold the copy as `[section]` + `key = "value"` TOML; a small `build.rs` parses all four and emits one `Msg` variant per key plus `const TABLE: [[&str; 4]; N]`, the columns ordered to match the `Language` discriminants (En, De, Fr, Es). Because the table is `const`, the whole catalogue lands in flash `.rodata` — nothing touches the device's tight 256 KB RAM. English is the canonical key set: the build **fails with a named list of offenders** if any of de/fr/es is missing a key, carries an extra one, or changes the load-bearing leading/trailing spaces English glues into a concatenated readout (`AVG ` + a value, `grade ` + a percent) — so a half-translated *or* mis-spaced string can't ship silently. (A separate `obc-app` test walks the finished table and asserts every character is in the device font's repertoire — Latin-1 + Latin Extended-A — so a stray curly quote or em-dash fails CI rather than rendering as a `?` on-glass.)

The words are translated; the *formats* are not. The 24-hour clock, ISO / `Mon DD` dates, and the metric/imperial unit suffixes (`KPH`, `km`, `m`) stay identical across languages — only the twelve month abbreviations are localized. Symbol-like labels (`KPH`) are language-independent by design; only word-bearing enum labels (a `Units` name, a `ClimbMode` name) route through the catalogue.

**Adding a string:** add the key under the right `[section]` to **all four** `i18n/*.toml` files, then use `Msg::SectionKey` (PascalCased from `section.key`) via `rx.t(…)` at the draw site. Forget one file and the build stops with a `MISSING key` error naming it. **Adding a language** is a handful of matched edits: a new `xx.toml` catalog (with every English key), its code in the `build.rs` language list, a new `Language` enum variant carrying its endonym in `name()`, a slot in the picker's `ORDER`, a bump to `Language::COUNT`, and the one-byte codec's `from_byte` mapping — the append-only `Settings` codec already stores the language as a single byte, so no version bump is needed for the byte itself. Miss the catalog column and the build *won't link*: a `const` assertion ties `TABLE`'s width to `Language::COUNT`, turning a forgotten column into a compile error rather than a first-draw out-of-bounds panic.

## Logic and drawing get different views of the world

`handle` and `draw` are handed deliberately different contexts. `handle` gets `Ctx` — the **mutable** slice of app state a screen is allowed to change (the camera, the ride mode, the clock). `draw` gets `Render` — a **read-only** view plus the resources it needs to paint (the map reader, the renderer, the active route, its elevation profile, the active climb, the breadcrumb, the in-flight hold-progress). A screen literally cannot mutate state while drawing, because it isn't given the means to — `Render` carries no mutable state at all, so a frame's only outputs are pixels and the map's render statistics.

The one thing that used to break that rule was the POI browser: its snapshot and hours reads needed the map `Reader`, which lives in the draw path, so they wrote back into the draw context mid-paint. That acquisition now happens in a third, even narrower context — **`Prepare`**, handed to a screen's optional `prepare` method in a pass that runs *before* the draw loop, carrying just the `Reader`, the shared POI buffer, and the fix. `prepare` does the side-effectful read; `draw` consumes the frozen result read-only. So the split is now clean in both directions: `prepare` may touch the `Reader`-backed one-shots, `handle` may change app state, and `draw` may do neither.

<figure class="fig">
<svg viewBox="0 0 720 220" role="img" aria-label="Two side-by-side contexts. On the left, handle receives Ctx, the mutable half: app state (camera, zoom, pan), the activity (ride mode), the route catalog, and the clock. On the right, draw receives Render, the read-only half: the map reader, the renderer, read-only state, the active route, its elevation profile, the breadcrumb, size, and hold-progress.">
  <text class="d-tag" x="20" y="24">Two halves of the world, handed to two methods</text>

  <!-- Ctx -->
  <rect class="d-panel" x="36" y="44" width="300" height="150" rx="12" />
  <text class="d-label" x="56" y="68">handle(g, &amp;mut Ctx)</text>
  <text class="d-tag" x="56" y="84">mutable — change the world</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="56" y="104">state &nbsp;&nbsp;— camera · zoom · pan</text>
    <text class="d-sub" x="56" y="124">activity — ride mode · accumulators</text>
    <text class="d-sub" x="56" y="144">settings — units · clock · intervals</text>
    <text class="d-sub" x="56" y="164">routes &nbsp;— the catalog (read)</text>
    <text class="d-sub" x="56" y="184">now_ms &nbsp;— the clock</text>
  </g>

  <!-- Render -->
  <rect class="d-panel-2" x="384" y="44" width="300" height="150" rx="12" />
  <text class="d-label" x="404" y="68">draw(target, &amp;Render)</text>
  <text class="d-tag" x="404" y="84">read-only — paint the world</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="404" y="106">reader · renderer — draw the map</text>
    <text class="d-sub" x="404" y="126">state · route · profile · breadcrumb</text>
    <text class="d-sub" x="404" y="146">w · h &nbsp;— panel size</text>
    <text class="d-sub" x="404" y="166">hold_progress — the confirm ring</text>
  </g>
</svg>
<figcaption>Splitting the context by role is a small thing that pays off constantly: drawing is provably side-effect-free, and the heavy render resources (the streaming map <code>Reader</code>, the reusable <code>MapRenderer</code>) are gathered once by the host and lent to whichever screen is on top.</figcaption>
</figure>

## Render on demand — and the idle path is free

A reflective memory-LCD holds its image with no redraw, and a map frame is the expensive thing the device does (tens of milliseconds). So the UI is **render-on-demand**: each frame the host ticks the app, feeds it input, then asks `take_dirty()` what actually changed — and renders only that. A screen sitting still draws **zero** times.

The dirty signal has two independent planes, mirroring the [two-plane architecture](../architecture/#staying-responsive-the-two-planes):

- **map** — the screen stack. Set when a gesture was applied, when a fresh GPS fix moved the camera *on a view that shows live data*, when the route or ride session changed, when a screen's own timer poll (`tick_timers`) reported a change — the Statistics cursor springing back to the live position, or a wall-clock minute crossing (the Home screensaver's big clock, or the Map's small floating clock, each region-clipped to just the digits) — or, on a riding view, when the GPS fix goes stale or returns (raising or clearing the "No GPS Fix" chip, a timer edge surfaced from the per-frame `tick`). A parked Home screen fires none of the camera edges — so between those once-a-minute clock ticks the map stays clean.
- **overlay** — the transient hold "bulge" and confirm ring. Derived from the live hold state by the input plane, which on the device runs preemptively so press-feedback latency never waits on a long map render.

```rust
loop {
    app.tick(RideClock(now), sensors, route);     // fold in sensors, move the camera
    app.handle_input(InputClock(now), &mut io);   // recognise gestures, run transitions
    let dirty = app.take_dirty();
    if dirty.map     { app.render_map(&mut display, &reader, route, w, h, color_fn); }
    if dirty.overlay { app.render_overlay(&mut display, w, h, color_fn); }
}
```

The conservative rule — *any* applied gesture dirties the map — is what keeps the idle path exact: when nothing is touched, no gesture is recognised, `apply_gesture` never runs, and the panel isn't redrawn at all.

One refinement rides on the timer poll: a screen that knows its timed change is spatially small can attach a **containing region** to it. Two screens use it: the route-planning screen's spinning compass needle repaints many times a second for several seconds while the router runs, yet only the needle's disc ever changes; and the Map's floating clock, which ticks its `HH:MM` over once a minute but only inside the small top-centre digit box — so a parked riding view never re-renders the whole map plane just to advance a digit (and when the clock overlay is hidden, no minute wake is armed at all). The dirty signal carries that region out to the host, which clips the repaint to it: the full draw sequence replays, but the framebuffer discards every pixel write outside the region (and rejects whole out-of-region primitives before rasterizing them), so the frame costs the digits/disc, not the chrome — and the [changed-rows-only push](../../hardware/display-protocol/#partial-update-a-span-masked-gate-scan) shrinks with it. The region is a promise, not a request: *any* other dirt in the same frame — a gesture, a fix, a popup — folds it away and the host full-repaints, so under-drawing can't happen; screens that never report a region behave exactly as before.

## Overlays can composite over a live map

Most navigation *replaces* the view, but the stack also supports screens that draw *over* the one below: `render_map` finds the topmost **opaque** screen and draws from there upward, so an `Overlay`-kind screen composites on top of whatever is beneath it — and because the host keeps folding in GPS fixes behind it, the map underneath doesn't visually freeze. No current screen uses the kind (the pause menu did, until it grew into the full Paused page); the mechanism is kept for what it's really shaped for — transient notifications, like a route arriving from the companion app mid-ride.

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="On the left, the stack: Home at the bottom, Map above it marked opaque, and a notification on top marked overlay. An arrow shows render_map starts from the topmost opaque screen, Map, and draws upward. On the right, a device screen mock: the map fills it, with a small route-received toast floating over the lower half, and a note that the map still updates underneath.">
  <text class="d-tag" x="20" y="24">An overlay draws over the screen below it</text>

  <!-- stack -->
  <g>
    <rect x="48" y="58" width="170" height="34" rx="6" class="d-hot" style="fill:#f8efe4" />
    <text class="d-label" x="133" y="80" text-anchor="middle" style="fill:#a9501c">Notification</text>
    <text class="d-sub" x="232" y="79" style="font-size:9px">overlay (future)</text>
    <rect x="48" y="98" width="170" height="34" rx="6" class="d-forest" />
    <text class="d-label" x="133" y="120" text-anchor="middle" style="fill:#fff">Map</text>
    <text class="d-sub" x="232" y="119" style="font-size:9px">← opaque · draw starts here</text>
    <rect x="48" y="138" width="170" height="34" rx="6" class="d-muted" />
    <text class="d-label" x="133" y="160" text-anchor="middle">Home</text>
  </g>
  <text class="d-sub" x="48" y="196" style="font-size:10px">base = topmost opaque screen;</text>
  <text class="d-sub" x="48" y="210" style="font-size:10px">everything above it composites on top</text>

  <!-- device mock -->
  <rect x="470" y="42" width="170" height="196" rx="14" style="fill:#3c6b39;stroke:#2c5230;stroke-width:2" />
  <rect x="482" y="54" width="146" height="172" rx="6" style="fill:#c3dab4" />
  <!-- faux map lines -->
  <g stroke="#7c9a63" stroke-width="2" fill="none" opacity="0.8">
    <path d="M482 120 L540 96 L600 132" /><path d="M500 200 L548 150 L628 168" />
  </g>
  <path d="M520 200 L560 150" stroke="#cf6a2a" stroke-width="2.5" fill="none" />
  <!-- notification toast -->
  <rect x="496" y="140" width="118" height="52" rx="7" style="fill:#f3f0df;stroke:#2c5230;stroke-width:1.5" />
  <rect x="496" y="140" width="118" height="18" rx="4" style="fill:#2e251a" />
  <text class="d-sub" x="555" y="153" text-anchor="middle" style="fill:#fff;font-size:9px">NEW ROUTE</text>
  <text class="d-sub" x="504" y="172" style="font-size:8.5px">Kandel Loop — 42 km</text>
  <text class="d-sub" x="504" y="186" style="font-size:8.5px">press to view</text>
  <text class="d-sub" x="555" y="232" text-anchor="middle" style="font-size:9px">map still updates underneath</text>
</svg>
<figcaption>Every current screen is opaque and replaces the view; the <code>Overlay</code> kind stays for transient panels that should not steal the whole display — a notification composites over the map, and the host keeps folding in GPS fixes behind it, so the ride doesn't visually freeze.</figcaption>
</figure>

## Screens the companion link pushes

Almost every screen is opened by a **gesture** — a press or a menu pick. A few are opened by the **BLE link instead**: the host distils the radio into a tiny app-side [`BleStatus`](../companion-link/) snapshot each pass, and when it changes the app pushes (or pops) a card the rider never navigated to. The screen system needs *nothing* new for this — a host-pushed card is a plain `Nav` screen and a `Push`/`Pop` on the same stack — but the *policy* around it is what makes the link feel like part of the device rather than a separate app.

### The passkey card

Pairing puts a **6-digit code** on the glass for the rider to type into the phone. That code is a full-screen card, but unlike every other screen it's **host-pushed**: [`set_ble_status`](../companion-link/#pairing-and-staying-paired) opens it the instant the seam's passkey goes `Some` and pops it the instant it clears (pairing done, failed, or link dropped). It is deliberately **non-dismissible** — `Back` and `press` both do nothing — so a stray button can't lose the code mid-pairing; the SMP handshake time-boxes the window, so the app runs no timeout of its own. The card is opaque, so when it pops the map plane repaints whatever was underneath exactly once.

### Route-upload prompts

A route arriving over BLE surfaces as one of three **advisory** cards — advisory because the route is committed to the store (and the Route menu) *before* the prompt, so dismissing loses nothing. All three auto-close after 30 s (timeout = dismiss), and the display **wakes** to show them (an upload usually means the phone is right there). Which one appears depends on the rider's state: an idle *"Route received — View route / Dismiss"* (with the route's stats and a mini elevation sparkline; *View route* opens the same Route overview a Route-menu pick does), a mid-ride guarded *swap* prompt (the same shape a mid-ride route pick uses), or — when the upload *replaced the route being navigated* — an info-only card, because the device has already adopted the new version (the old file is gone). The [companion-link page](../companion-link/#when-a-route-lands-the-devices-side) tells the full story; the UI-side rules are the interesting part: a prompt **never lands while a hold is charging** (it defers a tick, so a half-done *Finish & new* hold can't complete onto it — the same [stack-change hold-cancel](#hold-to-confirm) at work), **consecutive uploads replace** the prompt by object id rather than stacking, and the **passkey card outranks** it (a pending prompt is dropped, not queued).

### The connected indicator

A single **static** Bluetooth rune says "a phone is linked right now." It appears in exactly two places: the **main Menu's title bar** (the right slot of the framed header, inset left of the battery readout) and the **Home screen**, in a fixed top-right corner status slot — the two screens a rider glances at between rides. It is deliberately **absent from the riding views**. That's not an oversight but a [render-on-demand](#render-on-demand-and-the-idle-path-is-free) decision: the Map and Statistics screens repaint only when something they show moves, and a phone connecting or dropping is *not* something the ride cares about — putting a link glyph there would dirty an expensive map frame on every BLE edge. So the indicator lives only where the panel is already cheap to redraw.

### The Bluetooth settings screen

Settings ▸ Connections ▸ **Phone** is where the link's few knobs live. It's an ordinary settings screen (two-level focus like the rest), carrying: an **on/off** toggle for the radio (persisted in `Settings` like every other setting, and pushed to the radio plane by the host), a read-only **status line** (Off / Advertising / Connected, straight from the seam), a **"Paired: yes/no"** row (no phone name — deliberately not worth a protocol addition), and a hold-guarded **Forget phone** row. Forget uses the [delete-footer's guarded hold](#deleting-things-the-hold-to-delete-footer) — a completed hold fills it warning-red and clears the bond — and it matters more than it looks: because [a stored bond now rejects new pairings](../companion-link/#pairing-and-staying-paired), Forget phone is the **only** way to re-pair a replaced or reset phone. The guarded hold is the confirmation; there's no extra popup.

### The Sensors screen

Settings ▸ Connections ▸ **Sensors** pairs the ride's BLE sensors, and sits beside Phone under the Connections menu. It's three rows — **Heart rate**, **Power**, **Cadence** — each a kind label over a live status line: *Not set*, *Searching*, *Connecting*, or *Connected · 78%* (the battery percent shown only once the sensor reports it). **Press** a row to open its **scan list** — the discovered sensors of that kind, each a name (or its address when the advert carried none) over an RSSI reading; a press there **saves + connects** the highlighted sensor and pops back to the now-*Connecting* row. On a **saved** row a **hold** forgets it, using the very same [guarded delete footer](#deleting-things-the-hold-to-delete-footer) as Bluetooth's Forget phone — a plain prompt until the row is selected, then the base fills warning-red on the live hold. Saving or forgetting is just a `Settings` edit; the host reconciles the change to the radio and persists it, so there's one durable path and a saved sensor auto-reconnects across a reboot. How those sensors are actually scanned, connected, and decoded — the device's [central role](../companion-link/#sensors-the-device-as-ble-central) — is on the companion-link page.

The **live values** land on the riding grid as three [stat tiles](#settings-a-second-level-of-focus) — Heart rate, Power, Cadence — single-column raw integers (bpm / watts / rpm) that read `--` until a fresh sample arrives, and again once one goes stale (older than 5 s). They join the field catalogue like any other tile, so a rider picks and places them in the Ride screen's Data fields editor; per-ride averages and maxima are recorded into the ride object.

## Climbs get their own panel

A planned route's hard parts are its climbs, so a third riding view is given over to the one you're on. The **Climb** screen draws the current climb the way a dedicated climb computer does — a tall elevation trace with the **gradient shown as colour**, each column tinted by its local steepness from green through red. A "you are here" cursor rides the trace, and four tiles below read only *this* climb: the ascent and distance still to the top, the gradient here, and the average gradient of what's left.

**The climbs are found once, at load.** Because the route is planned, its shape is known before you turn a pedal — so finding climbs is an offline *segmentation*, not a live detector. The same load-time pass that builds the whole-route [elevation profile](../formats/) walks the height-against-distance signal and cuts it into a handful of climbs on plain rules: a stretch counts only if it gains enough, averages steeply enough, and runs long enough; a shallow dip is bridged (a false flat is still one climb) while a deep col splits it in two. Deciding up front turns the live question — *am I on a climb?* — into an interval test on the matched distance, and the found climbs cost about a kilobyte of resident state.

**Detail without a bigger buffer.** The whole-route profile is decimated to a few hundred columns — plenty for the Statistics band, far too coarse to read one climb's gradient. So the Climb screen draws from a *second*, finer profile scoped to the active climb alone: one small resident buffer, rebuilt only when you cross into a new climb, never per frame. It's the profile pyramid's trick again — precompute on load, read cheaply while riding — and, like the profile, it's a runtime structure. Nothing new is stored in the route file or sent over the link.

**It appears on its own — or waits to be asked.** A *Climb* setting picks the manner: **Auto** (the default) switches to the panel the moment a climb starts and drops back to the Map when you crest; **Manual** keeps it out of the way but in reach; **Off** hides it. With the panel live, the riding views' `back` becomes a three-stop ring — **Map → Statistics → Climb → Map** — that collapses to the plain Map ↔ Statistics swap the instant the climb ends. The auto-switch is polite: it fires only from a riding view, never yanking you out of a menu.

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="Left, a device mock of the Climb screen: a wood CLIMB title bar reading a summit height, a rising elevation profile whose columns are tinted by local gradient from green through red, an amber you-are-here cursor, and four apricot stat tiles. Right, the gradient-to-colour ramp — under 3 percent green, 3 to 6 yellow, 6 to 9 amber, 9 to 12 orange, over 12 red — with notes that the climbs are segmented once at load and drawn from a finer profile scoped to the active climb.">
  <text class="d-tag" x="20" y="24">The climb panel — gradient shown as colour</text>

  <rect x="40" y="44" width="150" height="192" rx="10" style="fill:#ffffff;stroke:#aaaa55;stroke-width:1.5" />
  <rect x="46" y="50" width="138" height="20" rx="5" style="fill:#aa5500" />
  <text x="56" y="64" style="fill:#fff;font-family:var(--mono);font-size:9px">CLIMB</text>
  <text x="178" y="64" text-anchor="end" style="fill:#fff;font-family:var(--mono);font-size:8px">1762 m</text>
  <g>
    <rect x="52" y="138" width="9" height="12" style="fill:#00aa00" />
    <rect x="62.5" y="132" width="9" height="18" style="fill:#00aa00" />
    <rect x="73" y="126" width="9" height="24" style="fill:#ffff00" />
    <rect x="83.5" y="120" width="9" height="30" style="fill:#ffff00" />
    <rect x="94" y="112" width="9" height="38" style="fill:#ffaa00" />
    <rect x="104.5" y="105" width="9" height="45" style="fill:#ffaa00" />
    <rect x="115" y="98" width="9" height="52" style="fill:#ff5500" />
    <rect x="125.5" y="92" width="9" height="58" style="fill:#ff5500" />
    <rect x="136" y="88" width="9" height="62" style="fill:#ff0000" />
    <rect x="146.5" y="86" width="9" height="64" style="fill:#ff5500" />
    <rect x="157" y="84" width="9" height="66" style="fill:#ffaa00" />
    <rect x="167.5" y="82" width="9" height="68" style="fill:#ffff00" />
  </g>
  <line x1="105" y1="78" x2="105" y2="150" stroke="#ffaa00" stroke-width="2" />
  <circle cx="105" cy="108" r="3.5" style="fill:#000" /><circle cx="105" cy="108" r="2.2" style="fill:#ffaa00" />
  <line x1="52" y1="151" x2="176" y2="151" stroke="#aaaa55" stroke-width="1" />
  <rect x="52" y="160" width="59" height="30" rx="4" style="fill:#ffaa55" />
  <rect x="117" y="160" width="59" height="30" rx="4" style="fill:#ffaa55" />
  <rect x="52" y="196" width="59" height="30" rx="4" style="fill:#ffaa55" />
  <rect x="117" y="196" width="59" height="30" rx="4" style="fill:#ffaa55" />
  <text x="57" y="172" style="fill:#6b5a2a;font-family:var(--mono);font-size:7px">TO CLIMB</text>
  <text x="122" y="172" style="fill:#6b5a2a;font-family:var(--mono);font-size:7px">KM TO GO</text>
  <text x="57" y="208" style="fill:#6b5a2a;font-family:var(--mono);font-size:7px">GRADE</text>
  <text x="122" y="208" style="fill:#6b5a2a;font-family:var(--mono);font-size:7px">AVG GRAD</text>

  <text class="d-sub" x="300" y="60" style="font-size:10.5px">local gradient → stripe colour</text>
  <g>
    <rect x="300" y="72" width="70" height="26" style="fill:#00aa00" />
    <rect x="370" y="72" width="70" height="26" style="fill:#ffff00" />
    <rect x="440" y="72" width="70" height="26" style="fill:#ffaa00" />
    <rect x="510" y="72" width="70" height="26" style="fill:#ff5500" />
    <rect x="580" y="72" width="70" height="26" style="fill:#ff0000" />
  </g>
  <text class="d-sub" x="335" y="114" text-anchor="middle" style="font-size:9px">&lt; 3%</text>
  <text class="d-sub" x="405" y="114" text-anchor="middle" style="font-size:9px">3–6</text>
  <text class="d-sub" x="475" y="114" text-anchor="middle" style="font-size:9px">6–9</text>
  <text class="d-sub" x="545" y="114" text-anchor="middle" style="font-size:9px">9–12</text>
  <text class="d-sub" x="615" y="114" text-anchor="middle" style="font-size:9px">&gt; 12%</text>

  <text class="d-sub" x="300" y="150" style="font-size:10px">· climbs are segmented once, when the route loads</text>
  <text class="d-sub" x="300" y="170" style="font-size:10px">· a dip is bridged, a deep col splits — plain gates on</text>
  <text class="d-sub" x="312" y="186" style="font-size:10px">gain, average grade, and length</text>
  <text class="d-sub" x="300" y="208" style="font-size:10px">· a finer profile, scoped to the active climb, rebuilt</text>
  <text class="d-sub" x="312" y="224" style="font-size:10px">only when you cross into the next one</text>
</svg>
<figcaption>The Climb screen is the profile the way a paper route card would draw it — the trace tinted by gradient, hottest where it's steepest. The climbs themselves are found in one pass when the route loads (the same moment the whole-route profile is built), so riding only has to ask which segment the matched distance falls in; the panel then reads a second, finer profile scoped to that one climb — a small buffer rebuilt on each new climb, never per frame. Nothing new is stored in the route file.</figcaption>
</figure>

## Waypoints on the route

A planned route can carry **named waypoints** — a water stop, a viewpoint, the pass at the top — pinned along it in an [along-route table](../formats/#the-file) in the route file (`OBCR` v2). In the riding views the device still treats them as calm furniture: three always-available cues plus two opt-in stat fields. The ride menu adds one deliberate plan browser without turning it into a fourth riding view; every surface shares the same resident table and matched-distance axis.

**The whole-plan list.** The ride menu's north **Waypoints** station opens on the next waypoint but keeps the route-ordered plan around it. Each two-line row shows the stored name, along-route distance still to go, and remaining ascent to that point. The climb figure is not a subtraction of waypoint elevations: it subtracts the cached profile's cumulative ascent at the live matched fraction from cumulative ascent at the waypoint fraction, so dips between here and the stop do not erase later climbing. Passed rows remain visible but muted, with both forward-looking figures clamped to zero; this is friendlier when you want to review the day's plan than silently dropping half of it. `back` returns to the ride compass, while press/hold have no row action in the MVP, so opening or browsing the list never changes the recording mode or session.

The fixed-memory boundary remains explicit. A normal route's complete named table fits the resident **32-waypoint** cache. If a file carries more, the existing next-waypoint machinery advances that cache as a 32-entry window; the list shows that current resident plan window, and passed entries evicted from an oversized route are not re-read just to reconstruct history. A route with no named entries shows *No waypoints*; a route-less ride adds *No route loaded* beneath the same state.

**Diamonds on the map.** Each named waypoint draws as a small ink diamond (~9 px) on the route line at its position — no label, the way the route's direction arrows read as furniture. They're always on when the loaded route has waypoints; the name lives in the chip, not on the glyph. A waypoint whose coordinate sits slightly off the drawn polyline shows its diamond slightly off the line, which is honest — the diamond marks the *point*, not the nearest pixel of route.

**The approach chip.** A one-line pill at the **bottom** of the Map — the same idiom as the off-route chip at the top, but calm ink-on-parchment rather than warning-orange — reads `◆ NAME  <distance>`: the along-route distance still to go to the next waypoint (the same arithmetic as the climb tiles' *to climb*). A three-state setting governs it, like the climb mode:

- **Off** — never shown. Route planners sprinkle artefact waypoints into their GPX exports; a route full of junk must be silenceable, so Off is the escape hatch.
- **Approach** *(the default)* — the chip appears only once the next waypoint is within **500 m** ahead and counts the metres down, so you notice the stop without standing chrome.
- **Always** — visible whenever a waypoint is still ahead (kilometres beyond 1 km, metres inside).

Two details keep it steady. **Passing** is distance-hysteresis, not time: the chip lingers on a waypoint until you're **100 m past** it — the shown distance pinned at 0 through the linger — before advancing to the next, so GPS jitter at the stop can't flap the readout. And the chip **hides off-route**: the along-route distance is meaningless once you've left the line, and the bottom slot belongs to the warning chip (*off route* / *No GPS Fix*) when it's up. The waypoint chip takes that slot only when it's clear — and the scale bar steps up above whichever chip is showing.

**Ticks on the progress bar.** Under the Statistics [elevation profile](../formats/#exact-stats-decimated-geometry) the amber live-fraction bar already shares the route's distance axis, so each waypoint gets a thin **black** tick at its fraction, and the fill sweeping toward the next tick is free "distance to the next stop" context. Black, not red, is deliberate: the bar tints warning-red when you're off-route, and a red tick would vanish against it exactly then. (Diamonds *on* the profile were tried and dropped — ten waypoints clutter the thin band; the calm ticks won.)

**Two opt-in stat fields.** For a waypoint readout on the numbers page, the [field picker](#settings-a-second-level-of-focus) offers two, so they cost nothing for riders who don't use them:

- a **2×1 "next waypoint" tile** — name and distance-to-go, a direct sibling of the wide clock tile;
- a **2×3 list panel** — the next few waypoints, name left / distance right, the first row emphasised. It's the one page-sized field: six slots, so it always begins a page, mirroring how a two-span tile always begins a row.

**Unnamed waypoints are ignored everywhere** — no diamond, no tick, no chip, no list row. An empty label carries no information anywhere it would surface, so a waypoint whose name is blank after trimming is dropped as the route's table loads; the diamond, tick and row counts then stay consistent by construction. Together with **Off**, that's the whole answer to junk-waypoint routes: nameless artefacts never appear, and a route full of *named* clutter is silenced with one setting.

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="Left, a device mock of the riding Map: a magenta route line down the middle, two small black diamonds on it marking named waypoints, a red heading arrow for the rider, and a bottom pill reading a diamond, the name Pass Summit and 299 m — the approach chip counting down. Right, the Statistics progress bar in close-up: an amber fill from the left with two black vertical ticks, one at the far left for a waypoint at the start and one near three-quarters for the pass, annotated: the fill sweeps toward the next tick, the ticks are ink not red so they survive the off-route red tint, and the chip hides off-route.">
  <text class="d-tag" x="20" y="24">Waypoints — diamonds, the approach chip, progress-bar ticks</text>

  <!-- device mock: the Map -->
  <rect x="40" y="48" width="150" height="188" rx="10" style="fill:#ffffff;stroke:#aaaa55;stroke-width:1.5" />
  <rect x="107" y="54" width="12" height="150" style="fill:#ff00ff" />
  <!-- waypoint diamonds on the route -->
  <path d="M113 92 l7 7 l-7 7 l-7 -7 z" style="fill:#000" />
  <path d="M113 132 l7 7 l-7 7 l-7 -7 z" style="fill:#000" />
  <!-- rider heading arrow -->
  <path d="M113 158 l8 15 l-8 -5 l-8 5 z" style="fill:#ff0000" />
  <!-- scale bar -->
  <line x1="50" y1="196" x2="72" y2="196" stroke="#000" stroke-width="1.4" />
  <text x="50" y="192" style="fill:#000;font-family:var(--mono);font-size:7px">20m</text>
  <!-- approach chip -->
  <rect x="46" y="208" width="138" height="22" rx="9" style="fill:#ffffff;stroke:#000;stroke-width:1" />
  <path d="M60 219 l4 4 l-4 4 l-4 -4 z" style="fill:#000" />
  <text x="70" y="223" style="fill:#000;font-family:var(--mono);font-size:8.5px">Pass Summit  299m</text>
  <text class="d-sub" x="115" y="248" text-anchor="middle" style="font-size:9px">ink diamonds + the bottom approach chip</text>

  <!-- progress bar close-up -->
  <text class="d-sub" x="250" y="70" style="font-size:10.5px">the Statistics progress bar shares the route's distance axis</text>
  <rect x="250" y="86" width="430" height="18" rx="9" style="fill:#eae3c9;stroke:#aaaa55;stroke-width:1" />
  <!-- live fill ~0.63 -->
  <rect x="250" y="86" width="271" height="18" rx="9" style="fill:#ffaa00" />
  <!-- ticks: start (~0) and pass (~0.77) -->
  <rect x="252" y="89" width="2.4" height="12" style="fill:#000" />
  <rect x="581" y="89" width="2.4" height="12" style="fill:#000" />
  <!-- annotations -->
  <line x1="253" y1="118" x2="253" y2="130" stroke="#6b5a2a" stroke-width="1" />
  <text class="d-sub" x="258" y="142" style="font-size:9px">waypoint at the start</text>
  <line x1="582" y1="118" x2="582" y2="130" stroke="#6b5a2a" stroke-width="1" />
  <text class="d-sub" x="582" y="142" text-anchor="middle" style="font-size:9px">the pass</text>
  <line x1="521" y1="76" x2="521" y2="84" stroke="#a9501c" stroke-width="1.2" marker-end="none" />
  <text class="d-sub" x="521" y="72" text-anchor="middle" style="fill:#a9501c;font-size:9px">you are here</text>
  <text class="d-sub" x="250" y="176" style="font-size:10px">· the fill sweeping toward the next tick is free "distance to go"</text>
  <text class="d-sub" x="250" y="194" style="font-size:10px">· ticks are <tspan style="fill:#000;font-weight:600">ink</tspan>, never red — the bar itself tints red off-route, where a</text>
  <text class="d-sub" x="262" y="210" style="font-size:10px">red tick would vanish</text>
  <text class="d-sub" x="250" y="228" style="font-size:10px">· off-route the chip hides and the bar freezes — the along-route</text>
  <text class="d-sub" x="262" y="244" style="font-size:10px">distance is meaningless once you've left the line</text>
</svg>
<figcaption>Three read-outs, one distance axis. On the map the diamonds mark the waypoints and the bottom chip names the next one and counts down (default: only inside 500 m); on the Statistics bar the same waypoints are black ticks with the amber fill closing on the next. All of it is derived on each matched fix from the route's along-route waypoint table — nothing extra stored, nothing new sent over the link — and all of it hides or freezes off-route, where an along-route distance has no meaning. Unnamed waypoints are filtered as the table loads, so every count stays consistent.</figcaption>
</figure>

## The whole flow

Put the pieces together and the navigation graph is small and legible. Two screens are always **riding views** — the Map and the Elevation/Statistics profile — and they're siblings: `back` swaps between them without growing the stack. On a climb a [third view](#climbs-get-their-own-panel) joins the ring between them. All three share `press` (pause) and `back-hold` (the ride-scoped compass); the Paused page accepts the same `back-hold`. Opening that menu never changes the activity mode, so it neither pauses a rolling rider nor resumes a paused one. The Map's Pan sub-mode keeps one deliberate override: there, `back-hold` exits Pan first instead of opening a menu.

<figure class="fig">
<svg viewBox="0 0 900 360" role="img" aria-label="A navigation graph. Home opens the main compass Menu. Its Routes station opens the Route menu, a route pick opens Overview, and START roots to Map. Map, Statistics, and Climb form the riding-view back ring. Back-hold from a riding view or Paused opens the ride compass without changing activity mode; its north station opens the Waypoints whole-plan list, while the other stations are Detour, POIs, Routes, and Main menu. Pan keeps back-hold as exit Pan. Press from Map pauses; Resume returns and held Finish or Discard clears to Home.">
  <defs>
    <marker id="aU7" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#5f7d3d" /></marker>
    <marker id="aU7c" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">The screen flow</text>

  <!-- nodes -->
  <rect class="d-panel" x="30"  y="160" width="100" height="40" rx="9" /><text class="d-label" x="80"  y="180" text-anchor="middle">Home</text><text class="d-sub" x="80" y="193" text-anchor="middle" style="font-size:8.5px">root</text>
  <rect class="d-panel-2" x="210" y="50" width="116" height="40" rx="9" /><text class="d-label" x="268" y="70" text-anchor="middle">Route menu</text><text class="d-sub" x="268" y="83" text-anchor="middle" style="font-size:8.5px">pick a route</text>
  <rect class="d-panel-2" x="220" y="160" width="104" height="40" rx="9" /><text class="d-label" x="272" y="180" text-anchor="middle">Overview</text><text class="d-sub" x="272" y="193" text-anchor="middle" style="font-size:8.5px">track · profile · stats</text>
  <rect class="d-panel-2" x="210" y="270" width="116" height="40" rx="9" /><text class="d-label" x="268" y="290" text-anchor="middle">Main menu</text><text class="d-sub" x="268" y="303" text-anchor="middle" style="font-size:8.5px">compass · 5 stations</text>
  <rect class="d-hot" x="410" y="50" width="104" height="40" rx="9" style="fill:#f8efe4" /><text class="d-label" x="462" y="74" text-anchor="middle" style="fill:#a9501c">Paused</text>
  <rect class="d-forest" x="410" y="160" width="104" height="40" rx="9" /><text class="d-label" x="462" y="184" text-anchor="middle" style="fill:#fff">Map</text>
  <rect class="d-panel-2" x="410" y="270" width="104" height="40" rx="9" /><text class="d-label" x="462" y="294" text-anchor="middle">Pan / Zoom</text>
  <rect class="d-panel-2" x="585" y="50" width="130" height="40" rx="9" /><text class="d-label" x="650" y="70" text-anchor="middle">Ride menu</text><text class="d-sub" x="650" y="83" text-anchor="middle" style="font-size:8.5px">fixed compass · 5</text>
  <rect class="d-panel-2" x="755" y="50" width="110" height="40" rx="9" /><text class="d-label" x="810" y="70" text-anchor="middle">Waypoints</text><text class="d-sub" x="810" y="83" text-anchor="middle" style="font-size:8.5px">whole plan list</text>
  <rect class="d-water" x="585" y="160" width="115" height="40" rx="9" /><text class="d-label" x="642" y="180" text-anchor="middle" style="fill:#fff">Statistics</text><text class="d-sub" x="642" y="193" text-anchor="middle" style="fill:#dfe6e0;font-size:8.5px">elevation</text>
  <rect class="d-water" x="755" y="160" width="110" height="40" rx="9" /><text class="d-label" x="810" y="180" text-anchor="middle" style="fill:#fff">Climb</text><text class="d-sub" x="810" y="193" text-anchor="middle" style="fill:#dfe6e0;font-size:8.5px">on a climb</text>

  <!-- edge from Home: both press and back-hold open the Menu (the single door in) -->
  <line x1="130" y1="180" x2="208" y2="286" stroke="#5f7d3d" stroke-width="1.6" marker-end="url(#aU7)" /><text class="d-sub" x="136" y="238" style="font-size:9px">press · back-hold</text>
  <!-- Menu -> Route menu -->
  <line x1="334" y1="274" x2="334" y2="92" stroke="#5f7d3d" stroke-width="1.6" marker-end="url(#aU7)" /><text class="d-sub" x="340" y="184" style="font-size:9px">Routes</text>
  <!-- Route menu -> Overview (press) -->
  <line x1="268" y1="90" x2="272" y2="158" stroke="#5f7d3d" stroke-width="1.6" marker-end="url(#aU7)" /><text class="d-sub" x="230" y="126" style="font-size:9px">press</text>
  <!-- Overview -> Map (START/Root) -->
  <line x1="324" y1="180" x2="408" y2="180" stroke="#cf6a2a" stroke-width="2" marker-end="url(#aU7c)" /><text class="d-sub" x="332" y="173" style="fill:#a9501c;font-size:9px">START · Root</text>
  <!-- Map <-> Statistics -->
  <line x1="514" y1="170" x2="583" y2="170" stroke="#5f7d3d" stroke-width="1.6" marker-end="url(#aU7)" />
  <line x1="585" y1="190" x2="516" y2="190" stroke="#5f7d3d" stroke-width="1.6" marker-end="url(#aU7)" /><text class="d-sub" x="550" y="212" text-anchor="middle" style="font-size:9px">off-climb return</text>
  <!-- On-climb ring: Statistics -> Climb -> Map. Off-climb, Statistics returns directly to Map. -->
  <line x1="700" y1="170" x2="753" y2="170" stroke="#5f7d3d" stroke-width="1.4" stroke-dasharray="4 4" marker-end="url(#aU7)" />
  <path d="M810 200 C 810 248, 560 248, 516 196" fill="none" stroke="#5f7d3d" stroke-width="1.4" stroke-dasharray="4 4" marker-end="url(#aU7)" /><text class="d-sub" x="700" y="260" text-anchor="middle" style="font-size:9px">on-climb back ring</text>
  <!-- Map -> Paused -->
  <line x1="454" y1="160" x2="454" y2="92" stroke="#cf6a2a" stroke-width="2" marker-end="url(#aU7c)" /><text class="d-sub" x="366" y="128" style="fill:#a9501c;font-size:9px">press · pause</text>
  <!-- Paused -> Map (resume) -->
  <line x1="472" y1="90" x2="472" y2="158" stroke="#5f7d3d" stroke-width="1.4" marker-end="url(#aU7)" /><text class="d-sub" x="480" y="126" style="font-size:9px">resume</text>
  <!-- Map -> Pan -->
  <line x1="454" y1="200" x2="454" y2="268" stroke="#5f7d3d" stroke-width="1.6" marker-end="url(#aU7)" /><text class="d-sub" x="418" y="238" style="font-size:9px">hold</text>
  <line x1="472" y1="270" x2="472" y2="202" stroke="#5f7d3d" stroke-width="1.4" marker-end="url(#aU7)" /><text class="d-sub" x="480" y="246" style="font-size:8.5px">back-hold exits</text>
  <!-- Ride-menu access: same Push from riding views and Paused; no activity-mode write. -->
  <line x1="514" y1="70" x2="583" y2="70" stroke="#5f7d3d" stroke-width="1.5" marker-end="url(#aU7)" /><text class="d-sub" x="548" y="61" text-anchor="middle" style="font-size:8.5px">back-hold</text>
  <line x1="642" y1="160" x2="642" y2="92" stroke="#5f7d3d" stroke-width="1.5" marker-end="url(#aU7)" /><text class="d-sub" x="650" y="128" style="font-size:8.5px">back-hold</text>
  <path d="M514 164 C 538 116, 558 94, 585 82" fill="none" stroke="#5f7d3d" stroke-width="1.4" marker-end="url(#aU7)" />
  <path d="M755 164 C 738 124, 726 94, 715 82" fill="none" stroke="#5f7d3d" stroke-width="1.4" stroke-dasharray="4 4" marker-end="url(#aU7)" />
  <!-- The north/default ride station opens the route-ordered waypoint plan. -->
  <line x1="717" y1="70" x2="753" y2="70" stroke="#5f7d3d" stroke-width="1.5" marker-end="url(#aU7)" /><text class="d-sub" x="735" y="61" text-anchor="middle" style="font-size:8.5px">press</text>
  <!-- The ride ring's Main-menu station keeps the full app one detent away. -->
  <path d="M585 82 C 560 330, 398 340, 326 300" fill="none" stroke="#5f7d3d" stroke-width="1.4" marker-end="url(#aU7)" /><text class="d-sub" x="430" y="338" style="font-size:8.5px">Main menu station</text>
  <!-- Paused -> Home (finish/discard) -->
  <path d="M410 60 C 260 12, 82 70, 80 158" fill="none" stroke="#cf6a2a" stroke-width="1.6" stroke-dasharray="4 4" marker-end="url(#aU7c)" /><text class="d-sub" x="236" y="28" style="fill:#a9501c;font-size:9px">Finish / Discard (hold) → Home</text>
</svg>
<figcaption>Green edges are ordinary moves; coral marks the "go ride / stop riding" path. Home still opens the full <b>Main menu</b>, whose Routes station leads through the Route menu and Overview to a clean <code>[Home, Map]</code>. During a recording, <code>back-hold</code> from Map, Statistics, Climb, or Paused instead pushes the fixed <b>Ride menu</b>; opening it leaves <code>Activity::mode</code> alone, and <code>back</code> returns to the exact view that called it. Its clockwise ring is Waypoints, Detour, POIs, Routes, Main menu. Waypoints opens the route-ordered whole-plan list shown at upper right; Detour temporarily replaces the compass with its rejoin chooser (and, past it, the plan spinner and detour preview), then commit or cancel returns to the exact caller. POIs and Routes reuse their existing mid-ride flows, and Main menu opens the full compass. A route-less ride keeps all five detents but dims Waypoints and Detour. Pan is the exception shown at the bottom: its <code>back-hold</code> exits Pan rather than opening ride chrome. Picking a different route mid-ride still detours through the guarded swap prompt; Paused still owns Resume / Finish / Discard; and the idle-return timeout still clears abandoned chrome back to Home or — while tracking — the Map.</figcaption>
</figure>

## The "field map" look

The UI is styled like a weatherproof field map — a wood frame, a parchment panel, ink text, an amber selection highlight — and it shares the map's colour pipeline exactly. Screen colours are authored in RGB565 and resolved through the **same `color_fn`** the map styles use, so chrome and map quantise together. That matters because the panel is only **64 colours** (RGB222): every palette value is chosen for how it looks *after* quantisation, and a test asserts each one's device-64 result so a retune can't silently drift.

<figure class="fig">
<svg viewBox="0 0 720 200" role="img" aria-label="A palette swatch strip showing the device-64 colours: parchment white, wood brown, ink black, amber, warning orange, route magenta, and breadcrumb navy. Beside it, a small framed-screen mock with a wood title bar, a parchment body, and an amber-highlighted list row with a pointer bullet.">
  <text class="d-tag" x="20" y="24">One palette, tuned to the 64-colour panel</text>

  <!-- swatches -->
  <g>
    <rect x="36"  y="48" width="54" height="40" rx="6" style="fill:#ffffff;stroke:#9aa884;stroke-width:1" /><text class="d-sub" x="63" y="104" text-anchor="middle" style="font-size:9px">parchment</text>
    <rect x="98"  y="48" width="54" height="40" rx="6" style="fill:#aa5500" /><text class="d-sub" x="125" y="104" text-anchor="middle" style="font-size:9px">wood</text>
    <rect x="160" y="48" width="54" height="40" rx="6" style="fill:#000000" /><text class="d-sub" x="187" y="104" text-anchor="middle" style="font-size:9px">ink</text>
    <rect x="222" y="48" width="54" height="40" rx="6" style="fill:#ffaa00" /><text class="d-sub" x="249" y="104" text-anchor="middle" style="font-size:9px">amber</text>
    <rect x="284" y="48" width="54" height="40" rx="6" style="fill:#ff5500" /><text class="d-sub" x="311" y="104" text-anchor="middle" style="font-size:9px">warning</text>
    <rect x="346" y="48" width="54" height="40" rx="6" style="fill:#ff00ff" /><text class="d-sub" x="373" y="104" text-anchor="middle" style="font-size:9px">route</text>
    <rect x="408" y="48" width="54" height="40" rx="6" style="fill:#0000aa" /><text class="d-sub" x="435" y="104" text-anchor="middle" style="font-size:9px">breadcrumb</text>
  </g>
  <text class="d-sub" x="36" y="140" style="font-size:10px">authored in RGB565 · resolved through the same color_fn as the map ·</text>
  <text class="d-sub" x="36" y="156" style="font-size:10px">every value asserted against its device-64 (RGB222) result</text>

  <!-- mini framed screen -->
  <rect x="556" y="44" width="128" height="138" rx="10" style="fill:#ffffff;stroke:#aaaa55;stroke-width:1.5" />
  <rect x="562" y="50" width="116" height="22" rx="5" style="fill:#aa5500" /><text class="d-sub" x="572" y="65" style="fill:#fff;font-size:9px">MENU</text>
  <rect x="566" y="82" width="108" height="26" rx="5" style="fill:#ffaa00" />
  <path d="M578 88 L578 102 L588 95 z" fill="#000" /><text class="d-sub" x="596" y="99" style="font-size:9px">Routes</text>
  <text class="d-sub" x="596" y="129" style="font-size:9px">Settings</text>
  <line x1="566" y1="138" x2="674" y2="138" stroke="#aaaa55" stroke-width="1" />
</svg>
<figcaption>Drawing happens through the renderer's small <code>Surface</code> vocabulary (rounded rects, text in four font tiers, triangles, lines, rules), implemented once by its <code>Canvas</code> — the same primitives for every screen, so the Menu, the Route list and the Elevation header share one look. There's no theming engine; the palette is a handful of named constants.</figcaption>
</figure>

---

## Where this lives

- The screen system, stack, and `Transition`: [`obc-app/src/screen/mod.rs`](src:firmware/obc-app/src/screen/mod.rs)
- The Climb screen: [`obc-app/src/screen/climb.rs`](src:firmware/obc-app/src/screen/climb.rs); the climb detection + per-climb profile it reads: [`obc-route/src/climb.rs`](src:firmware/obc-route/src/climb.rs), [`obc-route/src/climb_profile.rs`](src:firmware/obc-route/src/climb_profile.rs)
- The waypoint UI — the ride-menu whole-plan list: [`obc-app/src/screen/ride_menu.rs`](src:firmware/obc-app/src/screen/ride_menu.rs); map diamonds + the approach chip: [`obc-app/src/screen/map.rs`](src:firmware/obc-app/src/screen/map.rs); progress-bar ticks: [`obc-app/src/screen/statistics.rs`](src:firmware/obc-app/src/screen/statistics.rs); next-waypoint tracking (resident-window advance + pass linger): [`obc-app/src/ride_engine.rs`](src:firmware/obc-app/src/ride_engine.rs); the two stat fields: [`obc-app/src/stat_fields.rs`](src:firmware/obc-app/src/stat_fields.rs)
- The host→app BLE seam (`BleStatus` — the connected indicator, passkey, paired): [`obc-app/src/ble.rs`](src:firmware/obc-app/src/ble.rs)
- The host-pushed cards — the passkey card and the route-upload prompts: [`obc-app/src/screen/passkey.rs`](src:firmware/obc-app/src/screen/passkey.rs), [`obc-app/src/screen/route_received.rs`](src:firmware/obc-app/src/screen/route_received.rs)
- The Rides screen, its Ride detail, and the Bluetooth settings screen: [`obc-app/src/screen/rides.rs`](src:firmware/obc-app/src/screen/rides.rs), [`obc-app/src/screen/ride_detail.rs`](src:firmware/obc-app/src/screen/ride_detail.rs), [`obc-app/src/screen/settings/bluetooth.rs`](src:firmware/obc-app/src/screen/settings/bluetooth.rs)
- The settings screens (the two-level editors + the shared kit): [`obc-app/src/screen/settings/`](src:firmware/obc-app/src/screen/settings)
- The `Settings` value + its byte codec and the `Language` enum: [`obc-app/src/settings.rs`](src:firmware/obc-app/src/settings.rs); the dependency-free `SettingsStore` seam: [`obc-ports/src/lib.rs`](src:firmware/obc-ports/src/lib.rs); direct adapters: [`obc-sim/src/settings_store.rs`](src:firmware/obc-sim/src/settings_store.rs), [`obc-fw-nrf54l/src/settings.rs`](src:firmware/obc-fw-nrf54l/src/settings.rs)
- The i18n catalogue + codegen — the per-language TOMLs, the `build.rs` that generates `Msg`/`TABLE`, and the `t()`/`rx.t()` lookup: [`obc-app/i18n/`](src:firmware/obc-app/i18n), [`obc-app/build.rs`](src:firmware/obc-app/build.rs), [`obc-app/src/i18n.rs`](src:firmware/obc-app/src/i18n.rs); the font-repertoire guard: [`obc-app/tests/i18n.rs`](src:firmware/obc-app/tests/i18n.rs)
- The gesture recognizer: [`obc-app/src/input.rs`](src:firmware/obc-app/src/input.rs)
- The input + overlay plane: [`obc-app/src/input_plane.rs`](src:firmware/obc-app/src/input_plane.rs)
- The app driver (frame loop, render-on-demand, compositing): [`obc-app/src/app.rs`](src:firmware/obc-app/src/app.rs)
- The injected-hardware traits, settings persistence seam, and input types: [`obc-ports/src/lib.rs`](src:firmware/obc-ports/src/lib.rs); platform adapters: [`obc-platform/src/`](src:firmware/obc-platform/src); compatibility-only app re-exports: [`obc-app/src/hal.rs`](src:firmware/obc-app/src/hal.rs)

For how the two planes keep input responsive under a long render, and where the HAL fits, see [system architecture](../architecture/). For how a screen's `draw` actually puts pixels on the panel, see the [rendering pipeline](../rendering/).
