# OBC simulator

`obc-sim` runs the same `obc-app` and `obc-render` code as the device in a desktop window or a
deterministic headless PNG render. Both paths use the panel's RGB222/64-colour gamut. The GUI adds
a control panel for location, sensors, BLE, housing colorway, and display calibration; those live
controls are intentionally not duplicated as startup flags.

Build and run it from the repository root:

```sh
cargo build -p obc-sim --release
target/release/obc-sim freiburg.obcm
target/release/obc-sim freiburg.obcm --png frame.png
```

Run `target/release/obc-sim --help` for the compact reference. The sections below document every
remaining option.

## Device controls in the window

The four housing buttons are clickable, and the keyboard drives the same raw edges the firmware
gets from GPIO:

| Key | Button |
| --- | --- |
| Left arrow | Up |
| Right arrow | Down |
| Enter | Select |
| Backspace | Back |

Because these are held-state edges, the device-wide **chords** work as they do on the device. Press
Left and Enter (Up + Select) **within 100 ms of each other** to open the universal quick drawer; a
larger gap is two ordinary gestures. Down + Back (Right and Backspace) is the contextual drawer's
chord, on the same window; it is recognised, but it has no content yet.

The mouse wheel over the screen injects selection steps directly, so it does not model a button and
makes no chord.

## Map and output

The ordinary OBCM path is an import input. Startup copies the map through a 16 KiB buffer into a
temporary sparse card, then imports the route, trip and saved-ride fixtures. All runtime readers use that same
card. Input files stay unchanged. The final reader releases the temporary card.

On Unix, create a persistent card explicitly, then reopen it without importing the files again:

```sh
target/release/obc-sim freiburg.obcm --create-card ride.obc --routes-dir routes/
target/release/obc-sim --card ride.obc
target/release/obc-sim --import next-stage.gpx --card ride.obc
```

After an interrupted recording, `--card` offers Continue or Discard for the last valid checkpoint.
Continue retains its card identity and accepted totals. If Save already journalled its footer,
startup completes the catalog commit and lists the saved ride instead. Failed recovery validation
can show a damaged-ride card; failed durability confirmation or terminal settlement stops startup.
Close all readers and reopen after an uncertain card write. Startup never resets or migrates a card.

Creation refuses an existing path and exits after importing. If an import fails, it reports failure
and leaves the partial card for inspection. Reopening never initializes or resets the file. It
requires exactly one readable map and complete route and trip catalogs. The last reader keeps the
card's exclusive file lock, even after the session closes. Persistent cards are not supported on
Windows; ordinary temporary sessions remain available.

A reopened map is labelled **Card map**. Planning and map-referenced altitude sample the terrain
inside that retained map object in both import and reopen sessions. They do not read an external
`.obcd` sidecar. Missing terrain leaves elevations unavailable; invalid or unreadable terrain
reports a diagnostic and keeps the map usable. Peak View keeps its separate bounded terrain cache.

- `--no-card` simulates an absent storage card.
- `--size WxH` changes the frame geometry from the device default (240×320).
- `--scale N` applies an integer scale to the window or saved PNG (default 1).
- `--png PATH` renders one device-gamut frame and exits. This is the screenshot-test interface.
- `--palette` shows the device's 64-colour palette, or saves it when combined with `--png`.
- `--center LON,LAT` sets the headless camera centre in integer microdegrees.
- `--zoom MULT` multiplies the headless bbox-fit zoom.
- `--heading DEG` starts heading-up at the given clockwise course.
Peak View appears in the normal menu when the loaded map contains indexed terrain. It uses the
simulator's current GPS position and the selected map's summit records. The background job reads
the same immutable map bytes as the map screen. Without a GPS fix it waits; it does not use the
camera centre as an observer. Normal framing widens when a nearby summit requires more vertical
headroom. The normal view is at least 90° wide. Low-relief observers receive up to a 2.4× boost over the base 1.25× vertical scale, fixed while turning;
steep views keep the base scale. The summit candidates reserve a slot for each bearing sector's tallest
landmark. Explicit fixture frames retain their configured bounds. In a headless test,
`--center LON,LAT --heading DEG` supplies an explicit simulated fix. Use the menu and `f` to complete generation before saving the frame.

- `--peak-view gornergrat|scheidegg|glockner` selects an explicit geographic test fixture and
  opens Peak View in the GUI. This overrides the selected map's terrain for that test. It generates a
  panorama from geographic terrain, with the current direction first. The compass and partial terrain appear
  at once; three static dots indicate background work on the remaining directions.
  Progress redraws occur at most twice per second.
  Background work extends both edges in about 17-degree batches. Turning prioritizes the new
  direction. Completed terrain stays visible and follows the
  heading; a light hatch marks pending parts until they fill in. Back cancels. Drag **Compass (heading when
  stopped)** in Controls to turn. Changing the GPS position by more than 20 m rebuilds the view;
  smaller changes keep the current panorama to limit GPS jitter. The fixture has a limited area
  of fine terrain around each preset. Missing distant coverage shows
  dashed marks over affected bearings. Moving outside observer coverage shows Terrain unavailable.
  Each geographic terrain file contains heights, lower-resolution levels and baked height
  bounds. The renderer skips hidden blocks and shades visible terrain slopes under fixed
  illustration lighting. It does not store viewpoints or show snow and current sunlight.
  Named summits are aligned and checked for visibility during generation.
  Live has no selection. Select enters Browse on the most prominent visible peak. Up/Down steps
  through visible peaks and turns the view by 15° past an edge. Select returns to Live.

  Fetch the checksummed terrain package once:

  ```sh
  obc fixtures sync sim-peak-view
  target/release/obc-sim apps/obc-sim/assets/grimsel-demo.obcm --peak-view scheidegg
  ```

  Files use the standard fixture cache and its `OBC_FIXTURE_ROOT` / `OBC_FIXTURE_CACHE` overrides.
  For a local bake, first run `cargo build --release -p obc-dem`, then
  `python3 fixtures/generate_peak_view.py --download --terrain-dir PATH`. This writes one
  indexed OBCT file per site. Run with `OBC_PEAK_TERRAIN_DIR=PATH`; the sim never downloads terrain.
  See the [fixture notes](../../fixtures/sources/peak-view/README.md) for provenance.
  Headless scripts start on the normal screen. Use `f` to finish generation before Browse input:

  ```sh
  target/release/obc-sim apps/obc-sim/assets/grimsel-demo.obcm --peak-view scheidegg \
    --script "B d d d d p f d" --expect-screen PeakView --png peak-view.png
  ```

  The log separates generation time from the final cached frame's drawing time.
  These are host measurements. See the [board README](../../firmware/obc-fw-nrf54l/README.md)
  for device setup and timing checks.

## Ride and storage fixtures

### Ride Assistant interaction study

Run the shop-visit prototype on the supplied map:

```sh
cargo build -p obc-sim --release
target/release/obc-sim apps/obc-sim/assets/grimsel-demo.obcm --assistant-demo
```

This opt-in study runs in `obc-app`, with the device's 240×320 layout, fonts, map renderer,
button handling, and recording system. It starts a ride to Grimselpass on a temporary card.
The two shops and their access paths are fictional. Preloaded route legs supply the distance
and climbing figures. This is an English, metric UI study; it does not search for real shops,
calculate access routes, or detect arrival from GPS.

Open the top drawer with Up + Select (Left arrow + Enter). Step once to **Ride Assistant**,
then select **Find a place → Shop**. Compare **On the way** with **Nearest**, preview a visit,
and select **Add stop**. The preview shows the extra distance and climbing for the full visit,
including the return. The other questions and categories are grey placeholders.
A/B identify the results during comparison. After selection, the shop uses the existing basket icon.
The study sets Climb mode to Manual so a climb does not interrupt the map comparison.

Adding a stop opens the ordinary navigation map. Select still pauses, Back still opens Statistics,
and Up/Down still zoom. The same ride remains open. Use **Arrive at shop** in the simulator's
Controls panel to raise the arrival screen. **Continue ride** starts the prepared return leg.
Use **Rejoin original route** in Controls to restore the original route. These two Controls buttons
simulate location events; they are not new device buttons.

During a visit, the drawer opens the current visit rather than the question list:

| Ride stage | Navigation | Ride Assistant |
| --- | --- | --- |
| Going to the shop | Follow the selected path to the shop. | Show the visit, or remove the stop. |
| At the shop | The current path ends at the shop. | Show the arrival screen with **Continue ride**. |
| Returning to the route | Follow the prepared return leg. | Show the current visit. |
| Back on the original route | Resume the original route. | Show the question list again. |

Back on the arrival screen dismisses that screen and shows the map. It does not cancel the visit,
pause recording, or start return guidance. Open the drawer and select **Ride Assistant** to show
the same arrival screen again. Select **Continue ride** to start the return leg. Back on the normal
navigation map still opens Statistics.

Before arrival, the Assistant also has **Skip stop**. Confirming removal restores the original
route at the current position; it does not calculate a path back. The original destination remains
Grimselpass. This explicit arrival confirmation is the current prototype behavior.

The flag replaces the drawer's Bluetooth shortcut for this study. Bluetooth settings remain in
Settings. Without the flag, the Assistant is absent. The flag requires a temporary card and cannot
be combined with `--card`, `--create-card`, or `--gpx`. Do not change route catalogs during a study.

For headless captures, `A` simulates arrival, or rejoining after Continue ride. For example:

```sh
target/release/obc-sim apps/obc-sim/assets/grimsel-demo.obcm --assistant-demo \
  --script "Q w d p p p d p" --expect-screen Assistant --png shop-preview.png
target/release/obc-sim apps/obc-sim/assets/grimsel-demo.obcm --assistant-demo \
  --script "Q w d p p p d p p f A" --expect-screen Assistant --png shop-arrival.png
```

Physical-device execution, real POI data, automatic arrival detection, and the remaining Assistant
questions are outside this prototype.

### Storage inputs

- `--gpx PATH` replays a GPX track as the location source.
- `--at SECONDS` chooses the GPX playback instant for a headless frame (default: midpoint).
- `--routes-dir DIR` imports sorted `.obcr` and `.obt` fixtures once (default `routes/`). It cannot be combined with `--card`. Trip stage references are remapped to committed route IDs; missing stages remain missing.
- `--tracks-dir DIR` selects saved-ride import inputs and GPX export output (default `tracks/`).
  A new session imports valid `ride-{number}.obcr` files without changing them. `--card` does not
  import or rescan that directory. Runtime recording and the ride catalog use the shared card.
  A successful Save can export `ride-{card-id}.gpx`; an existing output file is not overwritten.
  Export failure leaves the committed ride on the card.
- `--import PATH` commits a GPX as a route to `--card`, or converts it to an `.obcr` file in `--routes-dir`, then exits. No map is required.
- `--route-retention LEVEL:AGE` commits route-retention metadata to the session card and reloads it. `LEVEL` is 0–5; `AGE` accepts
  seconds, `h`, `d`, or `unknown` (for example `3:2d`).

## Device state

- `--boot` starts a headless render at the real power-on Home state rather than Map.
- `--battery PCT` sets the initial battery charge (0–100).
- `--clock YYYY-MM-DDTHH:MM` pins the UTC wall-clock anchor.
- `--lang en|de|fr|es` chooses the headless UI language.
- `--stat-fields LIST` replaces the Statistics grid with comma-separated field ids.
- `--physical` uses saved physical-size calibration for the GUI. Open calibration and choose any
  housing colorway in the GUI control panel.
- `--ble connected|paired|passkey=N` sets typed BLE facts; join independent facts with `+` (for
  example, `connected+paired`). Passkeys are 0–999999.
- `--sensors demo|screen` selects either fixed live HR/power/cadence tiles or the saved-sensor and
  scan-list fixture.

## Scripted snapshots

- `--script TOKENS` applies device input before a headless render. `d`/`u` step, `p` selects, `h`
  holds Select, `b` goes back, `B` holds Back, `H`/`M` leave a partial hold, `Q` squeezes the
  Up+Select chord that opens the universal quick drawer, `w` settles animation,
  `f` draws one preparation frame, `T` performs one route-aware tick, and `I` triggers idle return.
- `--expect-screen NAME` refuses the render if the script lands on another screen.
- `--hold nav|detour` consumes exactly one planner request without starting it, preserving its
  spinner snapshot.
- `--inject EVENT` injects one mutually-exclusive host event:
  `nav-fail=exhausted|nopath`, `detour-fail=exhausted|nopath`, `upload=ID`,
  `upload-replace=ID`, `trip-upload=N`, `map-transfer=receiving:RECEIVED/TOTAL`,
  `map-transfer=installed`, `map-transfer=failed:KIND`, or `warning=LIST`. Warning tokens are
  `gps,altimeter,compass,map,rec`; map-transfer failure kinds are `storage`, `damaged`, `notamap`,
  and `refused`. `trip-upload=N` names the file `TP{N}.OBT` in the `--routes-dir` and is not available with `--card`. The map-transfer figures
  are kibibytes — the unit the board's own progress seam carries. An aborted or unplugged transfer
  has no form: it clears the card rather than raising one.
- `--dfu STATE` selects one complete DFU fixture state: `scan=KIND`, `progress=KIND`,
  `installing=KIND`, `error=ERR`, `confirmed=VERSION`, or `failed=WHY[:VERSION]`. Scan kinds are
  `normal`, `same`, and `first`; errors are `notfound`, `unreadable`, `damaged`, `toolarge`,
  `fragmented`, and `untrusted`; failure reasons are `notstarted` and `reverted`.
- `--freeze` engages the production recalculation freeze for an over-map banner snapshot.

## Help

- `-h` or `--help` prints the grouped command reference and exits successfully without a map.

The committed snapshot sweep is [`firmware/ui-snapshots.sh`](../../firmware/ui-snapshots.sh). When
changing command spelling or fixture ownership, compare the surviving `--png` outputs byte for
byte; delete a scenario only when its capability was intentionally removed.
