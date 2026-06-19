#!/usr/bin/env python3
"""Stage-2 quadtree harness: replicate pack.py up to (but not including) the
quadtree build, dumping the per-LOD **pre-quadtree** feature list (post-simplify,
post-min_lod-filter) AND the reference `.obcm` (Python's quadtree + serialize).

The Rust `build_from_features` then builds its own quadtree from the SAME
simplified features and serializes. Comparing the two `.obcm` isolates the
quadtree port: simplify is shared (dumped from Python), so the only expected
difference is last-digit GEOS-version divergence in the boundary clip — surfaced
by `obcm_diff` (feature-multiset), per the render+multiset gate.

Coordinates are dumped as exact f64 bits (see dump_tree.py for why).

Usage:
  dump_features.py <pbf> <config.json> <features.json> <ref.obcm> [--no-land] [--chunk-size N]
"""
import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

import shapely
from obcm.config import load_config
from obcm.ingest import ingest_osm
from obcm.quadtree import QuadtreeNode
from obcm.serialize import serialize_lods

# Reuse the bit-exact coordinate encoder + style extraction from the Stage-1 harness.
from dump_tree import _fbits, style_list


def feature_to_dump(style_id, geom):
    if geom.geom_type == "Polygon":
        rings = [[[_fbits(x), _fbits(y)] for (x, y) in geom.exterior.coords]]
        rings += [[[_fbits(x), _fbits(y)] for (x, y) in r.coords] for r in geom.interiors]
        kind = "polygon"
    else:
        rings = [[[_fbits(x), _fbits(y)] for (x, y) in geom.coords]]
        kind = "line"
    return {"style_id": style_id, "kind": kind, "rings": rings}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("pbf")
    ap.add_argument("config")
    ap.add_argument("features")
    ap.add_argument("ref")
    ap.add_argument("--no-land", action="store_true")
    ap.add_argument("--chunk-size", type=int, default=4096)
    args = ap.parse_args()

    config = load_config(args.config)
    chunk_size = config.get("chunk_size", args.chunk_size)

    features, coastlines = ingest_osm(args.pbf, config)
    if not features and not coastlines:
        print("No features.", file=sys.stderr)
        sys.exit(1)

    all_geoms = [f["geometry"] for f in features] + coastlines
    min_lon, min_lat, max_lon, max_lat = shapely.total_bounds(all_geoms)
    global_bbox = (int(min_lon * 1e6), int(min_lat * 1e6), int(max_lon * 1e6), int(max_lat * 1e6))

    has_land = "natural" in config.get("features", {}) and "land" in config["features"]["natural"]
    if has_land and not args.no_land:
        from obcm.land_ingest import get_land_polygons
        land_style = config["features"]["natural"]["land"]["id"]
        land_min_lod = config["features"]["natural"]["land"].get("min_lod", 0)
        bbox_deg = (global_bbox[0] / 1e6, global_bbox[1] / 1e6, global_bbox[2] / 1e6, global_bbox[3] / 1e6)
        for poly in get_land_polygons(bbox_deg):
            features.append({"style_id": land_style, "min_lod": land_min_lod, "geometry": poly})

    lods_config = config.get("lods") or [{"max_mpp": None, "simplify": 0}]
    dumped_lods = []
    built_lods = []
    for i, lod_def in enumerate(lods_config):
        level_feats = [f for f in features if f.get("min_lod", 0) <= i]
        simplify_m = lod_def.get("simplify") or 0
        tol_deg = simplify_m / 111320.0 if simplify_m else 0.0

        feat_dump = []
        root = QuadtreeNode(global_bbox, chunk_size=chunk_size)
        for f in level_feats:
            geom = f["geometry"]
            if tol_deg:
                geom = geom.simplify(tol_deg)
                if geom.is_empty:
                    continue
            feat_dump.append(feature_to_dump(f["style_id"], geom))
            root.insert({"style_id": f["style_id"], "geometry": geom})
        dumped_lods.append({"max_mpp": lod_def.get("max_mpp"), "features": feat_dump})
        built_lods.append({"root": root, "chunk_size": chunk_size, "max_mpp": lod_def.get("max_mpp")})

    # Reference .obcm via the Python quadtree + serializer.
    Path(args.ref).write_bytes(serialize_lods(built_lods, config, global_bbox))

    marker_color = config.get("marker", {}).get("color", 0xF800)
    if isinstance(marker_color, str):
        marker_color = int(marker_color, 16)
    out = {
        "marker_color": marker_color,
        "global_bbox": list(global_bbox),
        "chunk_size": chunk_size,
        "styles": style_list(config),
        "lods": dumped_lods,
    }
    Path(args.features).write_text(json.dumps(out))
    print(f"ref={args.ref}  features={args.features}", file=sys.stderr)


if __name__ == "__main__":
    main()
