"""Prototype: the ways that the router reads from one PBF, as flat arrays in an .npz file."""

import sys
import time

import numpy as np
import osmium
import shapely

# `highway_access` in planner/router-build/src/source.rs, without motorways and bridleways; a link counts as its road.
KINDS = [
    "cycleway", "residential", "living_street", "unclassified", "service", "tertiary", "secondary", "primary",
    "trunk", "track", "road", "path", "footway", "pedestrian", "steps",
]  # fmt: skip
TAGS = [
    "surface", "tracktype", "smoothness", "bicycle", "foot", "segregated", "lit", "maxspeed", "width", "service",
    "access", "motor_vehicle", "mtb:scale", "sac_scale", "bridge", "tunnel",
    "footway", "lanes", "oneway", "trail_visibility", "ref",
]  # fmt: skip
ROUTES = {"bicycle": 0, "mtb": 1, "hiking": 2, "foot": 2}
LEVELS = {"lcn": 1, "rcn": 2, "ncn": 3, "icn": 4, "lwn": 1, "rwn": 2, "nwn": 3, "iwn": 4}
LAND = ["none", "forest", "residential", "commercial", "farmland", "meadow", "vineyard_orchard", "park"]


def land_class(tags):
    use, natural, leisure = tags.get("landuse"), tags.get("natural"), tags.get("leisure")
    if use == "forest" or natural == "wood":
        return 1
    if use == "residential":
        return 2
    if use in ("commercial", "retail", "industrial", "railway", "education", "institutional", "military"):
        return 3
    if use == "farmland":
        return 4
    if use in ("meadow", "grass") or natural in ("grassland", "heath", "scrub"):
        return 5
    if use in ("vineyard", "orchard", "plant_nursery"):
        return 6
    if leisure in ("park", "garden", "playground", "pitch", "sports_centre", "golf_course") or use in (
        "cemetery", "allotments", "recreation_ground", "village_green",
    ):  # fmt: skip
        return 7
    return 0


def main(pbf, out):
    start = time.time()
    routes = {}
    for rel in osmium.FileProcessor(pbf, osmium.osm.RELATION).with_filter(osmium.filter.KeyFilter("route")):
        column = ROUTES.get(rel.tags.get("route"))
        if column is None:
            continue
        level = LEVELS.get(rel.tags.get("network", ""), 1)
        for member in rel.members:
            if member.type == "w":
                row = routes.setdefault(member.ref, [0, 0, 0])
                row[column] = max(row[column], level)

    kind_of = {kind: index for index, kind in enumerate(KINDS)}
    ids, kinds, lengths, lons, lats, firsts, lasts, names, way_routes = [], [], [], [], [], [], [], [], []
    tags = {tag: [] for tag in TAGS}
    node_way, node_ref = [], []
    name_ids = {"": 0}
    skipped = 0
    ways = (
        osmium.FileProcessor(pbf, osmium.osm.NODE | osmium.osm.WAY)
        .with_locations()
        .with_filter(osmium.filter.EntityFilter(osmium.osm.WAY))
        .with_filter(osmium.filter.KeyFilter("highway"))
    )
    for way in ways:
        kind = kind_of.get(way.tags.get("highway", "").removesuffix("_link"))
        if kind is None or way.tags.get("area") == "yes" or len(way.nodes) < 2:
            continue
        try:
            length = osmium.geom.haversine_distance(way.nodes)
            middle = way.nodes[len(way.nodes) // 2].location
            lon, lat = middle.lon, middle.lat
        except (osmium.InvalidLocationError, RuntimeError, ValueError):
            skipped += 1
            continue
        refs = [node.ref for node in way.nodes]
        node_way.extend([len(ids)] * len(refs))
        node_ref.extend(refs)
        ids.append(way.id)
        kinds.append(kind)
        lengths.append(length)
        lons.append(lon)
        lats.append(lat)
        firsts.append(refs[0])
        lasts.append(refs[-1])
        names.append(name_ids.setdefault(way.tags.get("name", ""), len(name_ids)))
        way_routes.append(routes.get(way.id, (0, 0, 0)))
        for tag, values in tags.items():
            values.append(way.tags.get(tag, "")[:24])
    print(f"  {len(ids)} ways, {skipped} skipped for missing nodes, {time.time() - start:.0f} s", flush=True)

    areas = (
        osmium.FileProcessor(pbf)
        .with_areas(osmium.filter.KeyFilter("landuse", "natural", "leisure"))
        .with_filter(osmium.filter.EntityFilter(osmium.osm.AREA))
    )
    factory = osmium.geom.WKBFactory()
    shapes, classes = [], []
    for area in areas:
        land = land_class(area.tags)
        if land:
            try:
                shapes.append(factory.create_multipolygon(area))
                classes.append(land)
            except RuntimeError:
                pass
    polygons = shapely.from_wkb(shapes)
    valid = np.array([polygon is not None for polygon in polygons], bool) if len(shapes) else np.zeros(0, bool)
    polygons, classes = polygons[valid], np.array(classes, np.int8)[valid]
    land = np.zeros(len(ids), np.int8)
    if len(polygons):
        points, hits = shapely.STRtree(polygons).query(shapely.points(lons, lats), predicate="within")
        # The smallest polygon around the middle node wins.
        order = np.argsort(-shapely.area(polygons[hits]))
        land[points[order]] = classes[hits[order]]
    print(f"  {len(polygons)} land-use areas, {time.time() - start:.0f} s", flush=True)

    np.savez_compressed(
        out,
        id=np.array(ids, np.int64),
        kind=np.array(kinds, np.int8),
        length=np.array(lengths, np.float64),
        lon=np.array(lons, np.float64),
        lat=np.array(lats, np.float64),
        first=np.array(firsts, np.int64),
        last=np.array(lasts, np.int64),
        name=np.array(names, np.int32),
        routes=np.array(way_routes, np.int8).reshape(-1, 3),
        land=land,
        node_way=np.array(node_way, np.int32),
        node_ref=np.array(node_ref, np.int64),
        **{"tag_" + tag: np.array(values, "U24") for tag, values in tags.items()},
    )


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
