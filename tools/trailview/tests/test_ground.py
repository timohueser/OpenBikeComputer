"""The ground tiles hold the bytes of specs/trailview-tiles.md: the archive, the material chain on a
georeferenced tile and the region walk."""

import io
import json
import math
import tempfile
import unittest
from pathlib import Path

import numpy as np
import rasterio
import shapely
from affine import Affine
from PIL import Image
from pmtiles.reader import MmapSource, Reader
from pyproj import Transformer

from tools import planner_geo as geo
from tools.trailview import ground, tiles

VOCABULARY = ground.Vocabulary(json.loads(Path("data/trailview/materials.json").read_text()))
M = VOCABULARY.material
# A tile of zoom 13 over Engelberg, and one of zoom 10 around it.
Z, X, Y = 13, *map(int, geo.mercator(8.40, 46.82, 13))


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

    def test_the_archive_holds_each_byte_in_red_and_leaves_out_tiles_without_data(self):
        west, south, east, north = geo.tile_bounds(Z, X, Y)
        bounds = [west, south, west + (east - west) / 3, north]
        bake = lambda z, x, y: np.where(tiles.inside(bounds, z, x, y), np.arange(tiles.TILE, dtype=np.uint8)[None, :] % 64, 128).astype(np.uint8)
        with tempfile.TemporaryDirectory() as tmp:
            sizes = ground.bake_layer("material", bake, Z, bounds, Path(tmp), {"trailview_format": tiles.FORMAT})
            self.assertEqual(sorted(sizes), list(range(Z + 1)))
            with open(Path(tmp) / "material.pmtiles", "rb") as stream:
                reader = Reader(MmapSource(stream))
                rgb = np.asarray(Image.open(io.BytesIO(reader.get(Z, X, Y))).convert("RGB"))
                self.assertEqual(reader.metadata()["archive"], "material")
                self.assertIsNone(reader.get(Z, X + 1, Y))
        self.assertTrue((rgb[..., 0] == bake(Z, X, Y)).all())
        self.assertTrue((rgb[..., 0] == rgb[..., 2]).all())


class MaterialTest(unittest.TestCase):
    def test_each_source_refines_only_what_it_names_more_precisely_and_outside_is_no_data(self):
        west, south, east, north = geo.tile_bounds(Z, X, Y)
        mid_lon, mid_lat = (west + east) / 2, (south + north) / 2
        with tempfile.TemporaryDirectory() as tmp:
            # WorldCover: forest in the west half, grassland in the east half.
            path = Path(tmp) / "worldcover.tif"
            px = 1 / 12000
            width, height = math.ceil((east - west) / px) + 2, math.ceil((north - south) / px) + 2
            classes = np.full((height, width), 30, np.uint8)
            classes[:, : width // 2] = 10
            with rasterio.open(path, "w", driver="GTiff", width=width, height=height, count=1, dtype="uint8", nodata=0,
                               crs="EPSG:4326", transform=Affine(px, 0, west - px, 0, -px, north + px)) as dst:
                dst.write(classes, 1)
            # OSM: a needleleaved forest over the north half. CORINE: natural grassland in the north,
            # heath in the south, at 100 m.
            forests = [(box_m(west - 1, mid_lat, east + 1, north + 1), M["needleleaf-forest"])]
            to_laea = Transformer.from_crs("EPSG:4326", "EPSG:3035", always_xy=True)
            x0, y0, x1, y1 = to_laea.transform_bounds(west - 0.01, south - 0.01, east + 0.01, north + 0.01)
            corine = np.full((math.ceil((y1 - y0) / 100), math.ceil((x1 - x0) / 100)), M["heath"], np.uint8)
            corine[: round((y1 - to_laea.transform(mid_lon, mid_lat)[1]) / 100)] = M["rough-grass"]
            bounds = [west, south, west + 0.9 * (east - west), north]
            material = ground.Material(VOCABULARY, bounds, [path], [(corine, Affine(100, 0, x0, 0, -100, y1))], forests, [])
            texels = material(Z, X, Y)
        n, w, s, e = 100, 100, 412, 300
        self.assertEqual(texels[n, w], M["needleleaf-forest"])
        self.assertEqual(texels[s, w], M["forest"], "heath does not name a forest more precisely")
        self.assertEqual(texels[n, e], M["rough-grass"])
        self.assertEqual(texels[s, e], M["grassland"], "heath refines only open ground that WorldCover does not judge")
        self.assertEqual(texels[s, -1], ground.NO_DATA)


class RegionTest(unittest.TestCase):
    def test_a_texel_takes_its_ecoregion_or_the_nearest_within_reach_then_its_subregion(self):
        z, x, y = 10, *map(int, geo.mercator(8.40, 46.82, 10))
        west, south, east, north = geo.tile_bounds(z, x, y)
        km = 1 / (111.32 * math.cos(math.radians((south + north) / 2)))
        # The ecoregion ends 15 km west of the tile; the subregion is a box 1 to 4 km into the tile.
        edge = west - 15 * km
        regions = {
            "regions": [{"id": "alps", "extends": "world"},
                        {"id": "alps.sub", "extends": "alps", "where": [[west + 1 * km, south, west + 4 * km, north]]},
                        {"id": "world"}],
            "ecoregions": [{"short": "alps", "polygons": [[[[edge - 1, south - 1], [edge, south - 1], [edge, north + 1], [edge - 1, north + 1]]]]}],
        }
        region = ground.Region(regions, [west, south, east, north])(z, x, y)
        column = lambda lon: int((lon - west) / (east - west) * tiles.TILE)
        self.assertEqual(region[256, column(west + 2 * km)], 1)
        self.assertEqual(region[256, column(west + 6 * km)], 0, "21 km from the ecoregion")
        self.assertEqual(region[256, column(west + 15 * km)], 2, "30 km from the ecoregion")


if __name__ == "__main__":
    unittest.main()
