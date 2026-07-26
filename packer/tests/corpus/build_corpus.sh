#!/usr/bin/env bash
# Regenerate the obc-pack test fixtures (tiny.osm.pbf, poi.osm.pbf,
# unsorted.osm.pbf) from the committed XML. Idempotent. Requires `osmium`
# (brew install osmium-tool) — for `osmium cat`, which is the only thing in the
# tree that still needs it: nothing here packs a map.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DATA="$HERE/data"
mkdir -p "$DATA"

command -v osmium >/dev/null 2>&1 || {
  echo "error: osmium not found (brew install osmium-tool)"; exit 1;
}

# tiny — hand-authored XML -> pbf. The hand-authored tiny/tiny.osm is the source
# of truth; the derived .pbf is ALSO committed (the obc-pack ingest test
# `ingest::tests::tiny_truth_table` hard-fails without it), so re-run this script
# and commit the regenerated .pbf whenever tiny.osm changes.
echo ">> tiny"
osmium cat "$HERE/tiny/tiny.osm" -o "$DATA/tiny.osm.pbf" --overwrite
echo "done: $DATA/tiny.osm.pbf"

# poi — same deal for the POI-extraction fixture (#422): poi/poi.osm is the
# source of truth; `ingest::tests::poi_fixture_end_to_end` hard-fails without
# the committed .pbf.
echo ">> poi"
osmium cat "$HERE/poi/poi.osm" -o "$DATA/poi.osm.pbf" --overwrite
echo "done: $DATA/poi.osm.pbf"

# unsorted — a way written before its nodes (#910). `osmium cat` copies the
# order as-is, which is what makes this fixture possible; do NOT pipe it through
# `osmium sort`, that would defeat the point. `ingest::tests::
# bbox_refuses_an_unsorted_pbf` hard-fails without the committed .pbf.
echo ">> unsorted"
osmium cat "$HERE/unsorted/unsorted.osm" -o "$DATA/unsorted.osm.pbf" --overwrite
echo "done: $DATA/unsorted.osm.pbf"
