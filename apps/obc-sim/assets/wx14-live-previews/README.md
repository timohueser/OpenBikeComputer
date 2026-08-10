# WX14 live-weather captures (dated evidence — not fixtures, not regenerable)

These frames were rendered by `--weather live` against the **production** weather service on
2026-08-09 ~23:1xZ. Unlike `../wx10-rain-previews` and `../wx11-weather-previews`, they cannot be
reproduced byte-for-byte: the weather has moved on. They exist to show that real service bytes
reach the production screens, and nothing pins them.

They were also shot before the corridor became a *projection*: the frames that pass no
`--weather-radius-km` used a fixed 15 km disc, where the default today is the phone's undirected
10 km disc (and a directed corridor once the fix vouches for a bearing and a speed). What they show
is unaffected — every product covering a 15 km disc around the Grimsel covers a 10 km one — but a
re-run asks a slightly smaller question.

```sh
cargo build --release -p obc-sim
S=target/release/obc-sim; G=apps/obc-sim/assets/grimsel.obcm; M=apps/obc-sim/assets/monaco.obcm
O=apps/obc-sim/assets/wx14-live-previews
NAV="p d d d d w p"    # Home -> Menu -> Weather station -> dashboard

$S $G --boot --weather live --script "$NAV"                              --png $O/dash-live-dwd.png
$S $G --boot --weather live --script "$NAV p"                            --png $O/hourly-live-met.png
$S $G --boot --weather live --weather-radius-km 25 --script "$NAV d p"   --png $O/rainmap-live-dwd.png
$S $G --boot --weather live --weather-offline --script "$NAV"            --png $O/dash-live-offline.png
$S $G        --weather live --weather-radius-km 25                       --png $O/map-live-dwd.png
$S $M --boot --weather live --script "$NAV"                              --png $O/dash-live-monaco-icon-eu.png
```

| Frame | What it proves |
| --- | --- |
| `dash-live-dwd.png` | The Grimsel corridor selected **dwd-rv** (tier 1) and built a 2,302 B bundle from 148 KB of Range reads. The card reads WEATHER UPDATE NEEDED because DWD's frames end at `reference + 2 h` and the reference is ~20 min behind the clock — genuinely incomplete two-hour coverage, correctly refused rather than rounded up to a dry claim. |
| `hourly-live-met.png` | Real MET Locationforecast hours for the pass: 11 °C, clear night, S 4 km/h. |
| `rainmap-live-dwd.png` | A real DWD RV frame with its real timestamp in the title, nearest-cell over the basemap. It is dry over the Alps at this instant, which is what the radar said. |
| `dash-live-offline.png` | `--weather-offline`: no bundle at all, the no-data state, no fabricated map. |
| `dash-live-monaco-icon-eu.png` | Monaco is outside DWD RV's window, so the manifest-driven selection falls to **icon-eu** (tier 2) with no code change and no country check. |

## Where the rain was

The night these were shot, the radar's precipitation was over the north German coast and there is
no packed map for it in the repo, so no *rendered* frame here contains rain. The cells are real
regardless — `obc-wx-client --dump` prints the current frame through the **device's** OBCW reader,
and a 25 km corridor at Hamburg (53.55, 9.99) returned a full frontal field:

```text
frame 1/9 valid_at 1786317000 — 55x51 cells at 1000 m
  1233333333322221112211112222110000000111110000000000000
  1233333333332212222111232221111000000011110000000000100
  1123233333332233222112322221111011100011111000000001100
  1222333333223333221222222211211111011221110000000110000
```

To shoot a rendered rain frame, run the dashboard/rain-map commands above with a map that covers a
raining corridor:

```sh
target/release/obc-wx-client fetch --lat 48.00 --lon 7.85 --radius-km 25 --dump   # is it raining yet?
```
