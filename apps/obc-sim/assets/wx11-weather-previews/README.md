# WX11 weather-screen previews (PR review frames — not fixtures)

Committed 240 × 320 frames of the WX11 weather surfaces (epic #1185) for PR-body review, like
`../wx10-rain-previews`. Nothing pins these bytes in CI; regenerate after any visual change with
the commands below (they are also the `ui-snapshots.sh` weather block, which stays the byte-stable
sweep).

```sh
cargo build --release -p obc-sim
S=target/release/obc-sim; M=apps/obc-sim/assets/grimsel.obcm; O=apps/obc-sim/assets/wx11-weather-previews
NAV="p d d d d w p"     # Home -> Menu -> Weather station -> dashboard

$S $M --boot --weather demo:dry                                --script "$NAV"           --png $O/dash-dry.png
$S $M --boot --weather demo:incoming --weather-now 1800001500  --script "$NAV"           --png $O/dash-rain.png
$S $M --boot --weather demo:storm                              --script "$NAV"           --png $O/dash-storm.png
$S $M --boot --weather demo:storm --weather-now 1800012000     --script "$NAV"           --png $O/dash-stale.png
$S $M --boot --weather demo:hourly                             --script "$NAV"           --png $O/dash-hourly-only.png
$S $M --boot                                                   --script "$NAV"           --png $O/dash-nodata.png
$S $M --boot --weather demo:incoming --weather-refreshing      --script "$NAV"           --png $O/dash-refreshing.png
$S $M --boot --weather demo:incoming                           --script "$NAV p"         --png $O/hourly.png
$S $M --boot --weather demo:incoming                           --script "$NAV p d d d d d d" --png $O/hourly-scrolled.png
$S $M --boot --weather demo:scattered                          --script "$NAV d p"       --png $O/rainmap-now.png
$S $M --boot --weather demo:scattered                          --script "$NAV d p d d"   --png $O/rainmap-step2.png
$S $M --boot --weather demo:storm --weather-now 1800012000     --script "$NAV d p"       --png $O/rainmap-stale.png
$S $M --boot --weather demo:hourly                             --script "$NAV d p"       --png $O/rainmap-hourly-only.png
$S $M --boot --weather demo:scattered --zoom 0.02              --script "$NAV d p"       --png $O/rainmap-out-of-regime.png
$S $M --boot --weather demo:storm --weather-alert storm:28                               --png $O/alert-storm.png
$S $M --boot --weather demo:incoming --weather-now 1800001500 --weather-alert rain:34    --png $O/alert-rain.png
$S $M --boot --script "p d d d d d w p d d p"                                            --png $O/settings-weather.png
# Language spot-checks (full sweep: ui-snapshots.sh's de/fr/es loop)
$S $M --boot --weather demo:incoming --weather-now 1800001500 --lang de --script "$NAV"  --png $O/dash-rain-de.png
$S $M --boot --weather demo:incoming --lang de --script "$NAV p"                         --png $O/hourly-de.png
$S $M --boot --weather demo:storm --lang fr --weather-alert storm:28                     --png $O/alert-storm-fr.png
$S $M --boot --weather demo:storm --weather-now 1800012000 --lang es --script "$NAV"     --png $O/dash-stale-es.png
```

The demo bundles anchor the app clock on their first frame (08:00 UTC, 2027-01-15) when no
`--clock` is passed, so every derivation — card countdown, strip, freshness line, frame labels —
is deterministic. `--weather-now 1800001500` views the `incoming` front 25 min in (card reads
RAIN IN 34 MIN); `1800012000` is past every frame's currency (the honest stale states).
