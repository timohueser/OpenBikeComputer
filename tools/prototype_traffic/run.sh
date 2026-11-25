#!/bin/sh
# Throwaway prototype for issue #3836: estimate car traffic on small roads from the road network.
# Fits in the East of England, reports on its east part, and applies the model unchanged to Baden-Wuerttemberg and Norway.
# Usage: tools/prototype_traffic/run.sh DATA_DIR   (DATA_DIR outside the checkout; about 9 GB of downloads; about 45 minutes)
# Needs: pip install osmium numpy pandas pyarrow scipy numba scikit-learn matplotlib
# Baden-Wuerttemberg also needs, by hand: DATA_DIR/bw/locations.csv, the file SVZ-Zaehlstellen_*_augmented_SVZ2024.csv
# of https://www.mobidata-bw.de/dataset/karte_strassenverkehrszaehlung.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
d=$1
osm=https://download.openstreetmap.fr/extracts/europe
mkdir -p "$d/dft" "$d/osm" "$d/bw" "$d/no" "$d/feat"
get() { [ -f "$2" ] || curl -sS --retry 3 -o "$2" "$1"; }
get https://storage.googleapis.com/dft-statistics/road-traffic/downloads/data-gov-uk/dft_traffic_counts_aadf.zip "$d/dft/aadf.zip"
[ -f "$d/dft/dft_traffic_counts_aadf.csv" ] || unzip -o -q "$d/dft/aadf.zip" -d "$d/dft"
for r in east greater_london south_east east_midlands; do get "$osm/united_kingdom/england/$r-latest.osm.pbf" "$d/osm/$r.osm.pbf"; done
get "$osm/germany-latest.osm.pbf" "$d/osm/germany.osm.pbf"
get "$osm/norway-latest.osm.pbf" "$d/osm/norway.osm.pbf"
get https://mobidata-bw.de/vm/Endergebnisse_Strassenverkehrszaehlung_BW/Ergebnisse_2024.zip "$d/bw/census.zip"
[ -d "$d/bw/census" ] || unzip -o -q -j "$d/bw/census.zip" -d "$d/bw/census"

python3 -I "$here/counts_england.py" "$d/dft/dft_traffic_counts_aadf.csv" -1.0,51.4,1.8,53.1 "$d/england_counts.csv"
python3 -I "$here/counts_bw.py" "$d/bw/census" "$d/bw/locations.csv" 7.5,47.5,10.1,48.8 "$d/bw/counts_std.csv"
python3 -I "$here/counts_norway.py" "$d/no"
[ -f "$d/osm/england.npz" ] || python3 -I "$here/extract.py" "$d/osm/england.npz" -1.3,51.2,2.0,53.3 \
  "$d/osm/east.osm.pbf" "$d/osm/greater_london.osm.pbf" "$d/osm/south_east.osm.pbf" "$d/osm/east_midlands.osm.pbf"
[ -f "$d/osm/bw.npz" ] || python3 -I "$here/extract.py" "$d/osm/bw.npz" 7.3,47.3,10.3,49.0 "$d/osm/germany.osm.pbf"
[ -f "$d/osm/norway.npz" ] || python3 -I "$here/extract.py" "$d/osm/norway.npz" 4.0,57.8,31.5,71.5 "$d/osm/norway.osm.pbf"
for r in england bw norway; do
  [ -f "$d/feat/$r.parquet" ] || python3 -I "$here/features.py" "$d/osm/$r.npz" "$d/feat/$r.parquet" 1500
done
python3 -I "$here/match.py" "$d/feat/england.parquet" "$d/england_counts.csv" "$d/feat/england_matched.parquet"
python3 -I "$here/match.py" "$d/feat/bw.parquet" "$d/bw/counts_std.csv" "$d/feat/bw_matched.parquet"
python3 -I "$here/match.py" "$d/feat/norway.parquet" "$d/no/counts_std.csv" "$d/feat/norway_matched.parquet"
python3 -I "$here/match.py" "$d/feat/norway.parquet" "$d/no/counts_periodic_std.csv" "$d/feat/norway_periodic_matched.parquet"
python3 -I "$here/model.py" "$d/feat/england_matched.parquet" "$d/feat/model.pkl" \
  "$d/feat/bw_matched.parquet" "$d/feat/norway_matched.parquet" "$d/feat/norway_periodic_matched.parquet" | tee "$d/report.txt"
python3 -I "$here/map.py" "$d/feat/england.parquet" "$d/feat/model.pkl" "$d/feat/england_matched.parquet" 52.245 0.715 "England, Bury St Edmunds" "$d/england_bury.png"
python3 -I "$here/map.py" "$d/feat/bw.parquet" "$d/feat/model.pkl" "$d/feat/bw_matched.parquet" 47.95 8.00 "Baden-Wuerttemberg, east of Freiburg" "$d/bw_freiburg_east.png"
python3 -I -c "import sys, pandas as p; p.concat([p.read_parquet(f) for f in sys.argv[2:]]).to_parquet(sys.argv[1])" \
  "$d/feat/norway_all_matched.parquet" "$d/feat/norway_matched.parquet" "$d/feat/norway_periodic_matched.parquet"
python3 -I "$here/map.py" "$d/feat/norway.parquet" "$d/feat/model.pkl" "$d/feat/norway_all_matched.parquet" 60.76 11.08 "Norway, Hamar" "$d/norway_hamar.png"
