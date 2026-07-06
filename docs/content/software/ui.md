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
<svg viewBox="0 0 720 300" role="img" aria-label="On the left, the Screen enum lists its variants: Home, Map, Statistics, RideControl, Menu, RouteMenu, RouteOverview, RouteSwap, plus the Settings tree. The Map variant points to its module on the right, which holds typed state, a handle method returning a Transition, and a draw method emitting pixels. A tag notes static match dispatch, no dyn and no allocation.">
  <defs>
    <marker id="aU1" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">A screen is a value — no retained widget tree</text>

  <!-- enum Screen -->
  <rect class="d-panel" x="36" y="44" width="210" height="232" rx="11" />
  <text class="d-label" x="56" y="66">enum Screen</text>
  <g font-family="var(--mono)">
    <rect x="52" y="78"  width="178" height="22" rx="5" class="d-hot-fill" /><text class="d-sub" x="62" y="93" style="fill:#fff">Map(MapScreen)</text>
    <rect x="52" y="104" width="178" height="20" rx="5" class="d-muted" /><text class="d-sub" x="62" y="118">Home(HomeScreen)</text>
    <rect x="52" y="126" width="178" height="20" rx="5" class="d-muted" /><text class="d-sub" x="62" y="140">Statistics(…)</text>
    <rect x="52" y="148" width="178" height="20" rx="5" class="d-muted" /><text class="d-sub" x="62" y="162">RideControl(…)</text>
    <rect x="52" y="170" width="178" height="20" rx="5" class="d-muted" /><text class="d-sub" x="62" y="184">Menu(…)</text>
    <rect x="52" y="192" width="178" height="20" rx="5" class="d-muted" /><text class="d-sub" x="62" y="206">RouteMenu(…)</text>
    <rect x="52" y="214" width="178" height="20" rx="5" class="d-muted" /><text class="d-sub" x="62" y="228">RouteOverview(…)</text>
    <rect x="52" y="236" width="178" height="20" rx="5" class="d-muted" /><text class="d-sub" x="62" y="250">RouteSwap(…)</text>
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
    RideControl(RideControl) => Nav,       // the Paused page: ride-so-far ledger + Resume/Finish/Discard
    Menu(MenuScreen) => Nav,               // the compass dial
    PoiMenu(PoiMenuScreen) => Nav,         // POIs browser: the category list
    PoiList(PoiListScreen) => Nav,         // one category's nearest-16, with live bearing arrows
    PoiDetail(PoiDetailScreen) => Nav,     // one POI: full name, subtype, arrow, today's hours + open/closed
    RouteMenu(RouteMenuScreen) => Nav,
    Rides(RidesScreen) => Nav,             // see + delete stored rides (hold-to-delete, unsynced guard)
    RouteOverview(RouteOverviewScreen) => Nav, // look-before-you-ride: profile + stats + START
    RouteSwap(RouteSwapScreen) => Nav,
    // Host-pushed cards — opened by the BLE seam, not a gesture (see "Screens the companion link pushes")
    RouteReceived(RouteReceivedScreen) => Nav, // idle "ROUTE RECEIVED" — Start navigation / Dismiss
    RouteUpdated(RouteUpdatedScreen) => Nav,   // info-only: the actively-navigated route was replaced
    Passkey(PasskeyScreen) => Nav,             // the 6-digit pairing code, modal + non-dismissible
    // The Settings tree — a list plus one screen each for Date & Time, Units, Power, Bluetooth,
    // Reset, and Stats (which opens onto Fields → AddField, the stat-grid panel manager). The
    // `Settings` kind is what holds the debounced settings save while one of these is on top.
    Settings(SettingsScreen) => Settings,  DateTime(DateTimeScreen) => Settings,
    Units(UnitsScreen) => Settings,        Stats(StatsScreen) => Settings,
    StatFields(StatFieldsScreen) => Settings, AddField(AddFieldScreen) => Settings,
    Power(PowerScreen) => Settings,        Bluetooth(BluetoothScreen) => Settings,
    Reset(ResetScreen) => Settings,
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

Here's a whole screen's logic — the main Menu, a compass dial whose amber needle sweeps to the
selected station — to show how little a screen has to say. Even with an animation, the logic is
three lines per gesture: the sweep itself runs through the same timer-poll contract the Home
clock uses (see *Render on demand* below), so it costs nothing once the needle has landed:

```rust
fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
    match g {
        Gesture::Turn(n) => {
            self.target_deg += n as f32 * DETENT_DEG; // the needle chases this in tick_timers
            list::on_turn(&mut self.selected, n, ITEMS.len()) // Routes / Rides / POIs / Map / Settings
        }
        Gesture::Press   => match self.selected {
            0 => Transition::Push(Screen::RouteMenu(RouteMenuScreen::new())), // Routes
            1 => Transition::Push(Screen::Rides(RidesScreen::new())),         // Rides
            2 => Transition::Push(Screen::PoiMenu(PoiMenuScreen::new())),     // POIs
            4 => Transition::Push(Screen::Settings(SettingsScreen::new())),   // Settings
            _ => Transition::None,                                            // Map — future screen
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

A hold is also cancelled if the **screen stack changes** while it charges — the two buttons recognise independently, so a Back tap can dismiss a popup mid-hold, and a long-press that started over one screen must never complete onto whatever replaced it (a hold aimed at a prompt's "Save & new" landing on the Route menu's hold-to-delete footer would silently delete a route). The transition cancels the in-flight hold; the bar retracts, the release stays silent, and the next press starts clean.

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

The same guarded hold does duty as a **delete** control. Rather than a modal "are you sure?", a screen that can delete its selected item reserves a **footer band** below the list: a rule and a *"hold to delete"* row whose bar fills with the live hold-progress, exactly like a guarded confirm row. The completed hold *is* the confirmation — there is no second popup. This began on the Stats **Fields** editor (remove a panel) and now drives deletion on **three** screens with one shared footer: the **Route menu**, the **Rides** screen, and Fields. One idiom, three places — a rider who learns it once knows it everywhere.

Two behaviours make the footer safe to press without thinking:

- **Greyed when the item is in use.** The footer disables (a hold does nothing) when deleting would break live state: the Route menu greys it for the route you're *actively navigating* (deleting the file under an open geometry handle mid-ride would break navigation — but a route merely *previewed* from idle is still deletable), and the Rides screen greys it for the ride you're *currently recording* (its file isn't even written until Finish, and the filesystem refuses to delete an open handle).
- **Warning-red when a ride is unsynced.** A tracked ride the phone hasn't downloaded yet is unrecoverable if deleted, so the Rides footer renders **warning-red with a "not synced" cue** for those — still deletable, just *informed*. (Routes get no such cue: the phone can always re-upload one.)

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="The hold-to-delete footer across three screens. On the left, a list with a footer band below it holding a rule and a hold-to-delete row that fills with a progress bar as you hold. On the right, the footer's three states stacked: normal — hold to delete, an active green route or a recording ride greyed and disabled, and an unsynced ride shown warning-red with a not-synced cue.">
  <text class="d-tag" x="20" y="24">One footer, three screens — Route menu · Rides · Fields</text>

  <!-- the screen with a footer -->
  <rect class="d-panel" x="24" y="42" width="228" height="188" rx="11" />
  <rect x="32" y="50" width="212" height="18" rx="3" style="fill:#aa5500" /><text class="d-sub" x="42" y="63" style="fill:#fff;font-size:9px">RIDES</text>
  <rect x="32" y="74" width="212" height="30" rx="4" class="d-amber" />
  <text class="d-sub" x="42" y="88" style="fill:#000;font-size:9.5px">Kandel Loop · Sat</text>
  <text class="d-sub" x="42" y="100" style="fill:#000;font-size:8.5px">42 km · 3:10 · 780 m</text>
  <text class="d-sub" x="42" y="126" style="font-size:9.5px">Rhine flats · Thu</text>
  <text class="d-sub" x="42" y="152" style="font-size:9.5px">Vosges climb · Mon</text>
  <!-- footer band -->
  <line x1="36" y1="182" x2="240" y2="182" stroke="#aaaa55" stroke-width="1" />
  <rect x="36" y="190" width="120" height="30" rx="6" style="fill:#c0492e" />
  <rect x="156" y="190" width="84" height="30" rx="6" class="d-muted" />
  <text class="d-sub" x="138" y="209" text-anchor="middle" style="fill:#fff;font-size:9px">hold to delete</text>
  <text class="d-sub" x="138" y="238" text-anchor="middle" style="font-size:8.5px">bar fills on the live hold</text>

  <!-- the three footer states -->
  <text class="d-tag" x="292" y="60">the footer, three states</text>
  <rect x="292" y="72" width="404" height="30" rx="6" class="d-muted" />
  <text class="d-sub" x="308" y="91" style="font-size:10px">hold to delete</text>
  <text class="d-sub" x="470" y="91" style="font-size:9px;fill:#6b7758">— normal · a completed hold deletes</text>

  <rect x="292" y="110" width="404" height="30" rx="6" style="fill:#e7e6dd" />
  <text class="d-sub" x="308" y="129" style="font-size:10px;fill:#9a9a86">hold to delete</text>
  <text class="d-sub" x="470" y="129" style="font-size:9px;fill:#6b7758">— greyed · item is active / recording</text>

  <rect x="292" y="148" width="404" height="30" rx="6" style="fill:#f6e3dc;stroke:#c0492e;stroke-width:1.2" />
  <path d="M306 157 l6 12 l-12 0 z" fill="#c0492e" /><text class="d-sub" x="303" y="167" style="font-size:8px;fill:#fff">!</text>
  <text class="d-sub" x="322" y="167" style="font-size:10px;fill:#a9501c">hold to delete · not synced</text>
  <text class="d-sub" x="322" y="176" style="font-size:8.5px;fill:#a9501c">— unsynced ride · deletes for good</text>
</svg>
<figcaption>The footer reuses the guarded-hold machinery wholesale — the same <code>confirm_row</code> fill, driven by the same live <code>hold_progress</code> — so there's no new gesture and no new confirmation dialog. What varies per screen is only the <b>guard</b>: greyed when the highlighted item is in use (an actively-navigated route, a recording ride), and warning-red for an unsynced ride. A device-side delete then flows through the object store, so the phone reconciles it on the next connect (see the <a href="../companion-link/#staying-in-sync-the-change-digest">companion link</a>).</figcaption>
</figure>

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

The list is a **static snapshot**, frozen the moment you enter. Membership, order and distances don't move — rows never reshuffle under the cursor as you turn, and the SD card isn't re-scanned every frame. Re-enter the category to refresh it against your current position. Under the hood the [nearest-16 query](../formats/#pois-a-nearest-list-not-a-map-layer) needs the streaming map `Reader`, which lives only in the [`draw` context](#logic-and-drawing-get-different-views-of-the-world) — so the snapshot is taken *lazily on the first draw* that has both a reader and a fix, into a single buffer the app owns (holding it per-screen would inflate every slot of the screen stack). Opening a list invalidates that buffer, so the next draw re-queries.

The one thing that *is* live is the **bearing arrow** — recomputed every frame from the POI's stored coordinates and the rider's current heading, pure trig with zero SD access. It points from you toward the POI **relative to your heading**, so "straight up" means "dead ahead." That heading has two sources, and which one is used depends on whether you're moving:

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

Pressing a list row opens the **detail view** for that POI — one more `Nav` screen, carrying the selected POI out of the frozen snapshot. It shows the same thing the row does, but unabridged: the **full stored name** (the row ellipsizes to fit its width; the detail wraps it to a second line instead of truncating), the **subtype label** as a muted subtitle, and the **same live bearing arrow** — the identical element and heading seam as the row, still hidden when neither course nor compass is known. What the row can't fit is the reason the detail exists: **today's opening hours** and whether the place is **open right now**.

<figure class="fig">
<svg viewBox="0 0 720 250" role="img" aria-label="The POI detail view. On the left, the screen: a full POI name at the top, a muted subtype subtitle beneath it, then a Today heading with a bearing arrow, one or two opening-hours ranges stacked below, and an OPEN or CLOSED badge at the bottom. On the right, the three heading states for the hours block: Today with time ranges when open some hours today, Closed today when the schedule has no interval for this weekday, and Hours not listed when the POI has no schedule at all. Below, the open-now badge is derived from the live local clock.">
  <defs>
    <marker id="aPD" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">The detail screen, and where "open now" comes from</text>

  <!-- the screen mock -->
  <rect class="d-panel" x="24" y="40" width="232" height="196" rx="10" />
  <rect x="42" y="54" width="196" height="18" rx="3" style="fill:#aa5500" /><text class="d-sub" x="52" y="67" style="fill:#fff;font-size:9px">POI</text>
  <text class="d-sub" x="42" y="98" font-family="var(--mono)" style="font-size:12px">Stadtbaeckerei</text>
  <text class="d-sub" x="42" y="114" style="font-size:9px">Bakery</text>
  <!-- today + arrow -->
  <path d="M52 138 l7 -7 l7 7 l-4 0 l0 9 l-6 0 l0 -9 z" fill="#cf6a2a" />
  <text class="d-sub" x="72" y="146" style="font-size:9px">Today</text>
  <text class="d-sub" x="42" y="168" font-family="var(--mono)" style="font-size:11px">08:00-12:00</text>
  <text class="d-sub" x="42" y="184" font-family="var(--mono)" style="font-size:11px">14:00-18:00</text>
  <!-- badge -->
  <rect x="42" y="200" width="66" height="22" rx="6" style="fill:#3c6b39" />
  <text class="d-sub" x="75" y="215" text-anchor="middle" style="fill:#fff;font-size:10px">OPEN</text>

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
<figcaption>The hours block reads the POI's [pooled schedule](../formats/#opening-hours-a-pooled-weekly-schedule) once (lazily, on the first draw with a map <code>Reader</code> — the same reader-in-draw seam the list snapshot uses), then picks <b>today's</b> intervals. Three states: <b>Today</b> with the day's one or two ranges stacked (<code>08:00-12:00</code> / <code>14:00-18:00</code> — stacked because a two-range line won't fit the 240 px panel); <b>Closed today</b> when the schedule has no interval for this weekday; and <b>Hours not listed</b> when the POI had no parseable hours at all. The <b>OPEN / CLOSED</b> badge is the only live piece — recomputed every frame from the device's local wall-clock. That local time comes from the user's <b>UTC offset</b> plus the <b>GPS clock</b> (the same clock that sets the ride time), and <code>weekday_from_ymd</code> (Zeller's congruence, Mon = 0) picks today's row; the minute-of-day then decides open vs. closed, including overnight intervals that opened last evening.</figcaption>
</figure>

## Settings: a second level of focus

Most screens have one focus: the row cursor. The **Settings** screens — Date & Time, Units, Stats, Power, Bluetooth, and the factory Reset — add a *second* level. A value isn't a separate sub-screen; it's edited **in place**. Rotating still moves the amber row cursor, but once you press a value row, focus drops *into* a field: a `▲▼` box marks the live one, rotating now changes *its* value, pressing steps to the next field, and back steps out. The same two-level model drives every editor — a date, a UTC offset, a fix interval — and the same toggle row flips GPS-set-time or the power saver. No new gestures; the existing five just mean different things at each level.

The **Stats** screen configures the riding grid itself. Its *Page cycle* row sets how fast the grid auto-flips between pages; *Fields* opens the grid **editor — which simply *is* the grid**: the same 3×2 tile pages the riding view shows, placed by the same layout walk and painted by the same tile renderer, with live values. The grid draws from a predefined, in-code catalogue of fields (speed, distance, climb, grade, elevation, clock, …) — the rider picks which to show and in what order, and a field is either one column or a full-width two. The cursor is the amber tile (walking past a page's last tile flips pages), and reordering reuses the grab idiom: press *lifts* the tile (move arrows appear), rotating moves it through the order — the grid reflows live, so a two-column panel's row-aligned hops are something you watch, not infer — and press drops it. A ghost `+` tile in the first free slot opens the field picker, so a new panel visibly lands where the ghost sits; a panel is removed by a **hold-to-delete** bar, the same guarded hold as Reset's. The chosen panels lay out six to a page (3×2) and auto-cycle on the timer, so a long list stays glanceable. The selection and period live in the same persisted `Settings` value, so they survive a reboot like every other setting.

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

- **map** — the screen stack. Set when a gesture was applied, when a fresh GPS fix moved the camera *on a view that shows live data*, when the route or ride session changed, when a screen's own timer poll (`tick_timers`) reported a change — the Statistics cursor springing back to the live position, or the Home screensaver's wall clock crossing into a new minute — or, on a riding view, when the GPS fix goes stale or returns (raising or clearing the "No GPS Fix" banner, a timer edge surfaced from the per-frame `tick`). A parked Home screen fires none of these — neither the first three nor the two timed edges, which are gated to the live-data views — so between those once-a-minute clock ticks the map stays clean.
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

One refinement rides on the timer poll: a screen that knows its timed change is spatially small can attach a **containing region** to it. The route-planning screen is the one user so far — its spinning compass needle repaints many times a second for several seconds while the router runs, yet only the needle's disc ever changes. The dirty signal carries that region out to the host, which clips the repaint to it: the full draw sequence replays, but the framebuffer discards every pixel write outside the region (and rejects whole out-of-region primitives before rasterizing them), so the frame costs the disc, not the chrome — and the [changed-rows-only push](../../hardware/display-protocol/#partial-update--a-span-masked-gate-scan) shrinks with it. The region is a promise, not a request: *any* other dirt in the same frame — a gesture, a fix, a popup — folds it away and the host full-repaints, so under-drawing can't happen; screens that never report a region behave exactly as before.

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

A route arriving over BLE surfaces as one of three **advisory** cards — advisory because the route is committed to the store (and the Route menu) *before* the prompt, so dismissing loses nothing. All three auto-close after 30 s (timeout = dismiss), and the display **wakes** to show them (an upload usually means the phone is right there). Which one appears depends on the rider's state: an idle *"Route received — Start navigation / Dismiss"*, a mid-ride guarded *swap* prompt (the same shape a mid-ride route pick uses), or — when the upload *replaced the route being navigated* — an info-only card, because the device has already adopted the new version (the old file is gone). The [companion-link page](../companion-link/#when-a-route-lands-the-devices-side) tells the full story; the UI-side rules are the interesting part: a prompt **never lands while a hold is charging** (it defers a tick, so a half-done *Save & start new* hold can't complete onto it — the same [stack-change hold-cancel](#hold-to-confirm) at work), **consecutive uploads replace** the prompt by object id rather than stacking, and the **passkey card outranks** it (a pending prompt is dropped, not queued).

### The connected indicator

A single **static** Bluetooth rune says "a phone is linked right now." It appears in exactly two places: the **main Menu's title bar** (the right slot of the framed header) and the **Home screen**, next to the battery gauge — the two screens a rider glances at between rides. It is deliberately **absent from the riding views**. That's not an oversight but a [render-on-demand](#render-on-demand-and-the-idle-path-is-free) decision: the Map and Statistics screens repaint only when something they show moves, and a phone connecting or dropping is *not* something the ride cares about — putting a link glyph there would dirty an expensive map frame on every BLE edge. So the indicator lives only where the panel is already cheap to redraw.

### The Bluetooth settings screen

Settings ▸ **Bluetooth** is where the link's few knobs live. It's an ordinary settings screen (two-level focus like the rest), carrying: an **on/off** toggle for the radio (persisted in `Settings` like every other setting, and pushed to the radio plane by the host), a read-only **status line** (Off / Advertising / Connected, straight from the seam), a **"Paired: yes/no"** row (no phone name — deliberately not worth a protocol addition), and a hold-guarded **Forget phone** row. Forget uses the [delete-footer's guarded hold](#deleting-things-the-hold-to-delete-footer) — a completed hold fills it warning-red and clears the bond — and it matters more than it looks: because [a stored bond now rejects new pairings](../companion-link/#pairing-and-staying-paired), Forget phone is the **only** way to re-pair a replaced or reset phone. The guarded hold is the confirmation; there's no extra popup.

## The whole flow

Put the pieces together and the navigation graph is small and legible. Two screens are **riding views** — the Map and the Elevation/Statistics profile — and they're siblings: `back` swaps between them without growing the stack, and both share the same `press` (pause) and `back-hold` (Menu) bindings. Each also has a `hold`-entered sub-mode (Pan on the Map, Zoom on the profile).

<figure class="fig">
<svg viewBox="0 0 720 340" role="img" aria-label="A navigation graph. Home, the root, opens the Route menu on press and the compass Menu on back-hold. The Menu opens the Route menu (Routes). Picking a route opens the Route overview; its START truncates to Home and pushes the Map (Root). The Map and Statistics are siblings swapped by back. The Map opens the Paused page on press and enters Pan on hold. From Paused, Resume pops back to the Map and Finish or Discard (held) clears to Home.">
  <defs>
    <marker id="aU7" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#5f7d3d" /></marker>
    <marker id="aU7c" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="24">The screen flow</text>

  <!-- nodes -->
  <rect class="d-panel" x="36"  y="150" width="104" height="40" rx="9" /><text class="d-label" x="88"  y="170" text-anchor="middle">Home</text><text class="d-sub" x="88" y="183" text-anchor="middle" style="font-size:8.5px">root</text>
  <rect class="d-panel-2" x="232" y="56"  width="116" height="40" rx="9" /><text class="d-label" x="290" y="76"  text-anchor="middle">Route menu</text><text class="d-sub" x="290" y="89" text-anchor="middle" style="font-size:8.5px">pick a route</text>
  <rect class="d-panel-2" x="246" y="150" width="104" height="40" rx="9" /><text class="d-label" x="298" y="170" text-anchor="middle">Overview</text><text class="d-sub" x="298" y="183" text-anchor="middle" style="font-size:8.5px">profile · stats</text>
  <rect class="d-panel-2" x="232" y="246" width="116" height="40" rx="9" /><text class="d-label" x="290" y="266" text-anchor="middle">Menu</text><text class="d-sub" x="290" y="279" text-anchor="middle" style="font-size:8.5px">compass · 4 stations</text>
  <rect class="d-forest" x="436" y="150" width="104" height="40" rx="9" /><text class="d-label" x="488" y="174" text-anchor="middle" style="fill:#fff">Map</text>
  <rect class="d-water" x="596" y="150" width="104" height="40" rx="9" /><text class="d-label" x="648" y="170" text-anchor="middle" style="fill:#fff">Statistics</text><text class="d-sub" x="648" y="183" text-anchor="middle" style="fill:#dfe6e0;font-size:8.5px">elevation</text>
  <rect class="d-hot" x="436" y="56" width="104" height="40" rx="9" style="fill:#f8efe4" /><text class="d-label" x="488" y="80" text-anchor="middle" style="fill:#a9501c">Paused</text>
  <rect class="d-panel-2" x="436" y="246" width="104" height="40" rx="9" /><text class="d-label" x="488" y="270" text-anchor="middle">Pan / Zoom</text>

  <!-- edges from Home -->
  <line x1="140" y1="164" x2="230" y2="86"  stroke="#5f7d3d" stroke-width="1.6" marker-end="url(#aU7)" /><text class="d-sub" x="168" y="116" style="font-size:9px">press</text>
  <line x1="140" y1="178" x2="230" y2="258" stroke="#5f7d3d" stroke-width="1.6" marker-end="url(#aU7)" /><text class="d-sub" x="150" y="232" style="font-size:9px">back-hold</text>
  <!-- Menu -> Route menu -->
  <line x1="356" y1="250" x2="356" y2="98" stroke="#5f7d3d" stroke-width="1.6" marker-end="url(#aU7)" /><text class="d-sub" x="362" y="178" style="font-size:9px">Routes</text>
  <!-- Route menu -> Overview (press) -->
  <line x1="292" y1="96" x2="296" y2="148" stroke="#5f7d3d" stroke-width="1.6" marker-end="url(#aU7)" /><text class="d-sub" x="248" y="122" style="font-size:9px">press</text>
  <!-- Overview -> Map (START/Root) -->
  <line x1="350" y1="170" x2="432" y2="170" stroke="#cf6a2a" stroke-width="2" marker-end="url(#aU7c)" /><text class="d-sub" x="358" y="163" style="fill:#a9501c;font-size:9px">START · Root</text>
  <!-- Map <-> Statistics -->
  <line x1="540" y1="160" x2="596" y2="160" stroke="#5f7d3d" stroke-width="1.6" marker-end="url(#aU7)" />
  <line x1="596" y1="180" x2="540" y2="180" stroke="#5f7d3d" stroke-width="1.6" marker-end="url(#aU7)" /><text class="d-sub" x="568" y="200" text-anchor="middle" style="font-size:9px">back ⇄</text>
  <!-- Map -> Paused -->
  <line x1="482" y1="150" x2="482" y2="98" stroke="#cf6a2a" stroke-width="2" marker-end="url(#aU7c)" /><text class="d-sub" x="396" y="126" style="fill:#a9501c;font-size:9px">press · pause</text>
  <!-- Paused -> Map (resume) -->
  <line x1="500" y1="98" x2="500" y2="148" stroke="#5f7d3d" stroke-width="1.4" marker-end="url(#aU7)" /><text class="d-sub" x="508" y="126" style="font-size:9px">resume</text>
  <!-- Map -> Pan -->
  <line x1="488" y1="190" x2="488" y2="244" stroke="#5f7d3d" stroke-width="1.6" marker-end="url(#aU7)" /><text class="d-sub" x="496" y="220" style="font-size:9px">hold</text>
  <!-- Paused -> Home (finish/discard) -->
  <path d="M436 66 C 250 20, 90 70, 88 148" fill="none" stroke="#cf6a2a" stroke-width="1.6" stroke-dasharray="4 4" marker-end="url(#aU7c)" /><text class="d-sub" x="250" y="34" style="fill:#a9501c;font-size:9px">Finish / Discard (hold) → Home</text>
</svg>
<figcaption>Green edges are ordinary moves; coral marks the "go ride / stop riding" path. Picking a route opens the <b>Overview</b> — profile, distance/climb/descent, START — while the route streams open behind it; START uses <code>Root</code>, so you always land on a clean <code>[Home, Map]</code> instead of a Map buried under stale menus. Picking a <i>different</i> route mid-ride still detours through a guarded "swap or save &amp; start new" prompt, and the <b>Paused</b> page shows the ride-so-far ledger above its guarded Finish / Discard rows.</figcaption>
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
- The host→app BLE seam (`BleStatus` — the connected indicator, passkey, paired): [`obc-app/src/ble.rs`](src:firmware/obc-app/src/ble.rs)
- The host-pushed cards — the passkey card and the route-upload prompts: [`obc-app/src/screen/passkey.rs`](src:firmware/obc-app/src/screen/passkey.rs), [`obc-app/src/screen/route_received.rs`](src:firmware/obc-app/src/screen/route_received.rs)
- The Rides screen and the Bluetooth settings screen: [`obc-app/src/screen/rides.rs`](src:firmware/obc-app/src/screen/rides.rs), [`obc-app/src/screen/settings/bluetooth.rs`](src:firmware/obc-app/src/screen/settings/bluetooth.rs)
- The settings screens (the two-level editors + the shared kit): [`obc-app/src/screen/settings/`](src:firmware/obc-app/src/screen/settings)
- The `Settings` value + its byte codec, and the `SettingsStore` seam: [`obc-app/src/settings.rs`](src:firmware/obc-app/src/settings.rs), [`obc-app/src/hal.rs`](src:firmware/obc-app/src/hal.rs)
- The gesture recognizer: [`obc-app/src/input.rs`](src:firmware/obc-app/src/input.rs)
- The input + overlay plane: [`obc-app/src/input_plane.rs`](src:firmware/obc-app/src/input_plane.rs)
- The app driver (frame loop, render-on-demand, compositing): [`obc-app/src/app.rs`](src:firmware/obc-app/src/app.rs)
- The injected-hardware traits and input types: [`obc-app/src/hal.rs`](src:firmware/obc-app/src/hal.rs)

For how the two planes keep input responsive under a long render, and where the HAL fits, see [system architecture](../architecture/). For how a screen's `draw` actually puts pixels on the panel, see the [rendering pipeline](../rendering/).
