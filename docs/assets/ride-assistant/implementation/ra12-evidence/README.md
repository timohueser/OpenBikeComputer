# RA12 production entry evidence

Code: `035e9696`, `2cffd2d7`, `4f4cdde7`, and `04233098`. Public documentation is in separate
commits. The integration base was `39eaff27`; the final parent composition is `85b78dfc`.

## Production behavior

Up + Select opens the normal drawer. Assistant offers Find a place, What's next, Easier route,
and Landmarks. The three remaining questions are inactive. Bluetooth stays in Settings, and
Detour stays in the ride context. Find retains all categories and More places. Explore retains
source and category filters and authored waypoint details. Selected place details and Visit use
one owner. The read-only current visit can be reopened from Assistant's context drawer.
Replacing its route bytes invalidates that view without changing the durable checkpoint. A
covered visit view cannot stop a new Find request.

Shipping study data, compiled landmark photos, synthetic alternatives, the temporary `L` key,
and the temporary guided `find` command have been removed. The old POI category menu, Up Ahead
screen and NavConfirm screen have been removed; the shared PlanRoute executor is retained.
The public web tour now uses normal context and Assistant button gestures before its real
Find, Visit preview and acceptance. The simulator's `T` token polls the selected captured GPX
position through the normal host loop before its route-aware tick.

## Source data and captures

All commands below ran with network access denied by the macOS sandbox:

```sh
/usr/bin/sandbox-exec -p '(version 1)(allow default)(deny network*)' "$SIM" ...
```

`SIM` was the shared debug simulator at
`/Users/timo/Documents/OSM-agents/ra09-photo-phase/target/debug/obc-sim`.
The source-derived West Cork package is
`3098b38214e3206bea8947927aa4d0a44cf618535b91dc063a4b3baeaa826a26`.
Its map SHA-256 is `a48ebe53b9a545492b94ef4d59cdd2f371e70705683f092d9370112cccc29b23`.
The persistent-card text capture reopened the existing RA10 card, copied with a filesystem
clone. The other Cork captures used the published map. Dunlough Castle is Q5315471 with two
source text pages, an actual 8,096-byte photo, and 16 full attribution pages.

All captures use `--boot`. Cork uses `--center -9829419,51482665` unless a row states otherwise.
`LAND` is `Q d p d d d d d p f`.

| Frame | Language / entry | Observed result |
| --- | --- | --- |
| [Assistant](assistant-de.png) | de, `Q d p` | Four active questions and inactive placeholders; labels fit |
| [Text](landmark-text.png) | en, persistent card, `LAND p f` | Actual source article and three-page text/photo sequence |
| [Photo](photo-fr.png) | fr, `LAND p f u f` | Actual castle image and French controls |
| [Sources](sources-fr.png) | fr, `LAND p f C p f` | Full wrapped article/image credits, page 1/16 |
| [More places](find-more-es.png) | es, `Q d p p p f u u f` | Bounded 10 km / partial 50 km scope |
| [Imperial](landmarks-imperial.png) | Normal Settings Units change, then `LAND` | Feet in the landmark distance |
| [No route](whats-next-no-route.png) | en, `Q d p d p f` | Explicit no-active-route state |
| [Visit preview](visit-preview.png) | Origin `-9825560,51485575`, `LAND p p f p f` | Actual 110 m mapped connection |
| [Accepted visit](visit-accepted.png) | Same origin, add `p f` | Accepted Map, without a confirmation bypass |
| [Riding Assistant](quick-assistant-de.png) | de, actual Grimsel route + GPX30, `p p p p Q d p w` | Normal riding drawer opens Assistant |
| [Easier](easier-route.png) | Grimsel route + GPX30, `p p p p b T Q d p d d p f` | Actual admitted comparison, no useful alternative |
| [Explore source editor](explore-sources.png) | Monaco route + GPX60, `UP C d p w d` | Source editor over the real timeline |
| [Explore place](explore-poi.png) | Monaco route + GPX60, `UP C d p w d d p w b f f f f f f f f p f` | Selected Ambassador detail, explicit no-road-access state |
| [Detour](detour-preview.png) | Monaco route + GPX60, `p p p p T C d p d d p f` | Actual alternative, +533 m, unknown ascent shown as unknown |

`UP` is `p p p p T Q d p d p f p f`. Monaco uses the normal simulator import of
`sim-monaco/tracks/monaco-upahead.gpx`. Grimsel uses only the captured
`sim-grimsel/routes/grimsel-climb.obcr` and `tracks/grimsel-climb.gpx`.
The eight prepared frames after a source change let the bounded query finish before selecting
a row. The final Detour capture uses the new actual GPX input; it replaces an earlier +72 m
capture made before that input was composed.

Earlier Monaco Maison Mullot probes could not offer Visit because that source has no mapped
access. One Explore probe selected before its query finished and remained on the timeline.
These failed probes are not acceptance evidence. The inherited RA03 `detour-chooser.png`
mentioned in earlier parent notes is also an earlier failed probe, not a final result.

## Verification

- `tools/obc test -p obc-app -p obc-sim -p obc-web-demo`: App library 901 tests and all App
  integration suites passed. This run then found a simulator test that assumed the removed BLE
  drawer action; its setup now explicitly tests the real brightness page.
- `tools/obc test fixtures -p obc-sim -p obc-web-demo`: 67 simulator tests, 11 simulator integration
  tests and 16 web tests passed. This includes actual Visit acceptance while recording and the
  simulator presentation dwell checks.
- `cargo clippy --locked -p obc-app -p obc-sim -p obc-web-demo --all-targets -- -D warnings`: passed.
- `cargo build --locked -p obc-sim --bin obc-sim`: passed; this binary produced the final named
  Easier, riding drawer, Explore and Detour frames above.
- `cargo fmt --all`, and `cargo fmt` in all three standalone Cargo roots: passed.
- `tools/obc suites check`: passed; 68 suites and 323 discovered execution units.
- `python3 docs/build_docs.py --check-links`: passed.
- `bash -n firmware/ui-snapshots.sh` and `git diff --check`: passed.

Independent review found three issues: replaced accepted-route bytes, a covered read-only visit
blocking Find, and long imperial comparison totals. Commit `2cffd2d7` fixes them. The focused
regression uses ordinary nested entry, preserves the checkpoint and recording session, and
checks both table columns without changing font size. The independent delta review was clean.
The normal-entry tour and final snapshot recipe composition have a separate delta review.

## Remaining integrated acceptance

RA13 owns the single final snapshot sweep, manifest hashes, exact resource measurement and
independent integrated acceptance. No local sweep, shipping-image build, base-resource rebuild,
or wake profile was run for RA12. The Easier named frame proves real comparison admission and
an honest no-alternative result; it does not claim a successful Easier acceptance.

The orchestrator is implementing the remaining RA05 production controls separately: explicit
Visit cancellation through the existing semantic API, and the once-only informative arrival
notice. Back on the current-visit view remains dismissal only. Final ordinary-entry recording,
return and recovery acceptance uses the integrated head. Hardware acceptance is pending; the
device is not connected. All independent software work continues.
