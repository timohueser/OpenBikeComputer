---
title: UI system
description: Screen state, navigation, input, repaint policy, and rider-facing data views.
copy: ai
---

# The UI system

The UI is a `no_std`, allocation-free system for a 240×320-pixel display. It uses four buttons and immediate-mode drawing.

The screen drawings below are schematics. They explain behavior; they are not current pixel captures.

## Screen model

Each screen is one `Screen` enum variant. The variant owns its state by value.

A `screens!` table defines each variant and its `Caps`. The table generates the enum, normal input
and drawing dispatch, and capability metadata. A small manual `prepare` match delegates only the
four reader-backed screens that need a one-shot operation before drawing.

`Caps` declares cross-cutting behavior. It covers base content, overlays, timers, holds, reader access, idle return, catalog remapping, and the render key.

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
  <text class="d-sub" x="324" y="258" style="font-size:12px">add a screen = 1 module + 1 row in the screens! table</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>Each screen owns typed state. The <code>Screen</code> enum provides static dispatch without heap allocation.</figcaption>
</figure>

A normal screen implements these operations:

- `handle` reads one `Gesture` and returns a `Transition`.
- `draw` writes the current frame.

Four reader-backed screens also implement `prepare` for a required one-shot reader operation. The
manual dispatch is partial because other screens do not need this operation.

The `Ctx` input context contains mutable application state. The `Render` context contains read-only state and borrowed rendering resources.

## Navigation

The screen stack is a `heapless::Vec<Screen, 10>`. Home is always the first item.

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
<figcaption>The top screen returns a transition. The UI runtime applies it to the stack.</figcaption>
</figure>

| Transition | Stack operation |
| --- | --- |
| `None` | Keep the stack. |
| `Push(screen)` | Add a top screen. |
| `Pop` | Remove the top screen, except Home. |
| `Replace(screen)` | Replace the top screen. |
| `Root(screen)` | Keep Home and add one screen. |
| `Home` | Remove all screens above Home. |

A stack change cancels all incomplete holds. This rule prevents a hold from completing on a new screen.

Each screen owns its ordinary `Back` policy in its typed `handle` method. Back can pop a screen,
leave an editor, cancel domain work, or move to a sibling view. Back-hold and button chords are
handled above screen dispatch.

### Detour flow

The Detour command is available during route navigation. It requires a routing graph and a matched route position.

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

Up and Down move the rejoin point in 100 m steps. The minimum rejoin distance is 600 m.

A successful plan shows distance and climb differences. Commit stores the splice as the active route.

The splice keeps completed route geometry, adds the detour, and continues from the rejoin point. It then rebuilds route-derived data.

## Input

The device has Up, Down, Select, and Back buttons.

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

The recognizer emits these gestures:

| Gesture | Source |
| --- | --- |
| `Step(n)` | Up or Down step |
| `Press` | Select release within 200 ms |
| `Hold` | Select held for 500 ms |
| `Back` | Back release within 200 ms |
| `BackHold` | Back held for 500 ms |

A release after 200 ms and before 500 ms emits no gesture. Long holds emit at the threshold, not on release.

`BackHold` is the **global escape**. The app answers it above the screen stack, so it never reaches a
screen: it closes any open drawer and goes to the main menu, from every screen. With a main menu
already on the stack it returns to that one instead of opening a second, so repeated holds cannot
grow the stack.

Three states refuse it, because the rider must finish them first: a blocking card (the pairing
passkey, a map transfer, the terminal update card), the recovered-ride card, and a shutdown that the
rider has already confirmed. A two-button squeeze is refused in the same three states.

## Chords

Two buttons pressed within 100 ms of each other are one **chord**, not two gestures. The recognizer
reports the chord above the screen stack and emits nothing for the two buttons: no step, no tap, no
long press, and no release. The chord stays latched until both buttons are up.

| Chord | Meaning |
| --- | --- |
| Up + Select, released before 500 ms | Open or close the universal quick drawer |
| Up + Select, held for 500 ms | Open Ride Assistant |
| Down + Back | Open or close the current screen's contextual drawer |
| Up + Down | Reserved |
| Select + Back | Reserved |

The Assistant hold starts when the second button goes down. Matching bulges grow beside Up and
Select, then pop together when Assistant opens. A release before the threshold retracts both.
Select and Back holds use the same bulge size, at equal distances above and below the screen centre.
Quick taps do not show a bulge.

A reserved chord is recognized and swallowed. It does nothing. This keeps a squeeze of two buttons
from becoming two unrelated actions.

Because a chord can start with a direction button, the first step of Up or Down waits for the
100 ms window. A release inside the window steps immediately, so a tap does not feel slower.
Automatic repeat measures its delay from the press edge, so a held button keeps its usual cadence.

## Drawers

A drawer is a sheet that the device draws over the current screen. There are two, and only one of
them can be open: the second chord replaces the sheet instead of adding one.

The **universal quick drawer** comes down from the top and holds the device-wide controls:
brightness, the Bluetooth radio, the central settings, and power. Brightness and power open a nested
page. Back closes the sheet and returns the rider to the screen below it.

A platform whose panel has no controllable light does not show the brightness control. The sheet
has the remaining three controls.

Nothing lands on top of a drawer. A card that arrives while a sheet is open takes the sheet with it,
so dismissing the card returns the rider to the screen they were on.

The **contextual drawer** comes up from the bottom and holds the current screen's secondary
actions. A screen does not build a drawer: it declares a static table of rows, and one generic
drawer supplies the cursor, the transitions and the drawing. A screen that declares no table gets no
sheet, and the chord does nothing on it — an empty drawer is never shown.

The four riding views (Map, Statistics, Climb, and the paused page) offer the same four actions in
the same order: Up ahead, Detour, POIs, and Routes. A row that cannot act right now is drawn
recessed and does nothing — the Detour row without a route, without map routing data, or off the
route. A row that can act replaces the sheet with its screen, so one Back returns the rider to the
riding view they squeezed from.

The Map also offers **Map display**. It contains switches for the clock, scale bar and
contour layer, plus **Map icons**. The icon menu has independent switches for peaks,
landmarks and POIs. Its category menu controls water, campsites, lodging, resupply,
pharmacies, bike shops and train stations. The category row shows the selected count.
Turning POIs off keeps the category selection. These preferences persist through restart
and do not change Find a Place or routing filters. All icon groups start enabled.

Category menus show at most five rows and scroll within the same sheet. Back from the
category menu returns to the icon menu; Back from the icon menu returns to Map display.
Back from Map display closes the sheet. A replacement sheet arrives without an entrance
animation.

Map icons stay upright at their source coordinates. Peaks include unnamed summits. A
small white backing keeps each glyph visible over terrain. Peaks appear at 50 metres per
pixel or closer, landmarks at 20, and service POIs at 10. A stable overlap rule gives water
and campsites priority, then peaks, landmarks and other services. The rider, waypoints,
clock and bottom map controls have reserved space. Route lines draw above icons.

The shared application retains at most 64 candidates and draws at most 24 icons, with
lower limits at wider scales. A padded viewport cache avoids storage reads during small
camera movements. Each preparation advances at most eight POI index or 512-byte record
steps and eight landmark query steps. Work continues on a timer and stops when complete.
Map, visibility or coverage changes invalidate the selection. Visible candidates take
priority over points in the padding. A transient read failure gets two delayed retries;
a persistent failure stops until the view or map changes.

A row can also hold a **value** in place of a screen. Such a row slides the sheet to a nested
editor: `Up` and `Down` change the staged choice, `Select` writes it and returns to the row table,
and `Back` discards it. The editor keeps a mark on the choice that is already in effect. The sheet
becomes as high as the editor needs and goes back to its table height. The Up-ahead view declares
two such rows, Filter and Sources. The create-route card declares one, Bike type —
the routing profile the device plans with. These rows are the only place those controls are set.

The bike-type row shows what a value row does when its choices come from the loaded map. The
choices are the map's own routing-profile names. A map built with a custom profile offers that
profile with no change to the device software. A map with only one profile, or no map at all,
offers no choice. The row is then drawn recessed and does nothing. The row is on the create-route
card because that card is where the choice is used: the next press asks for a plan. The route
overview shows the profile a route was planned with. It does not let the rider change it, because
a change would make the page say something untrue about the route it shows.

A row can also be a **switch**. It shows its state and flips in place. The sheet stays
open while the rider changes a group of preferences. The covered map stays still; the
changes appear when the map is exposed again.

The drawer is the only home for a setting that belongs to one screen. A control that moves into a
drawer is removed from the central settings tree in the same change. A check in the build fails if
a drawer and a settings screen write the same stored setting.

The screen under a drawer is **frozen**. A drawer states its own facts as its render key — the page,
the selected control, the staged value and the value in effect — and that key replaces the facts of
the screens below. So a moving map under a drawer causes no repaint, and the timed content of the
screen below stops with it.

Whether the screen below is **dimmed** is a property of that screen. A map view is not dimmed: its
second drawing is a map render, hundreds of milliseconds on the device, and the map reads well under
the sheet at full colour. Menus, lists and settings pages are dimmed through a colour table, because
drawing them again costs almost nothing and the recess helps the sheet read as being in front.

A screen that is not dimmed is also frozen **on the panel**. While a sheet grows over such a screen,
the device draws the sheet alone and leaves the rows below it exactly as they are, so the open costs
the sheet and no more. This needs no extra frame buffer: the panel keeps the last frame, and the
sheet writes over it. A screen that *is* dimmed is drawn again on every one of those frames, because
the dim is that drawing — but it is a menu, so the drawing is cheap.

Three cases draw the screen below again whatever it is, and all three are cases where the sheet
stops purely covering. Every frame of a **page slide** does, because the two pages travel through the
narrow margin either side of the sheet, where the screen below shows; when the two pages differ in
height, the same drawing puts back the rows the shrinking sheet gives up. The first frame of a drawer
that **replaces the other drawer** does, once, because the departed sheet's rows are still on the
panel at the opposite edge. And the frame that **closes** the drawer does, once.

The sheet **slides** in from its edge over about 440 milliseconds, in steps timed to what the panel
can complete. A step that does not move the sheet is not drawn. Closing is immediate, on every
screen: the sheet goes, the screen below is drawn once, and the device sends only the rows that
changed.

A drawer is refused while a blocking card is on the screen: the pairing passkey, a map transfer, and
the terminal update card.

## Hold to confirm

A destructive or irreversible action can require `Hold`. The screen must also declare `hold_fill` in its capabilities.

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

The input plane supplies progress from 0.0 through 1.0. The selected guarded row draws this progress.

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

Delete actions exist on specific detail or confirmation rows. A hold elsewhere does not delete data.

### Deleting things — the hold-to-delete footer

The delete footer is a guarded row. The action runs only after a complete hold on that row.

## Find a place

Find combines places within 10 km by air with places along the next 20 km of the accepted route,
within 300 m of its line. It takes four eligible nearby places and four from the corridor page,
alternates sources, and removes duplicate OSM identities. The corridor selection estimates arrival
using the same route occurrence as Visit and the straight distance from that point to the place.
This avoids a later pass being treated as an early stop on an overlapping route. Known-closed
places are excluded before these limits. The shared corridor page keeps its route order.

The shared Visit planner measures at most eight distinct candidates, one at a time. It stores each
measured route on the card and releases the planner before the next plan starts. The Finding
indicator stays visible through the complete batch. Its small compass turns by one third of a revolution once per
second without redrawing the map. Planning a suggestion does not activate a route or change the recording session. Up to four useful choices remain. A choice on the way adds at most 400 m to
the complete visit. A nearer alternative remains when its measured costs provide a useful choice.
Unknown ascent cannot eliminate a measured choice.

With an accepted route, a visit follows that route to the point nearest the place's access coordinate
within the next 20 km. Equal whole-metre distances use the first forward occurrence. Two
directed route searches connect that point to the place and back. The original route before and
after the excursion, including its waypoints and loops, stays in the visit. A place on the route
can have no return leg distance; guidance continues after the rider leaves the stop.

An imported route can differ from the road graph. At departure and return, a connection within
the normal 100 m snap limit retains both coordinates and counts toward the visit distance.
Its surface and elevation are unknown. A larger gap refuses the visit. The preview fits the path
through the place and back to the original route; the stored journey retains the full continuation.

The card shows route distance and ascent to arrival. Added costs compare the complete visit,
including its return, with the remaining accepted route. With no accepted route, the review is a
direct destination and has no return cost. Membership, order, map bounds, and cost origin stay fixed
while the cached inputs remain valid. The last category stays cached until another category is
calculated or the Assistant closes. A changed map, route, bike profile, clock authority, or stale
origin invalidates the choices. These bounded results do not establish a global nearest place.
**More places** opens the full paged category browser. Back returns to the category overview
without route calculation. The browser plans only the place selected for review.

Selecting a Find suggestion opens Visit review directly and loads its stored route without another
route calculation. With an active route, the action button offers **Add detour** or **Route here**.
Use Up or Down to switch; the card updates its geometry and costs before Select accepts it.
**Route here** plans from the current position to the place and replaces the current goal. It has
no return leg or original-route continuation. **Add detour** is the initial choice in every category.
With no active route, the card offers only **Route here**. Switching modes stays on the same card.
Back and reselect reuse the category's original route while its inputs remain valid. Closing
the Assistant removes unused routes. Restart removes abandoned previews while preserving accepted
checkpoint routes. More places retains the place detail page and plans the
selected place through the shared Visit planner. Ordinary service places without an explicit OSM approach
use normal coordinate destination routing. The review binds the exact map revision and actual
route endpoint. The preview draws the rider and destination pin above the route. When an explicit
approach is more than 100 m from the place's map coordinate, a dotted line connects the route end
to the pin. The pin can mark the center of a feature; the line does not describe a walking path.
A small pill shows the current opening interval when trusted hours are available. Otherwise it
shows that the hours are unknown or that the place is closed. A known-closed place, a changed source, or a stale origin
prevents acceptance. Missing elevation remains unknown. Acceptance
uses the shared durable Visit transaction. After the route is accepted, the device starts recording
if no ride session exists. An existing ride keeps its session and pause state. Browsing and
cancellation leave the active route intact.

Implementation: [Find preparation](src:firmware/obc-app/src/find_place.rs),
[Find and Visit review screens](src:firmware/obc-app/src/screen/find_place.rs).

## POI browser

The main POI menu contains water, campsite, lodging, resupply, pharmacy, bicycle-shop, and train categories. The schematic shows six example categories.

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

A category query uses a 50 km radius and returns pages of eight matching places. The list stores one application-owned page. Forward and reverse paging keep every result reachable. Known-closed places do not consume a result slot. Missing map coverage remains partial.

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

Page membership and order stay fixed while the list is open. Current opening status can make a place unavailable without selecting another place. The bearing arrow uses the latest fix and heading.

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

The detail screen reads the POI schedule once through `prepare`. It calculates today's open state from the current local clock.

## Settings

Settings screens use two focus levels. The row cursor selects a setting. Edit focus changes the selected value.

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

The application marks settings dirty when a value changes. `SettingsMachine` waits until the user leaves the Settings subtree before it requests a write. The host writes the snapshot through `SettingsStore` and reports the result.

The settings blob is independent of the SD card. The current UI languages are English, German, French, and Spanish.

Settings use layout version 20. The device rejects older or newer layouts and uses defaults. A valid current layout preserves the rider’s settings across updates.

The build generates a complete translation table from four TOML catalogs. The build fails if a catalog has missing or extra keys.

## Route cleanup

The device does not delete routes or rides automatically.
When a route upload fails because storage is full, the device offers a cleanup dialog.
Choose an age in weeks, then hold **Delete old routes** to confirm. Cancel is selected first.
The dialog reports completion, no matching routes, or a failure. Retry the upload after cleanup.

Age starts when the current route copy is uploaded under a trusted clock. Navigation does not
change this date. Cleanup scans the complete store, including routes beyond the visible menu.
It keeps the active route, routes with unknown dates, and routes newer than the selected age.
If the current date is unknown, use manual deletion from the Routes menu.

Ride archive proof controls the synced indicator. It never starts a deletion timer.
See [ride reconciliation](../companion-link/#reconciliation).

## Easier routes

Easier routes compares the remaining journey under the current bike profile. It runs one shared
baseline and two fixed trials for each goal: less climbing, smoother surfaces, and shorter distance.
These are bounded alternatives, not a claim that the router found a global optimum. The saved bike
profile and its road and surface prohibitions do not change.

Each trial passes through the remaining authored waypoints' on-route access points in order. It
keeps their display positions, names, categories, heights, offsets, and source references. A display
position beside the route does not become a visit destination. If all remaining records do not fit,
the comparison is unavailable. An active visit or an unresolved road avoidance also prevents it.

The comparison uses measured route geometry. It requires at least 50 m less ascent, 500 m less rough
surface, or 500 m less distance. Climb and surface choices can add at most 2 km or 25% of the remaining
distance, whichever is greater. Surface and distance choices can add at most 100 m or 25% of the
remaining ascent. Required elevation facts must be complete. A smoother choice must use comparable
map attribution and cannot increase the distance with unknown surface.

Only useful, distinct choices appear. The camera stays fixed while the rider compares the magenta
current route and blue proposed route. The review shows the saving and a Current/New table. Back
keeps the selected choice. **Use this route** starts the existing Navigator acceptance process; it
checks the exact sources and the rider's position again. Recording continues through acceptance.
Reopening the comparison uses the accepted route as the new current journey.

The trials run one at a time in the existing planner arena. The app keeps only small result
records. It releases each trial before starting the next. Opening a selected review reconstructs
that one route from the same frozen sources and verifies its measured costs and payload checksum.

## Runtime boundaries

Input logic and drawing receive different data views.

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

The screen table also declares whether a screen needs the map reader. A map screen needs it each frame.

The POI list and detail screens need the reader only until their one-shot data is ready. Other chrome screens do not build a reader.

## Repaint policy

The application renders on demand. `Dirty` separates base-frame changes from transient overlay changes.

- `map` requests a base-frame render.
- `overlay` requests a transient overlay render.
- `region` can limit a base-frame update to one rectangle.

A static screen with no new input, data, or timer event does not render. A scrolling name is a
region update of its text row, and its next step is the only timer it arms.

### The render key

Each screen row **declares a render-key kind** — the name of the facts its drawing reads. The frame
**builds the key** from that declaration: it reads the named facts out of the current state and
returns their exact values.

Each kind names what its screen draws. The Map names the camera, the fix, the pan mode, the
route-relative chrome and the low-battery cue. The
riding grid names the ride readouts and the live sensor values of the fields the rider pinned. The
Climb view names the climb and the cursor on it. The Up-ahead timeline names the progress its rows
measure from. Home names the battery level, the connected indicator, and the screensaver backdrop. A
screen whose content moves only on input declares no facts of its own.

One frame builds the visible screens' key before its work and again after it. A changed key requests
a base-frame render. The rule this keeps is per screen, not per screen class: a heart-rate reading
repaints the grid that shows it and not the map beside it.

Five kinds of change cannot move a key, and each asks for its render directly. A host feeds some
data between two frames, so the change is already in both keys. A screen keeps its own selection and
scroll position, so each recognized gesture requests a render. The card scheduler answers for the
cards it owns. A planner landing rewrites the screen stack. Some resident data — the catalogs, the
derived route data — no row names. Over-redraw is safe. Under-redraw is a defect.

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

The high-priority input plane recognizes gestures and advances hold-feedback state.
The map plane draws that feedback, renders screens, and owns panel output.
See the [board presenter](src:firmware/obc-fw-nrf54l/src/map_plane.rs).

The overlay presenter reads the clean base frame, adds the overlay, and presents the result. It leaves the base frame unchanged.

## Screens the companion link pushes

The companion can open modal cards for pairing, route updates, trip updates, and warnings.

The card scheduler assigns a fixed priority to each card type. A new card does not replace a hold in progress.

### The passkey card

The passkey card shows the six-digit pairing code. The rider cannot dismiss it before pairing ends.

### The Sensors screen

The Sensors settings screen shows heart-rate, power, and cadence sensor slots. It also opens the sensor scan list.

## Riding data

### Climbs

The route processor supplies climb segments and profiles. The Climb screen reads the active segment and its resident profile.

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

The riding-view cycle contains Map and Statistics. It also contains Climb when a climb is active and Climb mode is on.

### Waypoints

The route file supplies route-ordered waypoints. Navigator tracks the next waypoint from matched route progress.

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

The map shows waypoint markers and an approach chip. Statistics shows waypoint progress and configured values.

### Up ahead

The Up-ahead view merges route waypoints with map POIs near the route.

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

The corridor query sorts results by distance along the route. It excludes POIs behind the snapshot anchor.
A continuous pass within the configured radius of a place produces one encounter, at the nearest
point on that pass. Equal distances keep the earlier route position and its side. Small bends
within a pass do not add rows. Leaving the radius and returning produces a later encounter.
Passes can cross route-chunk and page boundaries. The query completes the nearest-point calculation
before it publishes the encounter, so changing pages does not change its identity or position.


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

The merge walks both sorted inputs. It does not allocate or copy list rows.

A category filter changes the corridor snapshot key. A source scope selects waypoints, map POIs, or
both. The rider sets both from the view's own contextual drawer, which replaced an in-view mode the
`Select` hold used to open. The filter is a selection that starts again at "Everything" each time the
view opens; the source scope is stored.

A cursor the rider set counts only against the list they set it in. While the list shows something
else, the cursor is the first row still ahead; it comes back if the rider sets the controls back.
Without this a rider who scrolls and then filters lands on the last match instead of the nearest
one.

Replacing active route geometry clears the corridor snapshot. The open view keeps its frozen progress anchor and requests new rows from the replacement route. A card that covers the view does not by itself clear the snapshot.

Configured `Next: category` fields use cached per-category corridor results. A visible Up-ahead screen has priority over these background requests.

## What is next on the accepted route

The Assistant overview uses one frozen interval: the entry position through the next 5 km or 10 km,
clipped at the end of the accepted journey. An accepted visit's approach, stop, return, and original
tail share this distance axis. A leg boundary is not the destination. Select opens the timeline;
Select-hold refreshes the anchor. Replacing the route or map invalidates the view and requires a
refresh.

The overview reads ascent and descent from measured route facts. Missing elevation produces an
unknown value. The existing elevation profile supplies the chart and its measured grade colors;
missing spans remain gaps. The overview selects the climb at the frozen anchor, or the next climb
that starts inside the interval. Its climb length and gain describe the whole climb, including any
part after the window. The next authored waypoint can be after the window. Generic and categorized
waypoints retain their stored name and category.

Water and resupply summaries share the interval and use current trusted opening hours. A completed
empty query differs from missing coverage or failed reads. A known-closed service is not an
available choice.

The timeline has a four-row display page. It streams the complete authored waypoint section and
uses the existing corridor query's continuation keys for map places. The riding waypoint cache is
not changed. Distance and ascent figures refer to the frozen window start; a separate passed cue
uses current progress. Page boundaries use route occurrence and source identity, so ties and later
passes through the same place remain distinct. Previous pages and Back from detail preserve the
selection.

The timeline's context drawer selects category and source. Category starts at Everything on a fresh
entry; source is the stored preference. Train is a service category. Generic waypoints appear only
under Everything; climbs appear only under Everything with both sources enabled. Known-closed
unselected places leave the page without reordering surviving rows. A selected closed place remains
visible but cannot start a Visit. Map places use the shared place detail and Visit review. Authored
waypoint details do not offer Add stop.

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

Home opens the main menu. A route selection opens its overview. Start uses `Root(Map)` to create a clean ride stack.

During a ride, Back cycles through riding views. Press pauses. A Down plus Back squeeze raises the
ride context sheet. Back-hold opens the main menu from any of them.

Map Inspect uses Select-hold to enter. Back exits Inspect before it changes riding views. Inside
Inspect a Select tap walks the mode ring: route movement, free movement, then zoom.

Idle return removes abandoned chrome. It returns to Home when idle and to Map during an active ride.

## Visual vocabulary

Screens use shared primitives for titles, lists, rows, bands, tiles, text, and status indicators.

The `chrome`, `rows`, `list`, and `tiles` modules contain composable drawing parts. The `band`,
`spinner`, `pager`, and `marquee` modules each own one shared mechanism. The `fmt` module owns
shared quantity formatting.

The `marquee` module fits a long name into its field. A name that does not fit is cut with `..`.
One name per frame scrolls instead: the highlighted row of a list, the title of a detail screen or
card, the selected peak in the Peak View ledger. The draw names it with the text row it occupies,
and the UI runtime steps it by one character every 250 ms after a 1 s rest at the head, rests 1.5 s
at the tail, and returns to the head. Every font is monospace, so a step draws a different
substring and no new primitive. The riding view's waypoint tile scrolls once when the name changes
and then rests at the head, so nothing moves on the panel while the rider rides.

`ActionRows` owns card-row selection, wrapping, Back dismissal, Press or Hold activation, guard
state, and row drawing. A card screen maps `CardEvent` to typed domain work and owns its body
layout. Cards compose the existing `chrome` helpers. There is no universal card-body or frame
abstraction.

`draw_rows` is for selected, actionable lists. A read-only timeline can compose `list_frame` and
`scrollbar` without inventing a selection.

Overlays and screen-specific drawing layers stay local. The Climb grade renderer is different from
the shared elevation band.

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

Palette constants use RGB565. The framebuffer converts them to the device's 64-color RGB222 gamut.

## Source map

- Screen table, capabilities, contexts, and transitions: [`screen/mod.rs`](src:firmware/obc-app/src/screen/mod.rs)
- Gesture recognition: [`input.rs`](src:firmware/obc-app/src/input.rs)
- Input and overlay plane: [`input_plane.rs`](src:firmware/obc-app/src/input_plane.rs)
- Repaint state and UI runtime: [`dirty.rs`](src:firmware/obc-app/src/dirty.rs), [`render_key.rs`](src:firmware/obc-app/src/render_key.rs), [`ui_runtime.rs`](src:firmware/obc-app/src/ui_runtime.rs)
- Shared screen primitives: [`screen/vocab/`](src:firmware/obc-app/src/screen/vocab)
- Settings and translations: [`settings.rs`](src:firmware/obc-app/src/settings.rs), [`i18n/`](src:firmware/obc-app/i18n), [`i18n.rs`](src:firmware/obc-app/src/i18n.rs)
- POI and Up-ahead views: [`poi_list.rs`](src:firmware/obc-app/src/screen/poi_list.rs), [`poi_detail.rs`](src:firmware/obc-app/src/screen/poi_detail.rs), [`up_ahead.rs`](src:firmware/obc-app/src/screen/up_ahead.rs)
- Route cleanup: [`route_cleanup.rs`](src:firmware/obc-storage/src/flat/route_cleanup.rs)

See [system architecture](../architecture/) for the host loop. See [rendering pipeline](../rendering/) for pixel generation.
