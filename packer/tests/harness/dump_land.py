#!/usr/bin/env python3
"""Dump the oracle land set (`obcm.land_ingest.get_land_polygons`) as JSON for the
Stage-5 land-parity gate — the oracle counterpart of the Rust `land_probe` bin.
MultiPolygons (and any GeometryCollection) are flattened to individual polygons so
the two sides line up (the Rust port emits one polygon per face). Raw lon/lat
floats; `compare_land.py` checks area + count, not vertex identity.

Usage:  dump_land.py <min_lon> <min_lat> <max_lon> <max_lat> <out.json>
"""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))  # repo/packer
from obcm.land_ingest import get_land_polygons


def rings(poly):
    ext = [[x, y] for (x, y) in poly.exterior.coords]
    holes = [[[x, y] for (x, y) in r.coords] for r in poly.interiors]
    return {"ext": ext, "holes": holes}


def flatten(geom, out):
    t = geom.geom_type
    if t == "Polygon":
        out.append(rings(geom))
    elif t in ("MultiPolygon", "GeometryCollection"):
        for g in geom.geoms:
            flatten(g, out)
    # non-polygonal pieces (stray clip lines/points) carry no land fill — drop.


def main():
    min_lon, min_lat, max_lon, max_lat, out = sys.argv[1:6]
    bbox = (float(min_lon), float(min_lat), float(max_lon), float(max_lat))
    flat = []
    for g in get_land_polygons(bbox):
        flatten(g, flat)
    Path(out).write_text(json.dumps({"polygons": flat}))


if __name__ == "__main__":
    main()
