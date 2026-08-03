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
# It also bakes the committed OBCT terrain sidecars (`./repack.sh terrain`) —
# a separate, self-contained block near the bottom, because terrain comes from
# Copernicus GLO-30 and has no OSM in it at all.
#
# Usage:
#   ./repack.sh grimsel      [switzerland.osm.pbf]
#   ./repack.sh grimsel-demo [switzerland.osm.pbf]
#   ./repack.sh monaco       [monaco.osm.pbf]
#   ./repack.sh terrain      [dem_dir]
#   ./repack.sh all          [switzerland.osm.pbf] [monaco.osm.pbf] [dem_dir]
#
# With no source argument the current Geofabrik snapshot is downloaded (the
# Switzerland file is ~600 MB). Needs only the workspace toolchain (obc-pack
# builds with system GEOS) — the crop is `obc-pack --bbox`, which keeps complete
# ways and completes renderable area relations during ingest, so osmium-tool is
# no longer required to regenerate a fixture. The three bboxes below remain the
# canonical camera coverage. Relation completion can recover polygons (and
# therefore change bytes) when a fixture is next deliberately refreshed.
#
# After re-packing, run the full workspace test suite — a few sim/reader tests
# exercise fixture content (they are written content-agnostic, but verify) —
# and note the Geofabrik snapshot date in README.md alongside the commit.

set -euo pipefail

ASSETS_DIR="$(cd "$(dirname "$0")" && pwd)"
FIRMWARE_DIR="$(cd "$ASSETS_DIR/../.." && pwd)"
PRESET="$FIRMWARE_DIR/../builder/presets/schema.json"

# --- Pinned provenance (canonical — do not derive from fixture headers) -----
GRIMSEL_SOURCE_URL="https://download.geofabrik.de/europe/switzerland-latest.osm.pbf"
GRIMSEL_BBOX="8.15034,46.48261,8.46007,46.72070" # Grimsel Pass region (lon,lat,lon,lat)
# grimsel-demo: the landing-page live-demo map (epic #624 S4, #629). A tight
# corridor hand-picked around the `grimsel-climb.gpx` track (which spans
# 8.291,46.561 -> 8.340,46.654), padded ~2 km each side so the demo tours have
# accommodation POIs + a routable nav graph around the ride (verified: the POI
# reroute plans a real route to the nearest Lodging). NOT a shared test fixture —
# shipped in the wasm only; shrinks the payload ~5x vs the full grimsel.obcm.
# Canonical + hand-picked — do NOT self-source from the grimsel-demo header.
GRIMSEL_DEMO_BBOX="8.26,46.54,8.37,46.67" # Grimsel climb corridor, demo-only
MONACO_SOURCE_URL="https://download.geofabrik.de/europe/monaco-latest.osm.pbf"
MONACO_BBOX="7.39,43.71,7.47,43.77" # Monaco principality, tight
# -----------------------------------------------------------------------------

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

fetch() { # fetch <url> <dest>
    echo "downloading $1 ..."
    curl -sSL -o "$2" "$1"
}

repack() { # repack <name> <source_pbf> <bbox> [terrain_obcd]
    local name="$1" src="$2" bbox="$3" terrain="${4:-}"
    local extra=()
    # A map with a committed terrain sidecar is packed WITH it, so the fixture
    # carries real §8.3 ascent and (preset v5, #1094/#1095) the traced E3
    # contours. The sidecar itself never changes here — `./repack.sh terrain`
    # owns it, on the DEM's own revision track.
    [[ -n "$terrain" ]] && extra=(--terrain "$terrain")
    echo "packing $name.obcm (bbox $bbox${terrain:+, terrain $(basename "$terrain")}) ..."
    (cd "$FIRMWARE_DIR" && cargo run --release --bin obc-pack -- \
        "$src" "$PRESET" "$ASSETS_DIR/$name.obcm" --bbox "$bbox" "${extra[@]}")
    ls -la "$ASSETS_DIR/$name.obcm"
}

do_grimsel() {
    local src="${1:-}"
    if [[ -z "$src" ]]; then
        src="$WORK/switzerland.osm.pbf"
        fetch "$GRIMSEL_SOURCE_URL" "$src"
    fi
    repack grimsel "$src" "$GRIMSEL_BBOX" "$ASSETS_DIR/grimsel.obcd"
}

do_grimsel_demo() {
    local src="${1:-}"
    if [[ -z "$src" ]]; then
        src="$WORK/switzerland.osm.pbf"
        fetch "$GRIMSEL_SOURCE_URL" "$src"
    fi
    repack grimsel-demo "$src" "$GRIMSEL_DEMO_BBOX"
}

do_monaco() {
    local src="${1:-}"
    if [[ -z "$src" ]]; then
        src="$WORK/monaco.osm.pbf"
        fetch "$MONACO_SOURCE_URL" "$src"
    fi
    repack monaco "$src" "$MONACO_BBOX"
}

# === Terrain sidecars (OBCT, epic #1068 / #1070) =============================
# `./repack.sh terrain [dem_dir]` bakes the two committed `.obcd` terrain
# companions from Copernicus GLO-30. Self-contained: it shares nothing with the
# `.obcm` path above, because terrain has no OSM in it at all.
#
#   apps/obc-sim/assets/grimsel.obcd          — beside grimsel.obcm
#   host/obc-bake/assets/teningen-preview.obcd — beside teningen-preview.obcm
#
# The bboxes are LATITUDE FIRST (min_lat,min_lon,max_lat,max_lon), which is the
# opposite of `obc-pack --bbox` above — `obc-dem` selects grid *cells*, and every
# grid expression in the platform puts latitude first. Nothing catches the
# mix-up for an Alpine box, so read the order before editing these.
#
# They are the same canonical extract bboxes as the maps beside them, restated in
# latitude-first order — NOT derived from any `.obcm` header, and not from a
# previous `.obcd`. That extract box is what the README calls the map's canonical
# camera coverage; the header bbox reaches further only because complete-way
# retention drags stray geometry outside it, which is not space to pan into. The
# cell rectangle rounds outward to whole 2^16 cells anyway, so each sidecar
# already covers a good margin beyond its box.
#
# `--cell-log2 16` rather than the published v1 `19`: the *posting* is the real
# one (2^9 µdeg — the posting is what decides the heights), while a 2^19 cell
# would make grimsel four 2 MiB blocks, most of them outside the map. OBCT §1.3
# makes both header data for exactly this reason, and §4.5 requires a reader to
# accept any legal pairing.
#
# With no dem_dir the GLO-30 tiles are downloaded (~44 MB each, two of them) into
# a temp dir; pass a directory to reuse a local cache.
GRIMSEL_TERRAIN_BBOX="46.48261,8.15034,46.72070,8.46007"  # = GRIMSEL_BBOX, lat first
TENINGEN_TERRAIN_BBOX="48.119,7.798,48.141,7.830"        # = the teningen-preview crop

do_terrain() {
    local dem="${1:-$WORK/dem}"
    mkdir -p "$dem"
    local bake_assets="$FIRMWARE_DIR/../host/obc-bake/assets"
    (cd "$FIRMWARE_DIR" && cargo build --release --bin obc-dem)
    local obc_dem="$FIRMWARE_DIR/../target/release/obc-dem"
    for bbox in "$GRIMSEL_TERRAIN_BBOX" "$TENINGEN_TERRAIN_BBOX"; do
        "$obc_dem" fetch --bbox "$bbox" --out "$dem"
    done
    "$obc_dem" bake --sources "$dem" --bbox "$GRIMSEL_TERRAIN_BBOX" \
        --cell-log2 16 --shard "$ASSETS_DIR/grimsel.obcd" --quiet
    "$obc_dem" bake --sources "$dem" --bbox "$TENINGEN_TERRAIN_BBOX" \
        --cell-log2 16 --shard "$bake_assets/teningen-preview.obcd" --quiet
    ls -la "$ASSETS_DIR/grimsel.obcd" "$bake_assets/teningen-preview.obcd"
}
# =============================================================================

case "${1:-}" in
grimsel) do_grimsel "${2:-}" ;;
grimsel-demo) do_grimsel_demo "${2:-}" ;;
monaco) do_monaco "${2:-}" ;;
terrain) do_terrain "${2:-}" ;;
all)
    do_grimsel "${2:-}"
    do_grimsel_demo "${2:-}"
    do_monaco "${3:-}"
    do_terrain "${4:-}"
    ;;
*)
    echo "usage: $0 grimsel|grimsel-demo|monaco|terrain|all [source.osm.pbf ...] [dem_dir]" >&2
    exit 2
    ;;
esac
