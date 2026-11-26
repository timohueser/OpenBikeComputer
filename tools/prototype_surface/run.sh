#!/bin/sh
# Prototype: run.sh DATA_DIR. Reads DATA_DIR/osm/freiburg.osm.pbf and the contrast extracts
# DATA_DIR/osm/alsace/alsace-latest.osm.pbf and DATA_DIR/osm/cambridge/Cambridge.osm.pbf when they exist.
# Writes the tables to DATA_DIR/surface/ and the extracted ways to DATA_DIR/surface/cache/.
set -eu
data=$(cd "$1" && pwd)
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$data/surface/cache"
py() { uv run -q --no-project --python 3.12 --with osmium --with numpy --with scikit-learn --with shapely python -I "$@"; }
for pair in freiburg:freiburg.osm.pbf alsace:alsace/alsace-latest.osm.pbf cambridge:cambridge/Cambridge.osm.pbf; do
    name=${pair%%:*}
    pbf=$data/osm/${pair#*:}
    if [ -f "$pbf" ] && [ ! -f "$data/surface/cache/$name.npz" ]; then
        echo "extract $name"
        py "$here/extract.py" "$pbf" "$data/surface/cache/$name.npz"
    fi
done
py "$here/estimate.py" "$data/surface/cache" "$data/surface"
