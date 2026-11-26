#!/usr/bin/env bash
# PROTOTYPE for GitHub issue 3836: the price list and the route census of the `touring` profile.
#
#   tools/prototype_route_census/run.sh           the price list and the census
#   tools/prototype_route_census/run.sh --grid    the grid of candidate cost changes (grid.md, grid/)
#   tools/prototype_route_census/run.sh --round3  the signal cost, Alsace, hiking (round3.md, round3/)
#   tools/prototype_route_census/run.sh --round4  the pairs for the map, Engelberg, Ireland
#                                                 (pairs.json, round4.md, round4/)
#
# LAB holds the data and the output: osm/freiburg.osm.pbf, osm/alsace/alsace-latest.osm.pbf,
# osm/switzerland/<canton>.osm.pbf, osm/ireland/ireland.osm.pbf, dem/ (GLO-30 tiles, fetched when
# absent) and census/ (the tables).
set -euo pipefail
LAB="${LAB:-/home/user/route-lab}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
FREIBURG="$LAB/osm/freiburg.osm.pbf"
ALSACE="$LAB/osm/alsace/alsace-latest.osm.pbf"
# The box of the Geofabrik extract of Alsace, to whole degrees of its GLO-30 tiles.
ALSACE_BOUNDS="6.80,47.40,8.25,49.10"
SWISS="$LAB/osm/switzerland"
# The box of the cantons of Lucerne, Obwalden, Nidwalden, Uri and Schwyz.
SWISS_BOUNDS="7.80,46.50,8.999,47.30"
IRELAND="$LAB/osm/ireland"
GALWAY_BOUNDS="-9.60,52.80,-8.60,53.40"

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
osmium() {
    uv run --no-project --with osmium python3 -I "$HERE/$1" "${@:2}"
}
# The import needs the nodes in ID order.
sorted() {
    osmium check_order.py "$1"
}
census() {
    cargo run --release -p planner-router-build --example prototype_census -- --dem "$LAB/dem" --out "$LAB/census" "$@"
}

cd "$ROOT"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$LAB/target}"
export OBC_DATA_WORK="$LAB/work"
tiles N47_00_E007 N47_00_E008 N48_00_E007 N48_00_E008
sorted "$FREIBURG"
case "${1:-}" in
--round3)
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
    ;;
--round4)
    tiles N46_00_E007 N46_00_E008 N52_00_W010 N52_00_W009 N53_00_W010 N53_00_W009
    if [ ! -s "$SWISS/central.osm.pbf" ]; then
        osmium prepare_osm.py merge "$SWISS/central.osm.pbf" "$SWISS"/{lucerne,nidwalden,obwalden,schwyz,uri}.osm.pbf
    fi
    if [ ! -s "$IRELAND/galway.osm.pbf" ]; then
        osmium prepare_osm.py clip "$IRELAND/galway.osm.pbf" "$IRELAND/ireland.osm.pbf" "$GALWAY_BOUNDS"
    fi
    sorted "$SWISS/central.osm.pbf"
    sorted "$IRELAND/galway.osm.pbf"
    census --pbf "$FREIBURG" --round4 freiburg
    census --pbf "$SWISS/central.osm.pbf" --bounds "$SWISS_BOUNDS" --round4 engelberg
    census --pbf "$IRELAND/galway.osm.pbf" --bounds "$GALWAY_BOUNDS" --round4 galway
    python3 -I "$HERE/join_pairs.py" "$LAB/census/pairs.json" \
        "$LAB/census/round4/pairs_freiburg.json" "$LAB/census/round4/pairs_engelberg.json"
    {
        echo "# Round four: pairs for the map, Engelberg, Ireland (prototype for issue 3836)"
        echo
        echo "The stand-ins measure the reach of an idea. They are not proposals, and nothing is tuned."
        cat "$LAB"/census/round4/1_pairs_freiburg.md "$LAB"/census/round4/1_pairs_engelberg.md \
            "$LAB"/census/round4/2_engelberg.md "$LAB"/census/round4/3_ireland.md "$LAB"/census/round4/4_freiburg.md \
            "$HERE/round4_notes.md"
    } >"$LAB/census/round4.md"
    echo "Wrote $LAB/census/round4.md"
    ;;
*)
    census --pbf "$FREIBURG" --package "$@"
    ;;
esac
