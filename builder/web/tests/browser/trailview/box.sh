#!/usr/bin/env bash
# Bakes the TrailView data of a box about Engelberg without `obc data`, by direct runs of the steps as
# the TrailView pull requests bake it, and builds the page against it.
#
#   builder/web/tests/browser/trailview/box.sh                 # bake, then build the page
#   builder/web/tests/browser/trailview/box.sh view [--view Q] # open the page on this GPU
#   builder/web/tests/browser/trailview/box.sh flight [ARGS]   # flight.mjs --gpu on the bake
#
# A step skips when its output exists, so a stopped run resumes; delete an output to bake it again.
# The features, the rules and the page follow the checkout: bake on the branch that you test.
# TRAILVIEW_DIR is the work folder; TRAILVIEW_BOX and TRAILVIEW_DETAIL change the boxes.
set -euo pipefail

OBC_ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
source "$OBC_ROOT/tools/obc-dev.sh"
obc_init
cd "$OBC_ROOT"

# The terrain, features, ground, colour and snow cover BOX. The LiDAR layers, the zoom 13 terrain
# (swissALTI3D) and the canopy, cover DETAIL only: it holds the flights and views of this folder.
BOX=${TRAILVIEW_BOX:-7.60,46.46,8.95,47.25}
DETAIL=${TRAILVIEW_DETAIL:-8.25,46.70,8.60,47.06}
WORK=${TRAILVIEW_DIR:-$HOME/.cache/obc/trailview-box}
SRC=$WORK/sources
OUT=$WORK/$BOX
GROUND=$OUT/ground
HERE=builder/web/tests/browser/trailview
CDSE=$HOME/.config/openbikecomputer/cdse-s3.env
CLMS=$HOME/.config/openbikecomputer/clms-key.json
YEAR=$(sed -n 's/^colour_year = //p' data/planner.toml)
VARIANT=$(sed -n 's/^colour = "\(.*\)"/\1/p' data/planner.toml)
SERVED=(--tiles "$OUT/terrain.mbtiles" --ground "$GROUND" --features "$OUT/features")

# The harness reads the terrain MBTiles with node:sqlite.
node -e 'require("node:sqlite")' 2>/dev/null || { _err "Node 22.13 or later is needed (node:sqlite)"; exit 1; }

case ${1:-bake} in
  view) shift; exec node "$HERE/view.mjs" "${SERVED[@]}" "$@" ;;
  flight) shift; exec node "$HERE/flight.mjs" --gpu "${SERVED[@]}" "$@" ;;
  bake) ;;
  *) _err "usage: $0 [bake | view [--view QUERY] | flight [ARGS]]"; exit 2 ;;
esac

for tool in curl osmium cargo uv node npm; do
  command -v "$tool" >/dev/null || { _err "$tool is not on the PATH"; exit 1; }
done
if [[ ! -f $CDSE ]]; then
  mkdir -p "$(dirname "$CDSE")"
  printf '%s\n' "# The S3 keys of the Copernicus Data Space: https://eodata-s3keysmanager.dataspace.copernicus.eu" \
    "CDSE_S3_ACCESS_KEY=" "CDSE_S3_SECRET_KEY=" > "$CDSE"
  chmod 600 "$CDSE"
fi
if ! grep -q '^CDSE_S3_SECRET_KEY=.' "$CDSE"; then
  _err "Sentinel-2 and the snow read CDSE S3: put the keys in $CDSE, then run again"
  exit 1
fi
if [[ ! -f $CLMS && ! -s $SRC/corine/U2018_CLC2018_V2020_20u1.tif && ! -s $SRC/corine-work/order.zip ]]; then
  _err "CORINE needs a CLMS service key: create a token under API Tokens of your land.copernicus.eu profile, save its JSON as $CLMS"
  exit 1
fi

PY=(env PYTHONHASHSEED=0 nice -n 10 uv run --quiet --locked --group trailview --group forest-reference python)
"${PY[@]}" -c ''

get() {  # URL FILE
  [[ -s $2 ]] && return
  mkdir -p "$(dirname "$2")"
  _say "download $(basename "$2")"
  curl -fL# --retry 3 -C - -o "$2.part" "$1"
  mv "$2.part" "$2"
}

# The tiles of a kind that a box touches, one per line: glo30 and worldcover squares, and the blocks
# of `forest` (`10-x-y`) and of `terrain` (`bi-bj`), each with its tiles in the box and `=ch`; of `colour`
# (`z10`, `x-y`).
tiles() {  # KIND BOX
  python3 - "$@" <<'EOF'
import math, sys
kind, (w, s, e, n) = sys.argv[1], map(float, sys.argv[2].split(","))
name = lambda lat, lon: f"{'N' if lat >= 0 else 'S'}{abs(lat):02}{'E' if lon >= 0 else 'W'}{abs(lon):03}"
if kind == "glo30":  # planner-dem reads the squares within 0.01° of the box
    for lat in range(math.floor(s - 0.01), math.floor(n + 0.01) + 1):
        for lon in range(math.floor(w - 0.01), math.floor(e + 0.01) + 1):
            ns, ew = ("N" if lat >= 0 else "S"), ("E" if lon >= 0 else "W")
            print(f"Copernicus_DSM_COG_10_{ns}{abs(lat):02}_00_{ew}{abs(lon):03}_00_DEM")
elif kind == "worldcover":
    for lat in range(math.floor(s / 3) * 3, math.floor(n / 3) * 3 + 1, 3):
        for lon in range(math.floor(w / 3) * 3, math.floor(e / 3) * 3 + 1, 3):
            print(name(lat, lon))
elif kind == "z10":
    x = lambda lon: math.floor((lon + 180) / 360 * 1024)
    y = lambda lat: math.floor((1 - math.asinh(math.tan(math.radians(lat))) / math.pi) / 2 * 1024)
    for tx in range(x(w), x(e) + 1):
        for ty in range(y(n), y(s) + 1):
            print(f"{tx}-{ty}")
else:
    blocks = {}
    if kind == "forest":  # zoom 12 tiles in zoom 10 blocks
        x = lambda lon: math.floor((lon + 180) / 360 * 4096)
        y = lambda lat: math.floor((1 - math.asinh(math.tan(math.radians(lat))) / math.pi) / 2 * 4096)
        for tx in range(x(w), x(e) + 1):
            for ty in range(y(n), y(s) + 1):
                blocks.setdefault(f"10-{tx >> 2}-{ty >> 2}", []).append(f"12-{tx}-{ty}=ch")
    elif kind == "terrain":  # specs/obct-terrain.md: origin -2^28 µdeg, tiles of 2^16 µdeg, 4 × 4 to a block
        t = lambda deg: (round(deg * 1e6) + 2**28) >> 16
        for ti in range(t(s), t(n) + 1):
            for tj in range(t(w), t(e) + 1):
                blocks.setdefault(f"{ti >> 2:04}-{tj >> 2:04}", []).append(f"{ti:04}-{tj:04}=ch")
    for block, tiles in sorted(blocks.items()):
        print(block, *tiles)
EOF
}

# Runs an `obc data` step of tools/ with a request as obc data writes it, into the empty folder
# $OUT/step. SNAPSHOTS maps each source to a folder: its data files are the snapshot.
step() {  # MODULE OPTIONS LAYERS SNAPSHOTS
  # As under `obc data`, a step gets its own TMPDIR, which goes when it ends.
  rm -rf "$OUT/step" "$OUT/step-tmp" && mkdir -p "$OUT/step" "$OUT/step-tmp"
  _say "step $1"
  python3 - "$OUT/step" "${@:2}" <<'EOF' | TMPDIR="$OUT/step-tmp" "${PY[@]}" -m "$1" --step
import json, sys
from pathlib import Path
output, options, layers, snapshots = sys.argv[1:]
data = lambda folder: {p.relative_to(folder).as_posix(): str(p) for p in sorted(Path(folder).rglob("*"))
                       if p.suffix in (".tif", ".png", ".json", ".gpkg", ".zip", ".pmtiles")}
print(json.dumps({"options": json.loads(options), "layers": json.loads(layers), "output": output,
                  "metrics": f"{output}.metrics.json",
                  "snapshots": {source: data(folder) for source, folder in json.loads(snapshots).items()}}))
EOF
  rm -rf "$OUT/step-tmp"
}

bounds() { echo "[$1]"; }

# ---- sources ----------------------------------------------------------------
get https://download.geofabrik.de/europe/switzerland-latest.osm.pbf "$SRC/switzerland.osm.pbf"
get https://egeologija.si/download/GeologicUnitView.gpkg "$SRC/egdi-geology/GeologicUnitView.gpkg"
get https://download.bgr.de/bgr/Geologie/IGME5000/shp/IGME5000.zip "$SRC/igme5000/IGME5000.zip"
for tile in $(tiles glo30 "$BOX"); do
  get "https://copernicus-dem-30m.s3.amazonaws.com/$tile/$tile.tif" "$SRC/glo30/$tile.tif"
done
for tile in $(tiles worldcover "$BOX"); do
  get "https://esa-worldcover.s3.eu-central-1.amazonaws.com/v200/2021/map/ESA_WorldCover_10m_2021_v200_${tile}_Map.tif" \
    "$SRC/esa-worldcover/$tile.tif"
done

mkdir -p "$OUT" "$GROUND"
[[ -s $OUT/osm.pbf ]] || _run osmium extract --overwrite -b "$BOX" "$SRC/switzerland.osm.pbf" -o "$OUT/osm.pbf"
# The CORINE raster of Europe; the fetch row of corine-land-cover in host/obc-data/src/fetch/capture.rs.
if [[ ! -s $SRC/corine/U2018_CLC2018_V2020_20u1.tif ]]; then
  mkdir -p "$SRC/corine" "$SRC/corine-work"
  _run "${PY[@]}" -m tools.clms --dataset 0407d497d3c44bcd93ce8fd5bf78596a --file 3d9e2413-46ca-4f6e-abf8-b6eb429a4e48 \
    --member U2018_CLC2018_V2020_20u1.tif --work "$SRC/corine-work" --fetch "$SRC/corine"
  rm -rf "$SRC/corine-work"
fi
# The Sentinel-2 texels of `colour`, one zoom 10 block at a time, as the prepare runs them.
for block in $(tiles z10 "$BOX"); do
  [[ -s $OUT/colour/$block.sha256 ]] && continue
  _run "${PY[@]}" -m tools.trailview.colour prepare --block "$block" --year "$YEAR" --work "$OUT/colour-work/$block" \
    --out "$OUT/colour"
done

# The blocks of the prepares, with only the tiles of DETAIL: at most JOBS at a time.
in_turn() {  # JOBS COMMAND…
  while (( $(jobs -rp | wc -l) >= $1 )); do wait -n; done
  "${@:2}" &
}
# Waits for each job that runs; under `set -e`, the first that fails ends the script.
drain() {
  while [[ -n $(jobs -rp) ]]; do wait -n; done
}

# Three LiDAR blocks at a time, as the prepare runs them; each writes `10/<x>/<y>.tif`.
_say "forest reference of $DETAIL"
while read -r block cells; do
  IFS=- read -r _ x y <<< "$block"
  [[ -e $OUT/forest/10/$x/$y.tif ]] && continue
  # shellcheck disable=SC2086 # the tiles are words
  in_turn 3 "${PY[@]}" -m tools.forest_reference fetch $cells --block "$block" --out "$OUT/forest"
done < <(tiles forest "$DETAIL")
drain

_say "terrain reference of $DETAIL"
while read -r block cells; do
  IFS=- read -r bi bj <<< "$block"
  [[ -e $OUT/reference/mean/18/$bi/$bj.tif || -e $OUT/reference/$block.none ]] && continue
  # shellcheck disable=SC2086 # the tiles are words
  in_turn 2 "${PY[@]}" host/obc-dem/reference/ingest.py fetch $cells --block "$block" \
    --work "$OUT/reference-raw/$block" --out "$OUT/reference" --shared "$OUT/reference-shared"
done < <(tiles terrain "$DETAIL")
drain
# The index of the blocks, and the mean archive of their tiles as `planner/reference` cuts it.
python3 - "$OUT/reference" <<'EOF'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
keys = {tile: made["sources"] for mark in sorted(root.glob("*.tiles"))
        for tile, made in json.loads(mark.read_text()).items()}
(root / "keys.json").write_text(json.dumps(keys))
(root / "blocks.json").write_text("[]")
(root / "biases.json").write_text("{}")
EOF
"${PY[@]}" host/obc-dem/reference/ingest.py index-terrain --fetched "$(date -u +%F)" --models ch \
  --keys "$OUT/reference/keys.json" --blocks "$OUT/reference/blocks.json" --biases "$OUT/reference/biases.json" \
  --out "$OUT/reference/index.json"
rm -rf "$OUT/reference-mean" && mkdir -p "$OUT/reference-mean"
python3 - "$OUT/reference" "$OUT/reference-mean" <<'EOF' | "${PY[@]}" host/obc-dem/reference/merge.py --step
import json, pathlib, sys
root, output = pathlib.Path(sys.argv[1]), sys.argv[2]
files = {p.relative_to(root).as_posix(): str(p) for p in root.rglob("*.tif")}
files["index.json"] = str(root / "index.json")
tiles = list(json.loads((root / "index.json").read_text())["contributors"])
print(json.dumps({"options": {"source": "terrain", "tiles": tiles, "pool": "mean"}, "output": output,
                  "metrics": f"{output}.metrics.json", "snapshots": {"terrain": files}}))
EOF

# ---- bakes ------------------------------------------------------------------
if [[ ! -s $OUT/terrain.mbtiles ]]; then
  rm -f "$OUT/terrain.tmp.mbtiles"
  _run nice -n 10 cargo run --release --locked -p obc-dem --features terrarium --bin planner-dem -- \
    --dem "$SRC/glo30" --reference "$OUT/reference-mean/reference" --bounds "$BOX" --output "$OUT/terrain.tmp.mbtiles"
  mv "$OUT/terrain.tmp.mbtiles" "$OUT/terrain.mbtiles"
fi

if [[ ! -d $OUT/features ]]; then
  _run nice -n 10 cargo run --release --locked -p obc-trailview-features -- --osm "$OUT/osm.pbf" \
    --terrain "$OUT/terrain.mbtiles" --bounds "$BOX" --output "$OUT/features.pmtiles"
  # The harness serves a folder of feature tiles `z/x/y.bin`.
  "${PY[@]}" - "$OUT/features.pmtiles" "$OUT/features.tmp" <<'EOF'
import sys
from pathlib import Path
from pmtiles.reader import MmapSource, all_tiles
with open(sys.argv[1], "rb") as stream:
    for (z, x, y), data in all_tiles(MmapSource(stream)):
        path = Path(sys.argv[2]) / f"{z}/{x}/{y}.bin"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
EOF
  mv "$OUT/features.tmp" "$OUT/features"
fi

# The rules bake each run: they are the ruleset of the checkout.
ruleset=$("${PY[@]}" -c 'from tools.trailview.rules import digest; print(digest())')
step tools.trailview.rules "{\"ruleset\": \"$ruleset\"}" '{}' '{}'
rm -rf "$OUT/rules" && mv "$OUT/step" "$OUT/rules"
cp "$OUT/rules/rules.json" "$GROUND/rules.json"
RULES="{\"planner/trailview-rules\": {\"materials.json\": \"$OUT/rules/materials.json\", \"regions.json\": \"$OUT/rules/regions.json\",
        \"ground.json\": \"$OUT/rules/ground.json\"}, \"planner/osm\": {\"osm.pbf\": \"$OUT/osm.pbf\"},
        \"planner/trailview-lithology\": {\"lithology.pmtiles\": \"$GROUND/lithology.pmtiles\"}}"

if [[ ! -s $GROUND/lithology.pmtiles ]]; then
  step tools.trailview.ground "{\"layer\": \"lithology\", \"bounds\": $(bounds "$BOX"), \"attribution\": \"EGDI, IGME 5000\"}" "$RULES" \
    "{\"egdi-geology\": \"$SRC/egdi-geology\", \"igme5000\": \"$SRC/igme5000\"}"
  mv "$OUT/step/lithology.pmtiles" "$GROUND/"
fi
if [[ ! -s $GROUND/material.pmtiles ]]; then
  step tools.trailview.ground "{\"layer\": \"material\", \"bounds\": $(bounds "$BOX"), \"year\": $YEAR, \"attribution\": \"ESA WorldCover, CORINE, Copernicus Sentinel data, OpenStreetMap\"}" "$RULES" \
    "{\"colour\": \"$OUT/colour\", \"corine-land-cover\": \"$SRC/corine\", \"esa-worldcover\": \"$SRC/esa-worldcover\"}"
  mv "$OUT/step/material.pmtiles" "$GROUND/"
fi
# The region tiles follow the regions of the ruleset, so they bake each run too; they are small.
rm -f "$GROUND/ground.pmtiles"
step tools.trailview.ground "{\"layer\": \"region\", \"bounds\": $(bounds "$BOX"), \"attribution\": \"RESOLVE Ecoregions, Natural Earth\"}" "$RULES" '{}'
mv "$OUT/step/region.pmtiles" "$GROUND/"
if [[ ! -s $GROUND/colour/$VARIANT/q1.pmtiles ]]; then
  step tools.trailview.colour "{\"variant\": \"$VARIANT\", \"bounds\": $(bounds "$BOX"), \"year\": $YEAR, \"attribution\": \"Contains modified Copernicus Sentinel data {year}\"}" \
    '{}' "{\"colour\": \"$OUT/colour\"}"
  mkdir -p "$GROUND/colour/$VARIANT" && mv "$OUT/step/"*.pmtiles "$GROUND/colour/$VARIANT/"
fi
if [[ ! -s $GROUND/trailview-canopy.pmtiles ]]; then
  step tools.planner_trailview_canopy "{\"references\": [\"forest-reference/ch\"], \"bounds\": $(bounds "$DETAIL"), \"attribution\": \"FOEN/WSL, Copernicus Land Monitoring Service\"}" \
    '{}' "{\"forest\": \"$OUT/forest\"}"
  mv "$OUT/step/trailview-canopy.pmtiles" "$GROUND/"
fi
if [[ ! -s $GROUND/snow.pmtiles ]]; then
  _run "${PY[@]}" -m tools.planner_snow --bounds "$BOX" --source copernicus-hr-wsi --output "$OUT/snow.tmp.pmtiles"
  mv "$OUT/snow.tmp.pmtiles" "$GROUND/snow.pmtiles"
fi
# The page reads the joined ground and the snow day bands.
if [[ ! -s $GROUND/ground.pmtiles ]]; then
  step tools.trailview.ground "{\"layer\": \"ground\", \"bounds\": $(bounds "$BOX"), \"attribution\": \"ESA WorldCover, CORINE, Copernicus Sentinel data, OpenStreetMap, EGDI, IGME 5000\"}" \
    "{\"planner/trailview-material\": {\"material.pmtiles\": \"$GROUND/material.pmtiles\"}, \"planner/trailview-lithology\": {\"lithology.pmtiles\": \"$GROUND/lithology.pmtiles\"}, \"planner/trailview-region\": {\"region.pmtiles\": \"$GROUND/region.pmtiles\"}}" '{}'
  mv "$OUT/step/ground.pmtiles" "$GROUND/"
fi
if [[ ! -d $GROUND/snow-days ]]; then
  _run "${PY[@]}" -m tools.trailview.snow --snow "$GROUND/snow.pmtiles" --output "$GROUND/snow-days.tmp"
  mv "$GROUND/snow-days.tmp" "$GROUND/snow-days"
fi

# ---- page -------------------------------------------------------------------
python3 - "$HERE/fixture/config.json" "$BOX" > "$OUT/config.json" <<'EOF'
import json, sys
config = json.load(open(sys.argv[1]))
host = "https://tiles.test"
names = ["trailview-features", "trailview-ground", "trailview-canopy", *(f"trailview-colour-q{q}" for q in range(1, 5)),
         *(f"trailview-snow-{band:02}" for band in range(46))]
config.update(name="TrailView box", bounds=[float(v) for v in sys.argv[2].split(",")],
              layers={name: f"{host}/{name}.json" for name in names}, trailview_rules=f"{host}/trailview/rules.json")
print(json.dumps(config, indent=2))
EOF
[[ -d builder/web/node_modules ]] || _run npm ci --prefix builder/web
[[ -d builder/web/tests/browser/node_modules ]] || _run npm ci --prefix builder/web/tests/browser
(cd builder/web/tests/browser && _run npx playwright install chromium)
VITE_PLANNER_CONFIG=$(cat "$OUT/config.json") _run npm run build:trailview --prefix builder/web

_ok "baked into $OUT"
_hint "$0 view      # the page on this GPU"
_hint "$0 flight    # the GPU time per pass of flights.json"
