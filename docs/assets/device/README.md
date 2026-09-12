# Device captures

These six WebP files are real device frames. The repository README uses them to show what the
firmware looks like. Each one is a headless `obc-sim` render of the production `obc-app` and
`obc-render` code at 3× the 240 × 320 panel, in the panel's own 64-colour gamut.

Regenerate them from the repository root:

```sh
cargo build -p obc-sim --release
python3 tools/fixtures.py sync sim

SIM=target/release/obc-sim
FIX="$(python3 tools/fixtures.py root)"
GRIMSEL="$FIX/sim-grimsel/grimsel.obcm"
CLIMB="$FIX/sim-grimsel/tracks/grimsel-climb.gpx"
MONACO="$FIX/sim-monaco/monaco.obcm"
OUT="$(mktemp -d)"
ROUTES="$(mktemp -d)"
"$SIM" --import "$CLIMB" --routes-dir "$ROUTES"

# The three riding views run the Grimsel replay at t = 1500 s. CLIMBOFF turns the automatic
# climb switch off first, so the Map and Statistics frames are not replaced by the Climb
# screen that the pass road otherwise triggers.
CLIMBOFF="B u p p d d d p b b b"
"$SIM" "$GRIMSEL" --boot --scale 3 --routes-dir "$ROUTES" --gpx "$CLIMB" --at 1500 \
    --clock 2025-06-29T14:40 --script "$CLIMBOFF p p p p" \
    --expect-screen Map --png "$OUT/map.png"
"$SIM" "$GRIMSEL" --boot --scale 3 --routes-dir "$ROUTES" --gpx "$CLIMB" --at 1500 \
    --script "$CLIMBOFF p p p p b" \
    --expect-screen Statistics --png "$OUT/stats.png"
"$SIM" "$GRIMSEL" --boot --scale 3 --routes-dir "$ROUTES" --gpx "$CLIMB" --at 1500 \
    --script "p p p p" \
    --expect-screen Climb --png "$OUT/climb.png"

# The main menu, with its needle settled on the Routes station. "w" settles the animation.
"$SIM" "$GRIMSEL" --boot --scale 3 --battery 45 --script "B w" \
    --expect-screen Menu --png "$OUT/menu.png"

# The weather frame uses a deterministic demo bundle. "p d d d d w p" walks
# Home -> Menu -> Weather.
"$SIM" "$GRIMSEL" --boot --scale 3 --weather demo:incoming --weather-now 1800001500 \
    --script "p d d d d w p" --expect-screen Weather --png "$OUT/weather.png"

# Opening hours need the hours-rich Monaco fixture and a fixed clock (Mon 12:00 -> OPEN).
"$SIM" "$MONACO" --boot --scale 3 --center 7418500,43732500 --heading 0 \
    --clock 2025-01-06T12:00 --script "B d d w p d d d p f p" \
    --expect-screen PoiDetail --png "$OUT/poi.png"

# The frames are flat palette images, so lossless WebP is exact and about half the size.
for f in "$OUT"/*.png; do
    cwebp -lossless -exact "$f" -o "docs/assets/device/$(basename "${f%.png}").webp"
done
```

Nothing checks these files, so they go stale silently. Run the block again after a change to
one of the screens it shows.
