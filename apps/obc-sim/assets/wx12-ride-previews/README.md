# WX12 ride-decision previews (PR review frames — not fixtures)

Committed 240 × 320 frames of the WX12 riding intelligence (epic #1185, #1197) for PR-body
review, like `../wx11-weather-previews`. Nothing pins these bytes in CI; regenerate after any
visual change with the commands below (they are also the `ui-snapshots.sh` WX12 block, which
stays the byte-stable sweep).

Every frame runs the **production** path: `p p p p` rides the Grimsel fixture route, the GPX
replay locks the matcher, and `--weather-decide` samples the demo bundle **route-projected**
(`App::ride_projection` → `WeatherSnapshot::sample_along`) and runs the real alert engine
(`App::weather_alert_tick` — thresholds, dedup, cooldown) on the final frame. The
`stormahead`/`rainahead` scenarios are stationary precipitation rings around the grid centre:
parked at the centre the dashboard honestly reads DRY FOR 2 HOURS; the projected ride crosses
the ring 25–45 minutes out at touring pace.

```sh
cargo build --release -p obc-sim
S=target/release/obc-sim; M=apps/obc-sim/assets/grimsel.obcm; O=apps/obc-sim/assets/wx12-ride-previews
G=apps/obc-sim/assets/grimsel-climb.gpx
R="$(mktemp -d)"; cp apps/obc-sim/assets/grimsel-climb.obcr "$R/"
NAV="p d d d d w p"                 # Home -> Menu -> Weather -> dashboard (no ride)
RIDE="p p p p B u p d d d d w p"    # ride the route -> ride menu -> Main menu -> Weather

# The same sky, two honest answers: parked = DRY (the ring never crosses the parking spot)…
$S $M --boot --weather demo:stormahead --script "$NAV" --png $O/dash-parked-dry.png
# …riding = the ride crosses the ring: RAIN IN NN on the decision card (band-6 ring, below
# every alert threshold — the clean decision-card shot)…
$S $M --boot --routes-dir "$R" --gpx $G --at 1500 --weather demo:rainahead --weather-decide --script "$RIDE" --png $O/dash-ride-rain-ahead.png
# …and at ≥10 mm/h the alert engine fires the RAIN AHEAD card over it (dedup/cooldown live).
$S $M --boot --routes-dir "$R" --gpx $G --at 1500 --weather demo:stormahead --weather-decide --script "$RIDE" --png $O/alert-storm-engine.png
# Dangerous gusts (hourly ≥ 20 m/s): dry dashboard, STRONG WIND card — the new WX12 face.
$S $M --boot --weather demo:gusty --weather-decide --script "$NAV" --png $O/alert-gust.png
# Route-relative wind: the hourly arrows ink green/orange/red against the ride's travel
# direction (the replay-locked route tangent); the routeless WX11 shots stay neutral.
$S $M --boot --routes-dir "$R" --gpx $G --at 1500 --weather demo:rainahead --weather-decide --script "$RIDE p" --png $O/hourly-wind-route.png
rm -rf "$R"
```

The demo bundles anchor the app clock on their first frame when no `--clock` is passed, so every
derivation — decision card, alert timing, arrow colors — is deterministic.

Re-verified byte-identical after the adversarial review round (the horizon-widened DRY-claim
corridor, the off-route projection gate and the route-end clamp): none of these five scenes is a
dry claim under projection — the parked shot has no projection at all, the two riding shots cross
the ring and answer with a *warning*, and the gust/hourly shots don't read the rain grid — so the
review's conservatism lands on paths none of them exercise. That gap is itself a note for the
on-glass round: there is no committed frame of a projected DRY FOR 2 HOURS.

Re-verified again after the WX14 (#1231) merge, which rewired the plumbing underneath these
commands (`--weather live`, the semantic companion, `SimWeather::sync_clock`). Every scene here
is a `demo:` bundle whose recipe re-anchors onto the store's own first frame, so the re-anchor
is a no-op for them and the bytes are unchanged. The decision path itself is source-agnostic
now: `--weather-decide` decides over whichever bundle is loaded, `demo:` or `live`.
