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

## Map and output

- `--size WxH` changes the frame geometry from the device default (240×320).
- `--scale N` applies an integer scale to the window or saved PNG (default 1).
- `--png PATH` renders one device-gamut frame and exits. This is the screenshot-test interface.
- `--palette` shows the device's 64-colour palette, or saves it when combined with `--png`.
- `--center LON,LAT` sets the headless camera centre in integer microdegrees.
- `--zoom MULT` multiplies the headless bbox-fit zoom.
- `--heading DEG` starts heading-up at the given clockwise course.

## Ride and storage fixtures

- `--gpx PATH` replays a GPX track as the location source.
- `--at SECONDS` chooses the GPX playback instant for a headless frame (default: midpoint).
- `--routes-dir DIR` mounts a route store (default `routes/`).
- `--tracks-dir DIR` mounts the ride/track store (default `tracks/`).
- `--import PATH` converts a GPX into the route store and exits; no map is required.
- `--route-retention LEVEL:AGE` stamps route-retention metadata. `LEVEL` is 0–5; `AGE` accepts
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
  holds Select, `b` goes back, `B` holds Back, `H`/`M` leave a partial hold, `w` settles animation,
  `f` draws one preparation frame, `T` performs one route-aware tick, and `I` triggers idle return.
  The temporary context-drawer prototype uses `c` for no backdrop, `D` for palette-LUT dimming, `S`
  for a stippled scrim, `C` for fullscreen, and `A` to advance one quarter through its rise (`w`
  settles it). In the GUI, Down+Back opens the LUT-dimmed drawer. Up/Down browses; Select opens or
  commits the POI-filter and bike-type editors, while Back cancels an editor or closes the drawer.
  The top quick-drawer uses `q` (plain) or `Q` (dimmed); in the GUI the physical upper pair,
  Up+Select, opens it (`Left Arrow`+`Enter` on the keyboard). Its brightness and power-confirmation
  pages use the same controls.
- `--expect-screen NAME` refuses the render if the script lands on another screen.
- `--hold nav|detour` consumes exactly one planner request without starting it, preserving its
  spinner snapshot.
- `--inject EVENT` injects one mutually-exclusive host event:
  `nav-fail=exhausted|nopath`, `detour-fail=exhausted|nopath`, `upload=ID`,
  `upload-replace=ID`, `trip-upload=ID`, `map-transfer=receiving:RECEIVED/TOTAL`,
  `map-transfer=installed`, `map-transfer=failed:KIND`, or `warning=LIST`. Warning tokens are
  `gps,altimeter,compass,map,rec`; map-transfer failure kinds are `storage`, `damaged`, `notamap`,
  and `refused`. `trip-upload=ID` names a trip in the `--routes-dir`, and the map-transfer figures
  are kibibytes — the unit the board's own progress seam carries. An aborted or unplugged transfer
  has no form: it clears the card rather than raising one.
- `--dfu STATE` selects one complete DFU fixture state: `scan=KIND`, `progress=KIND`,
  `installing=KIND`, `error=ERR`, `confirmed=VERSION`, or `failed=WHY[:VERSION]`. Scan kinds are
  `normal`, `same`, and `first`; errors are `notfound`, `unreadable`, `damaged`, `toolarge`,
  `fragmented`, and `untrusted`; failure reasons are `notstarted` and `reverted`.
- `--freeze` engages the production recalculation freeze for an over-map banner snapshot.

## Weather

These are independent product controls, not part of the simulator-fixture consolidation:

- `--weather FILE.obcw|demo[:SCENARIO]|live` loads one weather bundle, deterministic demo, or live service.
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
