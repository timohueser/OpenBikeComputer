"""Prototype: prepares an OSM PBF for the routing import, with pyosmium.

  prepare_osm.py merge OUT.osm.pbf IN.osm.pbf...        one sorted file, each object once
  prepare_osm.py clip OUT.osm.pbf IN.osm.pbf W,S,E,N    the roads with a node in the box

`clip` keeps each way with a `highway` tag or `route=ferry` and a node in the box, whole, with all
its nodes, and each relation with one of these ways or with a relation as a member. It is not a
general extract: it drops what the routing import does not read.

Usage: uv run --no-project --with osmium python3 -I prepare_osm.py ...
"""

import sys

import osmium


def merge(out, inputs):
    reader = osmium.MergeInputReader()
    for path in inputs:
        reader.add_file(path)
    writer = osmium.SimpleWriter(out, overwrite=True)
    reader.apply(writer, simplify=True)
    writer.close()


def clip(out, source, box):
    west, south, east, north = (float(value) for value in box.split(","))
    writer = osmium.BackReferenceWriter(out, ref_src=source, overwrite=True, remove_tags=False)
    objects = osmium.FileProcessor(source).with_locations().with_filter(osmium.filter.KeyFilter("highway", "route", "type"))
    kept = set()
    ways = relations = 0
    for obj in objects:
        if obj.is_way():
            if "highway" not in obj.tags and obj.tags.get("route") != "ferry":
                continue
            inside = any(
                node.location.valid() and west <= node.lon <= east and south <= node.lat <= north for node in obj.nodes
            )
            if inside:
                writer.add_way(obj)
                kept.add(obj.id)
                ways += 1
        elif obj.is_relation():
            if any(member.type == "r" or (member.type == "w" and member.ref in kept) for member in obj.members):
                writer.add_relation(obj)
                relations += 1
    writer.close()
    print(f"clip: {ways} ways, {relations} relations")


if sys.argv[1] == "merge":
    merge(sys.argv[2], sys.argv[3:])
elif sys.argv[1] == "clip":
    clip(sys.argv[2], sys.argv[3], sys.argv[4])
else:
    sys.exit(__doc__)
