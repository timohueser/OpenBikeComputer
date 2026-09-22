---
title: UI system
description: The screen model, the four buttons, the drawers, and what a rider can do from where.
copy: ai
---

# The UI system

The UI is a `no_std`, allocation-free system for a 240 × 320 display and four buttons. It draws
immediately: there is no retained widget tree, and a screen writes the frame it wants.

The screen drawings below are schematics. They explain behavior. They are not pixel captures.

## Screen model

Each screen is one variant of a `Screen` enum and owns its state by value. One table declares every
variant with its capabilities, and generates the enum, the dispatch, and the capability data. A
capability is a cross-cutting fact the row states once, so nothing else matches on the variant:

| Capability | What it decides |
| --- | --- |
| `kind` | `Base`, `Overlay` or `Settings`. The overlay and settings behaviors hang off it. |
| `base` | `Map`, `LiveRiding` or `Chrome`. It gates map reads, live-data repaint, and the Bluetooth indicator. |
| `reader` | When the screen needs the streamed map at draw: never, always, or for articles, a photo, or POI data. |
| `render_key` | Which set of facts a repaint of this screen depends on. |
| `recess` | A drawer draws this screen again one shade down, instead of leaving it standing. |
| `idle_exempt` | The idle-return timeout must never take this screen away. |
| `ride_view` | A deliberate ride view: the idle timeout leaves it while a ride is tracked. |
| `browse_exempt` | A deliberate browse view: the idle timeout does not return it to Home when no ride is tracked. |
| `blocks_chords` | While it is on top, the device-wide drawer chords are refused. |
| `blocks_escape` | While it is on top, the global Back-hold escape does not leave it. |

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 322" role="img" aria-label="On the left, the Screen enum lists representative variants: Home, Map, Statistics, RideControl, Menu, ContextDrawer, RouteMenu, RouteOverview, and RouteSwap. The Map variant points to its module on the right, which holds typed state, a handle method returning a Transition, and a draw method emitting pixels. A tag notes static match dispatch, no dyn and no allocation.">
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
    <rect x="52" y="192" width="178" height="20" rx="5" class="d-muted" /><text class="d-sub" x="62" y="206">ContextDrawer(…)</text>
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
    <text class="d-sub" x="352" y="123" style="fill:#a9501c;font-size:12px">typed state — owned, inline, no alloc</text>
    <rect x="340" y="134" width="328" height="32" rx="6" style="fill:#eef2df;stroke:#9aa884;stroke-width:1" />
    <text class="d-sub" x="352" y="148">fn handle(g, &amp;mut Ctx) → Transition</text>
    <text class="d-sub" x="352" y="161" style="fill:#a9501c;font-size:12px">logic — react to a gesture, ask to navigate</text>
    <rect x="340" y="172" width="328" height="32" rx="6" style="fill:#eef2df;stroke:#9aa884;stroke-width:1" />
    <text class="d-sub" x="352" y="186">fn draw(target, &amp;Render)</text>
    <text class="d-sub" x="352" y="199" style="fill:#a9501c;font-size:12px">pixels — read state, paint the panel</text>
  </g>

  <text class="d-tag" x="324" y="240">dispatched by match — static, zero-alloc</text>
  <text class="d-sub" x="324" y="258" style="font-size:12px">one row per screen, with its capabilities</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Each screen owns typed state. The <code>Screen</code> enum provides static dispatch without heap allocation.</figcaption>
</figure>

A screen handles one gesture, returns a transition, and draws the current frame.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 220" role="img" aria-label="Two side-by-side contexts. On the left, handle receives Ctx, the mutable half: app state, activity mode, Navigator route state, Recorder ride state, and settings. On the right, draw receives Render, the read-only half: the map reader, renderer, route guidance, route caches, breadcrumb, size, and hold-progress.">
  <text class="d-tag" x="20" y="24">Two halves of the world, handed to two methods</text>

  <!-- Ctx -->
  <rect class="d-panel" x="36" y="44" width="300" height="150" rx="12" />
  <text class="d-label" x="56" y="68">handle(g, &amp;mut Ctx)</text>
  <text class="d-tag" x="56" y="84">mutable — change the world</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="56" y="104">state &nbsp;&nbsp;— camera · zoom · pan</text>
    <text class="d-sub" x="56" y="124">activity — ride mode · UI requests</text>
    <text class="d-sub" x="56" y="144">navigator — route · match · guidance</text>
    <text class="d-sub" x="56" y="164">recorder — ride totals · sensors</text>
    <text class="d-sub" x="56" y="184">settings — units · clock · intervals</text>
  </g>

  <!-- Render -->
  <rect class="d-panel-2" x="384" y="44" width="300" height="150" rx="12" />
  <text class="d-label" x="404" y="68">draw(target, &amp;Render)</text>
  <text class="d-tag" x="404" y="84">read-only — paint the world</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="404" y="106">reader · renderer — draw the map</text>
    <text class="d-sub" x="404" y="126">state · guidance · route caches · breadcrumb</text>
    <text class="d-sub" x="404" y="146">w · h &nbsp;— panel size</text>
    <text class="d-sub" x="404" y="166">hold_progress — the confirm ring</text>
  </g>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Input handling uses mutable <code>Ctx</code>. Drawing uses read-only <code>Render</code> data and borrowed render resources.</figcaption>
</figure>

Input receives a mutable context; drawing receives a read-only one. A screen therefore cannot change
state while it draws. `Canvas` implements `Surface` and `RenderFrame` derefs to `Render`, so the
host's generics stop at the dispatch:

```rust
fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition

// The `screens!` dispatch, generic over the host's draw target and its color policy.
fn draw<D, F>(&self, cv: &mut Canvas<D, F>, rx: &mut RenderFrame<'_, '_>)
where D: DrawTarget, F: Fn(u16) -> D::Color

// What a screen module writes. The `RenderFrame` form stays with the map draws and their callers.
fn draw(&self, cv: &mut impl Surface, rx: &mut Render)
```

## Navigation

The screen stack holds at most ten screens, and Home is always the first.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 384" role="img" aria-label="A gesture reaches the top screen. Its handler returns a transition, which changes the stack. Home remains at the bottom. Back-hold is handled globally before screen dispatch.">
  <defs><marker id="software-ui-2" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker></defs>
  <text class="d-tag" x="20" y="26" text-anchor="start">A screen returns a navigation transition</text>
  <rect class="d-panel" x="20" y="56" width="195" height="66" rx="8" />
  <text class="d-title" x="117.5" y="81" text-anchor="middle">Gesture</text>
  <text class="d-sub" x="117.5" y="101" text-anchor="middle">Top screen receives input</text>
  <path class="d-flow" d="M215 89 L258 89" marker-end="url(#software-ui-2)" />
  <rect class="d-panel d-focus" x="260" y="56" width="195" height="66" rx="8" />
  <text class="d-title" x="357.5" y="81" text-anchor="middle">handle</text>
  <text class="d-sub" x="357.5" y="101" text-anchor="middle">Returns Transition</text>
  <path class="d-flow" d="M455 89 L502 89" marker-end="url(#software-ui-2)" />
  <rect class="d-panel" x="505" y="56" width="195" height="66" rx="8" />
  <text class="d-title" x="602.5" y="81" text-anchor="middle">Apply transition</text>
  <text class="d-sub" x="602.5" y="101" text-anchor="middle">Update the screen stack</text>
  <text class="d-title" x="20" y="174" text-anchor="start">Stack example</text>
  <rect class="d-panel" x="20" y="195" width="210" height="50" rx="8" />
  <text class="d-title" x="125" y="220" text-anchor="middle">Up ahead · top</text>
  <rect class="d-panel" x="20" y="253" width="210" height="50" rx="8" />
  <text class="d-title" x="125" y="278" text-anchor="middle">Map</text>
  <rect class="d-panel" x="20" y="311" width="210" height="50" rx="8" />
  <text class="d-title" x="125" y="336" text-anchor="middle">Home · root</text>
  <text class="d-label" x="280" y="210" text-anchor="start">Push / Pop</text>
  <text class="d-sub" x="455" y="210" text-anchor="start">Add / remove a top screen</text>
  <text class="d-label" x="280" y="242" text-anchor="start">Replace</text>
  <text class="d-sub" x="455" y="242" text-anchor="start">Change the top screen</text>
  <text class="d-label" x="280" y="274" text-anchor="start">Root(screen)</text>
  <text class="d-sub" x="455" y="274" text-anchor="start">Keep Home, then add screen</text>
  <text class="d-label" x="280" y="306" text-anchor="start">Home</text>
  <text class="d-sub" x="455" y="306" text-anchor="start">Keep only Home</text>
  <text class="d-label" x="280" y="338" text-anchor="start">None</text>
  <text class="d-sub" x="455" y="338" text-anchor="start">Keep the current stack</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>A screen returns a transition, and <code>screen::apply</code> runs it. Cards land on
the stack without it.</figcaption>
</figure>

| Transition | Stack operation |
| --- | --- |
| `None` | Keep the stack. |
| `Push(screen)` | Add a top screen. |
| `Pop` | Remove the top screen, except Home. |
| `Replace(screen)` | Replace the top screen. |
| `Root(screen)` | Keep Home and add one screen. |
| `Home` | Remove all screens above Home. |

Any stack change cancels an incomplete hold, so a hold cannot finish on a screen that did not start
it. Each screen owns its own Back policy: Back can leave an editor, cancel work, move to a sibling
view, or pop.

## Input

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 344" role="img" aria-label="Four buttons on the left — Up and Down on one flank, Select and Back on the other — feed a shared Gestures recognizer in the middle, which also takes a millisecond clock. It emits gestures and chords on the right: Step of n, Press, Hold, Back, and BackHold.">
  <defs>
    <marker id="aU3" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Four buttons → one recognizer → gestures and chords</text>

  <!-- left flank: Up / Down (step controls, auto-repeat) -->
  <text class="d-sub" x="86" y="52" text-anchor="middle" style="font-size:12px">left flank</text>
  <rect x="46" y="60" width="80" height="30" rx="7" class="d-panel-2" />
  <text class="d-label" x="86" y="80" text-anchor="middle">▲ Up</text>
  <rect x="46" y="96" width="80" height="30" rx="7" class="d-panel-2" />
  <text class="d-label" x="86" y="116" text-anchor="middle">▼ Down</text>
  <text class="d-sub" x="86" y="140" text-anchor="middle" style="font-size:12px">steps · auto-repeat</text>

  <!-- right flank: Select / Back (timed edges) -->
  <text class="d-sub" x="86" y="168" text-anchor="middle" style="font-size:12px">right flank</text>
  <rect x="46" y="176" width="80" height="30" rx="7" class="d-panel-2" />
  <text class="d-label" x="86" y="196" text-anchor="middle">Select</text>
  <rect x="46" y="212" width="80" height="30" rx="7" class="d-panel-2" />
  <text class="d-label" x="86" y="232" text-anchor="middle">Back</text>

  <!-- recognizer -->
  <line class="d-flow" x1="132" y1="96" x2="246" y2="120" marker-end="url(#aU3)" />
  <line class="d-flow" x1="132" y1="206" x2="246" y2="150" marker-end="url(#aU3)" />
  <rect class="d-hot" x="250" y="96" width="170" height="74" rx="12" style="fill:#f8efe4" />
  <text class="d-title" x="335" y="126" text-anchor="middle" style="fill:#a9501c">Gestures</text>
  <text class="d-sub" x="335" y="144" text-anchor="middle">raw events + ms clock</text>
  <text class="d-sub" x="335" y="158" text-anchor="middle">shared: sim = MCU</text>

  <!-- gestures out -->
  <line class="d-flow" x1="420" y1="133" x2="486" y2="133" marker-end="url(#aU3)" />
  <g font-family="var(--mono)">
    <rect x="496" y="50"  width="180" height="26" rx="6" class="d-panel-2" /><text class="d-sub" x="506" y="67">Step(n) — up / down</text>
    <rect x="496" y="82"  width="180" height="26" rx="6" class="d-panel-2" /><text class="d-sub" x="506" y="99">Press — short select</text>
    <rect x="496" y="114" width="180" height="26" rx="6" class="d-panel-2" /><text class="d-sub" x="506" y="131">Hold — long select</text>
    <rect x="496" y="146" width="180" height="26" rx="6" class="d-panel-2" /><text class="d-sub" x="506" y="163">Back — short back</text>
    <rect x="496" y="178" width="180" height="26" rx="6" class="d-panel-2" /><text class="d-sub" x="506" y="195">BackHold — long back</text>
  </g>
<path d="M20 259 L700 259" fill="none" stroke="#9aa884" stroke-width="1.3" /><text class="d-title" x="20" y="286" text-anchor="start">Two-button chords · within 100 ms</text><text class="d-sub" x="20" y="312" text-anchor="start">Up + Select: quick drawer</text><text class="d-sub" x="355" y="312" text-anchor="start">Down + Back: contextual drawer</text></svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The recognizer combines button state and elapsed time. A chord consumes both presses and does not also emit their individual gestures.</figcaption>
</figure>

| Gesture | Source |
| --- | --- |
| `Step(n)` | Up or Down step |
| `Press` | Select release within 200 ms |
| `Hold` | Select held for 500 ms |
| `Back` | Back release within 200 ms |
| `BackHold` | Back held for 500 ms |

A release between the tap window and the hold threshold emits nothing, which is how a rider cancels
a hold. A hold fires at the threshold, not on release, so the rider feels the moment it commits.

`BackHold` is the global escape. The application answers it above the screen stack, so it never
reaches a screen: it closes any drawer and opens the main menu from anywhere, and returns to a menu
already on the stack instead of adding a second one. Three states refuse it, because the rider must
finish them first: a blocking card, the recovered-ride card, and a confirmed shutdown.

### Chords

Two buttons pressed within 100 ms of each other are one chord, not two gestures. The recognizer
reports the chord above the screen stack and emits nothing for the two buttons, and the chord stays
latched until both are up.

| Chord | Meaning |
| --- | --- |
| Up + Select, released before 500 ms | Open or close the universal quick drawer |
| Up + Select, held for 500 ms | Open Ride Assistant |
| Down + Back | Open or close the current screen's contextual drawer |
| Up + Down | Reserved |
| Select + Back | Reserved |

A reserved chord is recognized and does nothing, so a squeeze never becomes two unrelated actions.
Because a chord can start with a direction button, the first step of Up or Down waits for the chord
window; a release inside the window steps at once, so a tap does not feel slower.

## Drawers

A drawer is a sheet drawn over the current screen. There are two, and only one can be open: the
second chord replaces the sheet instead of stacking another.

The **universal quick drawer** comes down from the top with the device-wide controls: brightness,
the Bluetooth radio, the central settings, and power. A platform with no controllable backlight
does not show brightness.

The **contextual drawer** comes up from the bottom with the current screen's secondary actions. A
screen does not build a drawer: it declares a static table of rows, and one generic drawer supplies
the cursor, the transitions, and the drawing. A screen with no table gets no sheet.

The four riding views offer the same four actions in the same order: Up ahead, Detour, POIs, and
Routes. A row that cannot act now is drawn recessed and does nothing. A row that acts replaces the
sheet with its screen, so one Back returns to the riding view the rider squeezed from.

A row can hold a **value** instead of a screen, and slides the sheet to a small editor. The bike
type is such a row, and its choices are the routing profile names of the loaded map, so a map built
with a custom profile offers that profile without a firmware change. A row can also be a **switch**
that flips in place, so a rider can change a group of preferences with the sheet open.

The drawer is the only home for a setting that belongs to one screen, and a build check fails if a
drawer and the settings tree write the same stored setting.

The screen under a drawer is **frozen**: the drawer states its own facts for repaint, so a moving
map under a sheet causes no work. Whether the screen below is **dimmed** is a property of that
screen. A map is not dimmed, because drawing it again is a whole map render and the map reads well
under the sheet. Menus are dimmed, because drawing them again is nearly free and the recess helps
the sheet read as being in front.

Nothing lands on top of a drawer. A card that arrives while a sheet is open takes the sheet with
it, so dismissing the card returns the rider to the screen they were on.

### What the map drawer controls

Map display holds switches for the clock, the scale bar, and the contour layer, and a Map icons
menu with switches for peaks, landmarks, and the service categories. Icons stay upright at their
source coordinates as the map rotates, and a group appears only at scales where its icons help
instead of crowd. Placement takes the nearest unobstructed icon from each enabled group in turn, so
one dense category cannot take the screen from the others.

Settlement names are always drawn; there is no switch. Each class of place shows its names inside
one band of scales: a name appears when the place roughly fits the screen, and goes when the place
is one dot among many, or when the rider is inside it and the name would only cover the roads.
Names keep clear of each other and of the chrome the frame actually draws.

### Detour

Detour leaves the route and comes back to it. It needs a routing graph in the map and a matched
position on the route.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 262" role="img" aria-label="The detour flow in three panels. Panel one, the chooser: the magenta route with an orange inner stroke marking the skipped stretch ahead of the rider and a ring at the candidate rejoin point; Up and Down move the rejoin in 100-metre steps. Panel two, the preview: the planned detour drawn in blue around the skipped stretch, with a panel showing two signed cost figures, here plus 434 metres of distance and 47 metres less climbing. Panel three, the commit: the spliced line — ridden part, detour, and the original route from the rejoin — is written as an ordinary route file and adopted; guidance continues.">
  <defs>
    <marker id="aDT" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Choose a rejoin · preview the cost · commit the splice</text>

  <!-- panel 1: chooser -->
  <text class="d-sub" x="30" y="52" style="font-size:12px;fill:#4d5b3c">① chooser — Up/Down move the rejoin</text>
  <rect x="30" y="60" width="200" height="150" rx="9" style="fill:#ffffff;stroke:#aaaa55;stroke-width:1.4" />
  <path d="M55 195 C 90 160, 100 120, 130 100 C 160 80, 185 80, 210 72" fill="none" stroke="#ff00ff" stroke-width="5" />
  <path d="M95 152 C 110 130, 118 112, 130 100 C 143 91, 152 87, 162 84" fill="none" stroke="#ff5500" stroke-width="2.4" />
  <path d="M62 188 l8 12 l-8 -4 l-8 4 z" fill="#ff0000" />
  <circle cx="162" cy="84" r="7" fill="none" stroke="#000" stroke-width="2" />
  <text class="d-sub" x="120" y="200" style="font-size:12px">skipped stretch</text>
  <text class="d-sub" x="162" y="112" text-anchor="middle" style="font-size:12px">rejoin</text>
  <text class="d-sub" x="30" y="228" style="font-size:12px">±100 m steps · 600 m minimum</text>

  <!-- panel 2: preview -->
  <line class="d-flow" x1="238" y1="135" x2="258" y2="135" marker-end="url(#aDT)" />
  <text class="d-sub" x="266" y="52" style="font-size:12px;fill:#4d5b3c">② preview — path and cost</text>
  <rect x="266" y="60" width="200" height="150" rx="9" style="fill:#ffffff;stroke:#aaaa55;stroke-width:1.4" />
  <path d="M291 195 C 326 160, 336 120, 366 100 C 396 80, 421 80, 446 72" fill="none" stroke="#ff00ff" stroke-width="5" />
  <path d="M331 152 C 346 130, 354 112, 366 100 C 379 91, 388 87, 398 84" fill="none" stroke="#ff5500" stroke-width="2.4" />
  <path d="M331 152 C 370 160, 410 130, 398 84" fill="none" stroke="#0000aa" stroke-width="3" />
  <rect x="280" y="178" width="150" height="20" rx="6" style="fill:#f3f0df;stroke:#3d3427;stroke-width:1" />
  <text x="291" y="192" style="font-family:var(--mono);font-size:12px;fill:#3d3427">+434 m</text>
  <path d="M372 194 l6 -9 l6 9 z" fill="#3d3427" />
  <text x="422" y="192" text-anchor="end" style="font-family:var(--mono);font-size:12px;fill:#3d3427">-47 m</text>
  <text class="d-sub" x="266" y="228" style="font-size:12px">what it costs: distance · climbing</text>

  <!-- panel 3: commit -->
  <line class="d-flow" x1="474" y1="135" x2="494" y2="135" marker-end="url(#aDT)" />
  <text class="d-sub" x="502" y="52" style="font-size:12px;fill:#4d5b3c">③ commit — use the detour</text>
  <rect x="502" y="60" width="200" height="150" rx="9" style="fill:#ffffff;stroke:#aaaa55;stroke-width:1.4" />
  <path d="M527 195 C 550 172, 558 152, 564 138 C 592 162, 630 132, 622 96 C 642 82, 662 76, 680 72" fill="none" stroke="#ff00ff" stroke-width="5" />
  <text class="d-sub" x="502" y="228" style="font-size:12px">an ordinary route file</text>
  <text class="d-sub" x="30" y="252" style="font-size:12px;fill:#4d5b3c">back cancels at any step · a failed plan suggests the one useful remedy: try a farther rejoin</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The rider selects a rejoin point, reviews the result, and commits the new route.</figcaption>
</figure>

Up and Down move the rejoin point along the route ahead. The plan shows the difference in distance
and climb against the stretch it replaces, and the rider commits it or leaves it. A commit keeps
the completed geometry, splices in the detour, and continues from the rejoin point.

## Hold to confirm

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 278" role="img" aria-label="Top: a timeline showing Select pressed down. A release within the 200ms tap window yields a Press; a release after the window but before the 500ms hold threshold is a cancelled long-press and yields nothing; holding past 500ms yields a Hold the instant it crosses. Bottom: a Discard row filling left to right with a warning bar at 0 percent, 60 percent holding, and 100 percent commit."><text class="d-tag" x="20" y="24">Hold to confirm — the guarded-action pattern</text><g transform="translate(0 28)">

  <!-- timeline -->
  <line x1="40" y1="70" x2="540" y2="70" stroke="#9aa884" stroke-width="1.5" />
  <circle cx="40" cy="70" r="5" class="d-forest" /><text class="d-sub" x="40" y="58" text-anchor="middle" style="font-size:12px">down</text>
  <!-- thresholds: the tap window and the hold threshold -->
  <line x1="168" y1="56" x2="168" y2="84" stroke="#9aa884" stroke-width="1.4" stroke-dasharray="3 3" />
  <text class="d-sub" x="168" y="98" text-anchor="middle" style="font-size:12px">tap · 200 ms</text>
  <line x1="360" y1="56" x2="360" y2="84" stroke="#c0492e" stroke-width="1.6" stroke-dasharray="3 3" />
  <text class="d-sub" x="360" y="98" text-anchor="middle" style="fill:#c0492e;font-size:12px">hold · 500 ms</text>
  <!-- press branch (release within the tap window) -->
  <circle cx="108" cy="70" r="5" class="d-amber" />
  <text class="d-sub" x="108" y="58" text-anchor="middle" style="font-size:12px">release</text>
  <text class="d-label" x="108" y="36" text-anchor="middle" style="font-size:12px">→ Press</text>
  <!-- cancelled branch (release between the two thresholds) -->
  <circle cx="264" cy="70" r="5" class="d-muted" />
  <text class="d-sub" x="264" y="58" text-anchor="middle" style="font-size:12px">release</text>
  <text class="d-sub" x="264" y="36" text-anchor="middle" style="font-size:12px">→ nothing</text>
  <!-- hold branch -->
  <circle cx="430" cy="70" r="5" class="d-hot-fill" />
  <text class="d-label" x="470" y="62" style="font-size:12px;fill:#a9501c">→ Hold fires</text>
  <text class="d-sub" x="470" y="78" style="font-size:12px">(commits, still held)</text>

  <!-- confirm fill states -->
  <text class="d-tag" x="20" y="138">the selected row, as you hold</text>
  <g>
    <!-- 0% -->
    <rect x="40" y="152" width="200" height="34" rx="6" class="d-muted" />
    <text class="d-label" x="56" y="174">Discard</text>
    <text class="d-sub" x="140" y="202" text-anchor="middle" style="font-size:12px">0% — idle</text>
    <!-- 60% -->
    <rect x="260" y="152" width="200" height="34" rx="6" class="d-muted" />
    <rect x="260" y="152" width="120" height="34" rx="6" style="fill:#c0492e" />
    <text class="d-label" x="276" y="174" style="fill:#fff">Discard</text>
    <text class="d-sub" x="360" y="202" text-anchor="middle" style="font-size:12px">holding — release = cancel</text>
    <!-- 100% -->
    <rect x="480" y="152" width="200" height="34" rx="6" style="fill:#c0492e" />
    <text class="d-label" x="496" y="174" style="fill:#fff">Discard ✓</text>
    <text class="d-sub" x="580" y="202" text-anchor="middle" style="font-size:12px">100% — committed</text>
  </g>
</g></svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>A guarded action runs only when the hold reaches its threshold. The live fill shows hold progress.</figcaption>
</figure>

A destructive action requires a hold, and the row draws its progress. The action runs only when the
hold reaches its threshold.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 278" role="img" aria-label="The guarded hold-to-delete. On the left, the Fields grid with a delete band that fills with a progress bar as you hold. On the right, its two states stacked: normal — hold to delete, and absent — an in-use route or recording ride simply shows no delete row.">
  <text class="d-tag" x="20" y="24">One guarded hold — the Fields footer, the Route overview + Ride detail rows</text>

  <!-- the one remaining footer screen: the Fields grid -->
  <rect class="d-panel" x="24" y="42" width="228" height="188" rx="11" />
  <rect x="32" y="50" width="212" height="18" rx="3" style="fill:#aa5500" /><text class="d-sub" x="42" y="63" style="fill:#fff;font-size:12px">FIELDS</text>
  <rect x="32" y="74" width="102" height="44" rx="4" class="d-amber" />
  <text class="d-sub" x="40" y="86" style="fill:#000;font-size:12px">SPEED</text><text class="d-sub" x="40" y="102" style="fill:#000;font-size:12px">24.3</text>
  <rect x="142" y="74" width="102" height="44" rx="4" class="d-muted" />
  <text class="d-sub" x="150" y="86" style="font-size:12px">AVG KPH</text><text class="d-sub" x="150" y="102" style="font-size:12px">17.0</text>
  <rect x="32" y="126" width="102" height="44" rx="4" class="d-muted" />
  <text class="d-sub" x="40" y="138" style="font-size:12px">KM DONE</text><text class="d-sub" x="40" y="154" style="font-size:12px">42.5</text>
  <rect x="142" y="126" width="102" height="44" rx="4" class="d-muted" />
  <text class="d-sub" x="150" y="138" style="font-size:12px">CLIMBED</text><text class="d-sub" x="150" y="154" style="font-size:12px">▲810</text>
  <!-- footer band -->
  <line x1="36" y1="182" x2="240" y2="182" stroke="#aaaa55" stroke-width="1" />
  <rect x="36" y="190" width="120" height="30" rx="6" style="fill:#c0492e" />
  <rect x="156" y="190" width="84" height="30" rx="6" class="d-muted" />
  <text class="d-sub" x="58" y="209" text-anchor="start" style="fill:#fff;font-size:12px">hold to delete</text>
  <text class="d-sub" x="138" y="257" text-anchor="middle" style="font-size:12px">bar fills on the live hold</text>

  <!-- the guarded hold's two states -->
  <text class="d-tag" x="292" y="60">the guarded hold, two states</text>
  <rect x="292" y="72" width="404" height="30" rx="6" class="d-muted" />
  <text class="d-sub" x="308" y="91" style="font-size:12px">hold to delete</text>
  <text class="d-sub" x="470" y="91" style="font-size:12px;fill:#4d5b3c">— normal · a completed hold deletes</text>

  <rect x="292" y="110" width="404" height="30" rx="6" fill="none" stroke="#c9c7b8" stroke-width="1" stroke-dasharray="4 4" />
  <text class="d-sub" x="470" y="129" style="font-size:12px;fill:#4d5b3c">— hidden · item is active / recording</text>

</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Delete actions use the same guarded-row contract. A hold on another row has no effect.</figcaption>
</figure>

Delete lives on one row. A hold anywhere else deletes nothing.

## Ride Assistant

Holding **Up + Select** opens Ride Assistant, which answers four questions from installed offline
data: find a place, what is next on the route, nearby landmarks, and easier routes. There is no
network in any of them.

### Find a place

Find combines places near the rider with places along the next part of the route, and alternates
the two sources so that neither crowds out the other. It plans a real route to each candidate and
shows the measured cost. The drawer sets how many results to calculate, and four is the default.

Places known to be closed now are hidden, which the rider can turn off for every category at once.
A place with unknown hours stays in the list and is never labelled open. Opening hours are
information: they never block a preview, and the device reports the status now, not a guess at the
status on arrival.

With an accepted route, a choice is a **visit**: the route to the place, the return, and the rest
of the original journey. The review compares the whole visit, return included, with the remaining
route, so the added cost is the true cost of stopping. Without a route, the same choice is a direct
destination. Accepting starts recording if no ride is open; browsing changes nothing.

A place is routable only where the map gives it a mapped approach. A straight-line distance never
promises a rideable connection.

### What is next

The overview freezes one window of the route, 5 km or 10 km, and shows what is in it: the ascent
and descent, the next climb, the next waypoint, and the next water and shop. Explore ahead opens
the full timeline for that window, filtered from the drawer. The window is frozen so the figures do
not move while the rider reads them; a hold refreshes it.

### Nearby landmarks

Landmarks answers what a place is. It keeps a page of the nearest sites, with the map and the card
stable while the rider moves the selection. Select opens short text pages and, where the map has
one, a photo. **Down + Back** opens the sources, because credits must travel with the content. A
closed site stays readable and can still be visited: a rider can look at a castle from outside it.

### Easier routes

Easier routes compares the rest of the journey against three goals: less climbing, a smoother
surface, and a shorter distance. Each goal is one bounded search under the rider's own bike
profile, not a claim that the router found the best route in the world. A choice appears only when
it improves its goal and the cost it adds stays within a bound.

The camera stays fixed while the rider compares the current route with the proposal, and Select
opens a current-and-new table. **Use this route** accepts it through the normal acceptance, and
recording continues. A shorter route does not claim a shorter time: the device has no arrival model
to support that.

## POI browser

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 250" role="img" aria-label="The POIs browser flow. The compass Menu's POIs station opens the category screen: a six-row list of Water, Campsite, Lodging, Resupply, Pharmacy and Bike shop, each with a small icon. Pressing a category opens the list screen: a page of that category's nearby POIs sorted by distance, each row a name, a bearing arrow and a distance. Back climbs one step; selecting a POI opens its detail view.">
  <defs>
    <marker id="aU9" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Menu → categories → pages of nearby places</text>

  <!-- Menu station -->
  <rect class="d-panel-2" x="24" y="88" width="120" height="44" rx="10" />
  <text class="d-label" x="84" y="108" text-anchor="middle">Menu</text>
  <text class="d-sub" x="84" y="123" text-anchor="middle" style="font-size:12px">POIs station</text>

  <!-- category screen -->
  <line class="d-flow" x1="146" y1="110" x2="196" y2="110" marker-end="url(#aU9)" />
  <text class="d-sub" x="171" y="102" text-anchor="middle" style="font-size:12px">press</text>
  <rect class="d-panel" x="202" y="44" width="188" height="168" rx="11" />
  <rect x="210" y="52" width="172" height="20" rx="4" style="fill:#aa5500" /><text class="d-sub" x="220" y="66" style="fill:#fff;font-size:12px">POIS</text>
  <g font-family="var(--mono)">
    <rect x="210" y="78" width="172" height="20" rx="4" class="d-amber" />
    <circle cx="222" cy="88" r="4" fill="#000" /><text class="d-sub" x="234" y="92" style="fill:#000;font-size:12px">Water</text>
    <circle cx="222" cy="110" r="4" fill="#24331c" /><text class="d-sub" x="234" y="114" style="font-size:12px">Campsite</text>
    <circle cx="222" cy="130" r="4" fill="#24331c" /><text class="d-sub" x="234" y="134" style="font-size:12px">Lodging</text>
    <circle cx="222" cy="150" r="4" fill="#24331c" /><text class="d-sub" x="234" y="154" style="font-size:12px">Resupply</text>
    <circle cx="222" cy="170" r="4" fill="#24331c" /><text class="d-sub" x="234" y="174" style="font-size:12px">Pharmacy</text>
    <circle cx="222" cy="190" r="4" fill="#24331c" /><text class="d-sub" x="234" y="194" style="font-size:12px">Bike shop</text>
  </g>

  <!-- list screen -->
  <line class="d-flow" x1="392" y1="110" x2="442" y2="110" marker-end="url(#aU9)" />
  <text class="d-sub" x="417" y="102" text-anchor="middle" style="font-size:12px">press</text>
  <rect class="d-panel" x="448" y="44" width="248" height="168" rx="11" />
  <rect x="456" y="52" width="232" height="20" rx="4" style="fill:#aa5500" /><text class="d-sub" x="466" y="66" style="fill:#fff;font-size:12px">Water · 1/8</text>
  <g font-family="var(--mono)">
    <!-- selected row -->
    <rect x="456" y="78" width="232" height="24" rx="4" class="d-amber" />
    <text class="d-sub" x="466" y="94" style="fill:#000;font-size:12px">Stadtbrunnen</text>
    <path d="M636 84 l6 -6 l6 6 l-4 0 l0 8 l-4 0 l0 -8 z" fill="#000" />
    <text class="d-sub" x="656" y="94" style="fill:#000;font-size:12px">210m</text>
    <!-- more rows -->
    <text class="d-sub" x="466" y="118" style="font-size:12px">Spring</text>
    <path d="M638 110 l8 4 l-8 4 l3 -4 z" fill="#24331c" /><text class="d-sub" x="656" y="118" style="font-size:12px">410m</text>
    <text class="d-sub" x="466" y="140" style="font-size:12px">Brunnen Nord</text>
    <path d="M642 132 l0 8 l-3 -3 M642 140 l3 -3" fill="none" stroke="#24331c" stroke-width="1.4" /><text class="d-sub" x="656" y="140" style="font-size:12px">820m</text>
    <text class="d-sub" x="466" y="162" style="font-size:12px">Drinking water</text>
    <text class="d-sub" x="656" y="162" style="font-size:12px">1km</text>
    <text class="d-sub" x="466" y="188" style="font-size:12px;fill:#a9501c">name · arrow · distance</text>
  </g>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The POI browser has seven categories. Each page holds at most eight nearby places.</figcaption>
</figure>

The POI menu lists the service categories, and a category returns pages of nearby places.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 210" role="img" aria-label="One POI list row, dissected. The row holds a name on the left, a bearing arrow, and a right-aligned distance. Below, the arrow's heading reference has two sources: while moving, the GPS course; while stationary, the electronic compass heading from the ICM-20948; when neither is known, the arrow is hidden rather than pointing wrong.">
  <text class="d-tag" x="20" y="24">The row, and where the arrow's "up" comes from</text>

  <!-- the row -->
  <rect class="d-panel" x="24" y="42" width="672" height="40" rx="8" />
  <text class="d-sub" x="44" y="66" font-family="var(--mono)" style="font-size:12px">Stadtbrunnen</text>
  <text class="d-sub" x="240" y="60" style="font-size:12px;fill:#a9501c">name (or subtype label if unnamed)</text>
  <!-- arrow -->
  <path d="M556 54 l8 -8 l8 8 l-5 0 l0 10 l-6 0 l0 -10 z" fill="#cf6a2a" />
  <text class="d-sub" x="540" y="78" style="font-size:12px;fill:#a9501c">bearing</text>
  <!-- distance -->
  <text class="d-sub" x="672" y="66" text-anchor="end" font-family="var(--mono)" style="font-size:12px">210m</text>
  <text class="d-sub" x="672" y="84" text-anchor="end" style="font-size:12px">distance</text>

  <!-- heading sources -->
  <text class="d-tag" x="20" y="118">the arrow's heading reference</text>
  <rect class="d-panel-2" x="24" y="128" width="216" height="66" rx="10" />
  <text class="d-label" x="40" y="150" style="font-size:12px">moving</text>
  <text class="d-sub" x="40" y="170" style="font-size:12px">GPS course over ground</text>
  <text class="d-sub" x="40" y="185" style="font-size:12px">— the direction you're going</text>

  <rect class="d-panel-2" x="252" y="128" width="216" height="66" rx="10" />
  <text class="d-label" x="268" y="150" style="font-size:12px">stationary</text>
  <text class="d-sub" x="268" y="170" style="font-size:12px">ICM-20948 compass</text>
  <text class="d-sub" x="268" y="185" style="font-size:12px">— which way you're facing</text>

  <rect class="d-hot" x="480" y="128" width="216" height="66" rx="10" style="fill:#f8efe4" />
  <text class="d-label" x="496" y="150" style="fill:#a9501c;font-size:12px">neither known</text>
  <text class="d-sub" x="496" y="170" style="font-size:12px">arrow hidden</text>
  <text class="d-sub" x="496" y="185" style="font-size:12px">— don't point wrong</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The list keeps a fixed snapshot. Only the bearing arrow uses live position data.</figcaption>
</figure>

The page and its order are fixed while the list is open, so a list does not reorder under a rider's
thumb as the position updates. Only the bearing arrow follows the live fix.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 250" role="img" aria-label="The POI detail view. On the left, the screen: the POI name with its category icon at the top, a muted subtype subtitle beneath it, then a promoted distance row with the 8-way bearing arrow, a Today heading with an opening-hours range below, a green OPEN pill, and a full-width amber Preview route bar at the bottom. On the right, the three heading states for the hours block: Today with time ranges when open some hours today, Closed today when the schedule has no interval for this weekday, and Hours not listed when the POI has no schedule at all. Below, the open-now pill is derived from the live local clock.">
  <defs>
    <marker id="aPD" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">The detail screen, and where "open now" comes from</text>

  <!-- the screen mock -->
  <rect class="d-panel" x="24" y="40" width="232" height="196" rx="10" />
  <rect x="42" y="54" width="196" height="18" rx="3" style="fill:#aa5500" /><text class="d-sub" x="52" y="67" style="fill:#fff;font-size:12px">POI</text>
  <!-- name row: category icon + name -->
  <circle cx="49" cy="93" r="6" fill="#3d3427" /><path d="M43 90 l6 -8 l6 8 z" fill="#3d3427" />
  <text class="d-sub" x="64" y="98" font-family="var(--mono)" style="font-size:12px">Stadtbaeckerei</text>
  <text class="d-sub" x="42" y="114" style="font-size:12px">Bakery</text>
  <!-- promoted distance + 8-way arrow row -->
  <path d="M52 136 l7 -7 l7 7 l-4 0 l0 9 l-6 0 l0 -9 z" fill="#cf6a2a" />
  <text class="d-sub" x="74" y="144" font-family="var(--mono)" style="font-size:12px">850m</text>
  <!-- hours -->
  <text class="d-sub" x="42" y="164" style="font-size:12px">Today</text>
  <text class="d-sub" x="42" y="182" font-family="var(--mono)" style="font-size:12px">08:00-18:00</text>
  <!-- badge pill -->
  <rect x="42" y="192" width="52" height="15" rx="3" style="fill:#3c6b39" />
  <text class="d-sub" x="68" y="203" text-anchor="middle" style="fill:#fff;font-size:12px">OPEN</text>
  <!-- footer action bar -->
  <rect x="38" y="214" width="204" height="16" rx="5" style="fill:#e3a52b" />
  <text class="d-sub" x="140" y="225" text-anchor="middle" style="fill:#3d3427;font-size:12px">&#9654; Preview route</text>

  <!-- the three heading states -->
  <text class="d-tag" x="292" y="60">the hours heading — three states</text>
  <rect class="d-panel-2" x="292" y="72" width="404" height="24" rx="6" />
  <text class="d-sub" x="304" y="88" font-family="var(--mono)" style="font-size:12px">Today</text>
  <text class="d-sub" x="380" y="88" style="font-size:12px">— open some hours today; ranges stacked below</text>
  <rect class="d-panel-2" x="292" y="100" width="404" height="24" rx="6" />
  <text class="d-sub" x="304" y="116" font-family="var(--mono)" style="font-size:12px">Closed today</text>
  <text class="d-sub" x="420" y="116" style="font-size:12px">— has hours, but none this weekday</text>
  <rect class="d-panel-2" x="292" y="128" width="404" height="24" rx="6" />
  <text class="d-sub" x="304" y="144" font-family="var(--mono)" style="font-size:12px">Hours not listed</text>
  <text class="d-sub" x="440" y="144" style="font-size:12px">— HoursRef was 0xFFFF, no badge</text>

  <!-- open-now derivation -->
  <text class="d-tag" x="292" y="178">the OPEN / CLOSED badge</text>
  <rect class="d-hot" x="292" y="188" width="404" height="48" rx="10" style="fill:#f8efe4" />
  <text class="d-sub" x="308" y="208" style="font-size:12px">local clock → weekday + minute-of-day</text>
  <line class="d-flow" x1="540" y1="204" x2="580" y2="204" marker-end="url(#aPD)" />
  <text class="d-sub" x="590" y="208" style="font-size:12px;fill:#a9501c">is_open?</text>
  <text class="d-sub" x="308" y="226" style="font-size:12px">read live every frame — the one part that isn't frozen</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The detail screen reads opening hours once. The bearing and distance remain live.</figcaption>
</figure>

The detail screen reads the schedule once and calculates today's state from the local clock. A
definite open or closed answer needs a trusted clock and a known local offset. Anything else is
Unknown.

## Settings

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 232" role="img" aria-label="Settings screens have two focus levels. In row focus, up and down move the amber row cursor, press flips a toggle or opens a value row's stepper, and back climbs one screen. Pressing a value row enters field focus, where up and down change the live field's value shown in an up-down arrow box, press advances to the next field, and back — or pressing past the last field — steps back out to row focus.">
  <defs>
    <marker id="aU8" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Two levels of focus — rows, then fields</text>

  <!-- Row focus -->
  <rect class="d-panel" x="40" y="46" width="262" height="160" rx="12" />
  <text class="d-label" x="60" y="70">Row focus</text>
  <rect x="58" y="80" width="226" height="24" rx="5" class="d-amber" />
  <text class="d-sub" x="68" y="96" style="fill:#000;font-size:12px">amber bar = the cursor</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="60" y="130" style="font-size:12px">up / down — move the cursor</text>
    <text class="d-sub" x="60" y="152" style="font-size:12px">press &nbsp;— toggle / open a value</text>
    <text class="d-sub" x="60" y="174" style="font-size:12px">back &nbsp;— climb one screen up</text>
  </g>

  <!-- transitions -->
  <line class="d-flow" x1="304" y1="104" x2="416" y2="104" marker-end="url(#aU8)" />
  <text class="d-sub" x="360" y="96" text-anchor="middle" style="font-size:12px">press a value row</text>
  <line class="d-flow" x1="416" y1="150" x2="304" y2="150" marker-end="url(#aU8)" />
  <text class="d-sub" x="360" y="166" text-anchor="middle" style="font-size:12px">back / last field</text>

  <!-- Field focus -->
  <rect class="d-panel-2" x="418" y="46" width="262" height="160" rx="12" />
  <text class="d-label" x="438" y="70">Field focus</text>
  <path d="M452 80 l7 -9 l7 9 z" fill="#ffaa00" />
  <rect x="445" y="84" width="42" height="22" rx="4" class="d-muted" style="stroke:#ffaa00;stroke-width:1.5" />
  <text class="d-sub" x="466" y="99" text-anchor="middle" style="font-size:12px">2025</text>
  <path d="M452 110 l7 9 l7 -9 z" fill="#ffaa00" />
  <text class="d-sub" x="500" y="99" style="font-size:12px">box = the live field</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="438" y="130" style="font-size:12px">up / down — change the value</text>
    <text class="d-sub" x="438" y="152" style="font-size:12px">press &nbsp;— step to the next field</text>
    <text class="d-sub" x="438" y="174" style="font-size:12px">back &nbsp;— step out of the field</text>
  </g>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>A press moves focus between the row and its value. Up and Down change the focused value.</figcaption>
</figure>

Settings have two focus levels: the cursor selects a row, and a press moves focus into the value. A
changed value is written when the rider leaves the Settings subtree, not once per step.

Settings do not live on the card, so they survive a card change. The UI languages are English,
German, French, and Spanish, and the build generates the translation table from four catalogs and
fails on a missing or extra key.

## Route cleanup

The device deletes nothing by itself. When a route upload fails because storage is full, it offers
cleanup: choose an age in weeks and hold to confirm. Cleanup keeps the active route, routes newer
than that age, and routes whose date is unknown, because an unknown date is not evidence that a
route is old. See [ride reconciliation](../companion-link/#reconciliation).

## Repaint policy

The application renders on demand. A static screen with no input, no new data, and no timer does
not render at all.

### The render key

Each screen declares which facts its drawing reads. Each frame reads those facts before and after
its work, and a change requests a repaint. The rule is per screen: a new heart-rate reading
repaints the grid that shows it and not the map beside it. Some changes cannot move a key, such as
a selection a screen keeps to itself, and they request their repaint directly. Drawing too often is
safe; drawing too rarely is a defect.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 392" role="img" aria-label="Three views show a clean map, a transparent overlay with a hold-progress window, and the combined panel image. The stored base map is unchanged; presenting its clean window removes the overlay.">
<defs><marker id="r49arrow" viewBox="0 0 10 10" refX="10" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker></defs>
<text class="d-tag" x="20" y="26" text-anchor="start">Transient feedback · composite a window over the clean frame</text>
<text class="d-title" x="20" y="61" text-anchor="start">Clean base frame</text>
<text class="d-title" x="272" y="61" text-anchor="start">Transient hold window</text>
<text class="d-title" x="524" y="61" text-anchor="start">Presented frame</text>
<rect x="20" y="83" width="175" height="215" fill="#ece8cf" stroke="#3c6b39" stroke-width="1.2" rx="8"/>
<path d="M25 203 C85 133 120 263 190 168" fill="none" stroke="#33575b" stroke-width="5"/>
<path d="M45 93 L82 148 L130 193 L165 287" fill="none" stroke="#cf6a2a" stroke-width="4"/>
<path d="M30 248 L106 199 L178 115" fill="none" stroke="#3c6b39" stroke-width="1.5" />
<path d="M111 196 l-7 19 15 -4 z" fill="#24331c"/>
<rect x="524" y="83" width="175" height="215" fill="#ece8cf" stroke="#3c6b39" stroke-width="1.2" rx="8"/>
<path d="M529 203 C589 133 624 263 694 168" fill="none" stroke="#33575b" stroke-width="5"/>
<path d="M549 93 L586 148 L634 193 L669 287" fill="none" stroke="#cf6a2a" stroke-width="4"/>
<path d="M534 248 L610 199 L682 115" fill="none" stroke="#3c6b39" stroke-width="1.5" />
<path d="M615 196 l-7 19 15 -4 z" fill="#24331c"/>
<rect x="272" y="83" width="175" height="215" fill="none" stroke="#3c6b39" stroke-width="1.2" stroke-dasharray="5 5" rx="8"/>
<text class="d-title" x="237" y="182" text-anchor="middle">+</text>
<path d="M462 182 H510" fill="none" stroke="#3c6b39" stroke-width="1.5" marker-end="url(#r49arrow)"/>
<rect x="284" y="221" width="151" height="61" fill="#f3f0df" stroke="#3c6b39" stroke-width="1.2" rx="6"/>
<text class="d-sub" x="359" y="242" text-anchor="middle">Hold to confirm</text>
<rect x="296" y="255" width="125" height="10" fill="#d6cda8" stroke="#3c6b39" stroke-width="1.2" />
<rect x="296" y="255" width="82" height="10" fill="#cf6a2a" stroke="#3c6b39" stroke-width="1.2" />
<rect x="536" y="221" width="151" height="61" fill="#f3f0df" stroke="#3c6b39" stroke-width="1.2" rx="6"/>
<text class="d-sub" x="611" y="242" text-anchor="middle">Hold to confirm</text>
<rect x="548" y="255" width="125" height="10" fill="#d6cda8" stroke="#3c6b39" stroke-width="1.2" />
<rect x="548" y="255" width="82" height="10" fill="#cf6a2a" stroke="#3c6b39" stroke-width="1.2" />
<text class="d-sub" x="20" y="330" text-anchor="start">Base pixels stay unchanged.</text>
<text class="d-sub" x="272" y="330" text-anchor="start">Only the active window draws.</text>
<text class="d-sub" x="524" y="330" text-anchor="start">Base + hold feedback</text>
<text class="d-sub" x="20" y="368" text-anchor="start">On release or completion, present the clean window again to remove the feedback.</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>This illustrates hold feedback, not a retained notification screen. Cards and drawers follow their own screen-stack and repaint rules.</figcaption>
</figure>

The overlay presenter reads the clean base frame, adds the overlay, and leaves the base frame
unchanged.

## Screens the companion pushes

The companion can open modal cards for pairing, route and trip updates, and warnings. The scheduler
gives each type a fixed priority, and a new card never replaces a hold in progress. The passkey card
cannot be dismissed before pairing ends.

## Riding data

### Climbs

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 250" role="img" aria-label="Left, a device mock of the Climb screen: a wood CLIMB title bar reading a summit height, a rising elevation profile whose columns are tinted by local gradient from green through red, an amber you-are-here cursor, and four apricot stat tiles. Right, the gradient-to-colour ramp — under 3 percent green, 3 to 6 yellow, 6 to 9 amber, 9 to 12 orange, over 12 red — with notes that the climbs are segmented once at load and drawn from a finer profile scoped to the active climb.">
  <text class="d-tag" x="20" y="24">The climb panel — gradient shown as colour</text>

  <rect x="40" y="44" width="150" height="192" rx="10" style="fill:#ffffff;stroke:#aaaa55;stroke-width:1.5" />
  <rect x="46" y="50" width="138" height="20" rx="5" style="fill:#aa5500" />
  <text x="56" y="64" style="fill:#fff;font-family:var(--mono);font-size:12px">CLIMB</text>
  <text x="178" y="64" text-anchor="end" style="fill:#fff;font-family:var(--mono);font-size:12px">1762 m</text>
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
  <text x="57" y="172" style="fill:#6b5a2a;font-family:var(--mono);font-size:12px">CLIMB</text>
  <text x="122" y="172" style="fill:#6b5a2a;font-family:var(--mono);font-size:12px">LEFT</text>
  <text x="57" y="208" style="fill:#6b5a2a;font-family:var(--mono);font-size:12px">GRADE</text>
  <text x="122" y="208" style="fill:#6b5a2a;font-family:var(--mono);font-size:12px">AVG</text>

  <text class="d-sub" x="300" y="60" style="font-size:12px">local gradient → stripe colour</text>
  <g>
    <rect x="300" y="72" width="70" height="26" style="fill:#00aa00" />
    <rect x="370" y="72" width="70" height="26" style="fill:#ffff00" />
    <rect x="440" y="72" width="70" height="26" style="fill:#ffaa00" />
    <rect x="510" y="72" width="70" height="26" style="fill:#ff5500" />
    <rect x="580" y="72" width="70" height="26" style="fill:#ff0000" />
  </g>
  <text class="d-sub" x="335" y="114" text-anchor="middle" style="font-size:12px">&lt; 3%</text>
  <text class="d-sub" x="405" y="114" text-anchor="middle" style="font-size:12px">3–6</text>
  <text class="d-sub" x="475" y="114" text-anchor="middle" style="font-size:12px">6–9</text>
  <text class="d-sub" x="545" y="114" text-anchor="middle" style="font-size:12px">9–12</text>
  <text class="d-sub" x="615" y="114" text-anchor="middle" style="font-size:12px">&gt; 12%</text>

  <text class="d-sub" x="300" y="150" style="font-size:12px">· climbs are segmented once, when the route loads</text>
  <text class="d-sub" x="300" y="170" style="font-size:12px">· a dip is bridged, a deep col splits — plain gates on</text>
  <text class="d-sub" x="312" y="186" style="font-size:12px">gain, average grade, and length</text>
  <text class="d-sub" x="300" y="208" style="font-size:12px">· a finer profile, scoped to the active climb, rebuilt</text>
  <text class="d-sub" x="312" y="224" style="font-size:12px">only when you cross into the next one</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The climb view shows the current climb profile and four climb values.</figcaption>
</figure>

The route processor supplies the climb segments and their profiles. Climb joins the riding views
only while a climb is active.

### Waypoints

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 270" role="img" aria-label="Left, a device mock of the riding Map: a magenta route line down the middle, two small black diamonds on it marking named waypoints, a red heading arrow for the rider, and a bottom pill reading a diamond, the name Pass and 299 m — the approach chip counting down. Right, the Statistics progress bar in close-up: an amber fill from the left with two black vertical ticks, one at the far left for a waypoint at the start and one near three-quarters for the pass, annotated: the fill sweeps toward the next tick, the ticks are ink not red so they survive the off-route red tint, and the chip hides off-route.">
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
  <text x="50" y="192" style="fill:#000;font-family:var(--mono);font-size:12px">20m</text>
  <!-- approach chip -->
  <rect x="46" y="208" width="138" height="22" rx="9" style="fill:#ffffff;stroke:#000;stroke-width:1" />
  <path d="M60 219 l4 4 l-4 4 l-4 -4 z" style="fill:#000" />
  <text x="70" y="223" style="fill:#000;font-family:var(--mono);font-size:12px">Pass · 299 m</text>
  <text class="d-sub" x="115" y="248" text-anchor="middle" style="font-size:12px">waypoint + approach chip</text>

  <!-- progress bar close-up -->
  <text class="d-sub" x="250" y="56" style="font-size:12px">the Statistics progress bar shares the route's distance axis</text>
  <rect x="250" y="86" width="430" height="18" rx="9" style="fill:#eae3c9;stroke:#aaaa55;stroke-width:1" />
  <!-- live fill ~0.63 -->
  <rect x="250" y="86" width="271" height="18" rx="9" style="fill:#ffaa00" />
  <!-- ticks: start (~0) and pass (~0.77) -->
  <rect x="252" y="89" width="2.4" height="12" style="fill:#000" />
  <rect x="581" y="89" width="2.4" height="12" style="fill:#000" />
  <!-- annotations -->
  <line x1="253" y1="118" x2="253" y2="130" stroke="#6b5a2a" stroke-width="1" />
  <text class="d-sub" x="258" y="142" style="font-size:12px">waypoint at the start</text>
  <line x1="582" y1="118" x2="582" y2="130" stroke="#6b5a2a" stroke-width="1" />
  <text class="d-sub" x="582" y="142" text-anchor="middle" style="font-size:12px">the pass</text>
  <line x1="521" y1="76" x2="521" y2="84" stroke="#a9501c" stroke-width="1.2" marker-end="none" />
  <text class="d-sub" x="521" y="72" text-anchor="middle" style="fill:#a9501c;font-size:12px">you are here</text>
  <text class="d-sub" x="250" y="176" style="font-size:12px">· the fill sweeping toward the next tick is free "distance to go"</text>
  <text class="d-sub" x="250" y="194" style="font-size:12px">· ticks are <tspan style="fill:#000;font-weight:600">ink</tspan>, never red — the bar itself tints red off-route, where a</text>
  <text class="d-sub" x="262" y="210" style="font-size:12px">red tick would vanish</text>
  <text class="d-sub" x="250" y="228" style="font-size:12px">· off-route the chip hides and the bar freezes — the along-route</text>
  <text class="d-sub" x="262" y="244" style="font-size:12px">distance is meaningless once you've left the line</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Map, route, and statistics views use the same route-distance axis for waypoints.</figcaption>
</figure>

Waypoints come from the route file, and Navigator tracks the next one from the matched position.
The map, route, and statistics views place them on the same route-distance axis, so a waypoint is
at the same progress in all three.

### Up ahead

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 320" role="img" aria-label="Two panels comparing the device's two spatial queries. On the left, near me: a dashed circle drawn around the rider's fix, with points of interest scattered inside it and one greyed out beyond the edge; the route line crosses the panel faintly and plays no part. On the right, up ahead: the magenta route line runs through a pale band three hundred metres wide to each side, the rider sits on the line, and two points of interest inside the band are joined by dashed leaders to tick marks on the line itself, labelled with their along-route distances of one point two kilometres and four point eight kilometres; a third point outside the band is greyed out. Below each panel, a summary box: the left produces a browser list sorted by straight-line distance with a live bearing arrow, the right produces the Up ahead timeline sorted along the route, each row carrying a distance to go, a climb to go, and which side it sits on.">
  <text class="d-tag" x="20" y="24">Two spatial questions — one map, one index, two windows</text>

  <text class="d-sub" x="30" y="52" style="font-size:12px;fill:#4d5b3c">① near me — a disc around the fix</text>
  <rect class="d-panel" x="24" y="60" width="320" height="186" rx="11" />
  <!-- the route is present but irrelevant to this question -->
  <path d="M56 232 C 106 198, 146 188, 186 158 C 226 128, 258 108, 320 90" fill="none" stroke="#ff00ff" stroke-width="4" opacity="0.16" />
  <circle cx="184" cy="158" r="66" fill="none" stroke="#6b7758" stroke-width="1.3" stroke-dasharray="4 4" />
  <g fill="#3d3427">
    <circle cx="146" cy="124" r="5" /><circle cx="228" cy="140" r="5" />
    <circle cx="162" cy="200" r="5" /><circle cx="232" cy="196" r="5" />
  </g>
  <circle cx="300" cy="104" r="5" fill="none" stroke="#c9c7b8" stroke-width="1.4" />
  <text class="d-sub" x="292" y="92" text-anchor="end" style="font-size:12px;fill:#9aa884">out of range</text>
  <path d="M184 158 L222 143" stroke="#cf6a2a" stroke-width="1.6" />
  <path d="M228 140 l-11 -1 l5 6 z" fill="#cf6a2a" />
  <text class="d-sub" x="222" y="122" text-anchor="end" style="font-size:12px;fill:#a9501c">bearing</text>
  <path d="M184 150 l7 14 l-7 -5 l-7 5 z" fill="#ff0000" />
  <text class="d-sub" x="184" y="236" text-anchor="middle" style="font-size:12px">nearest 16 · by straight-line distance · route-blind</text>

  <text class="d-sub" x="382" y="52" style="font-size:12px;fill:#4d5b3c">② up ahead — a corridor along the route</text>
  <rect class="d-panel" x="376" y="60" width="320" height="186" rx="11" />
  <path d="M402 228 C 450 206, 468 170, 510 148 C 552 126, 578 100, 668 84" fill="none" stroke="#e6e0c4" stroke-width="34" stroke-linecap="round" />
  <path d="M402 228 C 450 206, 468 170, 510 148 C 552 126, 578 100, 668 84" fill="none" stroke="#ff00ff" stroke-width="4" />
  <!-- inside the corridor: projected onto the line -->
  <circle cx="492" cy="170" r="5" fill="#3d3427" />
  <path d="M492 170 L503 150" stroke="#6b7758" stroke-width="1.1" stroke-dasharray="3 3" />
  <circle cx="503" cy="150" r="2.8" fill="#000" />
  <text class="d-sub" x="512" y="180" style="font-size:12px">1.2 km</text>
  <circle cx="572" cy="92" r="5" fill="#3d3427" />
  <path d="M572 92 L566 114" stroke="#6b7758" stroke-width="1.1" stroke-dasharray="3 3" />
  <circle cx="566" cy="114" r="2.8" fill="#000" />
  <text class="d-sub" x="584" y="90" style="font-size:12px">4.8 km</text>
  <!-- outside the corridor -->
  <circle cx="636" cy="164" r="5" fill="none" stroke="#c9c7b8" stroke-width="1.4" />
  <text class="d-sub" x="644" y="180" text-anchor="middle" style="font-size:12px;fill:#9aa884">off corridor</text>
  <path d="M444 201 l8 13 l-8 -5 l-8 5 z" fill="#ff0000" transform="rotate(53 444 201)" />
  <text class="d-sub" x="392" y="236" style="font-size:12px">±300 m either side · only what is still ahead of you</text>

  <rect class="d-panel-2" x="24" y="262" width="320" height="48" rx="9" />
  <text class="d-sub" x="40" y="282" style="font-size:12px">→ the <tspan class="d-label" style="font-size:12px">POIs browser</tspan> — rows by distance,</text>
  <text class="d-sub" x="52" y="298" style="font-size:12px">with a live bearing arrow</text>
  <rect class="d-hot" x="376" y="262" width="320" height="48" rx="9" style="fill:#f8efe4" />
  <text class="d-sub" x="392" y="282" style="font-size:12px">→ the <tspan class="d-label" style="font-size:12px;fill:#a9501c">Up ahead timeline</tspan> — rows along the route,</text>
  <text class="d-sub" x="404" y="298" style="font-size:12px">with distance-to-go, climb-to-go and a side</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Nearby POIs use geographic distance. Up-ahead entries use distance along the route.</figcaption>
</figure>

Nearby POIs use the distance through the air. Up ahead uses the distance along the route, which is
the distance a rider actually has to ride.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 192" role="img" aria-label="One timeline row dissected, plus the source cue legend. The row is amber because it is under the cursor: line one carries a category icon with a small diamond pip beside it and the ellipsized name Fontaine du port; line two carries the distance to go, a climb to go prefixed by an up triangle, and at the right edge a left-pointing triangle followed by 271 metres, the off-route side hint. To the right, four icon states: a map POI unselected in muted olive, a map POI under the cursor in ink, a custom waypoint in amber with a pip, and a custom waypoint under the cursor in ink with a pip.">
  <text class="d-tag" x="20" y="24">The row · the source cue</text>

  <!-- the row (selected: amber) -->
  <rect x="24" y="44" width="392" height="66" rx="6" class="d-amber" />
  <circle cx="52" cy="70" r="7" fill="#000" /><path d="M45 67 l7 -9 l7 9 z" fill="#000" />
  <path d="M66 56 l4 4 l-4 4 l-4 -4 z" fill="#000" />
  <text x="80" y="76" font-family="var(--mono)" style="font-size:12.5px;fill:#000">Fontaine du port</text>
  <text x="34" y="100" font-family="var(--mono)" style="font-size:12px;fill:#000">219m</text>
  <path d="M228 93 l5 -7 l5 7 z" fill="#000" />
  <text x="242" y="100" font-family="var(--mono)" style="font-size:12px;fill:#000">13m</text>
  <path d="M352 96 l9 -5 l0 10 z" fill="#000" />
  <text x="406" y="100" text-anchor="end" font-family="var(--mono)" style="font-size:12px;fill:#000">271m</text>

  <!-- callouts -->
  <text class="d-sub" x="24" y="128" style="font-size:12px;fill:#a9501c">icon + pip</text>
  <text class="d-sub" x="96" y="128" style="font-size:12px;fill:#a9501c">name, ellipsized to fit</text>
  <text class="d-sub" x="24" y="142" style="font-size:12px">distance-to-go</text>
  <text class="d-sub" x="214" y="142" style="font-size:12px">climb-to-go</text>
  <text class="d-sub" x="416" y="142" text-anchor="end" style="font-size:12px">side, past 50 m</text>

  <!-- source cue legend -->
  <text class="d-tag" x="444" y="60">the icon says which source</text>
  <g>
    <circle cx="460" cy="80" r="6" fill="#6b7758" /><text class="d-sub" x="478" y="84" style="font-size:12px">map POI</text>
    <circle cx="460" cy="102" r="6" fill="#24331c" /><text class="d-sub" x="478" y="106" style="font-size:12px">map POI · cursor</text>
    <circle cx="460" cy="124" r="6" fill="#ffaa00" /><path d="M472 116 l3.5 3.5 l-3.5 3.5 l-3.5 -3.5 z" fill="#ffaa00" />
    <text class="d-sub" x="484" y="128" style="font-size:12px">waypoint</text>
    <circle cx="460" cy="146" r="6" fill="#24331c" /><path d="M472 138 l3.5 3.5 l-3.5 3.5 l-3.5 -3.5 z" fill="#24331c" />
    <text class="d-sub" x="484" y="150" style="font-size:12px">waypoint · cursor</text>
  </g>
  <text class="d-sub" x="444" y="172" style="font-size:12px;fill:#4d5b3c">the pip is the colourblind-safe half of the pair</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The Up-ahead view merges two sorted sources without copying rows.</figcaption>
</figure>

One pass through the corridor of a place is one row, at the nearest point of that pass, so a bend
in the road does not add rows. Leaving and returning is a later encounter.

A cursor the rider set counts only against the list they set it in: while the list shows something
else, the cursor is the first row still ahead. Without that rule a rider who scrolls and then
filters lands on the last match instead of the nearest one.

## Main rider flow

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 535" role="img" aria-label="Start a ride through Home, the main menu, Routes, and Overview. Start opens Map. Back cycles Map, Statistics, and an enabled active Climb. Select pauses. Select-hold on Map enters Inspect and Back leaves it. Chords open drawers; Back-hold opens the main menu except in blocking states.">
  <defs><marker id="software-ui-18" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker></defs>
  <text class="d-tag" x="20" y="26" text-anchor="start">Main rider paths</text>
  <text class="d-title" x="20" y="65" text-anchor="start">Start a route</text>
  <rect class="d-panel" x="20" y="85" width="110" height="58" rx="8" />
  <text class="d-title" x="75" y="110" text-anchor="middle">Home</text>
  <rect class="d-panel" x="163" y="85" width="120" height="58" rx="8" />
  <text class="d-title" x="223" y="110" text-anchor="middle">Main menu</text>
  <rect class="d-panel" x="316" y="85" width="105" height="58" rx="8" />
  <text class="d-title" x="368.5" y="110" text-anchor="middle">Routes</text>
  <rect class="d-panel" x="454" y="85" width="130" height="58" rx="8" />
  <text class="d-title" x="519" y="110" text-anchor="middle">Overview</text>
  <path class="d-flow" d="M130 114 L160 114" marker-end="url(#software-ui-18)" />
  <path class="d-flow" d="M283 114 L313 114" marker-end="url(#software-ui-18)" />
  <path class="d-flow" d="M421 114 L451 114" marker-end="url(#software-ui-18)" />
  <path class="d-flow" d="M584 114 L624 114" marker-end="url(#software-ui-18)" />
  <rect class="d-panel d-focus" x="627" y="85" width="73" height="58" rx="8" />
  <text class="d-title" x="663.5" y="110" text-anchor="middle">Map</text>
  <text class="d-sub" x="604" y="76" text-anchor="middle">Start</text>
  <text class="d-title" x="20" y="190" text-anchor="start">During a ride</text>
  <rect class="d-panel d-focus" x="20" y="211" width="200" height="72" rx="8" />
  <text class="d-title" x="120" y="236" text-anchor="middle">Map</text>
  <text class="d-sub" x="120" y="256" text-anchor="middle">Back → Statistics</text>
  <path class="d-flow" d="M220 247 L258 247" marker-end="url(#software-ui-18)" />
  <rect class="d-panel" x="260" y="211" width="200" height="72" rx="8" />
  <text class="d-title" x="360" y="236" text-anchor="middle">Statistics</text>
  <text class="d-sub" x="360" y="256" text-anchor="middle">Back → Climb or Map</text>
  <path class="d-flow" d="M460 247 L498 247" marker-end="url(#software-ui-18)" />
  <rect class="d-panel" x="500" y="211" width="200" height="72" rx="8" />
  <text class="d-title" x="600" y="236" text-anchor="middle">Climb</text>
  <text class="d-sub" x="600" y="256" text-anchor="middle">Active + enabled · Back → Map</text>
  <text class="d-label" x="20" y="330" text-anchor="start">Select on Map</text>
  <text class="d-sub" x="255" y="330" text-anchor="start">Pause · Resume returns to the ride</text>
  <text class="d-label" x="20" y="364" text-anchor="start">Select-hold on Map</text>
  <text class="d-sub" x="255" y="364" text-anchor="start">Inspect · Back returns to Map</text>
  <text class="d-label" x="20" y="398" text-anchor="start">Down + Back</text>
  <text class="d-sub" x="255" y="398" text-anchor="start">Context drawer · Up ahead, Detour, POIs, Routes</text>
  <text class="d-label" x="20" y="432" text-anchor="start">Up + Select</text>
  <text class="d-sub" x="255" y="432" text-anchor="start">Quick drawer · device controls</text>
  <text class="d-label" x="20" y="466" text-anchor="start">Back-hold</text>
  <text class="d-sub" x="255" y="466" text-anchor="start">Main menu · except in blocking states</text>
<path d="M600 283 V306 H120 V283" fill="none" stroke="#3c6b39" stroke-width="1.5" marker-end="url(#software-ui-18)"/><text class="d-sub" x="360" y="302" text-anchor="middle">Back returns to Map</text><text class="d-sub" x="20" y="512" text-anchor="start">Back skips Climb when it is inactive or disabled.</text></svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Start opens Map on a clean ride stack. Back cycles the available riding views. Drawers and Inspect keep a direct return to the ride.</figcaption>
</figure>

Home opens the main menu, a route selection opens its overview, and Start opens Map on a clean ride
stack. During a ride, Back cycles the riding views, a press pauses, and Back-hold reaches the main
menu. Map Inspect is entered with a Select hold, and Back leaves Inspect before it changes views.
Idle return removes abandoned chrome: it returns to Home when there is no ride, and to Map during
one.

## Visual vocabulary

Each shared mechanism has one owner, one module per concept under `screen/vocab/`:

| Module | What it owns |
| --- | --- |
| `chrome` | The framed page header, the card glyphs, the Recalculating banner, and the shared text and stroke helpers. |
| `list` | The scrolling list: the wrapping cursor, the window math, the row cursor, the separators, and the scrollbar. |
| `rows` | The settings row and its cursor, the value picker, the stat-ledger row, and the guarded action rows. |
| `card` | Selection, input, and drawing for the action rows of full-screen cards. |
| `tiles` | The rounded stat panes of the riding grid and the Fields editor, and the waypoint panel. |
| `band` | The elevation band: the filled silhouette, the connected top stroke, and the peak label. |
| `fmt` | One formatter per printed quantity and output style. |
| `marquee` | The one long name a frame scrolls, by whole characters. |
| `pager` | The two-page auto-flip the detail compositions share. |
| `sheet` | The drawer sheets' shared motion and marks. |
| `spinner` | The compass needle a working screen waits behind. |

A name that does not fit its field is cut with two dots. One name per frame scrolls instead: the
highlighted list row, a detail title, the selected peak in Peak View. Every font is monospace, so a
scroll step is a different substring and needs no new primitive. The riding view's waypoint tile
scrolls once when the name changes and then rests, so nothing moves while the rider rides.

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 200" role="img" aria-label="A palette swatch strip showing the device-64 colours: parchment white, wood brown, ink black, amber, warning orange, route magenta, and breadcrumb navy. Beside it, a small framed-screen mock with a wood title bar, a parchment body, and an amber-highlighted list row with a pointer bullet.">
  <text class="d-tag" x="20" y="24">One palette, tuned to the 64-colour panel</text>

  <!-- swatches -->
  <g>
    <rect x="36"  y="48" width="54" height="40" rx="6" style="fill:#ffffff;stroke:#9aa884;stroke-width:1" /><text class="d-sub" x="63" y="104" text-anchor="middle" style="font-size:12px">parchment</text>
    <rect x="98"  y="48" width="54" height="40" rx="6" style="fill:#aa5500" /><text class="d-sub" x="125" y="104" text-anchor="middle" style="font-size:12px">wood</text>
    <rect x="160" y="48" width="54" height="40" rx="6" style="fill:#000000" /><text class="d-sub" x="187" y="104" text-anchor="middle" style="font-size:12px">ink</text>
    <rect x="222" y="48" width="54" height="40" rx="6" style="fill:#ffaa00" /><text class="d-sub" x="249" y="104" text-anchor="middle" style="font-size:12px">amber</text>
    <rect x="284" y="48" width="54" height="40" rx="6" style="fill:#ff5500" /><text class="d-sub" x="311" y="104" text-anchor="middle" style="font-size:12px">warning</text>
    <rect x="346" y="48" width="54" height="40" rx="6" style="fill:#ff00ff" /><text class="d-sub" x="373" y="104" text-anchor="middle" style="font-size:12px">route</text>
    <rect x="408" y="48" width="54" height="40" rx="6" style="fill:#0000aa" /><text class="d-sub" x="435" y="104" text-anchor="middle" style="font-size:12px">breadcrumb</text>
  </g>
  <text class="d-sub" x="36" y="140" style="font-size:12px">authored in RGB565 · resolved through the same color_fn as the map ·</text>
  <text class="d-sub" x="36" y="156" style="font-size:12px">every value asserted against its device-64 (RGB222) result</text>

  <!-- mini framed screen -->
  <rect x="556" y="44" width="128" height="138" rx="10" style="fill:#ffffff;stroke:#aaaa55;stroke-width:1.5" />
  <rect x="562" y="50" width="116" height="22" rx="5" style="fill:#aa5500" /><text class="d-sub" x="572" y="65" style="fill:#fff;font-size:12px">MENU</text>
  <rect x="566" y="82" width="108" height="26" rx="5" style="fill:#ffaa00" />
  <path d="M578 88 L578 102 L588 95 z" fill="#000" /><text class="d-sub" x="596" y="99" style="font-size:12px">Routes</text>
  <text class="d-sub" x="596" y="129" style="font-size:12px">Settings</text>
  <line x1="566" y1="138" x2="674" y2="138" stroke="#aaaa55" stroke-width="1" />
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>All screen colors use the shared RGB565-to-RGB222 conversion.</figcaption>
</figure>

Palette constants are written once and converted to the panel's 64 colors by the framebuffer, so a
screen never picks a device color by hand.

## Adding a screen

One module and one row declare a screen. The row alone does not put it on the glass:

| Step | Where |
| --- | --- |
| The state type, `handle`, and `draw`. | a new module under [`screen/`](src:firmware/obc-app/src/screen) |
| The module declaration, the variant, and its capabilities. | [`screen/mod.rs`](src:firmware/obc-app/src/screen/mod.rs): its `mod` line and the `screens!` table |
| Every string it prints, in four languages. | the catalogs under [`i18n/`](src:firmware/obc-app/i18n) |
| The way in: a menu row, a drawer row, a settings row, a companion card, or the module that owns the work the screen reports. | with that entry point, which usually sits outside `screen/` |
| A sweep frame, whose `expect` names the variant it must reach, and its digest row — one row per language for a `langs` frame. | [`ui-frames.toml`](src:firmware/ui-frames.toml), then `obc shot --accept` records the digest in [`ui-snapshots.sha256`](src:firmware/ui-snapshots.sha256) |

The acceptance test is the copy-fit gate over every reachable screen and language.

## Source map

| Subject | Source |
| --- | --- |
| Screen table, capabilities, contexts, and transitions | [`screen/mod.rs`](src:firmware/obc-app/src/screen/mod.rs) |
| Gesture recognition | [`input.rs`](src:firmware/obc-app/src/input.rs), [`input_plane.rs`](src:firmware/obc-app/src/input_plane.rs) |
| Repaint state | [`dirty.rs`](src:firmware/obc-app/src/dirty.rs), [`render_key.rs`](src:firmware/obc-app/src/render_key.rs), [`ui_runtime.rs`](src:firmware/obc-app/src/ui_runtime.rs) |
| Shared screen primitives | [`screen/vocab/`](src:firmware/obc-app/src/screen/vocab) |
| Settings and translations | [`settings.rs`](src:firmware/obc-app/src/settings.rs), [`i18n/`](src:firmware/obc-app/i18n) |
| Find a place and visits | [`find_place.rs`](src:firmware/obc-app/src/find_place.rs), [`visit.rs`](src:firmware/obc-route/src/visit.rs) |
| POI and Up-ahead views | [`poi_list.rs`](src:firmware/obc-app/src/screen/poi_list.rs), [`whats_next.rs`](src:firmware/obc-app/src/screen/whats_next.rs) |

See [system architecture](../architecture/) for the host loop and [rendering pipeline](../rendering/)
for pixel generation.
