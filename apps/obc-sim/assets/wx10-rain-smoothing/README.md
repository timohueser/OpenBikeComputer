# WX10 rain smoothing — the #1185 comparison round

Timo, on glass, on real 1 km radar: *"1 km square blobs are bigger than I thought and it does
look very blocky — maybe we should do some interpolation/smoothing after all."* This directory
keeps the reproducible comparison recipe and the decision record. Generated sheets belong in the
review or a temporary output directory, not in Git.

Six scenes. Each sheet holds the **same frame** — same source data, same camera, same zoom, same
heading — rendered four times, once per `obc_render::RainSampling` mode:

| | mode | what it does |
|---|---|---|
| **A** | `Nearest` | today's shipped behaviour: floor the sample point, take that cell |
| **B** | `Bilinear` | interpolate the 4-bit band index between cell centres, ordered-dither the fraction |
| **C** | `Jitter` | nearest neighbour of a point offset by a stratified ±½-cell ordered dither |
| **D** | `EdgeSoften` | the same at half amplitude (±¼ cell) — only the pixels near a cell boundary mix |

| sheet | scene |
|---|---|
| `1-us-storm-60mpp` | NOAA **MRMS** 1 km, inside a real storm over Wisconsin/Lake Michigan, 60 m/px. The pure "1 km blobs" case. |
| `2-us-storm-15mpp` | the same storm zoomed right in — the blockiest frame a rider can reach. |
| `3-grimsel-riding-20mpp` | DWD **RV** composite 1 km over the packed Grimsel map, typical riding zoom. |
| `4-grimsel-wide-200mpp` | the same ground near the locked 50 km view; the regime cap on a 1 km lattice is ~333 m/px. |
| `5-coverage-edge-60mpp` | the radar umbrella's own **coverage edge** — where smoothing would lie if it were going to. |
| `6-heading-up-40mpp` | heading-up at 37°, so the rotated fixed-point walk is in the picture too. |

The command below writes six sheets and 24 single 240 × 320 panels to a temporary directory.

## The data is real

The rain is decoded straight out of a baked **OBCG** object — `obc-wx-bake`'s real output from real
upstream bytes. Nothing is synthetic: a demo pattern cannot answer a complaint about how real radar
looks.

> **The repro below was re-derived on 2026-08-11 for #1246.** The recipe renders the same cells,
> from the same upstream bytes, through the same renderer. A baked object now comes from the one
> dataset at `wx/v2/<generation>/f<offset>/s<col>-<row>.obcg`
> — and the US half of the repro is now **more** reproducible than it was, because the event pack
> carries its own upstream bytes.

Reading OBCG rather than assembling an OBCW bundle is deliberate. Both containers carry the *same*
4-bit cells (`obc_formats::precip4` is shared), so this puts the actual upstream radar in front of
the actual renderer with nothing in between — and it does not need `api.met.no`, which an OBCW
assembly does.

**The two sources' ground does not overlap the registered maps.** MRMS is CONUS-only and every
registered `.obcm` is Alpine/Rhine, so scenes 1 and 2 render over an empty basemap — the raster is
judged on its own there, which is the honest way to describe them. Scenes 3–6 are painted by DWD RV,
which does cover Grimsel, and have real streets, contours and water underneath.

## Exact repro

```sh
# 1a. The US frame, fully offline: sync and re-bake the immutable event package, which carries
#     its own raw MRMS and HRRR bytes. f0 is the 1 km observation frame used by these comparisons.
python3 tools/fixtures.py sync weather-event-derecho
FIXTURE_ROOT=$(python3 tools/fixtures.py root)
PACK=$FIXTURE_ROOT/weather-event-derecho
cargo run --release -p obc-wx-bake --bin obc-wx-pack -- rebake $PACK --out /tmp/wx10-us
MRMS=/tmp/wx10-us/wx/v2/20200810T1845Z/f0/s0-0.obcg

# 1b. The German frame needs a live bake: nothing checked in covers Grimsel, and the baker's only
#     entry point is a whole cycle. This fetches every source and publishes one generation.
cargo run --release -p obc-wx-bake --bin obc-wx-bake -- cycle --store /tmp/wx10-dach
GEN=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["generation"])' \
        /tmp/wx10-dach/wx/v2/manifest.json)
# The Alps are shard (3,2) of the 6 x 4 grid - 4.32..65.76 E, 2.16..48.24 N; `manifest.json`'s
# frames[0].shards lists exactly which shards exist.
DWD=/tmp/wx10-dach/wx/v2/$GEN/f0/s3-2.obcg

# 2. Build the comparison renderer.
cargo build --release -p obc-sim --bin rain_sampling_sheet
BIN=./target/release/rain_sampling_sheet
python3 tools/fixtures.py sync sim-grimsel
OUT=/tmp/obc-rain-smoothing
MAP=$FIXTURE_ROOT/sim-grimsel/grimsel.obcm

# 3. The six scenes, verbatim.
$BIN --obcg $MRMS            --center -87760000,44320000 --mpp  60 --label 1-us-storm-60mpp        --out-dir $OUT
$BIN --obcg $MRMS            --center -87760000,44320000 --mpp  15 --label 2-us-storm-15mpp        --out-dir $OUT
$BIN --obcg $DWD  --map $MAP --center   8300000,46600000 --mpp  20 --label 3-grimsel-riding-20mpp  --out-dir $OUT
$BIN --obcg $DWD  --map $MAP --center   8300000,46600000 --mpp 200 --label 4-grimsel-wide-200mpp   --out-dir $OUT
$BIN --obcg $DWD  --map $MAP --center   8300000,46520000 --mpp  60 --label 5-coverage-edge-60mpp   --out-dir $OUT
$BIN --obcg $DWD  --map $MAP --center   8300000,46600000 --mpp  40 --heading 37 --label 6-heading-up-40mpp --out-dir $OUT
```

Two more modes of the same binary are worth knowing, because they are how the scenes were aimed:

```sh
$BIN --obcg $MRMS --survey                              # geometry + exhaustive histogram + wettest windows
$BIN --obcg $DWD  --center 8300000,46600000 --probe     # the raw cell field as ASCII, one char per 1 km cell
```

`--probe` is the one to look at before arguing about any of this: it prints the lattice cells
themselves. The blockiness in sheet A is not a rendering artefact — it is that field, drawn
faithfully.

## What the sheets are for

`RAIN_SAMPLING` in `firmware/obc-render/src/rain.rs` is the one-line switch that picks the mode.

**Decided (#1250): `Bilinear`** — Timo's pick from these six sheets. The losing three modes stay
compiled and covered by the same tests, so the round can be re-run on glass by editing that one
line; these sheets are the record of how the call was made, and the binary still renders all four.

Two consequences worth knowing before re-deciding it, both in `RAIN_TILE_SLOTS`' own docs:
bilinear is the only mode that can paint an intensity band no lattice cell reported (permitted for
display, forbidden for data queries — OBCW §5, OBCG §6), and it is the mode that sizes the tile
cache. `Nearest` or `EdgeSoften` alone would need 12 slots where the shipped configuration needs 16.
