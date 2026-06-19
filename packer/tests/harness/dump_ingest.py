#!/usr/bin/env python3
"""Stage-3 ingest oracle dump (handover §6.1).

Runs the REAL `obcm.ingest.OSMHandler` via a thin *observing* subclass — it only
calls `super().way()/area()`, so the geometry and feature list are exactly the
oracle's — and records per-feature provenance (`way` vs `area`, and for areas
`from_way` + tags). From that it derives the **Stage-3 expected set**: the
features the Rust port should also produce. Specifically:

  - every LineString from `way()`                 -> KEEP (open + closed-non-area)
  - `area()` polygons built from a single closed way that is a genuine area
    (`is_area(tags)` true; admin_level ones never reach area())  -> KEEP
  - `area()` polygons from a relation (`from_way()` false)        -> DROP (Stage 4)
  - `area()` polygons from a closed *line* way (`is_area` false — the oracle's
    double-emit bug)                                              -> DROP

Vertices are microdegree-rounded (`int(round(v*1e6))`, banker's). Output schema
matches the Rust `ingest_dump` bin so `compare_ingest.py` can diff them.

Usage:  dump_ingest.py <pbf> <config.json> <out.json>
"""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

import osmium
import osmium.area
import osmium.index
from obcm.config import load_config
from obcm.ingest import OSMHandler

# Mirror ingest.py::way's area_tags + the is_area heuristic exactly.
AREA_TAGS = ("building", "landuse", "amenity", "leisure", "natural", "waterway")


def is_area(tags):
    a = tags.get("area")
    if a == "yes":
        return True
    if a == "no":
        return False
    return any(k in tags for k in AREA_TAGS)


def ud(v):
    return int(round(v * 1e6))


class ProvenanceHandler(OSMHandler):
    """Observes the oracle handler: each appended feature gets a parallel meta
    entry recording where it came from. Calls super() so behaviour is unchanged."""

    def __init__(self, config):
        super().__init__(config)
        self.meta = []  # parallel to self.features

    def way(self, w):
        before = len(self.features)
        super().way(w)
        for _ in range(before, len(self.features)):
            self.meta.append(("way", None, None))

    def area(self, a):
        # Capture provenance before super() consumes the (callback-scoped) area.
        from_way = a.from_way()
        tags = {t.k: t.v for t in a.tags}
        before = len(self.features)
        super().area(a)
        for _ in range(before, len(self.features)):
            self.meta.append(("area", from_way, tags))


def feature_json(feat):
    geom = feat["geometry"]
    if geom.geom_type == "Polygon":
        rings = [[[ud(x), ud(y)] for (x, y) in geom.exterior.coords]]
        rings += [[[ud(x), ud(y)] for (x, y) in hole.coords] for hole in geom.interiors]
        return {"style_id": feat["style_id"], "kind": "polygon", "rings": rings}
    return {"style_id": feat["style_id"], "kind": "line", "rings": [[[ud(x), ud(y)] for (x, y) in geom.coords]]}


def main():
    pbf, config_path, out_path = sys.argv[1], sys.argv[2], sys.argv[3]
    config = load_config(config_path)
    handler = ProvenanceHandler(config)

    # 2-pass apply — copied verbatim from ingest.py::ingest_osm orchestration
    # (the oracle handler logic itself is untouched, only observed).
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

    features = []
    dropped_rel = dropped_blob = 0
    for feat, (src, from_way, tags) in zip(handler.features, handler.meta):
        if src == "area":
            if not from_way:
                dropped_rel += 1
                continue  # relation-sourced -> Stage 4
            if not is_area(tags):
                dropped_blob += 1
                continue  # closed line-way double-emit -> Rust emits the line only
        features.append(feature_json(feat))

    coastlines = [[[ud(x), ud(y)] for (x, y) in c.coords] for c in handler.coastlines]
    out = {"features": features, "coastlines": coastlines}
    Path(out_path).write_text(json.dumps(out))
    print(
        f"{out_path}: {len(features)} expected features (+{dropped_rel} relation, "
        f"+{dropped_blob} closed-line-way blob dropped), {len(coastlines)} coastlines",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
