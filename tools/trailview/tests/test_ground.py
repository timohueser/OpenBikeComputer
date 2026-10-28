"""The ground tiles hold the bytes of specs/trailview-tiles.md: the archive, the material chain of
ground.json on a georeferenced tile and the region walk."""

import io
import json
import math
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import numpy as np
import rasterio
import shapely
from affine import Affine
from PIL import Image
from pmtiles.reader import MmapSource, Reader
from pyproj import Transformer

from tools import planner_geo as geo
from tools.trailview import colour, ground, sources, tiles

VOCABULARY = sources.Vocabulary(json.loads(Path("data/trailview/materials.json").read_text()))
GROUND = json.loads(Path("data/trailview/ground.json").read_text())
M = VOCABULARY.material
# A tile of zoom 13 over Engelberg, and one of zoom 10 around it.
Z, X, Y = 13, *map(int, geo.mercator(8.40, 46.82, 13))
X10, Y10 = map(int, geo.mercator(8.40, 46.82, 10))


def box_m(west, south, east, north):
    (x0, x1), (y0, y1) = ground.mercator_m([west, east], [south, north])
    return shapely.box(x0, y0, x1, y1)


class ArchiveTest(unittest.TestCase):
    def test_a_parent_takes_the_most_common_child_and_no_data_only_where_all_are(self):
        child = lambda *four: np.tile(np.array(four, np.uint8).reshape(2, 2), (tiles.TILE // 2, tiles.TILE // 2))
        parent = tiles.mode({(0, 0): child(3, 3, 7, 5), (1, 0): child(7, 5, 9, 2), (0, 1): child(1, 1, 1, 1), (1, 1): None}, 128)
        self.assertEqual(parent[0, 0], 3)
        self.assertEqual(parent[0, -1], 7, "a tie goes to the first child in reading order")
        self.assertEqual(parent[-1, 0], 1)
        self.assertEqual(parent[-1, -1], 128, "a missing child is no data")
        self.assertEqual(tiles.mode({(0, 0): child(128, 128, 128, 4)}, 128)[0, 0], 4)

    def test_a_coarser_material_texel_takes_the_mean_barren_share_of_its_children_with_data(self):
        child = lambda byte, *four: np.stack([np.tile(np.array(byte, np.uint8).reshape(2, 2), (tiles.TILE // 2, tiles.TILE // 2)),
                                              np.tile(np.array(four, np.uint8).reshape(2, 2), (tiles.TILE // 2, tiles.TILE // 2))], axis=-1)
        parent = ground.material_parent({(0, 0): child([5, 5, 5, ground.NO_DATA], 0, 255, 255, 0), (1, 0): None})
        self.assertEqual(parent[0, 0].tolist(), [5, 170])
        self.assertEqual(parent[0, -1].tolist(), [ground.NO_DATA, 0], "a missing child is no data")

    def test_the_archive_holds_each_byte_in_red_and_the_barren_share_in_green_and_leaves_out_tiles_without_data_and_a_coarser_region_tile_has_no_data_where_its_children_are_missing(self):
        west, south, east, north = geo.tile_bounds(Z, X, Y)
        bounds = [west, south, west + (east - west) / 3, north]
        byte = lambda z, x, y: np.where(tiles.inside(bounds, z, x, y), np.arange(tiles.TILE, dtype=np.uint8)[None, :] % 64, 128).astype(np.uint8)
        share = np.arange(tiles.TILE, dtype=np.uint8)[:, None].repeat(tiles.TILE, axis=1)
        bake = lambda z, x, y: np.stack([byte(z, x, y), share], axis=-1)
        with tempfile.TemporaryDirectory() as tmp:
            sizes = ground.bake_layer("material", bake, Z, bounds, Path(tmp), {"trailview_format": ground.FORMAT})
            self.assertEqual(sorted(sizes), list(range(Z + 1)))
            with open(Path(tmp) / "material.pmtiles", "rb") as stream:
                reader = Reader(MmapSource(stream))
                rgb = np.asarray(Image.open(io.BytesIO(reader.get(Z, X, Y))).convert("RGB"))
                self.assertEqual(reader.metadata()["archive"], "material")
                self.assertIsNone(reader.get(Z, X + 1, Y))
        self.assertTrue((rgb[..., :2] == bake(Z, X, Y)).all())
        self.assertTrue((rgb[..., 2] == 0).all())

        west, south, east, north = geo.tile_bounds(10, X10, Y10)
        inner = [west + 1e-6, south + 1e-6, east - 1e-6, north - 1e-6]
        with tempfile.TemporaryDirectory() as tmp:
            ground.bake_layer("region", lambda z, x, y: np.full((tiles.TILE, tiles.TILE), 3, np.uint8), 10, inner, Path(tmp), {})
            with open(Path(tmp) / "region.pmtiles", "rb") as stream:
                parent = np.asarray(Image.open(io.BytesIO(Reader(MmapSource(stream)).get(9, X10 // 2, Y10 // 2))).convert("RGB"))[..., 0]
        mine = (slice((Y10 % 2) * 256, (Y10 % 2) * 256 + 256), slice((X10 % 2) * 256, (X10 % 2) * 256 + 256))
        self.assertTrue((parent[mine] == 3).all())
        self.assertEqual(sorted(np.unique(parent).tolist()), [3, ground.REGION_NO_DATA])

    def test_the_ground_tile_holds_the_lithology_and_the_region_of_the_texel_under_each_centre(self):
        from pmtiles.tile import Compression, TileType, zxy_to_tileid
        from pmtiles.writer import Writer

        def archive(path, found):
            with path.open("wb") as stream:
                writer = Writer(stream)
                for (z, x, y), texels in sorted(found.items(), key=lambda item: zxy_to_tileid(*item[0])):
                    writer.write_tile(zxy_to_tileid(z, x, y), tiles.lossless(texels)[1])
                writer.finalize({"tile_type": TileType.WEBP, "tile_compression": Compression.NONE}, {})
            return path

        ramp = (np.arange(tiles.TILE)[:, None] // 8 + np.arange(tiles.TILE)[None, :] // 8).astype(np.uint8)
        share, ids = np.full((tiles.TILE, tiles.TILE), 9, np.uint8), ramp % 7
        cells = np.stack([ramp, share], axis=-1)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            material = archive(root / "material.pmtiles", {(10, X10, Y10): cells, (10, X10, Y10 + 1): cells, (Z, X, Y): cells})
            lithology = archive(root / "lithology.pmtiles", {(10, X10, Y10): ramp})
            region = archive(root / "region.pmtiles", {(10, X10, Y10): ids})
            sizes = ground.join(material, lithology, region, root, {"trailview_format": ground.FORMAT})
            self.assertEqual(sorted(sizes), [10, Z])
            with open(root / "ground.pmtiles", "rb") as stream:
                reader = Reader(MmapSource(stream))
                same, alone, finer = (np.asarray(Image.open(io.BytesIO(reader.get(*tile))).convert("RGBA"))
                                      for tile in ((10, X10, Y10), (10, X10, Y10 + 1), (Z, X, Y)))
        self.assertTrue((same[..., 0] == ramp).all() and (same[..., 1] == 9).all())
        self.assertTrue((same[..., 2] == ramp).all() and (same[..., 3] == ids).all())
        self.assertTrue((alone[..., 2] == 0).all() and (alone[..., 3] == ground.REGION_NO_DATA).all(), "no tile of zoom 10 there")
        k = Z - 10
        size, row, col = tiles.TILE >> k, (Y % (1 << k)) * (tiles.TILE >> k), (X % (1 << k)) * (tiles.TILE >> k)
        under = lambda values: values[row:row + size, col:col + size].repeat(1 << k, 0).repeat(1 << k, 1)
        self.assertTrue((finer[..., 2] == under(ramp)).all() and (finer[..., 3] == under(ids)).all())


def raster(path, bounds, values, dtype="int16", nodata=-32768):
    """A GeoTIFF in EPSG:4326 over `bounds` at 1/12000°, from values(rows, cols) of its pixel centres."""
    west, south, east, north = bounds
    px = 1 / 12000
    width, height = math.ceil((east - west) / px), math.ceil((north - south) / px)
    lon = west + (np.arange(width) + 0.5) * px
    lat = north - (np.arange(height) + 0.5) * px
    path.parent.mkdir(parents=True, exist_ok=True)
    with rasterio.open(path, "w", driver="GTiff", width=width, height=height, count=1, dtype=dtype, nodata=nodata,
                       crs="EPSG:4326", transform=Affine(px, 0, west, 0, -px, north)) as dst:
        dst.write(values(lat[:, None], lon[None, :]).astype(dtype), 1)
    return path


def osm_file(path, areas):
    """An OSM file of closed ways, each [(lon, lat)] corners with its tags, named as a store object,
    with no extension that names its format."""
    import osmium

    written = path.with_name("areas.osm.pbf")
    with osmium.SimpleWriter(str(written)) as writer:
        node = 0
        for way, (corners, tags) in enumerate(areas, 1):
            ids = []
            for location in corners:
                node += 1
                writer.add_node(osmium.osm.mutable.Node(id=node, location=location))
                ids.append(node)
            writer.add_way(osmium.osm.mutable.Way(id=way, nodes=ids + ids[:1], tags=tags))
    return written.rename(path)


def corine(path, bounds, classes):
    """A CORINE raster over the bounds as upstream has it: the grid codes of classes(x, y) at the pixel
    centres in EPSG:3035 at 100 m, int8 with no data -128."""
    grid = {clc: int(value) for value, clc in next(entry for entry in GROUND["material"]["sources"]
                                                   if entry["source"] == "corine-land-cover")["values"].items()}
    x0, y0, x1, y1 = Transformer.from_crs("EPSG:4326", "EPSG:3035", always_xy=True).transform_bounds(*bounds)
    x0, y0, x1, y1 = math.floor(x0 / 100) * 100, math.floor(y0 / 100) * 100, math.ceil(x1 / 100) * 100, math.ceil(y1 / 100) * 100
    x = x0 + (np.arange(round((x1 - x0) / 100)) + 0.5) * 100
    y = y1 - (np.arange(round((y1 - y0) / 100)) + 0.5) * 100
    codes = np.vectorize(lambda clc: grid[str(clc)])(np.broadcast_to(classes(x[None, :], y[:, None]), (len(y), len(x))))
    with rasterio.open(path, "w", driver="GTiff", width=len(x), height=len(y), count=1, dtype="int8", nodata=-128,
                       crs="EPSG:3035", transform=Affine(100, 0, x0, 0, -100, y1)) as dst:
        dst.write(codes.astype(np.int8), 1)
    return {path.name: path}


class MaterialTest(unittest.TestCase):
    def test_each_source_of_the_chain_refines_only_what_it_names_more_precisely_and_outside_is_no_data(self):
        west, south, east, north = geo.tile_bounds(Z, X, Y)
        mid_lon, mid_lat = (west + east) / 2, (south + north) / 2
        around = [west - 0.01, south - 0.01, east + 0.01, north + 0.01]
        north_half, east_half = lambda lat, lon: lat > mid_lat, lambda lat, lon: lon > mid_lon
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            # WorldCover: forest in the west half, grassland in the east half.
            worldcover = raster(root / "worldcover.tif", around, lambda lat, lon: np.where(east_half(lat, lon), 30, 10), "uint8", 0)
            # Sentinel-2, the same in each quarter: forest in the west; in the north-east as red as green
            # with an NDVI of 0.6, which is heath; in the south-east bare, with an NDVI of 0.05.
            band = {"B02": (300, 400, 1400), "B03": (500, 650, 1500), "B04": (300, 600, 1500), "B08": (3000, 2500, 1650),
                    "observations": (10, 10, 10)}
            mosaics = {}
            for q in range(1, 5):
                for name, (forest, heath, bare) in band.items():
                    item = f"Sentinel-2_mosaic_2025_Q{q}_TEST_0_0"
                    mosaics[f"{item}/{name}.tif"] = raster(root / item / f"{name}.tif", around, lambda lat, lon: np.where(
                        east_half(lat, lon), np.where(north_half(lat, lon), heath, bare), forest))
            # OSM: a needleleaved forest over the north half and natural grassland over the south-east.
            box = lambda w, s, e, n: [(w, s), (e, s), (e, n), (w, n)]
            osm = osm_file(root / "7b0bf1524a29", [(box(west - 0.01, mid_lat, east + 0.01, north + 0.01), {"landuse": "forest", "leaf_type": "needleleaved"}),
                                                    (box(mid_lon, south - 0.01, east + 0.01, mid_lat), {"natural": "grassland"})])
            # CORINE: natural grassland in the north, heath in the south, at 100 m.
            mid_y = Transformer.from_crs("EPSG:4326", "EPSG:3035", always_xy=True).transform(mid_lon, mid_lat)[1]
            clc = corine(root / "corine.tif", around, lambda x, y: np.where(y > mid_y, 321, 322))
            files = {"esa-worldcover": {"worldcover.tif": worldcover}, "sentinel-2-mosaics": mosaics, "corine-land-cover": clc}
            bounds = [west, south, west + 0.9 * (east - west), north]
            steps = sources.chain("material", GROUND, VOCABULARY, files.get, bounds, osm)
            no_rock = lambda z, x, y: np.zeros((tiles.TILE, tiles.TILE), np.uint8)
            texels = ground.Material(bounds, steps, colour.mosaics(mosaics), no_rock)(Z, X, Y)
        n, w, s, e = 100, 100, 412, 300
        self.assertEqual(texels[n, w].tolist(), [M["needleleaf-forest"], 0])
        self.assertEqual(texels[s, w, 0], M["forest"], "heath does not name a forest more precisely")
        self.assertEqual(texels[n, e, 0], M["heath"], "the satellite's heath; natural grassland refines only grassland")
        self.assertEqual(texels[s, e].tolist(), [M["rough-grass"] | ground.BARREN, 255])
        self.assertEqual(texels[s, -1].tolist(), [ground.NO_DATA, 0])


class RegionTest(unittest.TestCase):
    def test_a_texel_takes_its_ecoregion_or_the_nearest_within_reach_then_its_subregion(self):
        z, x, y = 10, *map(int, geo.mercator(8.40, 46.82, 10))
        west, south, east, north = geo.tile_bounds(z, x, y)
        km = 1 / (111.32 * math.cos(math.radians((south + north) / 2)))
        # The ecoregion ends 15 km west of the tile; the subregion is a box 1 to 4 km into the tile.
        edge = west - 15 * km
        regions = {
            "ids": [["alps", "*"], ["alps.sub", "*"], ["world", "*"]],
            "regions": {"alps": {"extends": "world"}, "alps.sub": {"extends": "alps", "where": [[west + 1 * km, south, west + 4 * km, north]]}, "world": {}},
            "ecoregions": [{"short": "alps", "polygons": [[[[edge - 1, south - 1], [edge, south - 1], [edge, north + 1], [edge - 1, north + 1]]]]}],
            "places": [],
        }
        region = ground.Region(regions, [west, south, east, north])(z, x, y)
        column = lambda lon: int((lon - west) / (east - west) * tiles.TILE)
        self.assertEqual(region[256, column(west + 2 * km)], 1)
        self.assertEqual(region[256, column(west + 6 * km)], 0, "21 km from the ecoregion")
        self.assertEqual(region[256, column(west + 15 * km)], 2, "30 km from the ecoregion")

    def test_a_texel_takes_the_id_of_its_subdivision_else_of_its_country_else_of_its_region(self):
        z, x, y = 10, *map(int, geo.mercator(8.40, 46.82, 10))
        west, south, east, north = geo.tile_bounds(z, x, y)
        km = 1 / (111.32 * math.cos(math.radians((south + north) / 2)))
        box = lambda w, e: [[[w, south - 1], [e, south - 1], [e, north + 1], [w, north + 1], [w, south - 1]]]
        regions = {
            "ids": [["alps", "*"], ["alps", "ch"], ["alps", "ch-ti"], ["world", "*"]],
            "regions": {"alps": {"extends": "world"}, "world": {}},
            "ecoregions": [{"short": "alps", "polygons": [box(west - 1, east + 1)]}],
            # A subdivision before its country, as countries.json does not order them.
            "places": [{"iso": "ch-ti", "rings": box(west + 4 * km, west + 8 * km)}, {"iso": "ch", "rings": box(west - 1, west + 10 * km)},
                       {"iso": "fr", "rings": box(west + 20 * km, east + 1)}],
        }
        region = ground.Region(regions, [west, south, east, north])(z, x, y)
        column = lambda lon: int((lon - west) / (east - west) * tiles.TILE)
        self.assertEqual(region[256, column(west + 2 * km)], 1)
        self.assertEqual(region[256, column(west + 7 * km)], 2)
        self.assertEqual(region[256, column(west + 12 * km)], 1, "outside every place, nearest to Switzerland")
        self.assertEqual(region[256, column(west + 25 * km)], 0, "France, which the region does not name")


if __name__ == "__main__":
    unittest.main()
