#!/usr/bin/env bash
# Re-pack the committed simulator fixtures (grimsel.obcm, monaco.obcm) from
# their PINNED sources and extract bboxes. This script is the single source of
# truth for fixture provenance — see README.md next to it before changing
# anything here.
#
# The one rule: NEVER derive an extract bbox from an existing fixture's header.
# The packer's header bbox is computed from the packed content and is always a
# bit wider than the extract bbox (complete ways and stray coastline/boundary
# features stretch it), so self-sourcing ratchets the bbox wider on every
# re-pack. That is exactly how the pre-v9 fixtures drifted (monaco once grew to
# 14.5 MB). Extract bboxes below are canonical and hand-picked; change them
# only as a deliberate, reviewed decision.
#
# Usage:
#   ./repack.sh grimsel [switzerland.osm.pbf]
#   ./repack.sh monaco  [monaco.osm.pbf]
#   ./repack.sh all     [switzerland.osm.pbf] [monaco.osm.pbf]
#
# With no source argument the current Geofabrik snapshot is downloaded (the
# Switzerland file is ~600 MB). Requires `osmium` (brew install osmium-tool)
# and the workspace toolchain (obc-pack builds with system GEOS).
#
# After re-packing, run the full workspace test suite — a few sim/reader tests
# exercise fixture content (they are written content-agnostic, but verify) —
# and note the Geofabrik snapshot date in README.md alongside the commit.

set -euo pipefail

ASSETS_DIR="$(cd "$(dirname "$0")" && pwd)"
FIRMWARE_DIR="$(cd "$ASSETS_DIR/../.." && pwd)"
PRESET="$FIRMWARE_DIR/../packer/presets/default.json"

# --- Pinned provenance (canonical — do not derive from fixture headers) -----
GRIMSEL_SOURCE_URL="https://download.geofabrik.de/europe/switzerland-latest.osm.pbf"
GRIMSEL_BBOX="8.15034,46.48261,8.46007,46.72070" # Grimsel Pass region (lon,lat,lon,lat)
MONACO_SOURCE_URL="https://download.geofabrik.de/europe/monaco-latest.osm.pbf"
MONACO_BBOX="7.39,43.71,7.47,43.77" # Monaco principality, tight
# -----------------------------------------------------------------------------

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

fetch() { # fetch <url> <dest>
    echo "downloading $1 ..."
    curl -sSL -o "$2" "$1"
}

repack() { # repack <name> <source_pbf> <bbox>
    local name="$1" src="$2" bbox="$3"
    local extract="$WORK/$name-extract.osm.pbf"
    echo "extracting $name (bbox $bbox) ..."
    osmium extract --overwrite --bbox "$bbox" -o "$extract" "$src"
    echo "packing $name.obcm ..."
    (cd "$FIRMWARE_DIR" && cargo run --release --bin obc-pack -- \
        "$extract" "$PRESET" "$ASSETS_DIR/$name.obcm")
    ls -la "$ASSETS_DIR/$name.obcm"
}

do_grimsel() {
    local src="${1:-}"
    if [[ -z "$src" ]]; then
        src="$WORK/switzerland.osm.pbf"
        fetch "$GRIMSEL_SOURCE_URL" "$src"
    fi
    repack grimsel "$src" "$GRIMSEL_BBOX"
}

do_monaco() {
    local src="${1:-}"
    if [[ -z "$src" ]]; then
        src="$WORK/monaco.osm.pbf"
        fetch "$MONACO_SOURCE_URL" "$src"
    fi
    repack monaco "$src" "$MONACO_BBOX"
}

case "${1:-}" in
grimsel) do_grimsel "${2:-}" ;;
monaco) do_monaco "${2:-}" ;;
all)
    do_grimsel "${2:-}"
    do_monaco "${3:-}"
    ;;
*)
    echo "usage: $0 grimsel|monaco|all [source.osm.pbf ...]" >&2
    exit 2
    ;;
esac
