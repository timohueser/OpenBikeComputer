#!/bin/sh
# Throwaway prototype for issue #3836: does a car-trip score from the road network rank minor-road traffic?
# Usage: tools/prototype_traffic/run.sh DATA_DIR   (DATA_DIR outside the checkout; about 450 MB; about 5 minutes)
# Needs: pip install osmium numpy pandas scipy numba matplotlib
set -eu
here=$(cd "$(dirname "$0")" && pwd)
d=$1
mkdir -p "$d/dft" "$d/osm" "$d/out"
[ -f "$d/dft/dft_traffic_counts_aadf.csv" ] || {
  curl -sS -o "$d/dft/aadf.zip" https://storage.googleapis.com/dft-statistics/road-traffic/downloads/data-gov-uk/dft_traffic_counts_aadf.zip
  unzip -o -q "$d/dft/aadf.zip" -d "$d/dft"
}
[ -f "$d/osm/east.osm.pbf" ] || curl -sS -o "$d/osm/east.osm.pbf" https://download.openstreetmap.fr/extracts/europe/united_kingdom/england/east-latest.osm.pbf
[ -f "$d/osm/east.npz" ] || python3 -I "$here/extract.py" "$d/osm/east.osm.pbf" "$d/osm/east.npz"
python3 -I "$here/score.py" "$d/osm/east.npz" "$d/dft/dft_traffic_counts_aadf.csv" "$d/out" 1000 1
python3 -I "$here/inspect_misses.py" "$d/out" "$d/osm/east.npz"
