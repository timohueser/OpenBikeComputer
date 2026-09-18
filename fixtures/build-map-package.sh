#!/usr/bin/env bash
# Re-pack the simulator map fixtures (Grimsel, Monaco or Freiburg) from
# their PINNED sources and extract bboxes. This script is the single source of
# truth for map-fixture provenance — see README.md next to it before changing
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
# It also bakes the OBCT terrain sidecars (`./build-map-package.sh terrain`) —
# a separate, self-contained block near the bottom, because terrain comes from
# Copernicus GLO-30 and has no OSM in it at all.
#
# Usage:
#   fixtures/build-map-package.sh terrain [dem_dir]
#   fixtures/build-map-package.sh grimsel [switzerland.osm.pbf]
#   fixtures/build-map-package.sh monaco  [monaco.osm.pbf]
#   fixtures/build-map-package.sh freiburg [freiburg-regbez.osm.pbf]
#   fixtures/build-map-package.sh all     [switzerland.osm.pbf] [monaco.osm.pbf] [dem_dir]
#
# With no source argument the current Geofabrik snapshot is downloaded (the
# Switzerland file is ~600 MB). Needs only the workspace toolchain (obc-pack
# builds with system GEOS) — the crop is `obc-pack --bbox`, which keeps complete
# ways and completes renderable area relations during ingest, so osmium-tool is
# no longer required to regenerate a fixture. The three bboxes below remain the
# canonical camera coverage. Relation completion can recover polygons (and
# therefore change bytes) when a fixture is next deliberately refreshed.
#
# Set OBC_GRIMSEL_LANDMARKS to compiled content.json and OBC_GRIMSEL_PEAKS
# to compiled peaks.json for Grimsel. OBC_DEMO_PEAKS selects demo peak content.
# OBC_DEMO_SURFACE names an existing demo surface sidecar to embed again.
# After re-packing, run the fixture consumer suites from docs/testing.md and
# record source and output identities in fixtures/sources/.

set -euo pipefail

FIXTURES_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$FIXTURES_DIR/.." && pwd)"
BUILD_DIR="${OBC_FIXTURE_BUILD_DIR:-$FIXTURES_DIR/build}"
PRESET="$REPO_ROOT/builder/presets/schema.json"
mkdir -p "$BUILD_DIR/sim-grimsel/routes" "$BUILD_DIR/sim-grimsel/tracks" "$BUILD_DIR/sim-monaco/tracks" \
    "$BUILD_DIR/sim-freiburg"

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
# freiburg: the settlement-density fixture. A Rhine-plain box from Freiburg
# north to Emmendingen — one city, its towns, and the villages and hamlets
# between them. Canonical + hand-picked, like every box above.
FREIBURG_SOURCE_URL="https://download.geofabrik.de/europe/germany/baden-wuerttemberg/freiburg-regbez-latest.osm.pbf"
FREIBURG_BBOX="7.77,47.97,7.93,48.14" # Freiburg to Emmendingen (lon,lat,lon,lat)
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
    # carries real §8.3 ascent and (preset v6, #1094/#1095/#1104) the traced E3
    # contours. The sidecar itself never changes here — this script's `terrain`
    # owns it, on the DEM's own revision track.
    [[ -n "$terrain" ]] && extra=(--terrain "$terrain")
    if [[ "$name" == grimsel-demo && -n "${OBC_DEMO_LANDMARKS:-}" ]]; then
        extra+=(--landmarks "$OBC_DEMO_LANDMARKS")
    fi
    if [[ "$name" == grimsel && -n "${OBC_GRIMSEL_LANDMARKS:-}" ]]; then
        extra+=(--landmarks "$OBC_GRIMSEL_LANDMARKS")
    fi
    if [[ "$name" == grimsel && -n "${OBC_GRIMSEL_PEAKS:-}" ]]; then
        extra+=(--peaks "$OBC_GRIMSEL_PEAKS")
    fi
    if [[ "$name" == grimsel-demo && -n "${OBC_DEMO_PEAKS:-}" ]]; then
        extra+=(--peaks "$OBC_DEMO_PEAKS")
    fi
    local output
    case "$name" in
      grimsel) output="$BUILD_DIR/sim-grimsel/grimsel.obcm" ;;
      monaco) output="$BUILD_DIR/sim-monaco/monaco.obcm" ;;
      freiburg) output="$BUILD_DIR/sim-freiburg/freiburg.obcm" ;;
      grimsel-demo) output="$REPO_ROOT/apps/obc-sim/assets/grimsel-demo.obcm" ;;
      *) echo "unknown map package $name" >&2; exit 2 ;;
    esac
    echo "packing $output (bbox $bbox${terrain:+, terrain $(basename "$terrain")}) ..."
    # `${extra[@]+…}`, not a bare `"${extra[@]}"`: under `set -u` the bash 3.2 that ships
    # with macOS treats an EMPTY array expansion as an unbound variable, so the terrain-less
    # targets (monaco, grimsel-demo) died on the guard meant to protect them.
    (cd "$REPO_ROOT" && cargo run --release --bin obc-pack -- \
        "$src" "$PRESET" "$output" --bbox "$bbox" ${extra[@]+"${extra[@]}"})
    ls -la "$output"
}

do_grimsel() {
    local src="${1:-}"
    if [[ -z "$src" ]]; then
        src="$WORK/switzerland.osm.pbf"
        fetch "$GRIMSEL_SOURCE_URL" "$src"
    fi
    local terrain="$BUILD_DIR/sim-grimsel/grimsel.obcd"
    [[ -f "$terrain" ]] || { echo "build terrain first: fixtures/build-map-package.sh terrain" >&2; exit 1; }
    repack grimsel "$src" "$GRIMSEL_BBOX" "$terrain"
    cp "$FIXTURES_DIR/sources/sim-grimsel/tracks/grimsel-climb.gpx" "$BUILD_DIR/sim-grimsel/tracks/"
    cp "$FIXTURES_DIR/sources/sim-grimsel/routes/grimsel-climb.obcr" \
       "$FIXTURES_DIR/sources/sim-grimsel/routes/TP1.OBT" \
       "$BUILD_DIR/sim-grimsel/routes/"
    python3 "$REPO_ROOT/tools/fixtures.py" pack sim-grimsel "$BUILD_DIR/sim-grimsel" \
      --output "$BUILD_DIR/sim-grimsel.tar.gz"
}

do_grimsel_demo() {
    local src="${1:-}"
    if [[ -z "$src" ]]; then
        src="$WORK/switzerland.osm.pbf"
        fetch "$GRIMSEL_SOURCE_URL" "$src"
    fi
    local dem="${OBC_DEMO_DEM_DIR:-$BUILD_DIR/demo-dem}"
    local native="$BUILD_DIR/grimsel-demo-native.obcd"
    local surface="${OBC_DEMO_SURFACE:-$BUILD_DIR/grimsel-demo-surface.obcd}"
    # Wider than the ride corridor: Peak View needs the surrounding skyline.
    local terrain_bbox="46.3,7.9,46.95,8.75"
    # Terrain keeps its own revision track, so a map re-pack embeds the surface it already has.
    # Baking runs only when that file is absent.
    if [[ ! -f "$surface" ]]; then
        (cd "$REPO_ROOT" && cargo build --release --bin obc-dem)
        local obc_dem="$REPO_ROOT/target/release/obc-dem"
        "$obc_dem" fetch --bbox "$terrain_bbox" --out "$dem"
        "$obc_dem" bake --sources "$dem" --bbox "$terrain_bbox" --posting-log2 9 \
            --cell-log2 16 --shard "$native" --quiet
        "$obc_dem" surface "$native" "$surface"
    fi
    repack grimsel-demo "$src" "$GRIMSEL_DEMO_BBOX" "$surface"
    # obc-pack samples terrain for contours/ascent but leaves the terrain region empty.
    python3 - "$REPO_ROOT/apps/obc-sim/assets/grimsel-demo.obcm" "$surface" \
      "$REPO_ROOT/firmware/obc-formats/src/obcm.rs" <<'PY_EMBED'
from pathlib import Path
import re
import struct
import sys

path = Path(sys.argv[1])
map_bytes = bytearray(path.read_bytes())
terrain = Path(sys.argv[2]).read_bytes()
version = int(re.search(r"pub const VERSION: u8 = (\d+);", Path(sys.argv[3]).read_text())[1])
assert map_bytes[:5] == b"OBCM" + bytes([version]), "expected current OBCM version"
assert map_bytes[41:49] == bytes(8), "terrain region must be empty"
assert terrain[:5] == b"OBCT\x03" and terrain[7] & 2, "expected surface terrain"
unit = 1 << map_bytes[40]
align = max(512, unit)
offset = (len(map_bytes) + align - 1) // align * align
length = (len(terrain) + unit - 1) // unit * unit
struct.pack_into("<II", map_bytes, 41, offset // unit, length // unit)
map_bytes.extend(bytes(offset - len(map_bytes)))
map_bytes.extend(terrain)
map_bytes.extend(bytes(offset + length - len(map_bytes)))
path.write_bytes(map_bytes)
PY_EMBED
}

do_monaco() {
    local src="${1:-}"
    if [[ -z "$src" ]]; then
        src="$WORK/monaco.osm.pbf"
        fetch "$MONACO_SOURCE_URL" "$src"
    fi
    repack monaco "$src" "$MONACO_BBOX"
    cp "$FIXTURES_DIR/sources/sim-monaco/tracks/monaco-upahead.gpx" "$BUILD_DIR/sim-monaco/tracks/"
    python3 "$REPO_ROOT/tools/fixtures.py" pack sim-monaco "$BUILD_DIR/sim-monaco" \
      --output "$BUILD_DIR/sim-monaco.tar.gz"
}

do_freiburg() {
    local src="${1:-}"
    if [[ -z "$src" ]]; then
        src="$WORK/freiburg-regbez.osm.pbf"
        fetch "$FREIBURG_SOURCE_URL" "$src"
    fi
    repack freiburg "$src" "$FREIBURG_BBOX"
    python3 "$REPO_ROOT/tools/fixtures.py" pack sim-freiburg "$BUILD_DIR/sim-freiburg" \
      --output "$BUILD_DIR/sim-freiburg.tar.gz"
}

# === Terrain sidecars (OBCT, epic #1068 / #1070) =============================
# `build-map-package.sh terrain [dem_dir]` bakes the two `.obcd` terrain
# companions from Copernicus GLO-30. Self-contained: it shares nothing with the
# `.obcm` path above, because terrain has no OSM in it at all.
#
#   fixtures/build/sim-grimsel/grimsel.obcd  — beside the staged grimsel.obcm
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
    local bake_assets="$REPO_ROOT/host/obc-bake/assets"
    (cd "$REPO_ROOT" && cargo build --release --bin obc-dem)
    local obc_dem="$REPO_ROOT/target/release/obc-dem"
    for bbox in "$GRIMSEL_TERRAIN_BBOX" "$TENINGEN_TERRAIN_BBOX"; do
        "$obc_dem" fetch --bbox "$bbox" --out "$dem"
    done
    "$obc_dem" bake --sources "$dem" --bbox "$GRIMSEL_TERRAIN_BBOX" \
        --cell-log2 16 --shard "$BUILD_DIR/sim-grimsel/grimsel.obcd" --quiet
    "$obc_dem" bake --sources "$dem" --bbox "$TENINGEN_TERRAIN_BBOX" \
        --cell-log2 16 --shard "$bake_assets/teningen-preview.obcd" --quiet
    ls -la "$BUILD_DIR/sim-grimsel/grimsel.obcd" "$bake_assets/teningen-preview.obcd"
}
# =============================================================================

case "${1:-}" in
assistant) python3 "$FIXTURES_DIR/build-assistant-package.py" "${2:-all}" ;;
grimsel) do_grimsel "${2:-}" ;;
grimsel-demo) do_grimsel_demo "${2:-}" ;;
monaco) do_monaco "${2:-}" ;;
freiburg) do_freiburg "${2:-}" ;;
terrain) do_terrain "${2:-}" ;;
all)
    do_terrain "${4:-}"
    do_grimsel "${2:-}"
    do_grimsel_demo "${2:-}"
    do_monaco "${3:-}"
    do_freiburg
    ;;
*)
    echo "usage: $0 grimsel|grimsel-demo|monaco|freiburg|terrain|all|assistant \
[source.osm.pbf ... | meiringen|west-cork|all]" >&2
    exit 2
    ;;
esac
