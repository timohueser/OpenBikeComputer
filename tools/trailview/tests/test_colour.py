"""The colour tiles keep the rules of the prototype's bake: no snow in a quarter's colour, summer chroma
in winter shadow, and the alpha codes of specs/trailview-tiles.md."""

import io
import unittest

import numpy as np
from PIL import Image

from tools.trailview import colour, tiles


def quarter(rgb, alpha, shape=(1, 1)):
    return np.broadcast_to(np.array([*rgb, alpha], np.uint8), (*shape, 4)).copy()


class ColourTest(unittest.TestCase):
    def test_a_snowy_texel_takes_the_previous_quarter_without_snow_and_the_snow_mark(self):
        green, brown = (60, 110, 40), (120, 100, 80)
        quarters = {1: quarter((250, 252, 255), 120), 2: quarter(brown, 160), 3: quarter(green, 220), 4: quarter((90, 80, 60), 140)}
        clean = colour.snow_free(quarters)
        self.assertEqual(clean[1][0, 0].tolist(), [90, 80, 60, colour.SNOW_MARK], "the fourth quarter is the one before the first")
        self.assertEqual(clean[3][0, 0].tolist(), [*green, 220])
        again = colour.snow_free(clean)
        self.assertTrue(all((again[q] == clean[q]).all() for q in clean))

    def test_a_dark_winter_texel_takes_the_summer_chroma_at_its_own_brightness(self):
        shape = (1, 1)
        seen = {q: (np.full((3, *shape), 0.1, np.float32), np.full(shape, 0.5, np.float32), np.ones(shape, bool)) for q in (2, 3, 4)}
        seen[3] = (np.array([0.04, 0.08, 0.02], np.float32).reshape(3, 1, 1), np.full(shape, 0.7, np.float32), np.ones(shape, bool))
        seen[1] = (np.full((3, *shape), 0.01, np.float32), np.full(shape, 0.3, np.float32), np.ones(shape, bool))
        texels = colour.texels(seen)
        summer = np.array([0.04, 0.08, 0.02])
        winter = (texels[1][0, 0, :3] / 255.0) ** 2.2
        np.testing.assert_allclose(winter, summer * 0.01 / summer.mean(), rtol=0.05)
        self.assertEqual(texels[3][0, 0, 3], 1 + round(1.7 / 2 * 254))

    def test_a_coarser_texel_is_snow_only_where_more_children_saw_snow_than_measured(self):
        children = {}
        for (dx, dy), alphas in {(0, 0): (100, 200, 1, 1), (1, 0): (1, 1, 1, 50), (0, 1): (0, 0, 0, 0)}.items():
            stack = np.zeros((4, tiles.TILE, tiles.TILE, 4), np.uint8)
            stack[..., :3] = 128
            stack[..., 0::2, 0::2, 3], stack[..., 0::2, 1::2, 3] = alphas[0], alphas[1]
            stack[..., 1::2, 0::2, 3], stack[..., 1::2, 1::2, 3] = alphas[2], alphas[3]
            children[dx, dy] = stack
        parent = colour.average(children)
        self.assertEqual(parent[0, 0, 0, 3], 150, "two measured against two with snow: the mean NDVI")
        self.assertEqual(parent[0, 0, tiles.TILE // 2, 3], colour.SNOW_MARK)
        self.assertEqual(parent[0, tiles.TILE // 2, 0, 3], 0)
        self.assertEqual(parent[0, 0, 0, :3].tolist(), [128, 128, 128])

    def test_a_coarser_texel_leaves_the_black_snow_filler_out_of_its_colour(self):
        # Each coarser texel has two glacier children, black as snow_free leaves them, and two of rock.
        texels = quarter((120, 115, 110), 141, (tiles.TILE, tiles.TILE))
        texels[:, 0::2] = [0, 0, 0, colour.SNOW_MARK]
        parent = colour.average({(0, 0): np.stack([texels] * 4)})
        self.assertEqual(parent[2, 0, 0].tolist(), [120, 115, 110, 141])

    def test_a_stored_ndvi_code_is_its_block_mean_in_steps_and_keeps_the_marks_and_the_sign(self):
        alpha = np.full((tiles.TILE, tiles.TILE), 200, np.uint8)
        alpha[0, :2], alpha[1, :2] = (100, 0), (110, colour.SNOW_MARK)
        alpha[2:4, 0:2], alpha[2:4, 2:4] = 127, 128
        stored = colour.stored_ndvi(alpha)
        self.assertEqual(stored[:2, :2].tolist(), [[106, 0], [106, colour.SNOW_MARK]], "the mean of 100 and 110 in steps of 4")
        self.assertLess(stored[2, 0], 128, "a negative NDVI stays negative")
        self.assertGreaterEqual(stored[2, 2], 128)
        self.assertEqual(stored[4, 4], 202)

    def test_a_stored_ndvi_code_stays_on_the_step_grid_at_its_edges(self):
        alpha = np.full((tiles.TILE, tiles.TILE), 255, np.uint8)
        alpha[0:2, 0:2], alpha[0:2, 2:4] = 2, 3
        stored = colour.stored_ndvi(alpha)
        self.assertEqual(stored[:2, :4].tolist(), [[2, 2, 2, 2]] * 2)
        self.assertEqual(stored[4, 4], 254)

    def test_a_colour_body_is_the_ndvi_tile_then_the_colour_and_gives_back_the_stored_ndvi(self):
        rng = np.random.default_rng(1)
        stack = np.zeros((4, tiles.TILE, tiles.TILE, 4), np.uint8)
        stack[..., :3] = (np.arange(tiles.TILE) // 2)[None, :, None, None]
        stack[..., 3] = rng.choice([0, 1, 2, 90, 255], (4, tiles.TILE, tiles.TILE))
        encoded = colour.encoder("webp-12")(stack)
        self.assertEqual(sorted(encoded), ["q1", "q2", "q3", "q4"])
        body = encoded["q3"][1]
        split = 8 + int.from_bytes(body[4:8], "little")
        ndvi = np.asarray(Image.open(io.BytesIO(body[:split])).convert("RGB"))
        rgb = Image.open(io.BytesIO(body[split:]))
        self.assertEqual((ndvi.shape, rgb.size, rgb.mode), ((tiles.TILE // 2, tiles.TILE // 2, 3), (tiles.TILE, tiles.TILE), "RGB"))
        shift = 2 * (2 * (np.arange(tiles.TILE)[:, None] % 2) + np.arange(tiles.TILE)[None, :] % 2)
        mark = ndvi[..., 1].repeat(2, axis=0).repeat(2, axis=1) >> shift & 3
        alpha = np.where(mark == colour.MEASURED, ndvi[..., 0].repeat(2, axis=0).repeat(2, axis=1), mark)
        self.assertTrue((alpha == colour.stored_ndvi(stack[2, ..., 3])).all())


class FetchTest(unittest.TestCase):
    def test_a_subset_is_the_window_of_the_bounds_one_pixel_wider(self):
        import tempfile
        from pathlib import Path

        import rasterio
        from affine import Affine

        values = np.arange(100 * 100, dtype=np.uint16).reshape(100, 100)
        with tempfile.TemporaryDirectory() as directory:
            source, out = Path(directory) / "day.tif", Path(directory) / "out"
            out.mkdir()
            profile = {"driver": "GTiff", "width": 100, "height": 100, "count": 1, "dtype": "uint16",
                       "crs": "EPSG:4326", "transform": Affine(0.01, 0, 7, 0, -0.01, 48)}
            with rasterio.open(source, "w", **profile) as dst:
                dst.write(values, 1)
            colour.subset(str(source), [7.2, 47.2, 7.4, 47.4], out)
            colour.subset(str(source), [9.0, 47.2, 9.4, 47.4], out)
            self.assertEqual([path.name for path in out.iterdir()], ["day.tif"])
            with rasterio.open(out / "day.tif") as src:
                self.assertEqual(src.transform, Affine(0.01, 0, 7.19, 0, -0.01, 47.41))
                self.assertTrue((src.read(1) == values[59:81, 19:41]).all())


if __name__ == "__main__":
    unittest.main()
