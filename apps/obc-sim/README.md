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
temporary sparse card, then imports the route and trip fixtures. All runtime readers use that same
card. Input files stay unchanged. The final reader releases the temporary card.

On Unix, create a persistent card explicitly, then reopen it without importing the files again:

```sh
target/release/obc-sim freiburg.obcm --create-card ride.obc --routes-dir routes/ --weather forecast.obcw
target/release/obc-sim --card ride.obc
target/release/obc-sim --import next-stage.gpx --card ride.obc
```

Creation refuses an existing path and exits after importing. If an import fails, it reports failure
and leaves the partial card for inspection. Reopening never initializes or resets the file. It
requires exactly one readable map, complete route and trip catalogs, and valid installed weather
when present. The last reader keeps the card's exclusive file lock, even after the session closes.
Persistent cards are not supported on Windows; ordinary temporary sessions remain available.

A reopened map is labelled **Card map**. Planner elevation uses `NullElevation` in this mode;
it does not infer a path to an external terrain sidecar. Embedded map terrain remains available
to Peak View. Ordinary OBCM sessions keep the existing `.obcd` sidecar lookup for planning.

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
headroom. Low-relief observers receive up to 3× vertical exaggeration, fixed while turning;
steep views keep 1.25×. The summit candidates reserve a slot for each bearing sector's tallest
landmark. Explicit fixture frames retain their configured bounds. In a headless test,
`--center LON,LAT --heading DEG` supplies an explicit simulated fix. Use the menu and `f` to complete generation before saving the frame.

- `--peak-view gornergrat|scheidegg|glockner` selects an explicit geographic test fixture and
  opens Peak View in the GUI. This overrides the selected map's terrain for that test. It generates a
  panorama from geographic terrain, with the current direction first. The compass spinner stops
  when that view is ready; three static dots indicate background work on the remaining directions.
  Background work extends both edges in about 17-degree batches. Turning prioritizes the new
  direction. After the first view appears, completed terrain stays visible and follows the
  heading; a light hatch marks pending parts until they fill in. Back cancels. Drag **Compass (heading when
  stopped)** in Controls to turn. Changing the GPS position by more than 20 m rebuilds the view;
  smaller changes keep the current panorama to limit GPS jitter. The fixture has a limited area
  of fine terrain around each preset. Missing distant coverage shows
  dashed marks over affected bearings. Moving outside observer coverage shows Terrain unavailable.
  Each geographic terrain file contains heights, lower-resolution levels and baked height
  bounds. The renderer skips hidden blocks and shades visible terrain slopes under fixed
  illustration lighting. It does not store viewpoints or show snow and current sunlight.
  Named summits are aligned and checked for visibility during generation.
  Browse freezes the heading; Select returns to Live.

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

  The log separates generation time and storage reads from the final cached frame's drawing time.
  These are host measurements. See the [board README](../../firmware/obc-fw-nrf54l/README.md)
  for device setup and timing checks.

## Ride and storage fixtures

- `--gpx PATH` replays a GPX track as the location source.
- `--at SECONDS` chooses the GPX playback instant for a headless frame (default: midpoint).
- `--routes-dir DIR` imports sorted `.obcr` and `.obt` fixtures once (default `routes/`). It cannot be combined with `--card`. Trip stage references are remapped to committed route IDs; missing stages remain missing.
- `--tracks-dir DIR` mounts the ride/track store (default `tracks/`).
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

## Weather

These are independent product controls, not part of the simulator-fixture consolidation:

- `--weather FILE.obcw|demo[:SCENARIO]|live` imports weather from a file, deterministic demo, or live service
  into the session card. `--card` without this flag reopens the installed bundle. No folder data is
  migrated. A failed import leaves the prior committed data available; an uncertain commit requires
  closing the session and reopening the card. Reader-slot pressure after a successful commit retries
  reader acquisition without importing again.
  Demo scenarios are `scattered` (the default), `drizzle`, `frontal`, `storm`, `dry`, `incoming`,
  `stormahead`, `rainahead`, `gusty`, and `hourly`.
- `--weather-now UNIX` overrides the freshness instant.
- `--weather-refreshing` shows the non-blocking updating cue.
- `--weather-alert rain[:MIN]|storm[:MIN]|gust[:MIN]` displays an alert card.
- `--weather-decide` runs the production route-projected alert decision for the final frame.
- `--weather-service URL` changes the live service origin.
- `--weather-radius-km KM` changes the live corridor radius.
- `--weather-offline` forces the live client offline.
- `--weather-fault corrupt-request=N|truncate-request=N|fail-from=N:CODE|latency=MS` applies one
  typed live-client fault. Repeat the option to compose independent faults, matching the former
  independent flags.
- `--no-card` simulates no writable companion storage, suppressing weather requests.

## Help

- `-h` or `--help` prints the grouped command reference and exits successfully without a map.

The committed snapshot sweep is [`firmware/ui-snapshots.sh`](../../firmware/ui-snapshots.sh). When
changing command spelling or fixture ownership, compare the surviving `--png` outputs byte for
byte; delete a scenario only when its capability was intentionally removed.
