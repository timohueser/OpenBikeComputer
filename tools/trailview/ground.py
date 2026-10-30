"""Bake the TrailView ground tiles of one region, as specs/trailview-tiles.md defines them: the material
and the lithology from the ground sources of `sources.py`, and the region, joined into one archive.

The `obc data` steps (`--step`) are `planner/trailview-material`, `planner/trailview-lithology`,
`planner/trailview-region` and `planner/trailview-ground`; the option `layer` names the one to bake.
`ground` joins the other three. `--source ID --fetch OUT` is the capture of a ground source whose
protocol a capture reads; `--archive ZIP --work DIR` gives the file of an `archive-features` source.
"""

import argparse
import io
import itertools
import json
import math
import os
import sys
import tempfile
from pathlib import Path

import numpy as np

from . import sources, tiles
from .sources import Tile, mercator_m, spread
from .. import planner_geo as geo, step_pool, step_request

# `trailview_format` of the ground archive, which the readers check.
FORMAT = 3
MATERIAL_ZOOM = 13
LITHOLOGY_ZOOM = 12
REGION_ZOOM = 10
# The material byte: the code in the low 6 bits, then the barren bit; NO_DATA alone is outside the bake.
BARREN, NO_DATA = 0x40, 0x80
# A region byte with no region: no ecoregion while the walk runs, and no data in a tile.
REGION_NO_DATA = 255
# Ground tiles per batch of the workers.
BATCH = 256
# Barren by the year's highest NDVI: softly from 0.35 down to 0.15, as the prototype shader's ramp; the
# barren bit below the middle.
BARREN_RAMP = (0.15, 0.35)
BARREN_NDVI = sum(BARREN_RAMP) / 2
# A texel outside every ecoregion takes the nearest within this reach, as a cliff top that the
# simplified coast leaves in the sea; the search runs at FALLBACK_ZOOM, 0.4 km texels at 47° N.
REACH_M = 25_000.0
FALLBACK_ZOOM = 7
# A texel outside every place takes the nearest within this reach, as a house on a coast that the
# simplified border leaves in the sea; the search stays in the tile.
PLACE_REACH_M = 5_000.0
PLACE_NO_DATA = 0xFFFF


def barren_share(most):
    """The barren share × 255 of the year's highest NDVI codes; 0 where no quarter looked."""
    from . import colour

    share = np.where(most > 0, 1 - colour.smoothstep(*BARREN_RAMP, colour.ndvi(most)), 0)
    return np.round(255 * share).astype(np.uint8)


def material_parent(children):
    """A coarser material tile from its four children {(dx, dy): texels or None}: the most common byte,
    as `tiles.mode`, and the mean barren share of the children with data."""
    mosaic = np.zeros((2 * tiles.TILE, 2 * tiles.TILE, 2), np.uint8)
    mosaic[..., 0] = NO_DATA
    for (dx, dy), child in children.items():
        if child is not None:
            mosaic[dy * tiles.TILE:(dy + 1) * tiles.TILE, dx * tiles.TILE:(dx + 1) * tiles.TILE] = child
    four = lambda a: np.stack([a[0::2, 0::2], a[0::2, 1::2], a[1::2, 0::2], a[1::2, 1::2]])
    data, share = four(mosaic[..., 0] != NO_DATA), four(mosaic[..., 1]).astype(np.float32)
    mean = np.round((share * data).sum(axis=0) / np.maximum(data.sum(axis=0), 1)).astype(np.uint8)
    byte = tiles.mode({at: None if child is None else child[..., 0] for at, child in children.items()}, NO_DATA)
    return np.stack([byte, mean], axis=-1)


class Material:
    """The material chain of `sources.chain` on one tile, then the barren bit. A texel is the material
    byte and the barren share. `rock(z, x, y)` gives the lithology under the tile."""

    def __init__(self, bounds, steps, mosaics, rock):
        self.bounds, self.steps, self.mosaics, self.rock = bounds, steps, mosaics, rock

    def __call__(self, z, x, y):
        from . import colour

        transform, texel = tiles.transform(z, x, y), tiles.texel_m(z, y)
        quarters = colour.quarters(self.mosaics, transform, texel)
        most = np.max([texels[..., 3] for texels in quarters.values()], axis=0)
        tile = Tile(transform, texel, quarters, most, self.rock(z, x, y))
        material = np.zeros((tiles.TILE, tiles.TILE), np.uint8)
        for step in self.steps:
            material = step(material, tile)
        barren = (most > colour.SNOW_MARK) & (colour.ndvi(most) < BARREN_NDVI)
        material = np.where(barren, material | BARREN, material).astype(np.uint8)
        share, outside = barren_share(most), ~tiles.inside(self.bounds, z, x, y)
        material[outside], share[outside] = NO_DATA, 0
        return np.stack([material, share], axis=-1)


class Lithology:
    """The lithology chain of `sources.chain` on one tile."""

    def __init__(self, steps):
        self.steps = steps

    def __call__(self, z, x, y):
        tile = Tile(tiles.transform(z, x, y), tiles.texel_m(z, y), None, None, None)
        out = np.zeros((tiles.TILE, tiles.TILE), np.uint8)
        for step in self.steps:
            out = step(out, tile)
        return out


class Coarse:
    """The first byte of an archive under each texel of a tile of its zoom or finer, 0 where it has no
    tile."""

    def __init__(self, path):
        from pmtiles.reader import MmapSource, Reader

        self.stream = open(path, "rb")
        self.reader = Reader(MmapSource(self.stream))
        self.top = self.reader.header()["max_zoom"]

    def __call__(self, z, x, y):
        zoom = min(z, self.top)
        body = self.reader.get(zoom, x >> (z - zoom), y >> (z - zoom))
        return np.zeros((tiles.TILE, tiles.TILE), np.uint8) if body is None else under(pixels(body)[..., 0], z - zoom, x, y)


def area_of(where):
    if not isinstance(where[0], list):
        w, s, e, n = where
        return (e - w) * (n - s)
    return abs(sum(x0 * y1 - x1 * y0 for (x0, y0), (x1, y1) in zip(where, where[1:] + where[:1]))) / 2


def ring(where):
    if isinstance(where[0], list):
        return where
    w, s, e, n = where
    return [[w, s], [e, s], [e, n], [w, n]]


def shape(rings):
    """A GeoJSON polygon of [lon, lat] rings in EPSG:3857 metres."""
    return {"type": "Polygon", "coordinates": [np.stack(mercator_m(*np.array(r, float).T), axis=-1).tolist() for r in rings]}


class Region:
    """The region of each texel: the ecoregion that holds it, or the nearest within REACH_M, else
    `world`; from there it steps into the subregion whose `where` holds it, the smallest on a tie,
    until none does. Then it takes the id of the region and the texel's subdivision, else its
    country, else the region's own. Ids are indexes into the `ids` of `regions.json`."""

    def __init__(self, regions, bounds):
        from rasterio.features import rasterize

        self.ids = {name: i for i, (name, place) in enumerate(regions["ids"]) if place == "*"}
        self.placed = {}
        for i, (name, place) in enumerate(regions["ids"]):
            if place != "*":
                self.placed.setdefault(self.ids[name], []).append((place, i))
        self.subregions = {}
        for name, region in regions["regions"].items():
            for where in region.get("where", []):
                self.subregions.setdefault(region.get("extends"), []).append((area_of(where), name, where))
        # Countries first, so that a subdivision burns over its country; each with its box.
        self.places = [(place["iso"], place["rings"], (*np.min([np.min(r, axis=0) for r in place["rings"]], axis=0),
                                                       *np.max([np.max(r, axis=0) for r in place["rings"]], axis=0)))
                       for place in sorted(regions["places"], key=lambda place: "-" in place["iso"])]
        self.ecoregions = [(self.ids[e["short"]], polygon) for e in regions["ecoregions"] for polygon in e["polygons"]]
        # The fallback over the bounds and REACH_M around them, on whole tiles of FALLBACK_ZOOM.
        lat = max(abs(bounds[1]), abs(bounds[3]))
        pad = math.degrees(REACH_M / 6371008.8)
        wide = [bounds[0] - pad / math.cos(math.radians(lat)), bounds[1] - pad, bounds[2] + pad / math.cos(math.radians(lat)), bounds[3] + pad]
        cover = tiles.covered(wide, FALLBACK_ZOOM)
        (x0, y0), (x1, y1) = min(cover), max(cover)
        self.origin = (x0 * tiles.TILE, y0 * tiles.TILE)
        size = ((y1 - y0 + 1) * tiles.TILE, (x1 - x0 + 1) * tiles.TILE)
        index = self.ecoregion(rasterize, tiles.transform(FALLBACK_ZOOM, x0, y0), size)
        steps = round(REACH_M / tiles.texel_m(FALLBACK_ZOOM, (y0 + y1) // 2))
        self.fallback = spread(index, index == REGION_NO_DATA, self.ids["world"], steps=steps)

    def ecoregion(self, rasterize, transform, size):
        """The id of the ecoregion's region over each texel, REGION_NO_DATA under none."""
        shapes = [(shape(polygon), code) for code, polygon in self.ecoregions]
        return (rasterize(shapes, out_shape=size, transform=transform, fill=REGION_NO_DATA, dtype=np.uint8) if shapes
                else np.full(size, REGION_NO_DATA, np.uint8))

    def __call__(self, z, x, y):
        from rasterio.features import rasterize

        transform = tiles.transform(z, x, y)
        index = self.ecoregion(rasterize, transform, (tiles.TILE, tiles.TILE))
        gaps = index == REGION_NO_DATA
        if gaps.any():
            scale = 1 << (z - FALLBACK_ZOOM)
            rows = (y * tiles.TILE + np.arange(tiles.TILE)) // scale - self.origin[1]
            cols = (x * tiles.TILE + np.arange(tiles.TILE)) // scale - self.origin[0]
            index[gaps] = self.fallback[rows[:, None], cols[None, :]][gaps]
        names = {i: name for name, i in self.ids.items()}
        queue = [int(code) for code in np.unique(index)]
        while queue:
            code = queue.pop(0)
            held = index == code
            # The smallest burnt last, so it wins a tie.
            for _, name, where in sorted(self.subregions.get(names[code], []), reverse=True):
                inside = rasterize([(shape([ring(where)]), 1)], out_shape=index.shape, transform=transform, dtype=np.uint8).astype(bool) & held
                if inside.any():
                    index[inside] = self.ids[name]
                    if self.ids[name] not in queue:
                        queue.append(self.ids[name])
        if self.placed.keys() & set(np.unique(index).tolist()):
            index = self.placed_ids(rasterize, transform, (z, x, y), index)
        return index

    def placed_ids(self, rasterize, transform, tile, index):
        """Each texel of a region with places takes the id of its subdivision, else of its country."""
        w, s, e, n = geo.tile_bounds(*tile)
        pad = 2 * math.degrees(PLACE_REACH_M / 6371008.8) / math.cos(math.radians(max(abs(s), abs(n))))
        shapes = [(shape(rings), code) for code, (_, rings, (x0, y0, x1, y1)) in enumerate(self.places)
                  if x0 < e + pad and x1 > w - pad and y0 < n + pad and y1 > s - pad]
        at = (rasterize(shapes, out_shape=index.shape, transform=transform, fill=PLACE_NO_DATA, dtype=np.uint16) if shapes
              else np.full(index.shape, PLACE_NO_DATA, np.uint16))
        at = spread(at, at == PLACE_NO_DATA, PLACE_NO_DATA, steps=round(PLACE_REACH_M / tiles.texel_m(tile[0], tile[2])))
        iso = np.array([place for place, _, _ in self.places] + [""])
        names = iso[np.minimum(at, len(self.places))]
        countries = np.char.partition(names, "-")[..., 0]
        out = index.copy()
        for code, places in self.placed.items():
            held = index == code
            # Countries first, so that a subdivision wins over its country.
            for place, i in sorted(places, key=lambda entry: "-" in entry[0]):
                out[held & ((names == place) if "-" in place else (countries == place))] = i
        return out


def step():
    request = step_request.read()
    options, layers = request["options"], request["layers"]
    bounds, layer, output = options["bounds"], options["layer"], Path(request["output"])
    if layer == "ground":
        archives = [Path(layers[f"planner/trailview-{name}"][f"{name}.pmtiles"]) for name in ("material", "lithology", "region")]
        metadata = {"trailview_format": FORMAT, "archive": "ground", "attribution": options["attribution"]}
        sizes = join(*archives, output, metadata)
        step_request.metrics(request, {"bytes_per_zoom": sizes, "bytes_per_km2": round(sum(sizes.values()) / tiles.km2(bounds), 1)})
        return
    rules = layers["planner/trailview-rules"]
    if layer == "region":
        bake, top = Region(json.loads(Path(rules["regions.json"]).read_bytes()), bounds), REGION_ZOOM
    else:
        vocabulary = sources.Vocabulary(json.loads(Path(rules["materials.json"]).read_bytes()))
        ground = json.loads(Path(rules["ground.json"]).read_bytes())
        files = lambda source: step_request.files(request, source) if source in request["snapshots"] else None
        if layer == "lithology":
            bake, top = Lithology(sources.chain("lithology", ground, vocabulary, files, bounds)), LITHOLOGY_ZOOM
        else:
            from . import colour

            steps = sources.chain("material", ground, vocabulary, files, bounds, layers["planner/osm"]["osm.pbf"])
            rock = Coarse(layers["planner/trailview-lithology"]["lithology.pmtiles"])
            bake, top = Material(bounds, steps, colour.mosaics(files("sentinel-2-mosaics")), rock), MATERIAL_ZOOM
    bake_layer(layer, bake, top, bounds, output, {"layer": layer})


def bake_layer(layer, bake, top, bounds, output, metadata):
    """Write `<layer>.pmtiles` from zoom 0 to `top`; return its bytes per zoom. A tile with no data is
    left out. A material texel has the barren share in green."""
    no_data = {"material": NO_DATA, "region": REGION_NO_DATA}.get(layer)
    parent = material_parent if layer == "material" else lambda children: tiles.mode(children, no_data)

    def encode(texels):
        byte = texels[..., 0] if layer == "material" else texels
        return {} if no_data is not None and (byte == no_data).all() else {layer: tiles.lossless(texels)}

    sizes = tiles.pyramid(bounds, range(top + 1), bake, parent, encode, output, metadata)
    return sizes.get(layer, {})


def pixels(body):
    """The texels of a lossless WebP tile: (TILE, TILE, channels)."""
    from PIL import Image

    return np.asarray(Image.open(io.BytesIO(body)).convert("RGBA"))


def under(byte, levels, x, y):
    """The texels of tile x/y of a zoom `levels` finer than the tile `byte` that holds it: each takes
    the texel under its centre."""
    size, mask = tiles.TILE >> levels, (1 << levels) - 1
    block = byte[(y & mask) * size:((y & mask) + 1) * size, (x & mask) * size:((x & mask) + 1) * size]
    return block.repeat(1 << levels, axis=0).repeat(1 << levels, axis=1)


def ground_tile(entry):
    """The ground tile of one material tile: the material texels with the lithology and the region of
    their tiles, each (zoom, body or None, the byte of no tile)."""
    (z, x, y), body, planes = entry
    texels = pixels(body).copy()
    for channel, (zoom, coarse, empty) in zip((2, 3), planes):
        texels[..., channel] = empty if coarse is None else under(pixels(coarse)[..., 0], z - zoom, x, y)
    return tiles.lossless(texels)[1]


def join(material, lithology, region, output, metadata, workers=None):
    """Write `output/ground.pmtiles`: each tile of the material archive, with the lithology in blue and
    the region in alpha from their tile of the same zoom, or of their last zoom under a finer tile.
    Return its bytes per zoom."""
    from pmtiles.reader import MmapSource, Reader, all_tiles
    from pmtiles.tile import Compression, TileType, zxy_to_tileid
    from pmtiles.writer import Writer

    output.mkdir(parents=True, exist_ok=True)
    with material.open("rb") as land, lithology.open("rb") as rock, region.open("rb") as area:
        source, coarse = MmapSource(land), [(Reader(MmapSource(rock)), 0), (Reader(MmapSource(area)), REGION_NO_DATA)]
        header = Reader(source).header()
        tops = [reader.header()["max_zoom"] for reader, _ in coarse]

        def entry(zxy, body):
            z, x, y = zxy
            zooms = [min(z, top) for top in tops]
            return zxy, body, [(zoom, reader.get(zoom, x >> (z - zoom), y >> (z - zoom)), empty)
                               for zoom, (reader, empty) in zip(zooms, coarse)]

        sizes = {}
        with tempfile.NamedTemporaryFile(dir=output, prefix=".ground-", delete=False) as stream:
            try:
                writer, entries = Writer(stream), (entry(zxy, body) for zxy, body in all_tiles(source))
                pool = step_pool.pool(workers)
                try:
                    # Batches keep the archive out of memory, and `map` keeps the tile order.
                    while batch := list(itertools.islice(entries, BATCH)):
                        for (zxy, _, _), data in zip(batch, pool.map(ground_tile, batch)):
                            writer.write_tile(zxy_to_tileid(*zxy), data)
                            sizes[zxy[0]] = sizes.get(zxy[0], 0) + len(data)
                finally:
                    pool.shutdown(cancel_futures=True)
                keys = ("min_lon_e7", "min_lat_e7", "max_lon_e7", "max_lat_e7", "center_zoom", "center_lon_e7", "center_lat_e7")
                writer.finalize({"tile_type": TileType.WEBP, "tile_compression": Compression.NONE, **{key: header[key] for key in keys}}, metadata)
                stream.close()
                os.replace(stream.name, output / "ground.pmtiles")
            except BaseException:
                os.unlink(stream.name)
                raise
    return sizes


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--bounds", type=geo.bounds, required=True)
    parser.add_argument("--source", required=True, help="the id of the ground source")
    parser.add_argument("--fetch", type=Path, required=True, help="write the capture of the source here")
    parser.add_argument("--archive", type=Path, help="the zip in the archive of an archive-features source")
    parser.add_argument("--work", type=Path, help="unpack the zip here")
    args = parser.parse_args()
    sources.fetch(args.source, args.bounds, args.fetch, MATERIAL_ZOOM, archive=args.archive, work=args.work)


if __name__ == "__main__":
    step() if sys.argv[1:] == ["--step"] else main()
