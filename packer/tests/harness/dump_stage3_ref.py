#!/usr/bin/env python3
"""Stage-3 end-to-end **reference** `.obcm` (handover §6.2).

Builds the Python pipeline restricted to the *same* set the Rust port produces —
lines + genuine closed-way-area polygons, with relations and closed-line-way
blobs removed (via the `dump_ingest.ProvenanceHandler`) — then computes the bbox
over THAT set + coastlines and runs the oracle's own quadtree + serializer. So
the only differences from Rust's `obc-pack` output are the GEOS-version skew in
`simplify` (shapely 3.13 vs geos 3.14) and feature/ring ordering; `obcm_diff`
checks the structural + feature-multiset gate.

Usage:  dump_stage3_ref.py <pbf> <config.json> <ref.obcm> [--chunk-size N]
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
sys.path.insert(0, str(Path(__file__).resolve().parent))  # for dump_ingest

import osmium
import osmium.area
import osmium.index
import shapely
from obcm.config import load_config
from obcm.quadtree import QuadtreeNode
from obcm.serialize import serialize_lods

from dump_ingest import ProvenanceHandler, is_area


def stage3_features(pbf, config, with_relations=False):
    """Run the oracle handler and return the feature list restricted to the set the
    Rust port produces (each `{style_id, min_lod, geometry}`) + coastlines.
    `with_relations` keeps the relation-assembled polygons (the Stage-4 set)."""
    handler = ProvenanceHandler(config)
    idx = osmium.index.create_map("flex_mem")
    lh = osmium.NodeLocationsForWays(idx)
    lh.ignore_errors()
    am = osmium.area.AreaManager()
    r = osmium.io.Reader(pbf, osmium.osm.osm_entity_bits.RELATION)
    osmium.apply(r, am.first_pass_handler())
    r.close()
    r = osmium.io.Reader(pbf)
    osmium.apply(r, lh, am.second_pass_handler(handler), handler)
    r.close()

    feats = []
    for feat, (src, from_way, tags) in zip(handler.features, handler.meta):
        if src == "area":
            if not from_way:
                if not with_relations:
                    continue  # relation -> Stage 4
            elif not is_area(tags):
                continue  # closed line-way double-emit -> Rust emits the line only
        feats.append(feat)
    return feats, handler.coastlines


def main():
    import argparse

    ap = argparse.ArgumentParser()
    ap.add_argument("pbf")
    ap.add_argument("config")
    ap.add_argument("ref")
    ap.add_argument("--chunk-size", type=int, default=4096)
    ap.add_argument("--with-relations", action="store_true",
                    help="keep relation-assembled polygons (the Stage-4 reference set)")
    args = ap.parse_args()

    config = load_config(args.config)
    chunk_size = config.get("chunk_size", args.chunk_size)

    feats, coastlines = stage3_features(args.pbf, config, with_relations=args.with_relations)
    if not feats and not coastlines:
        print("No features.", file=sys.stderr)
        sys.exit(1)

    # bbox over the Stage-3 set + coastlines, int() truncation (NOT rounding).
    all_geoms = [f["geometry"] for f in feats] + list(coastlines)
    min_lon, min_lat, max_lon, max_lat = shapely.total_bounds(all_geoms)
    global_bbox = (int(min_lon * 1e6), int(min_lat * 1e6), int(max_lon * 1e6), int(max_lat * 1e6))

    # One quadtree per LOD (cumulative + per-level simplify), exactly like pack.py.
    lods_config = config.get("lods") or [{"max_mpp": None, "simplify": 0}]
    built_lods = []
    for i, lod_def in enumerate(lods_config):
        level_feats = [f for f in feats if f.get("min_lod", 0) <= i]
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

    Path(args.ref).write_bytes(serialize_lods(built_lods, config, global_bbox))
    print(f"{args.ref}: {len(feats)} features, {len(coastlines)} coastlines, bbox={global_bbox}", file=sys.stderr)


if __name__ == "__main__":
    main()
