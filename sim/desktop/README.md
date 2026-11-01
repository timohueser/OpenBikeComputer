# OBC simulator

`obc-sim` runs the device's `obc-app` and `obc-render` code in a desktop window or as a
deterministic headless PNG, both in the panel's RGB222 64-colour gamut. The window's control panel
sets location, sensors, BLE, housing colorway and display calibration; these controls have no
startup flags.

## Run

```sh
sudo apt-get install libasound2-dev   # Linux only: ALSA headers for cue playback
obc sim                               # build and open the Grimsel Pass demo scenario
cargo build -p obc-sim --release
target/release/obc-sim map.obcm       # any packed map
target/release/obc-sim map.obcm --png frame.png
```

The window plays cues on the default audio device. `--help` lists every flag.

## Device controls in the window

The four housing buttons are clickable. The keyboard drives the same raw GPIO edges as the
firmware sees, so the device-wide chords work as on the device.

| Key | Button |
| --- | --- |
| Left arrow | Up |
| Right arrow | Down |
| Enter | Select |
| Backspace | Back |

Press Up + Select (Left and Enter) **within 100 ms of each other**: release before 500 ms for the
quick drawer, hold both for 500 ms for Ride Assistant. A larger gap is two ordinary gestures.
Down + Back (Right and Backspace) opens the contextual drawer, with the same 100 ms window. The
mouse wheel over the screen injects selection steps; it models no button and makes no chord.

## The card

Startup copies the map through a 16 KiB buffer into a temporary sparse card, then imports the
route, trip and saved-ride fixtures. Every runtime reader uses that card; the input files stay
unchanged. Unix can keep a card; Windows cannot.

```sh
target/release/obc-sim freiburg.obcm --create-card ride.obc --routes-dir routes/
target/release/obc-sim --card ride.obc
target/release/obc-sim --import next-stage.gpx --card ride.obc
```

Reopening never initialises, resets or migrates a card; it needs exactly one readable map
and complete route and trip catalogs. The last reader keeps the card's exclusive file lock, even
after the session closes, so close every reader before you reopen after an uncertain write.

After an interrupted recording, `--card` offers Continue or Discard for the last valid checkpoint.
Continue keeps the card identity and the accepted totals. If Save already journalled its footer,
startup completes the catalog commit and lists the saved ride instead. Failed durability
confirmation or terminal settlement stops startup.

A reopened map is labelled **Card map**. Planning and map-referenced altitude sample the terrain
inside that map object; there is no external `.obcd` sidecar. Missing terrain leaves elevations
unavailable; unreadable terrain reports a diagnostic and keeps the map usable.

## Peak View

Peak View appears in the normal menu when the loaded map carries indexed terrain. It uses the
current GPS position and the map's own summit records, so a headless run needs
`--center LON,LAT --heading DEG`. Without a fix it waits; it never uses the camera centre as an
observer.

Fetch the `--peak-view` fixtures once:

```sh
obc fixtures sync sim-peak-view
target/release/obc-sim sim/desktop/assets/grimsel-demo.obcm --peak-view scheidegg
```

Files use the standard fixture cache and its `OBC_FIXTURE_ROOT` and `OBC_FIXTURE_CACHE`
overrides. For a local bake, build `obc-dem` and run
`python3 fixtures/generate_peak_view.py --download --terrain-dir PATH`, then set
`OBC_PEAK_TERRAIN_DIR=PATH`. **The simulator never downloads terrain.** Provenance is in the
[fixture notes](../../fixtures/sources/peak-view/README.md).

Generation runs in the background, so a headless script must send `f` to finish it before Browse
input:

```sh
target/release/obc-sim sim/desktop/assets/grimsel-demo.obcm --peak-view scheidegg \
  --script "B d d d d p f d" --expect-screen PeakView --png peak-view.png
```

To reach Mönch through the normal installed-map path instead of a preset:

```sh
target/release/obc-sim sim/desktop/assets/grimsel-demo.obcm \
  --center 7961000,46585000 --heading 141.25 \
  --script "B d d d p f p f p f" --expect-screen PeakArticle --png peak-article.png
```

Add `u f` for its photo, or `u f b b f` to come back to Browse. Fixture presets use synthetic
summit identities and have no articles. The log separates generation time from the final frame's
drawing time. Both are host times, not device times; the
[board README](../../firmware/obc-fw-nrf54l/README.md) covers device timing.

## Ride Assistant

Hold Up + Select for 500 ms to open Assistant, then pick **Find a place**, **What's next**,
**Easier route** or **Landmarks**. The map context drawer has the same menu. Assistant needs an
installed OBCM v17 or later map and either an explicit position or a played GPS track.

```sh
cargo build -p obc-sim --bin obc-sim --locked
target/debug/obc-sim --card west-cork.obc --center -9829419,51482665 --heading 0 \
  --script 'A d d d d d p f p f' --expect-screen Landmarks --png landmark-text.png
```

A map without landmark content, or a route without coverage, shows its unavailable state.

## GPX replay and scripts

While a `--gpx` track is paused, the GPS sensor refreshes its position once per host second, so
Assistant can use it. Playback and ride time stay paused, and loading a track starts neither
navigation nor recording. `--script-at 30 --at 120` runs the script at second 30 and then replays
90 seconds of motion. Both positions must be in the track, and the endpoint cannot be earlier.
Use `T` after you start a route and before you open a route action.

## Journey diagnostics

Add `--diagnostics PATH` to a headless `--png` run to write a JSONL trace:

```sh
cargo build -p obc-sim --bin obc-sim --locked
target/debug/obc-sim sim/desktop/assets/grimsel-demo.obcm --boot \
  --script 'Q f' --png drawer.png --diagnostics drawer.jsonl
```

Use the same map, GPX, card and input options as the reported problem. Each line has a sequence
number, an event name and a data object:

| Field or record | What it means |
| --- | --- |
| command, working directory, map identity, payload fingerprint | What was run, and against which bytes |
| script tokens, screen changes, pass inputs | What the input did |
| issued plans, pending outcomes, observed bulk feeders | What the app asked the host for |
| `host_since_pass_us` | Host time since pass entry, including observation overhead |
| `host_render_us` | Host time to draw the final frame |
| `finished` | The run completed. A trace without it is incomplete. |
| `failed` | A screen assertion failed. The observations before it are kept. |

Neither timing measures device performance. Domain values are Rust debug text, not a stable wire
contract. The trace holds no copy of the map or the GPX, no raw sensor samples and no
intermediate images. Records are written immediately, and an output error fails the command.

```sh
jq 'select(.event == "input_result" or .event == "pass_output" or .event == "executed")' drawer.jsonl
```

## Test

The committed sweep is the frame table [`firmware/ui-frames.toml`](../../firmware/ui-frames.toml),
which `obc shot` renders. `obc shot --list` names every frame and the screen it reaches. When you
change a command spelling or fixture ownership, compare the surviving frames byte for byte:
`obc shot <name> --vs origin/develop` prints the changed share of one frame. Delete a frame only
when its capability was intentionally removed.

## Reference

| Flag or token | Effect |
| --- | --- |
| `--help` | Lists every flag and the script tokens |
| `--create-card PATH` | Builds a card; refuses an existing path; leaves a partial card for inspection if an import fails |
| `--card PATH` | Opens a kept card |
| `--peak-view gornergrat\|scheidegg\|glockner` | Replaces the map's terrain with a geographic test fixture |
| `--center LON,LAT --heading DEG` | Supplies a position fix and a heading |
| `--lang en\|de\|fr\|es` | Sets the UI copy; article language comes from the installed source; the Units setting selects metric or imperial |
| `--gpx FILE` | Loads a track, paused at its first point |
| `--at SECONDS` | Sets the playback endpoint for a headless frame |
| `--script-at SECONDS` | Sets the position during `--script`; replay then advances from there to `--at` |
| `--script TOKENS` | Applies device input before a headless render |
| `--script-after TOKENS` | Applies device input after GPX replay; keeps the final position and ride clock |
| `--expect-screen NAME` | Refuses the render if the script lands on another screen |
| `--diagnostics PATH` | Writes a JSONL trace of a headless `--png` run; the file must be new and different from the PNG |
| token `f` | Draws one preparation frame; lets background work finish |
| token `T` | One route-aware tick; with `--gpx` it samples the real track position through the normal location input and active-route matcher |
