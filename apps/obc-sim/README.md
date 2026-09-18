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
Left and Enter (Up + Select) **within 100 ms of each other**. Release before 500 ms to open the
quick drawer, or hold both for 500 ms to open Ride Assistant directly. A larger gap is two ordinary
gestures. Down + Back (Right and Backspace) opens the contextual drawer with the same 100 ms window.

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
  through visible peaks and turns the view by 15° past an edge. Back returns to Live.
  For a selected map summit with readable installed text, an information mark appears and Select
  opens its article. Up/Down moves through text and the optional photo; **Down + Back → Sources**
  opens credits. Back restores the selected summit and Browse heading. Select on a peak without
  content returns to Live. Explicit terrain presets use synthetic summit identities without articles.

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

  To read Mönch through the normal installed-map path (without a terrain preset):

  ```sh
  target/release/obc-sim apps/obc-sim/assets/grimsel-demo.obcm \
    --center 7961000,46585000 --heading 141.25 \
    --script "B d d d p f p f p f" --expect-screen PeakArticle --png peak-article.png
  ```

  Add `u f` to open its photo, or `u f b b f` to return from the photo to Browse.
  Use `--expect-screen LandmarkPhoto` or `--expect-screen PeakView` for those destinations.
  The photo screen name refers to the shared presentation. Peak articles do not offer Visit.

  The log separates generation time from the final cached frame's drawing time.
  These are host measurements. See the [board README](../../firmware/obc-fw-nrf54l/README.md)
  for device setup and timing checks.

## Ride and storage fixtures

### Ride Assistant

Hold **Up + Select** (Left arrow + Enter) for **500 ms** to open Assistant, then select
**Find a place**, **What's next**, **Easier route**, or **Landmarks**. The same Assistant menu is
available from the map context. Tap **Up + Select** for the quick drawer, which includes the
Bluetooth on/off control. **Peak View** stays in the main menu; its screen reports missing GPS or
terrain data. Bluetooth is also under **Settings → Connections → Phone**.
The normal detour command remains in the map context. The three grey questions are inactive.

Use an installed OBCM v17 map and an explicit simulator position, or play a captured GPS track.
Find lists all place categories. More places opens the complete bounded result list. What's next
uses the accepted route; **Explore ahead** includes authored waypoints and map places. Its
**Down + Back** drawer retains category and source filters. Authored waypoint details do not add
another stop. Easier route compares real planner results and offers a preview before acceptance.

Landmarks reads the installed source text, article language, optional image and full attribution.
Up/Down pages through text and the photo. **Down + Back → Sources** opens article and image
credits; Back restores the selected site and reading page. Missing mapped access leaves a site
information-only. Opening hours and the selected bike profile govern Visit. The shared Visit
preview plans the actual connection before acceptance. Browsing does not start recording.

For a named capture through normal physical-button gestures:

```sh
cargo build -p obc-sim --bin obc-sim --locked
target/debug/obc-sim --card west-cork.obc --center -9829419,51482665 --heading 0 \
  --script 'A d d d d d p f p f' --expect-screen Landmarks --png landmark-text.png
```

The explicit center is a simulated GPS fix. Use `--lang en|de|fr|es` for UI copy; article language
comes from the installed source. Metric/imperial follows the normal Units setting. A map without
landmark content or a route without coverage shows its unavailable state. There are no study
fixtures, synthetic routes, scripted arrival events, or `--assistant-*` controls in this path.

### Storage inputs

- `--gpx PATH` replays a GPX track as the location source. The GUI loads it paused at the
  first point. While paused, the GPS sensor refreshes that position once per second of host
  time, so it remains usable in Assistant and other position-based views. Playback and ride
  time stay paused. Loading a track does not start navigation or recording.
- `--at SECONDS` chooses the GPX playback endpoint for a headless frame (default: midpoint).
- `--script-at SECONDS` sets the GPX position during `--script`, including its `T` inputs.
  It requires `--gpx`, `--png`, and `--script`. After the script, replay advances from this
  position to `--at`; both positions must be within the track and the endpoint cannot be earlier.
  Ride time starts at zero at the selected position. For example, `--script-at 30 --at 120`
  runs the script at second 30, then replays 90 seconds of actual GPS motion. Equal start and
  end positions run the script without further motion. Without `--script-at`, the script uses
  the endpoint position and the subsequent replay starts at the beginning, as before.
- `--routes-dir DIR` imports sorted `.obcr` and `.obt` fixtures once (default `routes/`). It cannot be combined with `--card`. Trip stage references are remapped to committed route IDs; missing stages remain missing.
- `--tracks-dir DIR` selects saved-ride import inputs and GPX export output (default `tracks/`).
  A new session imports valid `ride-{number}.obcr` files without changing them. `--card` does not
  import or rescan that directory. Runtime recording and the ride catalog use the shared card.
  A successful Save can export `ride-{card-id}.gpx`; an existing output file is not overwritten.
  Export failure leaves the committed ride on the card.
- `--import PATH` commits a GPX as a route to `--card`, or converts it to an `.obcr` file in `--routes-dir`, then exits. No map is required.

## Device state

- `--boot` starts a headless render at the real power-on Home state rather than Map.
- `--battery PCT` sets the initial battery charge (0–100).
- `--clock YYYY-MM-DDTHH:MM` supplies trusted UTC time and a known local offset (default `+00:00`)
  at startup in both GUI and headless modes. It uses the same clock entry as a phone time update.
  In GUI mode, an explicit clock starts with ambient GPS time disabled in the control panel. Without an
  explicit time, the boot clock stays untrusted and current opening status is unknown.
- `--clock-after-script YYYY-MM-DDTHH:MM` supplies another trusted UTC time with the same local offset
  in headless mode after the button script and before the final settle and render. Use it to observe an open detail
  after its place closes. It changes wall-clock time; it does not advance ride duration.
- `--utc-offset-min MINUTES` sets the local offset for both explicit clock options. It uses the
  device's range, -720 to 840 minutes, and requires an explicit clock. For example, use
  `--clock 2026-09-14T10:00 --utc-offset-min 120` for 12:00 local time in the Swiss replay,
  or offset `60` for 11:00 local time in the West Cork replay. The supplied UTC time is unchanged.
- `--route-cleanup` opens the storage-full cleanup dialog. Combine with `--clock` to preview the age picker; without it the dialog shows the unknown-date guidance.
- `--lang en|de|fr|es` chooses the headless UI language.
- `--stat-fields LIST` replaces the Statistics grid with comma-separated field ids.
- `--physical` uses saved physical-size calibration for the GUI. Open calibration and choose any
  housing colorway in the GUI control panel.
- `--ble connected|paired|passkey=N` sets typed BLE facts; join independent facts with `+` (for
  example, `connected+paired`). Passkeys are 0–999999.
- `--sensors demo|screen` selects either fixed live HR/power/cadence tiles or the saved-sensor and
  scan-list fixture.

## Scripted snapshots

- `--script-after TOKENS` applies normal device input after GPX replay, before the final render.
  It requires `--gpx` and `--png`. It continues the button clock and retains the final GPS position
  and ride clock. `T` refreshes that position without replaying earlier motion. Use `--script-after
  'p f d h f'` from the riding Map to pause, select Finish, hold to save, and complete pending writes
  before the process exits. This saves the recorder's final partial batch in the same session.
- `--script TOKENS` applies device input before a headless render. `d`/`u` step, `p` selects, `h`
  holds Select, `b` goes back, `B` holds Back, `H`/`M` leave a partial hold, `Q` squeezes the
  Up+Select chord that opens the universal quick drawer, `A` holds Up+Select to open Assistant,
  `w` settles animation,
  `f` draws one preparation frame, `T` performs one route-aware tick, and `I` triggers idle return.
  With `--gpx`, `T` samples the actual track position selected by `--script-at` (or `--at` when omitted) through the normal location
  input and active-route matcher. Use it after starting a route and before opening a route action.
  It keeps the interaction clock and the pre-replay ride epoch; the full GPX replay still follows
  the script.
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

## Journey diagnostics

Add `--diagnostics PATH` to a normal headless `--png` run to write a new JSONL trace:

```sh
cargo build -p obc-sim --bin obc-sim --locked
target/debug/obc-sim apps/obc-sim/assets/grimsel-demo.obcm --boot \
  --script 'Q f' --png drawer.png --diagnostics drawer.jsonl
```

Use the same map, GPX, card and input options as the reported problem. The trace records the
command, working directory, map object identity and stored payload fingerprint, script tokens
and screen changes, pass inputs, issued plans, pending outcomes, observed bulk feeders, and final
render counters. Effect and outcome debug values include their operation tokens. Each line has a
sequence number, event name and data object. Domain values use Rust debug text; they are diagnostic
information, not a stable wire contract. The trace does not contain a copy of the map or GPX.

The observer uses the existing host executor. It does not inject results or run extra passes.
`host_since_pass_us` is elapsed host time since pass entry, including observation overhead;
`host_render_us` measures the final render. Neither value measures device performance.
Raw sensor samples and intermediate images are not captured. The PNG remains the final frame.

The output file must be new and different from the PNG. Records are written immediately, so a
failed screen assertion retains its preceding observations and a `failed` record. A successful
run ends with `finished`; a missing terminal record means the trace is incomplete. Output errors
fail the command. Without `--diagnostics`, the host has no attached observer.

Inspect selected events with a JSONL reader, for example:

```sh
jq 'select(.event == "input_result" or .event == "pass_output" or .event == "executed")' drawer.jsonl
```

## Help

- `-h` or `--help` prints the grouped command reference and exits successfully without a map.

The committed snapshot sweep is the frame table
[`firmware/ui-frames.toml`](../../firmware/ui-frames.toml), which `obc shot` renders.
`obc shot --list` names every frame and the screen it reaches, so you do not have to read the table
to find a recipe. When changing command spelling or fixture ownership, compare the surviving frames
byte for byte — `obc shot <name> --vs origin/develop` prints the changed share of one frame — and
delete a frame only when its capability was intentionally removed.

### Ride Assistant validation

Use the ordinary drawer entry shown above. The RA10 [source and card evidence](../../docs/assets/ride-assistant/implementation/ra10-evidence/README.md)
records the real West Cork and Grimsel data. The temporary `L` entry has been removed.
Final integrated acceptance includes the ordinary journey while recording; hardware acceptance
remains a separate device check.
