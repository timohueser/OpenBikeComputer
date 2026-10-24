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

    def test_the_webp_variant_keeps_the_alpha_codes_exactly(self):
        rng = np.random.default_rng(1)
        stack = np.zeros((4, tiles.TILE, tiles.TILE, 4), np.uint8)
        stack[..., :3] = (np.arange(tiles.TILE) // 2)[None, :, None, None]
        stack[..., 3] = rng.choice([0, 1, 2, 90, 255], (4, tiles.TILE, tiles.TILE))
        encoded = colour.encoder("webp-12")(stack)
        self.assertEqual(sorted(encoded), ["q1", "q2", "q3", "q4"])
        alpha = np.asarray(Image.open(io.BytesIO(encoded["q3"][1])).convert("RGBA"))[..., 3]
        self.assertTrue((alpha == stack[2, ..., 3]).all())


if __name__ == "__main__":
    unittest.main()
