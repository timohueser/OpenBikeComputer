#!/usr/bin/env python3
"""Stage-1 serializer harness: replicate pack.py's pipeline, then emit BOTH the
reference `.obcm` (via the oracle's serialize_lods) AND a JSON dump of the exact
quadtrees that produced it. The Rust `serialize_from_dump` re-serializes the dump
and must byte-match the reference — isolating the serializer from ingest/quadtree
/GEOS, where byte-parity is genuinely achievable.

This re-implements pack.py's orchestration (it is not importable) using the SAME
obcm library functions, so the reference it writes is byte-identical to pack.py's
own output (verified by the driver). The Python pipeline itself is never modified.

Usage:
  dump_tree.py <pbf> <config.json> <dump.json> <ref.obcm> [--no-land] [--chunk-size N]
"""
import argparse
import json
import struct
import sys
from pathlib import Path


def _fbits(v):
    """Exact f64 -> u64 bit pattern. Coordinates are dumped as bits, not decimal
    text, because decimal round-trip is NOT lossless: serde_json can parse a
    shortest-repr string to a value 1 ULP off Python's, which flips a `*1e6`
    halfway case and changes a microdegree. Bits make the transport exact, so the
    serializer test exercises the real coordinate rounding without false diffs."""
    return struct.unpack("<Q", struct.pack("<d", float(v)))[0]

# Make the `obcm` package importable regardless of cwd.
sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

import shapely
from obcm.config import load_config
from obcm.ingest import ingest_osm
from obcm.quadtree import QuadtreeNode
from obcm.serialize import serialize_lods


def node_to_dump(node):
    bbox = [int(c) for c in node.bbox]
    if node.is_leaf:
        feats = []
        for feat in node.features:
            g = feat["geometry"]
            def ring_bits(coords):
                return [[_fbits(x), _fbits(y)] for (x, y) in coords]

            if g.geom_type == "Polygon":
                rings = [ring_bits(g.exterior.coords)] + [ring_bits(r.coords) for r in g.interiors]
                kind = "polygon"
            else:  # LineString / LinearRing both pack as a single non-polygon ring
                rings = [ring_bits(g.coords)]
                kind = "line"
            feats.append({"style_id": feat["style_id"], "kind": kind, "rings": rings})
        return {"bbox": bbox, "features": feats}
    return {"bbox": bbox, "children": [node_to_dump(c) for c in node.children]}


def style_list(config):
    out = []
    for feature_type in config.get("features", {}).values():
        for s in feature_type.values():
            color = s["color"]
            if isinstance(color, str):
                color = int(color, 16)
            out.append({
                "id": s["id"],
                "z_index": s.get("z_index", 0),
                "color": color,
                "weight": s.get("weight", 1),
                "priority": s.get("priority", 3),
            })
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("pbf")
    ap.add_argument("config")
    ap.add_argument("dump")
    ap.add_argument("ref")
    ap.add_argument("--no-land", action="store_true",
                    help="skip land-polygon generation (faster; the serializer test is land-agnostic)")
    ap.add_argument("--chunk-size", type=int, default=4096)
    args = ap.parse_args()

    config = load_config(args.config)
    chunk_size = config.get("chunk_size", args.chunk_size)

    features, coastlines = ingest_osm(args.pbf, config)
    if not features and not coastlines:
        print("No features found matching config.", file=sys.stderr)
        sys.exit(1)

    # Global bbox: int() truncation toward zero (NOT rounding) — plan §4.3.
    all_geoms = [f["geometry"] for f in features] + coastlines
    min_lon, min_lat, max_lon, max_lat = shapely.total_bounds(all_geoms)
    global_bbox = (int(min_lon * 1e6), int(min_lat * 1e6), int(max_lon * 1e6), int(max_lat * 1e6))

    # Land generation (mirrors pack.py); skippable for fast serializer iteration.
    has_land = "natural" in config.get("features", {}) and "land" in config["features"]["natural"]
    if has_land and not args.no_land:
        from obcm.land_ingest import get_land_polygons
        land_style = config["features"]["natural"]["land"]["id"]
        land_min_lod = config["features"]["natural"]["land"].get("min_lod", 0)
        bbox_deg = (global_bbox[0] / 1e6, global_bbox[1] / 1e6, global_bbox[2] / 1e6, global_bbox[3] / 1e6)
        for poly in get_land_polygons(bbox_deg):
            features.append({"style_id": land_style, "min_lod": land_min_lod, "geometry": poly})

    # Build one quadtree per LOD (cumulative + per-level simplify), exactly like pack.py.
    lods_config = config.get("lods") or [{"max_mpp": None, "simplify": 0}]
    built_lods = []
    for i, lod_def in enumerate(lods_config):
        level_feats = [f for f in features if f.get("min_lod", 0) <= i]
        simplify_m = lod_def.get("simplify") or 0
        tol_deg = simplify_m / 111320.0 if simplify_m else 0.0
        root = QuadtreeNode(global_bbox, chunk_size=chunk_size)
        for f in level_feats:
            geom = f["geometry"]
            if tol_deg:
                geom = geom.simplify(tol_deg)
                if geom.is_empty:
                    continue
            root.insert({"style_id": f["style_id"], "geometry": geom})
        built_lods.append({"root": root, "chunk_size": chunk_size, "max_mpp": lod_def.get("max_mpp")})

    # Reference .obcm via the oracle's own serializer.
    ref_bytes = serialize_lods(built_lods, config, global_bbox)
    Path(args.ref).write_bytes(ref_bytes)

    # JSON dump of the same trees for the Rust serializer.
    marker_color = config.get("marker", {}).get("color", 0xF800)
    if isinstance(marker_color, str):
        marker_color = int(marker_color, 16)
    dump = {
        "marker_color": marker_color,
        "global_bbox": list(global_bbox),
        "styles": style_list(config),
        "lods": [
            {"max_mpp": lod["max_mpp"], "chunk_size": lod["chunk_size"], "root": node_to_dump(lod["root"])}
            for lod in built_lods
        ],
    }
    Path(args.dump).write_text(json.dumps(dump))
    print(f"ref={args.ref} ({len(ref_bytes)} bytes)  dump={args.dump}", file=sys.stderr)


if __name__ == "__main__":
    main()
