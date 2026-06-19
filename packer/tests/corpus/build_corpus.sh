#!/usr/bin/env bash
# Regenerate the OBCM packer validation corpus (see README.md).
# Idempotent: skips work whose output already exists. Requires `osmium`.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DATA="$HERE/data"
REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
FREIBURG="$REPO_ROOT/freiburg-regbez-260618.osm.pbf"
PBF_CACHE="${PBF_CACHE:-$HOME/.cache/obcm/pbf}"
mkdir -p "$DATA"

have() { command -v "$1" >/dev/null 2>&1; }
have osmium || { echo "error: osmium not found (brew install osmium-tool)"; exit 1; }

# 1. tiny — hand-authored XML -> pbf (always rebuilt; it's tiny and the source
#    of truth is the committed XML).
echo ">> tiny"
osmium cat "$HERE/tiny/tiny.osm" -o "$DATA/tiny.osm.pbf" --overwrite

# 2. coastal extracts — reuse the web builder's cache, else download.
fetch() { # name geofabrik-path
  local name="$1" path="$2" out="$DATA/$1.osm.pbf"
  if [ -f "$out" ]; then echo ">> $name (exists)"; return; fi
  if [ -f "$PBF_CACHE/$name.osm.pbf" ]; then
    echo ">> $name (from cache)"; cp "$PBF_CACHE/$name.osm.pbf" "$out"
  else
    echo ">> $name (download)"
    curl -fL --retry 3 -o "$out" "https://download.geofabrik.de/$path-latest.osm.pbf"
  fi
}
fetch monaco europe/monaco
fetch malta  europe/malta

# 3. inland carves from the Freiburg target (offline). `smart` keeps complete
#    multipolygon relations so area assembly is exercised intact.
carve() { # name bbox(left,bottom,right,top) strategy
  local out="$DATA/$1.osm.pbf"
  if [ -f "$out" ]; then echo ">> $1 (exists)"; return; fi
  [ -f "$FREIBURG" ] || { echo "   skip $1: $FREIBURG missing"; return; }
  echo ">> $1 (carve $2)"
  osmium extract -b "$2" -s "$3" --overwrite -o "$out" "$FREIBURG"
}
carve freiburg-town   7.78,47.95,7.92,48.03 complete_ways
carve freiburg-forest 8.00,47.85,8.30,48.00 smart

echo
echo "corpus ready in $DATA:"
ls -lah "$DATA"/*.osm.pbf
