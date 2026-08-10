# WX11 weather-screen previews (PR review frames — not fixtures)

Committed 240 × 320 frames of the WX11 weather surfaces (epic #1185) for PR-body review, like
`../wx10-rain-previews`. Nothing pins these bytes in CI; regenerate after any visual change with
the commands below (they are also the `ui-snapshots.sh` weather block, which stays the byte-stable
sweep).

```sh
cargo build --release -p obc-sim
S=target/release/obc-sim; M=apps/obc-sim/assets/grimsel.obcm; O=apps/obc-sim/assets/wx11-weather-previews
NAV="p d d d d w p"     # Home -> Menu -> Weather station -> dashboard
E="--expect-screen WeatherRainMap"   # the rain-map walks state where they land

$S $M --boot --weather demo:dry                                --script "$NAV"           --png $O/dash-dry.png
$S $M --boot --weather demo:incoming --weather-now 1800001500  --script "$NAV"           --png $O/dash-rain.png
$S $M --boot --weather demo:storm                              --script "$NAV"           --png $O/dash-storm.png
$S $M --boot --weather demo:storm --weather-now 1800012000     --script "$NAV"           --png $O/dash-stale.png
$S $M --boot --weather demo:hourly                             --script "$NAV"           --png $O/dash-hourly-only.png
$S $M --boot                                                   --script "$NAV"           --png $O/dash-nodata.png
$S $M --boot --weather demo:incoming --weather-refreshing      --script "$NAV"           --png $O/dash-refreshing.png
$S $M --boot --weather demo:incoming                           --script "$NAV p"         --png $O/hourly.png
$S $M --boot --weather demo:incoming                           --script "$NAV p d d d d d d" --png $O/hourly-scrolled.png
$S $M --boot --weather demo:scattered                          --script "$NAV d p"  $E     --png $O/rainmap-now.png
$S $M --boot --weather demo:scattered                          --script "$NAV d p d d" $E --png $O/rainmap-step2.png
$S $M --boot --weather demo:storm --weather-now 1800012000     --script "$NAV d p"  $E     --png $O/rainmap-stale.png
$S $M --boot --weather demo:hourly                             --script "$NAV d p"  $E     --png $O/rainmap-hourly-only.png
$S $M --boot --weather demo:scattered --zoom 0.02              --script "$NAV d p"  $E     --png $O/rainmap-zoom-clamped.png
$S $M --boot --weather demo:storm --weather-alert storm:28                               --png $O/alert-storm.png
$S $M --boot --weather demo:incoming --weather-now 1800001500 --weather-alert rain:34    --png $O/alert-rain.png
$S $M --boot --script "p d d d d d w p d d p"                                            --png $O/settings-weather.png
# Language spot-checks (full sweep: ui-snapshots.sh's de/fr/es loop)
$S $M --boot --weather demo:incoming --weather-now 1800001500 --lang de --script "$NAV"  --png $O/dash-rain-de.png
$S $M --boot --weather demo:incoming --lang de --script "$NAV p"                         --png $O/hourly-de.png
$S $M --boot --weather demo:incoming --lang fr --script "$NAV p"                         --png $O/hourly-fr.png
$S $M --boot --weather demo:storm --lang fr --weather-alert storm:28                     --png $O/alert-storm-fr.png
$S $M --boot --weather demo:storm --weather-now 1800012000 --lang es --script "$NAV"     --png $O/dash-stale-es.png
```

`rainmap-zoom-clamped` enters the rain map with the camera parked far outside the product's
zoom regime: the round-2 zoom clamp snaps it to the regime floor (`rain_min_zoom`, per-product
cell density), so the out-of-regime banner is no longer reachable through the UI — it survives
in code only as a defensive fallback, which is why there is no longer an out-of-regime preview.

The demo bundles anchor the app clock on their first frame (08:00 UTC, 2027-01-15) when no
`--clock` is passed, so every derivation — card countdown, strip, freshness line, frame labels —
is deterministic. `--weather-now 1800001500` views the `incoming` front 25 min in (card reads
RAIN IN 34 MIN); `1800012000` is past every frame's currency (the honest stale states).

Byte-stability law (review #1230): a preview command must omit `--clock` or pass
`--weather-now` — otherwise the demo bundle re-stamps itself onto the given clock and the
rendered bytes are no longer the committed ones.
