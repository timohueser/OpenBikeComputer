# OBC simulator

`obc-sim` runs the same `obc-app` and `obc-render` code as the device, in a desktop window or as a
deterministic headless PNG. Both paths use the panel's RGB222 64-colour gamut. The GUI adds a
control panel for location, sensors, BLE, housing colorway and display calibration; those live
controls are deliberately not duplicated as startup flags.

```sh
cargo build -p obc-sim --release
target/release/obc-sim freiburg.obcm
target/release/obc-sim freiburg.obcm --png frame.png
```

`target/release/obc-sim --help` lists every flag. This file covers what the help cannot: the input
model, the card, and the diagnostics trace.

## Device controls in the window

The four housing buttons are clickable, and the keyboard drives the same raw GPIO edges the
firmware sees.

| Key | Button |
| --- | --- |
| Left arrow | Up |
| Right arrow | Down |
| Enter | Select |
| Backspace | Back |

Because these are held-state edges, the device-wide **chords** work as they do on the device.
Press Up + Select (Left and Enter) **within 100 ms of each other**: release before 500 ms for the
quick drawer, hold both for 500 ms for Ride Assistant. A larger gap is two ordinary gestures.
Down + Back (Right and Backspace) opens the contextual drawer, same 100 ms window.

The mouse wheel over the screen injects selection steps directly. It models no button and makes no
chord.

## The card

Startup copies the map through a 16 KiB buffer into a temporary sparse card, then imports the
route, trip and saved-ride fixtures. Every runtime reader uses that card, and the input files stay
unchanged.

On Unix a card can be kept. Persistent cards are not supported on Windows.

```sh
target/release/obc-sim freiburg.obcm --create-card ride.obc --routes-dir routes/
target/release/obc-sim --card ride.obc
target/release/obc-sim --import next-stage.gpx --card ride.obc
```

`--create-card` refuses an existing path, and leaves a partial card for inspection if an import
fails. Reopening never initialises, resets or migrates a card; it needs exactly one readable map
and complete route and trip catalogs. The last reader keeps the card's exclusive file lock, even
after the session closes, so close every reader before you reopen after an uncertain write.

After an interrupted recording, `--card` offers Continue or Discard for the last valid checkpoint.
Continue keeps the card identity and the accepted totals. If Save already journalled its footer,
startup completes the catalog commit and lists the saved ride instead. Failed durability
confirmation or terminal settlement stops startup.

A reopened map is labelled **Card map**. Planning and map-referenced altitude sample the terrain
inside that map object; there is no external `.obcd` sidecar. Missing terrain leaves elevations
unavailable, and unreadable terrain reports a diagnostic and keeps the map usable.

## Peak View

Peak View appears in the normal menu when the loaded map carries indexed terrain. It uses the
simulator's current GPS position and the map's own summit records, so a headless run needs
`--center LON,LAT --heading DEG` to supply a fix. Without a fix it waits; it never uses the camera
centre as an observer.

`--peak-view gornergrat|scheidegg|glockner` overrides the map's terrain with a geographic test
fixture. Fetch it once:

```sh
obc fixtures sync sim-peak-view
target/release/obc-sim apps/obc-sim/assets/grimsel-demo.obcm --peak-view scheidegg
```

Files use the standard fixture cache and its `OBC_FIXTURE_ROOT` and `OBC_FIXTURE_CACHE`
overrides. For a local bake, build `obc-dem` and run
`python3 fixtures/generate_peak_view.py --download --terrain-dir PATH`, then set
`OBC_PEAK_TERRAIN_DIR=PATH`. **The simulator never downloads terrain.** Provenance is in the
[fixture notes](../../fixtures/sources/peak-view/README.md).

Generation runs in the background from the current heading outward, so a headless script must send
`f` to finish it before Browse input:

```sh
target/release/obc-sim apps/obc-sim/assets/grimsel-demo.obcm --peak-view scheidegg \
  --script "B d d d d p f d" --expect-screen PeakView --png peak-view.png
```

To reach Mönch through the normal installed-map path instead of a preset:

```sh
target/release/obc-sim apps/obc-sim/assets/grimsel-demo.obcm \
  --center 7961000,46585000 --heading 141.25 \
  --script "B d d d p f p f p f" --expect-screen PeakArticle --png peak-article.png
```

Add `u f` for its photo, or `u f b b f` to come back to Browse. Fixture presets use synthetic
summit identities and have no articles.

The log separates generation time from the final frame's drawing time. Both are host
measurements and say nothing about the device; see the
[board README](../../firmware/obc-fw-nrf54l/README.md) for device timing.

## Ride Assistant

Hold Up + Select for 500 ms to open Assistant, then pick **Find a place**, **What's next**,
**Easier route** or **Landmarks**. The same menu is in the map context drawer. Assistant needs an
installed OBCM v17 or later map and either an explicit position or a played GPS track.

```sh
cargo build -p obc-sim --bin obc-sim --locked
target/debug/obc-sim --card west-cork.obc --center -9829419,51482665 --heading 0 \
  --script 'A d d d d d p f p f' --expect-screen Landmarks --png landmark-text.png
```

`--lang en|de|fr|es` sets the UI copy; article language comes from the installed source, and
metric or imperial follows the Units setting. A map without landmark content, or a route without
coverage, shows its unavailable state.

## GPX replay

`--gpx` loads a track paused at its first point. While paused the GPS sensor refreshes that
position once per host second, so it stays usable in Assistant; playback and ride time stay
paused, and loading a track starts neither navigation nor recording.

`--at` picks the playback endpoint for a headless frame. `--script-at` sets the position during
`--script`, and replay then advances from there to `--at`: `--script-at 30 --at 120` runs the
script at second 30 and then replays 90 seconds of motion. Both positions must be in the track and
the endpoint cannot be earlier.

## Scripted snapshots

`--script TOKENS` applies device input before a headless render; `--script-after` applies it after
GPX replay, keeping the final position and ride clock. `--expect-screen NAME` refuses the render
if the script lands somewhere else.

The tokens are in `--help`. Two need a word here: `f` draws one preparation frame and is what lets
background work finish, and `T` performs one route-aware tick — with `--gpx` it samples the real
track position through the normal location input and active-route matcher, so use it after
starting a route and before opening a route action.

## Journey diagnostics

Add `--diagnostics PATH` to a headless `--png` run to write a JSONL trace:

```sh
cargo build -p obc-sim --bin obc-sim --locked
target/debug/obc-sim apps/obc-sim/assets/grimsel-demo.obcm --boot \
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

Neither timing measures device performance. Domain values are Rust debug text: diagnostic
information, not a stable wire contract. The trace holds no copy of the map or the GPX, no raw
sensor samples and no intermediate images.

The output file must be new and different from the PNG. Records are written immediately, and an
output error fails the command.

```sh
jq 'select(.event == "input_result" or .event == "pass_output" or .event == "executed")' drawer.jsonl
```

## Snapshots

The committed sweep is the frame table [`firmware/ui-frames.toml`](../../firmware/ui-frames.toml),
which `obc shot` renders. `obc shot --list` names every frame and the screen it reaches.
When you change a command spelling or fixture ownership, compare the surviving frames byte for
byte — `obc shot <name> --vs origin/develop` prints the changed share of one frame. Delete a frame
only when its capability was intentionally removed.
