"""Prototype: one PMTiles archive (layer `roads`, zoom 10 to 14) of every road and path in a box, with slope class, surface class and trail grades.

pip install osmium rasterio numpy shapely mapbox-vector-tile pmtiles pillow
python3 -I build-data.py --bbox W,S,E,N --dem-dir DIR --out FILE.pmtiles [--terrain URL --slope-out FILE.png] EXTRACT.osm.pbf [...]

The heights come from the planner release's Terrarium tiles at zoom 12 (`--terrain`, a `{z}/{x}/{y}` URL), or else from
Copernicus GLO-30 tiles, downloaded into --dem-dir. `--slope-out` also writes the slope angle of the terrain as one
image in Web Mercator, with its corners in FILE.png.json.
"""
import argparse, gzip, json, math, os, subprocess, sys, urllib.request
import numpy as np
import osmium
import rasterio
import mapbox_vector_tile
import shapely
from pmtiles.tile import Compression, TileType, zxy_to_tileid
from pmtiles.writer import Writer
from PIL import Image

# planner/router-build/src/source.rs highway_access; motorways carry no bike or foot.
HIGHWAYS = {"cycleway", "residential", "living_street", "unclassified", "service", "tertiary", "tertiary_link",
            "trunk", "trunk_link", "primary", "primary_link", "secondary", "secondary_link", "track", "road",
            "path", "footway", "pedestrian", "bridleway", "steps"}
# planner/router-build/src/osm.rs surface_class: 0 unknown, 1 paved, 2 compacted, 3 gravel, 4 dirt, 5 rough.
SURFACE = {**dict.fromkeys(["asphalt", "paved", "concrete", "concrete:plates", "concrete:lanes", "paving_stones"], 1),
           **dict.fromkeys(["compacted", "fine_gravel"], 2), **dict.fromkeys(["gravel", "pebblestone"], 3),
           **dict.fromkeys(["ground", "dirt", "earth", "grass", "unpaved"], 4),
           **dict.fromkeys(["sand", "mud", "rock", "stone", "cobblestone", "sett"], 5)}
SAC = {"strolling": 0, "hiking": 1, "mountain_hiking": 2, "demanding_mountain_hiking": 3, "alpine_hiking": 4,
       "demanding_alpine_hiking": 5, "difficult_alpine_hiking": 6}
R = 6378137.0
SLOPE_BREAKS = [3, 6, 9, 12]  # percent: classes 0..4
STEP_M, WINDOW_M = 25.0, 100.0


class Dem:
    def __init__(self, directory, bbox):
        self.tiles = {}
        os.makedirs(directory, exist_ok=True)
        w, s, e, n = bbox
        for lat in range(math.floor(s), math.floor(n) + 1):
            for lon in range(math.floor(w), math.floor(e) + 1):
                name = f"Copernicus_DSM_COG_10_{'N' if lat >= 0 else 'S'}{abs(lat):02d}_00_{'E' if lon >= 0 else 'W'}{abs(lon):03d}_00_DEM"
                path = os.path.join(directory, name + ".tif")
                if not os.path.exists(path):
                    print("download", name, file=sys.stderr)
                    urllib.request.urlretrieve(f"https://copernicus-dem-30m.s3.amazonaws.com/{name}/{name}.tif", path)
                with rasterio.open(path) as src:
                    self.tiles[(lat, lon)] = (src.read(1).astype(np.float32), ~src.transform)

    def height(self, lon, lat):
        data, inv = self.tiles[(math.floor(lat), math.floor(lon))]
        col, row = inv * (lon, lat)
        col, row = col - 0.5, row - 0.5
        c0, r0 = int(col), int(row)
        c0, r0 = min(max(c0, 0), data.shape[1] - 2), min(max(r0, 0), data.shape[0] - 2)
        fc, fr = col - c0, row - r0
        q = data[r0:r0 + 2, c0:c0 + 2]
        return float(q[0, 0] * (1 - fc) * (1 - fr) + q[0, 1] * fc * (1 - fr) + q[1, 0] * (1 - fc) * fr + q[1, 1] * fc * fr)


class TerrainTiles:
    """A mosaic of the Terrarium tiles of one zoom over the box, cached in `directory`."""
    Z, SIZE = 12, 512

    def __init__(self, template, directory, bbox, margin=0.02):
        os.makedirs(directory, exist_ok=True)
        n = 2 ** self.Z
        tx = lambda lon: int((lon + 180) / 360 * n)
        ty = lambda lat: int((1 - math.asinh(math.tan(math.radians(lat))) / math.pi) / 2 * n)
        self.x0, self.y0 = tx(bbox[0] - margin), ty(bbox[3] + margin)
        x1, y1 = tx(bbox[2] + margin), ty(bbox[1] - margin)
        rows = []
        for y in range(self.y0, y1 + 1):
            row = []
            for x in range(self.x0, x1 + 1):
                path = os.path.join(directory, f"{self.Z}-{x}-{y}.webp")
                if not os.path.exists(path):
                    subprocess.run(["curl", "-sSf", "-o", path, template.format(z=self.Z, x=x, y=y)], check=True)
                rgb = np.asarray(Image.open(path).convert("RGB"), dtype=np.float32)
                row.append(rgb[..., 0] * 256 + rgb[..., 1] + rgb[..., 2] / 256 - 32768)
            rows.append(np.hstack(row))
        self.data = np.vstack(rows)

    def pixel(self, lon, lat):
        n = 2 ** self.Z * self.SIZE
        return ((lon + 180) / 360 * n - self.x0 * self.SIZE,
                (1 - math.asinh(math.tan(math.radians(lat))) / math.pi) / 2 * n - self.y0 * self.SIZE)

    def height(self, lon, lat):
        col, row = self.pixel(lon, lat)
        col, row = col - 0.5, row - 0.5
        c0, r0 = int(col), int(row)
        if not (0 <= c0 < self.data.shape[1] - 1 and 0 <= r0 < self.data.shape[0] - 1):
            raise KeyError((lon, lat))
        fc, fr = col - c0, row - r0
        q = self.data[r0:r0 + 2, c0:c0 + 2]
        return float(q[0, 0] * (1 - fc) * (1 - fr) + q[0, 1] * fc * (1 - fr) + q[1, 0] * (1 - fc) * fr + q[1, 1] * fc * fr)

    def corners(self):
        n = 2 ** self.Z
        lon = lambda x: x / n * 360 - 180
        lat = lambda y: math.degrees(math.atan(math.sinh(math.pi * (1 - 2 * y / n))))
        x1, y1 = self.x0 + self.data.shape[1] // self.SIZE, self.y0 + self.data.shape[0] // self.SIZE
        return [[lon(self.x0), lat(self.y0)], [lon(x1), lat(self.y0)], [lon(x1), lat(y1)], [lon(self.x0), lat(y1)]]


# Ski-touring classes: 30-35, 35-40, 40-45, above 45 degrees; below 30 is clear.
SLOPE_ANGLES = [(30, (242, 229, 0)), (35, (245, 150, 0)), (40, (224, 0, 26)), (45, (130, 40, 160))]


def write_slope_angle(terrain, path):
    rows = np.arange(terrain.data.shape[0]) + 0.5
    n = 2 ** terrain.Z * terrain.SIZE
    lat = np.arctan(np.sinh(np.pi * (1 - 2 * (rows + terrain.y0 * terrain.SIZE) / n)))
    pixel_m = 2 * math.pi * R / n * np.cos(lat)  # ground size of a pixel, by row
    dz_dy, dz_dx = np.gradient(terrain.data)
    angle = np.degrees(np.arctan(np.hypot(dz_dx, dz_dy) / pixel_m[:, None]))
    rgba = np.zeros((*angle.shape, 4), dtype=np.uint8)
    for limit, color in SLOPE_ANGLES:
        rgba[angle >= limit] = (*color, 255)
    Image.fromarray(rgba).save(path, optimize=True)
    with open(path + ".json", "w") as f:
        json.dump({"coordinates": terrain.corners()}, f)
    share = [float((angle >= limit).mean()) for limit, _ in SLOPE_ANGLES]
    print(f"{path}: {rgba.shape[1]}x{rgba.shape[0]} px, {os.path.getsize(path) / 1e6:.1f} MB; share of ground at or above "
          + ", ".join(f"{limit}\u00b0 {v:.1%}" for (limit, _), v in zip(SLOPE_ANGLES, share)), file=sys.stderr)


def metres(a, b):
    k = math.cos(math.radians((a[1] + b[1]) / 2))
    return math.hypot((b[0] - a[0]) * k, b[1] - a[1]) * 111_320


def slope_class(percent):
    return int(sum(percent >= b for b in SLOPE_BREAKS))


def stretches(coords, dem, structure):
    """The way split where its slope class changes: [(class, [coords])]."""
    lengths = [metres(a, b) for a, b in zip(coords, coords[1:])]
    total = sum(lengths)
    if total < 1:
        return []
    if structure:  # Bridges and tunnels: the terrain under them is wrong, so the slope runs end to end.
        rise = abs(dem.height(*coords[-1]) - dem.height(*coords[0]))
        return [(slope_class(100 * rise / total), coords)]
    # Samples every STEP_M along the way; the slope at a sample spans WINDOW_M of the way around it.
    at = np.concatenate([[0.0], np.cumsum(lengths)])
    samples = np.linspace(0, total, max(2, int(round(total / STEP_M)) + 1))
    xs, ys = np.interp(samples, at, [c[0] for c in coords]), np.interp(samples, at, [c[1] for c in coords])
    z = np.array([dem.height(x, y) for x, y in zip(xs, ys)])
    half = WINDOW_M / 2
    classes = []
    for d in samples:
        lo, hi = max(0.0, d - half), min(total, d + half)
        if hi - lo < WINDOW_M and total >= WINDOW_M:  # Near an end, keep the full window inside the way.
            lo, hi = (0.0, WINDOW_M) if lo == 0 else (total - WINDOW_M, total)
        rise = abs(np.interp(hi, samples, z) - np.interp(lo, samples, z))
        classes.append(slope_class(100 * rise / max(hi - lo, 1)))
    # A class holds from the midpoint between two samples; split the way geometry there.
    out, start, current = [], 0.0, classes[0]
    for i in range(1, len(classes)):
        if classes[i] != current:
            cut = (samples[i - 1] + samples[i]) / 2
            out.append((current, cut_line(coords, at, start, cut)))
            start, current = cut, classes[i]
    out.append((current, cut_line(coords, at, start, total)))
    return out


def cut_line(coords, at, a, b):
    point = lambda d: (float(np.interp(d, at, [c[0] for c in coords])), float(np.interp(d, at, [c[1] for c in coords])))
    inner = [coords[i] for i in range(len(coords)) if a < at[i] < b]
    return [point(a), *inner, point(b)]


def grade(value, table=None):
    if value is None:
        return None
    if table is not None:
        return table.get(value)
    try:
        n = int(value.rstrip("+-"))
    except ValueError:
        return None
    return n if 0 <= n <= 6 else None


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--bbox", required=True)
    p.add_argument("--dem-dir", required=True)
    p.add_argument("--terrain")
    p.add_argument("--slope-out")
    p.add_argument("--out", required=True)
    p.add_argument("extracts", nargs="+")
    args = p.parse_args()
    bbox = [float(v) for v in args.bbox.split(",")]
    w, s, e, n = bbox
    dem = TerrainTiles(args.terrain, args.dem_dir, bbox) if args.terrain else Dem(args.dem_dir, bbox)
    if args.slope_out:
        write_slope_angle(dem, args.slope_out)
    seen, features = set(), []
    for extract in args.extracts:
        for way in osmium.FileProcessor(extract, osmium.osm.NODE | osmium.osm.WAY).with_locations():
            tags = way.tags
            if not way.is_way() or way.id in seen or tags.get("highway") not in HIGHWAYS or tags.get("area") == "yes":
                continue
            try:
                coords = [(node.lon, node.lat) for node in way.nodes]
            except osmium.InvalidLocationError:
                continue
            if len(coords) < 2 or not any(w <= x <= e and s <= y <= n for x, y in coords):
                continue
            seen.add(way.id)
            structure = any(tags.get(k, "no") != "no" for k in ("bridge", "tunnel"))
            props = {"surface": SURFACE.get(tags.get("surface", ""), 0), "sac": grade(tags.get("sac_scale"), SAC),
                     "mtb": grade(tags.get("mtb:scale")), "path": tags.get("highway") in ("path", "footway", "bridleway", "steps", "track")}
            props = {k: v for k, v in props.items() if v is not None}
            try:
                parts = stretches(coords, dem, structure)
            except KeyError:  # A node beyond the terrain tiles of the box.
                continue
            for slope, line in parts:
                features.append(({**props, "slope": slope}, line))
    print(f"{len(seen)} ways, {len(features)} stretches", file=sys.stderr)
    write_tiles(features, bbox, args.out)


def mercator(lonlat):
    lon, lat = lonlat[:, 0], lonlat[:, 1]
    return np.column_stack([np.radians(lon) * R, np.log(np.tan(np.pi / 4 + np.radians(lat) / 2)) * R])


def write_tiles(features, bbox, path, zooms=range(10, 15)):
    lines = shapely.transform(np.array([shapely.LineString(line) for _, line in features]), mercator)
    tree, world = shapely.STRtree(lines), 2 * math.pi * R
    (x0, y1), (x1, y0) = mercator(np.array([[bbox[0], bbox[1]], [bbox[2], bbox[3]]]))
    tiles, sizes = [], {}
    for z in zooms:
        size = world / 2 ** z
        col = lambda x: int((x + world / 2) // size)
        row = lambda y: int((world / 2 - y) // size)
        for tx in range(col(x0), col(x1) + 1):
            for ty in range(row(y0), row(y1) + 1):
                minx, maxy = tx * size - world / 2, world / 2 - ty * size
                bounds = (minx, maxy - size, minx + size, maxy)
                pad = size / 64
                clip = shapely.box(bounds[0] - pad, bounds[1] - pad, bounds[2] + pad, bounds[3] + pad)
                pixel = size / 4096
                out = []
                for i in tree.query(clip):
                    geom = shapely.intersection(lines[i], clip).simplify(pixel)
                    if geom.is_empty or geom.length < 2 * pixel:
                        continue
                    out.append({"geometry": geom, "properties": features[i][0]})
                if not out:
                    continue
                data = gzip.compress(mapbox_vector_tile.encode([{"name": "roads", "features": out}],
                                     default_options={"quantize_bounds": bounds, "extents": 4096}))
                tiles.append((zxy_to_tileid(z, tx, ty), data))
                sizes.setdefault(z, []).append(len(data))
    with open(path, "wb") as f:
        writer = Writer(f)
        for tile_id, data in sorted(tiles):
            writer.write_tile(tile_id, data)
        e7 = lambda v: int(v * 1e7)
        writer.finalize({"tile_type": TileType.MVT, "tile_compression": Compression.GZIP,
                         "min_lon_e7": e7(bbox[0]), "min_lat_e7": e7(bbox[1]), "max_lon_e7": e7(bbox[2]), "max_lat_e7": e7(bbox[3]),
                         "center_zoom": 12, "center_lon_e7": e7((bbox[0] + bbox[2]) / 2), "center_lat_e7": e7((bbox[1] + bbox[3]) / 2)},
                        {"vector_layers": [{"id": "roads", "fields": {}}]})
    for z, s in sorted(sizes.items()):
        print(f"z{z}: {len(s)} tiles, largest {max(s) / 1e3:.0f} kB, median {sorted(s)[len(s) // 2] / 1e3:.0f} kB (gzip)", file=sys.stderr)
    print(f"{path}: {os.path.getsize(path) / 1e6:.1f} MB", file=sys.stderr)


if __name__ == "__main__":
    main()
