#!/usr/bin/env bash
# Regenerate the obc-pack test fixture: tiny.osm.pbf from the committed XML.
# Idempotent. Requires `osmium` (brew install osmium-tool).
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
