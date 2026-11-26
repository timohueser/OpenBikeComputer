#!/usr/bin/env bash
# PROTOTYPE for GitHub issue 3836: the price list and the route census of the `touring` profile.
#
#   tools/prototype_route_census/run.sh           the price list and the census
#   tools/prototype_route_census/run.sh --grid    the grid of candidate cost changes (grid.md, grid/)
#   tools/prototype_route_census/run.sh --round3  the signal cost, Alsace, hiking (round3.md, round3/)
#
# LAB holds the data and the output: osm/freiburg.osm.pbf, osm/alsace/alsace-latest.osm.pbf, dem/
# (GLO-30 tiles, fetched when absent) and census/ (the tables).
set -euo pipefail
LAB="${LAB:-/home/user/route-lab}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
FREIBURG="$LAB/osm/freiburg.osm.pbf"
ALSACE="$LAB/osm/alsace/alsace-latest.osm.pbf"
# The box of the Geofabrik extract of Alsace, to whole degrees of its GLO-30 tiles.
ALSACE_BOUNDS="6.80,47.40,8.25,49.10"

mkdir -p "$LAB/dem" "$LAB/census" "$LAB/work"
tiles() {
    for tile in "$@"; do
        stem="Copernicus_DSM_COG_10_${tile}_00_DEM"
        if [ ! -s "$LAB/dem/$stem.tif" ]; then
            curl -sS -f -o "$LAB/dem/$stem.tif.part" "https://copernicus-dem-30m.s3.amazonaws.com/$stem/$stem.tif"
            mv "$LAB/dem/$stem.tif.part" "$LAB/dem/$stem.tif"
        fi
    done
}
# The import needs the nodes in ID order.
sorted() {
    uv run --no-project --with osmium python3 -I "$HERE/check_order.py" "$1"
}
census() {
    cargo run --release -p planner-router-build --example prototype_census -- --dem "$LAB/dem" --out "$LAB/census" "$@"
}

cd "$ROOT"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$LAB/target}"
export OBC_DATA_WORK="$LAB/work"
tiles N47_00_E007 N47_00_E008 N48_00_E007 N48_00_E008
sorted "$FREIBURG"
if [ "${1:-}" = "--round3" ]; then
    tiles N47_00_E006 N48_00_E006 N49_00_E006 N49_00_E007 N49_00_E008
    sorted "$ALSACE"
    census --pbf "$FREIBURG" --round3 freiburg
    census --pbf "$ALSACE" --bounds "$ALSACE_BOUNDS" --round3 alsace
    {
        echo "# Round three: signal cost, Alsace, hiking (prototype for issue 3836)"
        echo
        echo "The stand-ins measure the reach of an idea. They are not proposals, and nothing is tuned."
        cat "$LAB"/census/round3/1_signal.md "$LAB"/census/round3/2_alsace.md \
            "$LAB"/census/round3/3_hiking.md "$LAB"/census/round3/4_query.md
    } >"$LAB/census/round3.md"
    echo "Wrote $LAB/census/round3.md"
else
    census --pbf "$FREIBURG" --package "$@"
fi
