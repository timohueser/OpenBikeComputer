#!/usr/bin/env bash
# Bake one small demo map per style preset — the maps the hosted site renders on its preset cards
# (epic #894, B2 / issue #899).
#
# The rule that makes a preset card mean anything: **one source extract, one box, every preset**.
# A preset that got a prettier valley would win the comparison without being a better style, so
# the only variable here is `builder/presets/<id>.json`. Everything else — the Geofabrik snapshot,
# the crop, the packer, the camera the site opens on — is identical across the row.
#
# Usage:
#   ./bake-previews.sh                       # use the cached/downloaded Switzerland extract
#   ./bake-previews.sh switzerland.osm.pbf   # use a local extract
#
# With no argument the PBF is taken from the builder's own download cache
# (${OBCM_CACHE_DIR:-~/.cache/obcm}/pbf/switzerland.osm.pbf, the same file the dev server fills)
# and downloaded there if missing — ~600 MB, once.
#
# Every preset in `builder/presets/` is baked; adding a preset needs no edit here. Needs only the
# workspace toolchain (obc-pack builds with system GEOS); the crop is `obc-pack --bbox`, the same
# ingest-time crop `apps/obc-sim/assets/repack.sh` switched to.
#
# Outputs, both committed:
#   builder/app/public/preview/<preset-id>.obcm   the demo map, ~200 KB – 300 KB each (OBCM v11)
#   builder/app/public/preview/previews.json      the site's index: the focus bbox, and what got baked
#
# They are committed rather than built by the Pages deploy for the same reason the simulator
# fixtures are: baking needs GEOS, a 600 MB download and minutes of CPU, none of which belongs in
# a static-site workflow. See the PR for #899.
#
# After baking, run `cargo test -p obc-web-preview` (the default preset's map is a compiled-in
# fixture there) and check the numbers below against `previews.json`.

set -euo pipefail

BUILDER_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$BUILDER_DIR/.." && pwd)"
PRESETS_DIR="$BUILDER_DIR/presets"
OUT_DIR="$BUILDER_DIR/app/public/preview"

# --- Pinned provenance (canonical — do not derive from a baked map's header) -------------------
# The header bbox of a packed map is always wider than the crop it was cut from (obc-pack
# completes ways that leave the box), and wider by a different amount per preset. Self-sourcing
# from one would both ratchet the box wider on every re-bake — the drift repack.sh's header
# documents — and hand each preset its own camera. The boxes below are canonical and hand-picked;
# change them only as a deliberate, reviewed decision.
SOURCE_URL="https://download.geofabrik.de/europe/switzerland-latest.osm.pbf"
# Meiringen in the Haslital, at the foot of the Grimsel and Susten passes: the valley floor where
# the Alpbach drops into the Aare, with the town, the Brünig rail line, the pass road climbing
# out, meadow, and forest coming down the flanks. Picked because it exercises the whole style
# table at once — every preset has something to say about it, and the sparse ones visibly say
# less. This is the box every card frames, and the only view any of them opens on.
#
# 1.2 × 1.6 km, shaped to the panel's 3:4 portrait (after the projection's cos(lat) correction)
# so a 240×320 frame fits it on both axes at ≈4.9 m/px — the scale a reflective 2.7" panel is
# legible at, where buildings are shapes rather than texture.
FOCUS_BBOX="8.183,46.7175,8.1983,46.7315" # lon,lat,lon,lat — one box for every preset
#
# …and the box actually handed to the packer: the focus, padded. The pad is not slack, it is a
# correctness fix. `--bbox` keeps ways with a node inside the box and completes them, so a
# landcover polygon whose vertices all lie *outside* the crop is dropped entirely — and the
# valley's forest and meadow polygons are exactly that shape. Crop tight and the preview loses
# ground cover the device would draw, which is the one thing a preview may not do. ~600 m of pad
# is what it took here for the frame to match a regional map of the same spot; the demo maps cost
# roughly 1.7× what a tight crop did, and that is the price.
PAD_DEG="0.008,0.0055" # lon,lat — added on every side of FOCUS_BBOX before packing
# ----------------------------------------------------------------------------------------------

CACHE_DIR="${OBCM_CACHE_DIR:-$HOME/.cache/obcm}"
PBF_CACHE="$CACHE_DIR/pbf"

src="${1:-}"
if [[ -z "$src" ]]; then
    src="$PBF_CACHE/switzerland.osm.pbf"
    if [[ ! -f "$src" ]]; then
        mkdir -p "$PBF_CACHE"
        echo "downloading $SOURCE_URL (~600 MB, cached at $src) ..."
        curl -sSL -o "$src.part" "$SOURCE_URL"
        mv "$src.part" "$src"
    else
        echo "using cached extract $src"
    fi
fi
[[ -f "$src" ]] || {
    echo "no such extract: $src" >&2
    exit 2
}

shopt -s nullglob
presets=("$PRESETS_DIR"/*.json)
((${#presets[@]})) || {
    echo "no presets in $PRESETS_DIR" >&2
    exit 2
}

mkdir -p "$OUT_DIR"

PACK_BBOX="$(python3 -c '
import sys
f = [float(v) for v in sys.argv[1].split(",")]
p = [float(v) for v in sys.argv[2].split(",")]
print(",".join(f"{v:.6g}" for v in (f[0] - p[0], f[1] - p[1], f[2] + p[0], f[3] + p[1])))
' "$FOCUS_BBOX" "$PAD_DEG")"

# Build the packer once; `cargo run` per preset would re-check the workspace every iteration.
echo "building obc-pack ..."
(cd "$REPO_DIR" && cargo build --release --bin obc-pack)
PACK="$REPO_DIR/target/release/obc-pack"

for preset in "${presets[@]}"; do
    id="$(basename "$preset" .json)"
    echo "baking $id.obcm (crop $PACK_BBOX, framed on $FOCUS_BBOX) ..."
    "$PACK" "$src" "$preset" "$OUT_DIR/$id.obcm" --bbox "$PACK_BBOX"
done

# The site's index. Deliberately *not* the OBCC catalog (that is the bakery's document, about
# whole regions): this is site data about three small files that ship with the app, and it exists
# so the page knows what was baked and where to point the camera without probing for 404s.
echo "writing previews.json ..."
python3 - "$OUT_DIR" "$FOCUS_BBOX" "$SOURCE_URL" "${presets[@]}" <<'PY'
import hashlib, json, os, subprocess, sys

out_dir, bbox, source_url, *presets = sys.argv[1:]
min_lon, min_lat, max_lon, max_lat = (float(v) for v in bbox.split(","))
udeg = lambda v: int(round(v * 1_000_000))

maps = []
for preset in sorted(presets):
    pid = os.path.basename(preset)[: -len(".json")]
    path = os.path.join(out_dir, f"{pid}.obcm")
    blob = open(path, "rb").read()
    meta = json.load(open(preset)).get("_meta", {})
    maps.append(
        {
            "preset_id": pid,
            "preset_version": meta.get("version"),
            "file": f"{pid}.obcm",
            "bytes": len(blob),
            "sha256": hashlib.sha256(blob).hexdigest(),
        }
    )

built_at = subprocess.run(
    ["date", "-u", "+%Y-%m-%dT%H:%M:%SZ"], capture_output=True, text=True, check=True
).stdout.strip()

doc = {
    "schema_version": 1,
    "built_at": built_at,
    "source": source_url,
    # Microdegrees, the unit the renderer's camera speaks. This is the *focus* box — neither the
    # padded crop nor any map's header bbox, both of which differ per preset. See the script
    # header: framing the same ground on every card is the whole point.
    "bbox": {
        "min_lon": udeg(min_lon),
        "min_lat": udeg(min_lat),
        "max_lon": udeg(max_lon),
        "max_lat": udeg(max_lat),
    },
    "maps": maps,
}
with open(os.path.join(out_dir, "previews.json"), "w") as f:
    json.dump(doc, f, indent=2)
    f.write("\n")
for m in maps:
    print(f"  {m['preset_id']:<12} {m['bytes'] / 1024:7.1f} KB")
PY

echo "done — $OUT_DIR"
