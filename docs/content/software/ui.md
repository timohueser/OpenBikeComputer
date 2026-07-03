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
<svg viewBox="0 0 720 300" role="img" aria-label="On the left, the Screen enum lists its variants: Home, Map, Statistics, RideControl, Menu, RouteMenu, RouteSwap, plus the Settings tree. The Map variant points to its module on the right, which holds typed state, a handle method returning a Transition, and a draw method emitting pixels. A tag notes static match dispatch, no dyn and no allocation.">
  <defs>
    <marker id="aU1" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">A screen is a value — no retained widget tree</text>

  <!-- enum Screen -->
  <rect class="d-panel" x="36" y="44" width="210" height="210" rx="11" />
  <text class="d-label" x="56" y="66">enum Screen</text>
  <g font-family="var(--mono)">
    <rect x="52" y="78"  width="178" height="22" rx="5" class="d-hot-fill" /><text class="d-sub" x="62" y="93" style="fill:#fff">Map(MapScreen)</text>
    <rect x="52" y="104" width="178" height="20" rx="5" class="d-muted" /><text class="d-sub" x="62" y="118">Home(HomeScreen)</text>
    <rect x="52" y="126" width="178" height="20" rx="5" class="d-muted" /><text class="d-sub" x="62" y="140">Statistics(…)</text>
    <rect x="52" y="148" width="178" height="20" rx="5" class="d-muted" /><text class="d-sub" x="62" y="162">RideControl(…)</text>
    <rect x="52" y="170" width="178" height="20" rx="5" class="d-muted" /><text class="d-sub" x="62" y="184">Menu(…)</text>
    <rect x="52" y="192" width="178" height="20" rx="5" class="d-muted" /><text class="d-sub" x="62" y="206">RouteMenu(…)</text>
    <rect x="52" y="214" width="178" height="20" rx="5" class="d-muted" /><text class="d-sub" x="62" y="228">RouteSwap(…)</text>
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
// The one screen table. Each row declares a variant, its state type, and its kind; a dumb local
// macro expands it into the Screen enum, the handle/draw delegation matches, and Screen::kind().
screens! {
    Home(HomeScreen) => Nav,
    Map(MapScreen) => Riding,
    Statistics(StatisticsScreen) => Riding,
    RideControl(RideControl) => Overlay,   // the pause menu — draws over the still-visible map
    Menu(MenuScreen) => Nav,
    RouteMenu(RouteMenuScreen) => Nav,
    RouteSwap(RouteSwapScreen) => Nav,
    // The Settings tree — a list plus one screen each for Date & Time, Units, Power, Reset, and
    // Stats (which opens onto Fields → AddField, the stat-grid panel manager). The `Settings`
    // kind is what holds the debounced settings save while one of these is on top.
    Settings(SettingsScreen) => Settings,  DateTime(DateTimeScreen) => Settings,
    Units(UnitsScreen) => Settings,        Stats(StatsScreen) => Settings,
    StatFields(StatFieldsScreen) => Settings, AddField(AddFieldScreen) => Settings,
    Power(PowerScreen) => Settings,        Reset(ResetScreen) => Settings,
}

// Each variant is a module with typed state and exactly two methods:
impl MenuScreen {
    fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition { /* logic  */ }
    fn draw(&self, cv: &mut impl Surface, rx: &mut Render)       { /* pixels */ }
}
```

## Navigation is a return value

A screen never reaches out and changes the UI. It *returns* what it wants — a `Transition` — and a tiny `apply` function runs that against the screen **stack** (a `heapless::Vec<Screen, 8>`). The bottom of the stack is always Home, which is never popped, so `back` always has somewhere to go and the stack can never empty.

<figure class="fig">
<svg viewBox="0 0 720 330" role="img" aria-label="A pipeline across the top: a gesture goes into the top screen's handle method, which returns a Transition, which apply runs against the stack. Below, the screen stack with Home locked at the bottom, then Map, then Menu on top. To the right, the six transitions are listed as stack operations: None stays, Push grows, Pop shrinks, Replace swaps the top, Root truncates to Home then pushes, and Home truncates to the root.">
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
    <rect x="40" y="200" width="150" height="34" rx="6" class="d-water" /><text class="d-label" x="115" y="222" text-anchor="middle" style="fill:#fff">Menu</text>
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

Here's a whole screen's logic — the main Menu — to show how little a screen has to say:

```rust
fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
    match g {
        Gesture::Turn(n) => { self.selected = step_selection(self.selected, n, ITEMS.len()); Transition::None }
        Gesture::Press   => match self.selected {
            0 => Transition::Push(Screen::RouteMenu(RouteMenuScreen::new())), // Routes
            _ => Transition::Push(Screen::Settings(SettingsScreen::new())),   // Settings
        },
        Gesture::Back    => Transition::Pop, // return to whoever opened the Menu
        _ => Transition::None,
    }
}
```

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

That the recognizer is fed by an injected `InputSource` is the key boundary: on the device that source is the encoder driver and GPIO edges; in the simulator it's the control-panel knob and your keyboard. Neither the recognizer nor any screen knows which. (The location, altimeter and track sinks cross the same kind of [HAL seam](../architecture/#two-hosts-one-core-and-the-seams-between-them).)

## Hold to confirm

Some actions are irreversible — finishing or discarding a ride. Rather than a modal "are you sure?", the UI uses a **guarded-action** pattern that's reusable across screens: a guarded option fires only on a *completed* `Hold`, and its row fills with a warning bar tracking the live hold-progress. Let go early and nothing happens — the recognizer makes that clean at the gesture level: a press is a `Press` only if released within a brief tap window (~200 ms); released *after* the window but *before* the hold completes, it's a **cancelled long-press** that yields nothing, never a surprise tap.

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

## Settings: a second level of focus

Most screens have one focus: the row cursor. The **Settings** screens — Date & Time, Units, Stats, Power, and the factory Reset — add a *second* level. A value isn't a separate sub-screen; it's edited **in place**. Rotating still moves the amber row cursor, but once you press a value row, focus drops *into* a field: a `▲▼` box marks the live one, rotating now changes *its* value, pressing steps to the next field, and back steps out. The same two-level model drives every editor — a date, a UTC offset, a fix interval — and the same toggle row flips GPS-set-time or the power saver. No new gestures; the existing five just mean different things at each level.

The **Stats** screen configures the riding grid itself. Its *Page cycle* row sets how fast the grid auto-flips between pages; *Fields* opens a manager for the data panels. The grid draws from a predefined, in-code catalogue of fields (speed, distance, climb, grade, elevation, clock, …) — the rider picks which to show and in what order, and a field is either one column or a full-width two. Reordering reuses the grab idiom: press *lifts* a panel (it holds pinned mid-screen while the rest slide past it), rotating moves it through the order — a two-column panel always begins a row — and press drops it. A panel is removed by a **hold-to-delete** bar, the same guarded hold as Reset's. The chosen panels lay out six to a page (3×2) and auto-cycle on the timer, so a long list stays glanceable. The selection and period live in the same persisted `Settings` value, so they survive a reboot like every other setting.

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

A setting is worthless if it's forgotten on power-off, so settings persist. The values live in a small `Settings` value (`Copy`, no floats) that the screens edit; the *medium* is a host concern behind one more trait, exactly like the sensor seams. The app seeds itself from `load()` at boot and asks the host to `save()` only when something actually changed — detected by the same one-`==` before/after compare the camera uses to decide it's dirty.

```rust
pub trait SettingsStore {
    fn load(&mut self) -> Option<Settings>; // None (blank/corrupt) → start from Settings::default()
    fn save(&mut self, s: &Settings);       // encode the blob, persist it
}
```

The simulator writes the blob to a file; the device writes it to a reserved slice of the nRF54L's on-chip **RRAM** — its program memory is RRAM, which is byte-writable with no flash-style erase cycle, so a tiny key-value store is cheap and needs no SD card present. Both sides share one versioned, CRC-checked byte codec, so a blank or corrupted read cleanly falls back to defaults rather than loading garbage — and the factory Reset is just writing the default blob back.

## Logic and drawing get different views of the world

`handle` and `draw` are handed deliberately different contexts. `handle` gets `Ctx` — the **mutable** slice of app state a screen is allowed to change (the camera, the ride mode, the clock). `draw` gets `Render` — a **read-only** view plus the resources it needs to paint (the map reader, the renderer, the active route, its elevation profile, the breadcrumb, the in-flight hold-progress). A screen literally cannot mutate state while drawing, because it isn't given the means to.

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

- **map** — the screen stack. Set when a gesture was applied, when a fresh GPS fix moved the camera *on a view that shows live data*, when the route or ride session changed, when a screen's own timed `animate` reported a change — the Statistics cursor springing back to the live position, or the Home screensaver's wall clock crossing into a new minute — or, on a riding view, when the GPS fix goes stale or returns (raising or clearing the "No GPS Fix" banner, a timer edge surfaced from the per-frame `tick`). A parked Home screen fires none of these — neither the first three nor the two timed edges, which are gated to the live-data views — so between those once-a-minute clock ticks the map stays clean.
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

## Overlays composite over a live map

Most navigation *replaces* the view, but one screen — Ride control, the pause menu — draws *over* the still-visible map. The stack supports this directly: `render_map` finds the topmost **opaque** screen and draws from there upward, so an overlay composites on top of whatever is beneath it. And because a paused ride still receives fixes, the map keeps moving under the pause panel.

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="On the left, the stack: Home at the bottom, Map above it marked opaque, and RideControl on top marked overlay. An arrow shows render_map starts from the topmost opaque screen, Map, and draws upward. On the right, a device screen mock: the map fills it, with a PAUSED panel floating over the lower half, and a note that the map still updates underneath.">
  <text class="d-tag" x="20" y="24">An overlay draws over the screen below it</text>

  <!-- stack -->
  <g>
    <rect x="48" y="58" width="170" height="34" rx="6" class="d-hot" style="fill:#f8efe4" />
    <text class="d-label" x="133" y="80" text-anchor="middle" style="fill:#a9501c">RideControl</text>
    <text class="d-sub" x="232" y="79" style="font-size:9px">overlay</text>
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
  <!-- paused panel -->
  <rect x="500" y="120" width="110" height="86" rx="7" style="fill:#f3f0df;stroke:#2c5230;stroke-width:1.5" />
  <rect x="500" y="120" width="110" height="18" rx="4" style="fill:#2e251a" />
  <text class="d-sub" x="555" y="133" text-anchor="middle" style="fill:#fff;font-size:9px">PAUSED</text>
  <rect x="506" y="144" width="98" height="16" rx="3" class="d-amber" /><text class="d-sub" x="512" y="156" style="font-size:8.5px">Resume</text>
  <text class="d-sub" x="512" y="174" style="font-size:8.5px">Finish</text>
  <text class="d-sub" x="512" y="190" style="font-size:8.5px">Discard</text>
  <text class="d-sub" x="555" y="232" text-anchor="middle" style="font-size:9px">map still updates underneath</text>
</svg>
<figcaption>Only Ride control is an overlay; everything else is opaque and replaces the view. The same mechanism that draws the pause panel over the map is what lets the host keep folding in GPS fixes behind it — the ride doesn't visually freeze just because you opened the menu.</figcaption>
</figure>

## The whole flow

Put the pieces together and the navigation graph is small and legible. Two screens are **riding views** — the Map and the Elevation/Statistics profile — and they're siblings: `back` swaps between them without growing the stack, and both share the same `press` (pause) and `back-hold` (Menu) bindings. Each also has a `hold`-entered sub-mode (Pan on the Map, Zoom on the profile).

<figure class="fig">
<svg viewBox="0 0 720 340" role="img" aria-label="A navigation graph. Home, the root, opens the Route menu on press and the Menu on back-hold. The Menu opens the Route menu (Routes). Loading a route truncates to Home and pushes the Map (Root). The Map and Statistics are siblings swapped by back. The Map opens RideControl on press (pause) and enters Pan on hold. From RideControl, Resume pops back to the Map and Finish or Discard (held) clears to Home.">
  <defs>
    <marker id="aU7" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#5f7d3d" /></marker>
    <marker id="aU7c" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">The screen flow</text>

  <!-- nodes -->
  <rect class="d-panel" x="36"  y="150" width="104" height="40" rx="9" /><text class="d-label" x="88"  y="170" text-anchor="middle">Home</text><text class="d-sub" x="88" y="183" text-anchor="middle" style="font-size:8.5px">root</text>
  <rect class="d-panel-2" x="232" y="56"  width="116" height="40" rx="9" /><text class="d-label" x="290" y="76"  text-anchor="middle">Route menu</text><text class="d-sub" x="290" y="89" text-anchor="middle" style="font-size:8.5px">pick a route</text>
  <rect class="d-panel-2" x="232" y="246" width="116" height="40" rx="9" /><text class="d-label" x="290" y="266" text-anchor="middle">Menu</text><text class="d-sub" x="290" y="279" text-anchor="middle" style="font-size:8.5px">Routes · Settings</text>
  <rect class="d-forest" x="436" y="150" width="104" height="40" rx="9" /><text class="d-label" x="488" y="174" text-anchor="middle" style="fill:#fff">Map</text>
  <rect class="d-water" x="596" y="150" width="104" height="40" rx="9" /><text class="d-label" x="648" y="170" text-anchor="middle" style="fill:#fff">Statistics</text><text class="d-sub" x="648" y="183" text-anchor="middle" style="fill:#dfe6e0;font-size:8.5px">elevation</text>
  <rect class="d-hot" x="436" y="56" width="104" height="40" rx="9" style="fill:#f8efe4" /><text class="d-label" x="488" y="80" text-anchor="middle" style="fill:#a9501c">RideControl</text>
  <rect class="d-panel-2" x="436" y="246" width="104" height="40" rx="9" /><text class="d-label" x="488" y="270" text-anchor="middle">Pan / Zoom</text>

  <!-- edges from Home -->
  <line x1="140" y1="164" x2="230" y2="86"  stroke="#5f7d3d" stroke-width="1.6" marker-end="url(#aU7)" /><text class="d-sub" x="168" y="116" style="font-size:9px">press</text>
  <line x1="140" y1="178" x2="230" y2="258" stroke="#5f7d3d" stroke-width="1.6" marker-end="url(#aU7)" /><text class="d-sub" x="150" y="232" style="font-size:9px">back-hold</text>
  <!-- Menu -> Route menu -->
  <line x1="356" y1="250" x2="356" y2="98" stroke="#5f7d3d" stroke-width="1.6" marker-end="url(#aU7)" /><text class="d-sub" x="362" y="178" style="font-size:9px">Routes</text>
  <!-- Route menu -> Map (load/Root) -->
  <line x1="350" y1="84" x2="432" y2="156" stroke="#cf6a2a" stroke-width="2" marker-end="url(#aU7c)" /><text class="d-sub" x="372" y="116" style="fill:#a9501c;font-size:9px">load · Root</text>
  <!-- Map <-> Statistics -->
  <line x1="540" y1="164" x2="596" y2="164" stroke="#5f7d3d" stroke-width="1.6" marker-end="url(#aU7)" />
  <line x1="596" y1="176" x2="540" y2="176" stroke="#5f7d3d" stroke-width="1.6" marker-end="url(#aU7)" /><text class="d-sub" x="568" y="200" text-anchor="middle" style="font-size:9px">back ⇄</text>
  <!-- Map -> RideControl -->
  <line x1="482" y1="150" x2="482" y2="98" stroke="#cf6a2a" stroke-width="2" marker-end="url(#aU7c)" /><text class="d-sub" x="396" y="126" style="fill:#a9501c;font-size:9px">press · pause</text>
  <!-- RideControl -> Map (resume) -->
  <line x1="500" y1="98" x2="500" y2="148" stroke="#5f7d3d" stroke-width="1.4" marker-end="url(#aU7)" /><text class="d-sub" x="508" y="126" style="font-size:9px">resume</text>
  <!-- Map -> Pan -->
  <line x1="488" y1="190" x2="488" y2="244" stroke="#5f7d3d" stroke-width="1.6" marker-end="url(#aU7)" /><text class="d-sub" x="496" y="220" style="font-size:9px">hold</text>
  <!-- RideControl -> Home (finish/discard) -->
  <path d="M436 66 C 250 20, 90 70, 88 148" fill="none" stroke="#cf6a2a" stroke-width="1.6" stroke-dasharray="4 4" marker-end="url(#aU7c)" /><text class="d-sub" x="250" y="34" style="fill:#a9501c;font-size:9px">Finish / Discard (hold) → Home</text>
</svg>
<figcaption>Green edges are ordinary moves; coral marks the "go ride / stop riding" path. Picking a route uses <code>Root</code>, so you always land on a clean <code>[Home, Map]</code> instead of a Map buried under stale menus — and picking a <i>different</i> route mid-ride detours through a guarded "swap or save &amp; start new" prompt first.</figcaption>
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
- The settings screens (the two-level editors + the shared kit): [`obc-app/src/screen/settings/`](src:firmware/obc-app/src/screen/settings)
- The `Settings` value + its byte codec, and the `SettingsStore` seam: [`obc-app/src/settings.rs`](src:firmware/obc-app/src/settings.rs), [`obc-app/src/hal.rs`](src:firmware/obc-app/src/hal.rs)
- The gesture recognizer: [`obc-app/src/input.rs`](src:firmware/obc-app/src/input.rs)
- The input + overlay plane: [`obc-app/src/input_plane.rs`](src:firmware/obc-app/src/input_plane.rs)
- The app driver (frame loop, render-on-demand, compositing): [`obc-app/src/app.rs`](src:firmware/obc-app/src/app.rs)
- The injected-hardware traits and input types: [`obc-app/src/hal.rs`](src:firmware/obc-app/src/hal.rs)

For how the two planes keep input responsive under a long render, and where the HAL fits, see [system architecture](../architecture/). For how a screen's `draw` actually puts pixels on the panel, see the [rendering pipeline](../rendering/).
