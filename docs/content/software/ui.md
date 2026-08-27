---
title: UI system
description: Screen state, navigation, input, repaint policy, and rider-facing data views.
---

# The UI system

The UI is a `no_std`, allocation-free system for a 240×320-pixel display. It uses four buttons and immediate-mode drawing.

## Screen model

Each screen is one `Screen` enum variant. The variant owns its state by value.

A `screens!` table defines each variant and its `Caps`. The table generates input, drawing, and preparation dispatch.

`Caps` declares cross-cutting behavior. It covers base content, overlays, timers, holds, reader access, idle return, rain, catalog remapping, and the render key.

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
<figcaption>Each screen owns typed state. The <code>Screen</code> enum provides static dispatch without heap allocation.</figcaption>
</figure>

A normal screen implements these operations:

- `handle` reads one `Gesture` and returns a `Transition`.
- `draw` writes the current frame.
- `prepare` performs a required one-shot reader operation.

The `Ctx` input context contains mutable application state. The `Render` context contains read-only state and borrowed rendering resources.

## Navigation

The screen stack is a `heapless::Vec<Screen, 10>`. Home is always the first item.

<figure class="fig">
<svg viewBox="0 0 720 330" role="img" aria-label="A pipeline across the top: a gesture goes into the top screen's handle method, which returns a Transition, which apply runs against the stack. Below, the screen stack with Home locked at the bottom, then Map, then Up ahead on top. To the right, the six transitions are listed as stack operations: None stays, Push grows, Pop shrinks, Replace swaps the top, Root truncates to Home then pushes, and Home truncates to the root.">
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
    <rect x="40" y="200" width="150" height="34" rx="6" class="d-water" /><text class="d-label" x="115" y="222" text-anchor="middle" style="fill:#fff">Up ahead</text>
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
<figcaption>The top screen returns a transition. <code>apply</code> is the only function that changes the stack.</figcaption>
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

### Detour flow

The Detour command is available during route navigation. It requires a routing graph and a matched route position.

<figure class="fig">
<svg viewBox="0 0 720 262" role="img" aria-label="The detour flow in three panels. Panel one, the chooser: the magenta route with an orange inner stroke marking the skipped stretch ahead of the rider and a ring at the candidate rejoin point; Up and Down move the rejoin in 100-metre steps. Panel two, the preview: the planned detour drawn in blue around the skipped stretch, with a panel showing two signed cost figures, here plus 434 metres of distance and 47 metres less climbing. Panel three, the commit: the spliced line — ridden part, detour, and the original route from the rejoin — is written as an ordinary route file and adopted; guidance continues.">
  <defs>
    <marker id="aDT" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Choose a rejoin · preview the cost · commit the splice</text>

  <!-- panel 1: chooser -->
  <text class="d-sub" x="30" y="52" style="font-size:9px;fill:#6b7758">① chooser — Up/Down move the rejoin</text>
  <rect x="30" y="60" width="200" height="150" rx="9" style="fill:#ffffff;stroke:#aaaa55;stroke-width:1.4" />
  <path d="M55 195 C 90 160, 100 120, 130 100 C 160 80, 185 80, 210 72" fill="none" stroke="#ff00ff" stroke-width="5" />
  <path d="M95 152 C 110 130, 118 112, 130 100 C 143 91, 152 87, 162 84" fill="none" stroke="#ff5500" stroke-width="2.4" />
  <path d="M62 188 l8 12 l-8 -4 l-8 4 z" fill="#ff0000" />
  <circle cx="162" cy="84" r="7" fill="none" stroke="#000" stroke-width="2" />
  <text class="d-sub" x="120" y="200" style="font-size:8.5px">skipped stretch</text>
  <text class="d-sub" x="162" y="112" text-anchor="middle" style="font-size:8.5px">rejoin</text>
  <text class="d-sub" x="30" y="228" style="font-size:9px">±100 m steps · 600 m minimum</text>

  <!-- panel 2: preview -->
  <line class="d-flow" x1="238" y1="135" x2="258" y2="135" marker-end="url(#aDT)" />
  <text class="d-sub" x="266" y="52" style="font-size:9px;fill:#6b7758">② preview — the planned path + what it costs</text>
  <rect x="266" y="60" width="200" height="150" rx="9" style="fill:#ffffff;stroke:#aaaa55;stroke-width:1.4" />
  <path d="M291 195 C 326 160, 336 120, 366 100 C 396 80, 421 80, 446 72" fill="none" stroke="#ff00ff" stroke-width="5" />
  <path d="M331 152 C 346 130, 354 112, 366 100 C 379 91, 388 87, 398 84" fill="none" stroke="#ff5500" stroke-width="2.4" />
  <path d="M331 152 C 370 160, 410 130, 398 84" fill="none" stroke="#0000aa" stroke-width="3" />
  <rect x="280" y="178" width="150" height="20" rx="6" style="fill:#f3f0df;stroke:#3d3427;stroke-width:1" />
  <text x="291" y="192" style="font-family:var(--mono);font-size:9.5px;fill:#3d3427">+434 m</text>
  <path d="M394 194 l6 -9 l6 9 z" fill="#3d3427" />
  <text x="422" y="192" text-anchor="end" style="font-family:var(--mono);font-size:9.5px;fill:#3d3427">-47 m</text>
  <text class="d-sub" x="266" y="228" style="font-size:9px">what it costs: distance · climbing</text>

  <!-- panel 3: commit -->
  <line class="d-flow" x1="474" y1="135" x2="494" y2="135" marker-end="url(#aDT)" />
  <text class="d-sub" x="502" y="52" style="font-size:9px;fill:#6b7758">③ commit — the detour is the route</text>
  <rect x="502" y="60" width="200" height="150" rx="9" style="fill:#ffffff;stroke:#aaaa55;stroke-width:1.4" />
  <path d="M527 195 C 550 172, 558 152, 564 138 C 592 162, 630 132, 622 96 C 642 82, 662 76, 680 72" fill="none" stroke="#ff00ff" stroke-width="5" />
  <text class="d-sub" x="502" y="228" style="font-size:9px">an ordinary route file</text>
  <text class="d-sub" x="30" y="252" style="font-size:9px;fill:#6b7758">back cancels at any step · a failed plan suggests the one useful remedy: try a farther rejoin</text>
</svg>
<figcaption>The rider selects a rejoin point, reviews the result, and commits the new route.</figcaption>
</figure>

Up and Down move the rejoin point in 100 m steps. The minimum rejoin distance is 600 m.

A successful plan shows distance and climb differences. Commit stores the splice as the active route.

The splice keeps completed route geometry, adds the detour, and continues from the rejoin point. It then rebuilds route-derived data.

## Input

The device has Up, Down, Select, and Back buttons.

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="Four buttons on the left — Up and Down on one flank, Select and Back on the other — feed a shared Gestures recognizer in the middle, which also takes a millisecond clock. It emits five gestures on the right: Step of n, Press, Hold, Back, and BackHold.">
  <defs>
    <marker id="aU3" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">Four buttons → one recognizer → five gestures</text>

  <!-- left flank: Up / Down (step controls, auto-repeat) -->
  <text class="d-sub" x="86" y="52" text-anchor="middle" style="font-size:9px">left flank</text>
  <rect x="46" y="60" width="80" height="30" rx="7" class="d-panel-2" />
  <text class="d-label" x="86" y="80" text-anchor="middle">▲ Up</text>
  <rect x="46" y="96" width="80" height="30" rx="7" class="d-panel-2" />
  <text class="d-label" x="86" y="116" text-anchor="middle">▼ Down</text>
  <text class="d-sub" x="86" y="140" text-anchor="middle" style="font-size:9px">steps · auto-repeat</text>

  <!-- right flank: Select / Back (timed edges) -->
  <text class="d-sub" x="86" y="168" text-anchor="middle" style="font-size:9px">right flank</text>
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
</svg>
<figcaption>The shared recognizer converts four buttons into five gestures. The simulator and device use the same recognizer.</figcaption>
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
| Up + Select | Open or close the universal quick drawer |
| Down + Back | Open or close the current screen's contextual drawer |
| Up + Down | Reserved |
| Select + Back | Reserved |

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

The four riding views (Map, Statistics, Climb, and the paused page) share one table: Up ahead,
Detour, POIs, and Routes. A row that cannot act right now is drawn recessed and does nothing — the
Detour row without a route, without map routing data, or off the route. A row that can act replaces
the sheet with its screen, so one Back returns the rider to the riding view they squeezed from.

A row can also hold a **value** in place of a screen. Such a row slides the sheet to a nested
editor: `Up` and `Down` change the staged choice, `Select` writes it and returns to the row table,
and `Back` discards it. The editor keeps a mark on the choice that is already in effect. The sheet
becomes as high as the editor needs and goes back to its table height. The Up-ahead view declares
two such rows, Filter and Sources, and they are the only place these two controls are set.

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

Two cases draw the screen below again whatever it is, and both are cases where the sheet stops
purely covering. Every frame of a **page slide** does, because the two pages travel through the
narrow margin either side of the sheet, where the screen below shows; when the two pages differ in
height, the same drawing puts back the rows the shrinking sheet gives up. And the frame that
**closes** the drawer does, once.

The sheet **slides** in from its edge over about 440 milliseconds, in steps timed to what the panel
can complete. A step that does not move the sheet is not drawn. Closing is immediate, on every
screen: the sheet goes, the screen below is drawn once, and the device sends only the rows that
changed.

A drawer is refused while a blocking card is on the screen: the pairing passkey, a map transfer, and
the terminal update card.

## Hold to confirm

A destructive or irreversible action can require `Hold`. The screen must also declare `hold_fill` in its capabilities.

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="Top: a timeline showing Select pressed down. A release within the 200ms tap window yields a Press; a release after the window but before the 500ms hold threshold is a cancelled long-press and yields nothing; holding past 500ms yields a Hold the instant it crosses. Bottom: a Discard row filling left to right with a warning bar at 0 percent, 60 percent holding, and 100 percent commit.">
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
<figcaption>A guarded action runs only when the hold reaches its threshold. The live fill shows hold progress.</figcaption>
</figure>

The input plane supplies progress from 0.0 through 1.0. The selected guarded row draws this progress.

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
<figcaption>Delete actions use the same guarded-row contract. A hold on another row has no effect.</figcaption>
</figure>

Delete actions exist on specific detail or confirmation rows. A hold elsewhere does not delete data.

### Deleting things — the hold-to-delete footer

The delete footer is a guarded row. The action runs only after a complete hold on that row.

## POI browser

The main POI menu contains water, campsite, lodging, resupply, pharmacy, and bicycle-shop categories.

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
<figcaption>The POI browser has six fixed categories. A category query returns at most 16 nearby POIs.</figcaption>
</figure>

A category query returns the nearest 16 matching POIs. The list stores one application-owned snapshot.

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
<figcaption>The list keeps a fixed snapshot. Only the bearing arrow uses live position data.</figcaption>
</figure>

The snapshot does not change while the list is open. The bearing arrow uses the latest fix and heading.

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
  <text class="d-sub" x="308" y="208" style="font-size:9.5px">local clock → weekday + minute-of-day</text>
  <line class="d-flow" x1="540" y1="204" x2="580" y2="204" marker-end="url(#aPD)" />
  <text class="d-sub" x="590" y="208" style="font-size:9.5px;fill:#a9501c">is_open?</text>
  <text class="d-sub" x="308" y="226" style="font-size:9px">read live every frame — the one part that isn't frozen</text>
</svg>
<figcaption>The detail screen reads opening hours once. The bearing and distance remain live.</figcaption>
</figure>

The detail screen reads the POI schedule once through `prepare`. It calculates today's open state from the current local clock.

## Settings

Settings screens use two focus levels. The row cursor selects a setting. Edit focus changes the selected value.

<figure class="fig">
<svg viewBox="0 0 720 232" role="img" aria-label="Settings screens have two focus levels. In row focus, up and down move the amber row cursor, press flips a toggle or opens a value row's stepper, and back climbs one screen. Pressing a value row enters field focus, where up and down change the live field's value shown in an up-down arrow box, press advances to the next field, and back — or pressing past the last field — steps back out to row focus.">
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
    <text class="d-sub" x="60" y="130" style="font-size:10.5px">up / down — move the cursor</text>
    <text class="d-sub" x="60" y="152" style="font-size:10.5px">press &nbsp;— toggle / open a value</text>
    <text class="d-sub" x="60" y="174" style="font-size:10.5px">back &nbsp;— climb one screen up</text>
  </g>

  <!-- transitions -->
  <line class="d-flow" x1="304" y1="104" x2="416" y2="104" marker-end="url(#aU8)" />
  <text class="d-sub" x="360" y="96" text-anchor="middle" style="font-size:9px">press a value row</text>
  <line class="d-flow" x1="416" y1="150" x2="304" y2="150" marker-end="url(#aU8)" />
  <text class="d-sub" x="360" y="166" text-anchor="middle" style="font-size:9px">back / last field</text>

  <!-- Field focus -->
  <rect class="d-panel-2" x="418" y="46" width="262" height="160" rx="12" />
  <text class="d-label" x="438" y="70">Field focus</text>
  <path d="M452 80 l7 -9 l7 9 z" fill="#ffaa00" />
  <rect x="445" y="84" width="42" height="22" rx="4" class="d-muted" style="stroke:#ffaa00;stroke-width:1.5" />
  <text class="d-sub" x="466" y="99" text-anchor="middle" style="font-size:10px">2025</text>
  <path d="M452 110 l7 9 l7 -9 z" fill="#ffaa00" />
  <text class="d-sub" x="500" y="99" style="font-size:10px">box = the live field</text>
  <g font-family="var(--mono)">
    <text class="d-sub" x="438" y="130" style="font-size:10.5px">up / down — change the value</text>
    <text class="d-sub" x="438" y="152" style="font-size:10.5px">press &nbsp;— step to the next field</text>
    <text class="d-sub" x="438" y="174" style="font-size:10.5px">back &nbsp;— step out of the field</text>
  </g>
</svg>
<figcaption>A press moves focus between the row and its value. Up and Down change the focused value.</figcaption>
</figure>

The application marks settings dirty when the user leaves the Settings subtree. The host then saves the settings through `SettingsStore`.

The weather alert cooldown is not a setting. It is device state, and it has its own record with its own lifecycle. A firing alert writes only that record, and it writes it immediately: an open settings screen does not hold it back, and a change to a setting does not touch it.

The settings blob is independent of the SD card. The current UI languages are English, German, French, and Spanish.

A firmware update does not erase the stored settings. The blob format is append-only: a new setting is added at the end, and it carries the version that first wrote it. Stored fields never move. The device reads a stored blob at the blob's own version, and the fields added after that version take their defaults. Two cases reset the settings, and both are deliberate: a blob older than the oldest version whose exact bytes are committed as a reference, and a downgrade, where the stored blob is newer than the firmware and its layout is unknown.

The build generates a complete translation table from four TOML catalogs. The build fails if a catalog has missing or extra keys.

## Retention

Routes have individual retention values. Synced rides use one global retention setting.

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
<figcaption>Retention can delete data only after this boot receives a trusted clock.</figcaption>
</figure>

The retention sweep requires a trusted clock from this boot. GPS or the companion can establish this clock.

### The device has no clock — so deletion waits for a trusted one

The sweep does not delete the active route. It does not delete unsynced rides.

An unknown route-use time starts a new retention period. It does not cause immediate deletion.

## Runtime boundaries

Input logic and drawing receive different data views.

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
<figcaption>Input handling uses mutable <code>Ctx</code>. Drawing uses read-only <code>Render</code> data and borrowed render resources.</figcaption>
</figure>

The screen table also declares whether a screen needs the map reader. A map screen needs it each frame.

The POI list and detail screens need the reader only until their one-shot data is ready. Other chrome screens do not build a reader.

## Repaint policy

The application renders on demand. `Dirty` separates base-frame changes from transient overlay changes.

- `map` requests a base-frame render.
- `overlay` requests a transient overlay render.
- `region` can limit a base-frame update to one rectangle.

A static screen with no new input, data, or timer event does not render.

### The render key

Each screen row **declares a render-key kind** — the name of the facts its drawing reads. The frame
**builds the key** from that declaration: it reads the named facts out of the current state and
returns their exact values.

Each kind names what its screen draws. The Map names the camera, the fix, the pan mode, the
route-relative chrome, the low-battery cue, and, on the rain map alone, the selected rain frame. The
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
  <text x="555" y="232" text-anchor="middle" style="font-family:var(--mono);font-size:9px;fill:#e7ead8">map still updates underneath</text>
</svg>
<figcaption>An overlay composites over the clean frame. It does not change the stored base frame.</figcaption>
</figure>

The high-priority input plane recognizes gestures and draws hold feedback. The map plane handles screen logic and expensive map rendering.

The overlay presenter reads the clean base frame, adds the overlay, and presents the result. It leaves the base frame unchanged.

## Screens the companion link pushes

The companion can open modal cards for pairing, route updates, trip updates, warnings, and weather alerts.

The card scheduler assigns a fixed priority to each card type. A new card does not replace a hold in progress.

### The passkey card

The passkey card shows the six-digit pairing code. The rider cannot dismiss it before pairing ends.

### The Sensors screen

The Sensors settings screen shows heart-rate, power, and cadence sensor slots. It also opens the sensor scan list.

## Riding data

### Climbs

The route processor supplies climb segments and profiles. The Climb screen reads the active segment and its resident profile.

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
<figcaption>The climb view shows the current climb profile and four climb values.</figcaption>
</figure>

The riding-view cycle contains Map and Statistics. It also contains Climb when a climb is active and Climb mode is on.

### Waypoints

The route file supplies route-ordered waypoints. The ride engine tracks the next waypoint from matched route progress.

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
  <text class="d-sub" x="250" y="56" style="font-size:10.5px">the Statistics progress bar shares the route's distance axis</text>
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
<figcaption>Map, route, and statistics views use the same route-distance axis for waypoints.</figcaption>
</figure>

The map shows waypoint markers and an approach chip. Statistics shows waypoint progress and configured values.

### Up ahead

The Up-ahead view merges route waypoints with map POIs near the route.

<figure class="fig">
<svg viewBox="0 0 720 320" role="img" aria-label="Two panels comparing the device's two spatial queries. On the left, near me: a dashed circle drawn around the rider's fix, with points of interest scattered inside it and one greyed out beyond the edge; the route line crosses the panel faintly and plays no part. On the right, up ahead: the magenta route line runs through a pale band three hundred metres wide to each side, the rider sits on the line, and two points of interest inside the band are joined by dashed leaders to tick marks on the line itself, labelled with their along-route distances of one point two kilometres and four point eight kilometres; a third point outside the band is greyed out. Below each panel, a summary box: the left produces a browser list sorted by straight-line distance with a live bearing arrow, the right produces the Up ahead timeline sorted along the route, each row carrying a distance to go, a climb to go, and which side it sits on.">
  <text class="d-tag" x="20" y="24">Two spatial questions — one map, one index, two windows</text>

  <text class="d-sub" x="30" y="52" style="font-size:9px;fill:#6b7758">① near me — a disc around the fix</text>
  <rect class="d-panel" x="24" y="60" width="320" height="186" rx="11" />
  <!-- the route is present but irrelevant to this question -->
  <path d="M56 232 C 106 198, 146 188, 186 158 C 226 128, 258 108, 320 90" fill="none" stroke="#ff00ff" stroke-width="4" opacity="0.16" />
  <circle cx="184" cy="158" r="66" fill="none" stroke="#6b7758" stroke-width="1.3" stroke-dasharray="4 4" />
  <g fill="#3d3427">
    <circle cx="146" cy="124" r="5" /><circle cx="228" cy="140" r="5" />
    <circle cx="162" cy="200" r="5" /><circle cx="232" cy="196" r="5" />
  </g>
  <circle cx="300" cy="104" r="5" fill="none" stroke="#c9c7b8" stroke-width="1.4" />
  <text class="d-sub" x="292" y="92" text-anchor="end" style="font-size:8.5px;fill:#9aa884">out of range</text>
  <path d="M184 158 L222 143" stroke="#cf6a2a" stroke-width="1.6" />
  <path d="M228 140 l-11 -1 l5 6 z" fill="#cf6a2a" />
  <text class="d-sub" x="222" y="122" text-anchor="end" style="font-size:8.5px;fill:#a9501c">bearing</text>
  <path d="M184 150 l7 14 l-7 -5 l-7 5 z" fill="#ff0000" />
  <text class="d-sub" x="184" y="236" text-anchor="middle" style="font-size:8px">nearest 16 · by straight-line distance · route-blind</text>

  <text class="d-sub" x="382" y="52" style="font-size:9px;fill:#6b7758">② up ahead — a corridor along the route</text>
  <rect class="d-panel" x="376" y="60" width="320" height="186" rx="11" />
  <path d="M402 228 C 450 206, 468 170, 510 148 C 552 126, 578 100, 668 84" fill="none" stroke="#e6e0c4" stroke-width="34" stroke-linecap="round" />
  <path d="M402 228 C 450 206, 468 170, 510 148 C 552 126, 578 100, 668 84" fill="none" stroke="#ff00ff" stroke-width="4" />
  <!-- inside the corridor: projected onto the line -->
  <circle cx="492" cy="170" r="5" fill="#3d3427" />
  <path d="M492 170 L503 150" stroke="#6b7758" stroke-width="1.1" stroke-dasharray="3 3" />
  <circle cx="503" cy="150" r="2.8" fill="#000" />
  <text class="d-sub" x="512" y="180" style="font-size:9px">1.2 km</text>
  <circle cx="572" cy="92" r="5" fill="#3d3427" />
  <path d="M572 92 L566 114" stroke="#6b7758" stroke-width="1.1" stroke-dasharray="3 3" />
  <circle cx="566" cy="114" r="2.8" fill="#000" />
  <text class="d-sub" x="584" y="90" style="font-size:9px">4.8 km</text>
  <!-- outside the corridor -->
  <circle cx="636" cy="164" r="5" fill="none" stroke="#c9c7b8" stroke-width="1.4" />
  <text class="d-sub" x="644" y="180" text-anchor="middle" style="font-size:8.5px;fill:#9aa884">off corridor</text>
  <path d="M444 201 l8 13 l-8 -5 l-8 5 z" fill="#ff0000" transform="rotate(53 444 201)" />
  <text class="d-sub" x="392" y="236" style="font-size:8px">±300 m either side · only what is still ahead of you</text>

  <rect class="d-panel-2" x="24" y="262" width="320" height="48" rx="9" />
  <text class="d-sub" x="40" y="282" style="font-size:9px">→ the <tspan class="d-label" style="font-size:9px">POIs browser</tspan> — rows by distance,</text>
  <text class="d-sub" x="52" y="298" style="font-size:9px">with a live bearing arrow</text>
  <rect class="d-hot" x="376" y="262" width="320" height="48" rx="9" style="fill:#f8efe4" />
  <text class="d-sub" x="392" y="282" style="font-size:9px">→ the <tspan class="d-label" style="font-size:9px;fill:#a9501c">Up ahead timeline</tspan> — rows along the route,</text>
  <text class="d-sub" x="404" y="298" style="font-size:9px">with distance-to-go, climb-to-go and a side</text>
</svg>
<figcaption>Nearby POIs use geographic distance. Up-ahead entries use distance along the route.</figcaption>
</figure>

The corridor query sorts results by distance along the route. It excludes POIs behind the snapshot anchor.

<figure class="fig">
<svg viewBox="0 0 720 192" role="img" aria-label="One timeline row dissected, plus the source cue legend. The row is amber because it is under the cursor: line one carries a category icon with a small diamond pip beside it and the ellipsized name Fontaine du port; line two carries the distance to go, a climb to go prefixed by an up triangle, and at the right edge a left-pointing triangle followed by 271 metres, the off-route side hint. To the right, four icon states: a map POI unselected in muted olive, a map POI under the cursor in ink, a custom waypoint in amber with a pip, and a custom waypoint under the cursor in ink with a pip.">
  <text class="d-tag" x="20" y="24">The row · the source cue</text>

  <!-- the row (selected: amber) -->
  <rect x="24" y="44" width="392" height="66" rx="6" class="d-amber" />
  <circle cx="52" cy="70" r="7" fill="#000" /><path d="M45 67 l7 -9 l7 9 z" fill="#000" />
  <path d="M66 56 l4 4 l-4 4 l-4 -4 z" fill="#000" />
  <text x="80" y="76" font-family="var(--mono)" style="font-size:12.5px;fill:#000">Fontaine du port</text>
  <text x="34" y="100" font-family="var(--mono)" style="font-size:10px;fill:#000">219m</text>
  <path d="M228 93 l5 -7 l5 7 z" fill="#000" />
  <text x="242" y="100" font-family="var(--mono)" style="font-size:10px;fill:#000">13m</text>
  <path d="M352 96 l9 -5 l0 10 z" fill="#000" />
  <text x="406" y="100" text-anchor="end" font-family="var(--mono)" style="font-size:10px;fill:#000">271m</text>

  <!-- callouts -->
  <text class="d-sub" x="24" y="128" style="font-size:9px;fill:#a9501c">icon + pip</text>
  <text class="d-sub" x="96" y="128" style="font-size:9px;fill:#a9501c">name, ellipsized to fit</text>
  <text class="d-sub" x="24" y="142" style="font-size:9px">distance-to-go</text>
  <text class="d-sub" x="214" y="142" style="font-size:9px">climb-to-go</text>
  <text class="d-sub" x="416" y="142" text-anchor="end" style="font-size:9px">side, past 50 m</text>

  <!-- source cue legend -->
  <text class="d-tag" x="444" y="60">the icon says which source</text>
  <g>
    <circle cx="460" cy="80" r="6" fill="#6b7758" /><text class="d-sub" x="478" y="84" style="font-size:9.5px">map POI</text>
    <circle cx="460" cy="102" r="6" fill="#24331c" /><text class="d-sub" x="478" y="106" style="font-size:9.5px">map POI · cursor</text>
    <circle cx="460" cy="124" r="6" fill="#ffaa00" /><path d="M472 116 l3.5 3.5 l-3.5 3.5 l-3.5 -3.5 z" fill="#ffaa00" />
    <text class="d-sub" x="484" y="128" style="font-size:9.5px">waypoint</text>
    <circle cx="460" cy="146" r="6" fill="#24331c" /><path d="M472 138 l3.5 3.5 l-3.5 3.5 l-3.5 -3.5 z" fill="#24331c" />
    <text class="d-sub" x="484" y="150" style="font-size:9.5px">waypoint · cursor</text>
  </g>
  <text class="d-sub" x="444" y="172" style="font-size:8.5px;fill:#6b7758">the pip is the colourblind-safe half of the pair</text>
</svg>
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

Configured `Next: category` fields use cached per-category corridor results. A visible Up-ahead screen has priority over these background requests.

## Main rider flow

<figure class="fig">
<svg viewBox="0 0 900 360" role="img" aria-label="A navigation graph. Home opens the main compass Menu. Its Routes station opens the Route menu, a route pick opens Overview, and START roots to Map. Map, Statistics, and Climb form the riding-view back ring. A Down plus Back squeeze from a riding view or Paused raises the ride context sheet without changing activity mode; its first row opens the Up ahead timeline, and the other rows are Detour, POIs, and Routes. Back-hold from anywhere opens the main menu. In Inspect, Back tap returns to Map, a Select tap walks the mode ring of Route move, Free move and Zoom, and Select-hold changes an active Free axis but is inert in Zoom. Press from Map pauses; Resume returns and held Finish or Discard clears to Home.">
  <defs>
    <marker id="aU7" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#5f7d3d" /></marker>
    <marker id="aU7c" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">The screen flow</text>

  <!-- nodes -->
  <rect class="d-panel" x="30"  y="160" width="100" height="40" rx="9" /><text class="d-label" x="80"  y="180" text-anchor="middle">Home</text><text class="d-sub" x="80" y="193" text-anchor="middle" style="font-size:8.5px">root</text>
  <rect class="d-panel-2" x="210" y="50" width="116" height="40" rx="9" /><text class="d-label" x="268" y="70" text-anchor="middle">Route menu</text><text class="d-sub" x="268" y="83" text-anchor="middle" style="font-size:8.5px">pick a route</text>
  <rect class="d-panel-2" x="220" y="160" width="104" height="40" rx="9" /><text class="d-label" x="272" y="180" text-anchor="middle">Overview</text><text class="d-sub" x="272" y="193" text-anchor="middle" style="font-size:8.5px">track · profile · stats</text>
  <rect class="d-panel-2" x="210" y="270" width="116" height="40" rx="9" /><text class="d-label" x="268" y="290" text-anchor="middle">Main menu</text><text class="d-sub" x="268" y="303" text-anchor="middle" style="font-size:8.5px">compass · 6 stations</text>
  <rect class="d-hot" x="410" y="50" width="104" height="40" rx="9" style="fill:#f8efe4" /><text class="d-label" x="462" y="74" text-anchor="middle" style="fill:#a9501c">Paused</text>
  <rect class="d-forest" x="410" y="160" width="104" height="40" rx="9" /><text class="d-label" x="462" y="184" text-anchor="middle" style="fill:#fff">Map</text>
  <rect class="d-panel-2" x="410" y="270" width="104" height="40" rx="9" /><text class="d-label" x="462" y="294" text-anchor="middle">Inspect</text>
  <rect class="d-panel-2" x="585" y="50" width="130" height="40" rx="9" /><text class="d-label" x="650" y="70" text-anchor="middle">Ride context</text><text class="d-sub" x="650" y="83" text-anchor="middle" style="font-size:8.5px">bottom sheet · 4</text>
  <rect class="d-panel-2" x="755" y="50" width="110" height="40" rx="9" /><text class="d-label" x="810" y="70" text-anchor="middle">Up ahead</text><text class="d-sub" x="810" y="83" text-anchor="middle" style="font-size:8.5px">merged timeline</text>
  <rect class="d-water" x="585" y="160" width="115" height="40" rx="9" /><text class="d-label" x="642" y="180" text-anchor="middle" style="fill:#fff">Statistics</text><text class="d-sub" x="642" y="193" text-anchor="middle" style="fill:#dfe6e0;font-size:8.5px">elevation</text>
  <rect class="d-water" x="755" y="160" width="110" height="40" rx="9" /><text class="d-label" x="810" y="180" text-anchor="middle" style="fill:#fff">Climb</text><text class="d-sub" x="810" y="193" text-anchor="middle" style="fill:#dfe6e0;font-size:8.5px">on a climb</text>

  <!-- edge from Home: both press and back-hold open the Menu (the single door in) -->
  <line x1="130" y1="180" x2="208" y2="286" stroke="#5f7d3d" stroke-width="1.6" marker-end="url(#aU7)" /><text class="d-sub" x="136" y="238" style="font-size:9px">press · back-hold</text>
  <!-- Menu -> Route menu -->
  <line x1="334" y1="274" x2="334" y2="92" stroke="#5f7d3d" stroke-width="1.6" marker-end="url(#aU7)" /><text class="d-sub" x="340" y="140" style="font-size:9px">Routes</text>
  <!-- Route menu -> Overview (press) -->
  <line x1="268" y1="90" x2="272" y2="158" stroke="#5f7d3d" stroke-width="1.6" marker-end="url(#aU7)" /><text class="d-sub" x="230" y="126" style="font-size:9px">press</text>
  <!-- Overview -> Map (START/Root) -->
  <line x1="324" y1="180" x2="408" y2="180" stroke="#cf6a2a" stroke-width="2" marker-end="url(#aU7c)" /><text class="d-sub" x="344" y="171" style="fill:#a9501c;font-size:9px">START · Root</text>
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
  <line x1="472" y1="270" x2="472" y2="202" stroke="#5f7d3d" stroke-width="1.4" marker-end="url(#aU7)" /><text class="d-sub" x="480" y="246" style="font-size:8.5px">back exits</text>
  <!-- Ride-context access: the same squeeze from every riding view and from Paused; no activity-mode write. -->
  <line x1="514" y1="70" x2="583" y2="70" stroke="#5f7d3d" stroke-width="1.5" marker-end="url(#aU7)" /><text class="d-sub" x="548" y="61" text-anchor="middle" style="font-size:8.5px">down+back</text>
  <line x1="642" y1="160" x2="642" y2="92" stroke="#5f7d3d" stroke-width="1.5" marker-end="url(#aU7)" /><text class="d-sub" x="650" y="128" style="font-size:8.5px">down+back</text>
  <path d="M514 164 C 538 116, 558 94, 585 82" fill="none" stroke="#5f7d3d" stroke-width="1.4" marker-end="url(#aU7)" />
  <path d="M755 164 C 738 124, 726 94, 715 82" fill="none" stroke="#5f7d3d" stroke-width="1.4" stroke-dasharray="4 4" marker-end="url(#aU7)" />
  <!-- The first context row opens the route-ordered Up-ahead timeline. -->
  <line x1="717" y1="70" x2="753" y2="70" stroke="#5f7d3d" stroke-width="1.5" marker-end="url(#aU7)" /><text class="d-sub" x="735" y="61" text-anchor="middle" style="font-size:8.5px">press</text>
  <!-- The global escape keeps the full app one Back-hold away from every screen. -->
  <path d="M462 210 C 440 330, 398 340, 326 300" fill="none" stroke="#5f7d3d" stroke-width="1.4" marker-end="url(#aU7)" /><text class="d-sub" x="380" y="338" style="font-size:8.5px">back-hold (from anywhere)</text>
  <!-- Paused -> Home (finish/discard) -->
  <path d="M410 60 C 260 12, 82 70, 80 158" fill="none" stroke="#cf6a2a" stroke-width="1.6" stroke-dasharray="4 4" marker-end="url(#aU7c)" /><text class="d-sub" x="236" y="28" style="fill:#a9501c;font-size:9px">Finish / Discard (hold) → Home</text>
</svg>
<figcaption>The screen graph keeps Home as the root. Ride views and menus use explicit transitions.</figcaption>
</figure>

Home opens the main menu. A route selection opens its overview. Start uses `Root(Map)` to create a clean ride stack.

During a ride, Back cycles through riding views. Press pauses. A Down plus Back squeeze raises the
ride context sheet. Back-hold opens the main menu from any of them.

Map Inspect uses Select-hold to enter. Back exits Inspect before it changes riding views. Inside
Inspect a Select tap walks the mode ring: route movement, free movement, then zoom.

Idle return removes abandoned chrome. It returns to Home when idle and to Map during an active ride.

## Visual vocabulary

Screens use shared primitives for titles, lists, rows, bands, tiles, text, and status indicators.

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
- Retention policy: [`retention.rs`](src:firmware/obc-app/src/retention.rs)

See [system architecture](../architecture/) for the host loop. See [rendering pipeline](../rendering/) for pixel generation.
