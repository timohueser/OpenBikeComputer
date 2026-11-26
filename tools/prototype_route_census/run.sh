#!/usr/bin/env bash
# PROTOTYPE for GitHub issue 3836: the price list and the route census of the `touring` profile.
#
#   tools/prototype_route_census/run.sh           the price list and the census
#   tools/prototype_route_census/run.sh --grid    the grid of candidate cost changes (grid.md, grid/)
#
# LAB holds the data and the output: osm/freiburg.osm.pbf, dem/ (GLO-30 tiles, fetched when
# absent) and census/ (the tables).
set -euo pipefail
LAB="${LAB:-/home/user/route-lab}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
PBF="$LAB/osm/freiburg.osm.pbf"

mkdir -p "$LAB/dem" "$LAB/census" "$LAB/work"
for tile in N47_00_E007 N47_00_E008 N48_00_E007 N48_00_E008; do
    stem="Copernicus_DSM_COG_10_${tile}_00_DEM"
    if [ ! -s "$LAB/dem/$stem.tif" ]; then
        curl -sS -f -o "$LAB/dem/$stem.tif.part" "https://copernicus-dem-30m.s3.amazonaws.com/$stem/$stem.tif"
        mv "$LAB/dem/$stem.tif.part" "$LAB/dem/$stem.tif"
    fi
done

# The import needs the nodes in ID order.
uv run --no-project --with osmium python3 -I "$HERE/check_order.py" "$PBF"

cd "$ROOT"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$LAB/target}"
export OBC_DATA_WORK="$LAB/work"
cargo run --release -p planner-router-build --example prototype_census -- \
    --pbf "$PBF" --dem "$LAB/dem" --out "$LAB/census" --package "$@"
