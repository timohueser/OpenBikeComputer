"""Bake the TrailView colour tiles of one region from the Sentinel-2 quarterly mosaics of CDSE, as the
prototype's `bake.py` reads them: per quarter the colour and the NDVI, as specs/trailview-tiles.md
defines them.

The `obc data` step is `planner/trailview-colour` (`--step`). `--fetch` is the capture of
`sentinel-2-mosaics`: the window over a box of each band of each mosaic tile of a year, over CDSE S3
with the keys of `tools/planner_snow.py`. `--variants` bakes each variant from a capture and prints
its bytes per km².
"""

import argparse
import io
import json
import re
import sys
import threading
import time
import urllib.parse
from pathlib import Path

import numpy as np

from . import tiles
from .. import planner_geo as geo, step_request

STAC = "https://stac.dataspace.copernicus.eu/v1/collections/sentinel-2-global-mosaics/items"
BANDS = ("B02", "B03", "B04", "B08", "observations")
# A pixel with fewer valid looks in its quarter falls back to the palette.
MIN_OBSERVATIONS = 3
# Below this visible reflectance a winter pixel is mostly terrain shadow, so its chroma comes from summer.
DARK = 0.025
# The alpha code of a quarter's texel that saw snow (snow_free); a measured NDVI never takes it.
SNOW_MARK = 1
# Heath from the satellite: open ground that reflects as much red as green (the midpoint of 1.1 to 1.4)
# while it grows. Mediterranean summer ground is as red, but dry: its NDVI is low and past its spring peak.
MOOR_GREEN_RED = 1.25
MOOR_NDVI = 0.45
MOOR_PEAK = 0.1
# The prototype's colour reaches 156 m texels; zoom 9 has 0.17 km texels at 47° N.
BOTTOM_ZOOM = 9
# Variant: codec, last zoom and chroma subsampling. 4:2:0 halves the chroma resolution of the texels:
# at zoom 12 the texels are 13 m at 47° N, the chroma 26 m and the NDVI in alpha 13 m.
VARIANTS = {
    "webp-13": ("webp", 13, "4:2:0"), "webp-12": ("webp", 12, "4:2:0"), "webp-11": ("webp", 11, "4:2:0"),
    "avif-12": ("avif", 12, "4:2:0"), "avif-11": ("avif", 11, "4:2:0"), "avif444-12": ("avif", 12, "4:4:4"),
}
# Pillow's default WebP quality is 80; AVIF uses the same number.
QUALITY = 80


def smoothstep(low, high, x):
    t = np.clip((x - low) / (high - low), 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


def ndvi(code):
    """The NDVI of an alpha code of 2 to 255."""
    return (np.asarray(code, np.float32) - 1.0) / 127 - 1


def mosaics(files):
    """{quarter: [{band: path}]}: the band windows of a `sentinel-2-mosaics` capture, by mosaic tile."""
    out = {}
    for name, path in sorted(files.items()):
        item, band = name.rsplit("/", 1)
        quarter = int(re.search(r"_Q([1-4])_", item)[1])
        out.setdefault(quarter, {}).setdefault(item, {})[band.removesuffix(".tif")] = path
    return {quarter: list(items.values()) for quarter, items in sorted(out.items())}


_open = threading.local()


def dataset(path):
    """The open raster `path`. A worker keeps it open, so that GDAL's block cache keeps the strips of a
    capture that the next tile reads too; a thread has its own, as a dataset is not thread-safe."""
    import rasterio

    opened = _open.__dict__.setdefault("datasets", {})
    if path not in opened:
        opened[path] = rasterio.open(path)
    return opened[path]


def read(items, transform, texel):
    """The visible reflectance (3, H, W), the NDVI and the valid texels of one quarter on the EPSG:3857
    grid `transform`."""
    import rasterio
    from rasterio.warp import Resampling, reproject

    resampling = Resampling.bilinear if texel < 10 else Resampling.average
    bands = {}
    for band in BANDS:
        out = np.full((tiles.TILE, tiles.TILE), np.nan, np.float32)
        for item in items:
            src = dataset(item[band])
            reproject(rasterio.band(src, 1), out, dst_transform=transform, dst_crs="EPSG:3857", resampling=resampling,
                      src_nodata=src.nodata, dst_nodata=np.nan, init_dest_nodata=False)
        bands[band] = out
    looks = np.nan_to_num(bands["observations"])
    valid = (looks >= MIN_OBSERVATIONS) & np.isfinite(bands["B04"])
    visible = np.clip(np.nan_to_num(np.stack([bands["B04"], bands["B03"], bands["B02"]])) / 10000.0, 0.0, 1.0)
    b04, b08 = np.nan_to_num(bands["B04"]), np.nan_to_num(bands["B08"])
    return visible, np.clip((b08 - b04) / np.maximum(b08 + b04, 1.0), -1.0, 1.0), valid


def texels(seen):
    """RGBA8 per quarter from {quarter: (visible, ndvi, valid)}: visible reflectance as refl^(1/2.2),
    and NDVI as 2–255, 0 where looks are few. Winter pixels in terrain shadow take their chroma from
    summer."""
    summer = seen[3]
    for q in (1, 4):
        visible, ndvi_, valid = seen[q]
        luma = visible.mean(axis=0)
        shaded_snow = (visible[2] > 1.15 * visible[0]) & (ndvi_ < 0.1)
        dark = valid & summer[2] & (luma < DARK) & ~shaded_snow
        scale = luma / np.maximum(summer[0].mean(axis=0), 1e-4)
        visible[:, dark] = (summer[0] * scale)[:, dark]
    out = {}
    for q, (visible, ndvi_, valid) in seen.items():
        rgb = np.round(255.0 * visible ** (1 / 2.2))
        a = np.where(valid, 1 + np.round((np.maximum(ndvi_, -0.99) + 1) / 2 * 254), 0)
        out[q] = np.stack([*rgb, a], axis=-1).astype(np.uint8)
    return out


def snow_free(quarters):
    """The quarters (RGBA8, as `texels` writes them) with no snow in their colour: the snow data says
    where snow lies, and the satellite only gives bare ground its colour. A texel that a quarter sees
    snow on takes the colour of the nearest quarter that does not, the previous one first, or black where
    none does, and the alpha SNOW_MARK, so that the engine can still read the satellite's snow where the
    snow data has none. Run again, it changes nothing."""
    def snowy(texels):
        # The prototype shader's snow test: as blue as it is red, and bright or in shade where nothing grows.
        a = texels[..., 3].astype(np.float32)
        refl = (texels[..., :3] / 255.0) ** 2.2
        red, blue = refl[..., 0], refl[..., 2]
        bright = smoothstep(0.36, 0.5, refl.min(axis=-1)) * smoothstep(0.9, 0.97, blue / np.maximum(red, 1e-3))
        shaded = (blue >= 1.15 * red) * ((a - 1) / 127 - 1 <= 0.1) * smoothstep(0.03, 0.06, blue)
        return (a == SNOW_MARK) | ((a > SNOW_MARK) & (np.maximum(bright, shaded) > 0.5))

    snow = {q: snowy(texels) for q, texels in quarters.items()}
    clean = {}
    for q, texels in quarters.items():
        texels = texels.copy()
        need = snow[q].copy()
        for other in ((q - 2) % 4 + 1, q % 4 + 1, (q + 1) % 4 + 1):
            take = need & (quarters[other][..., 3] > SNOW_MARK) & ~snow[other]
            texels[take, :3] = quarters[other][take, :3]
            need &= ~take
        texels[need, :3] = 0
        texels[snow[q], 3] = SNOW_MARK
        clean[q] = texels
    return clean


def quarters(mosaics, transform, texel):
    """The four quarters of one tile, RGBA8 and free of snow."""
    return snow_free(texels({q: read(mosaics.get(q, []), transform, texel) for q in range(1, 5)}))


def satellite_heath(quarters, most):
    """Where the satellite sees heath: in summer's quarter, or spring's where summer saw nothing, the
    ground reflects as much red as green while its NDVI is high and near the year's most."""
    texels = np.where((quarters[3][..., 3] > 0)[..., None], quarters[3], quarters[2])
    refl = (texels[..., :3] / 255.0) ** 2.2
    ndvi_ = (texels[..., 3] - 1.0) / 127 - 1
    return ((texels[..., 3] > 0) & (refl[..., 1] < MOOR_GREEN_RED * refl[..., 0]) & (ndvi_ >= MOOR_NDVI)
            & (ndvi_ >= (most - 1.0) / 127 - 1 - MOOR_PEAK))


def average(children):
    """A coarser tile of the four quarters (4, H, W, 4) from its children: each texel the mean colour, in
    linear light, of the children that looked, without the black that `snow_free` leaves where no quarter
    was free of snow, and the mean NDVI of those that measured one; SNOW_MARK where more of them saw snow
    than measured."""
    mosaic = np.zeros((4, 2 * tiles.TILE, 2 * tiles.TILE, 4), np.uint8)
    for (dx, dy), child in children.items():
        if child is not None:
            mosaic[:, dy * tiles.TILE:(dy + 1) * tiles.TILE, dx * tiles.TILE:(dx + 1) * tiles.TILE] = child
    four = np.stack([mosaic[:, 0::2, 0::2], mosaic[:, 0::2, 1::2], mosaic[:, 1::2, 0::2], mosaic[:, 1::2, 1::2]])
    a = four[..., 3].astype(np.float32)
    looked, measured = a > 0, a > SNOW_MARK
    n, m = looked.sum(axis=0), measured.sum(axis=0)
    coloured = looked & ~((a == SNOW_MARK) & (four[..., :3].max(axis=-1) == 0))
    linear = ((four[..., :3] / 255.0) ** 2.2 * coloured[..., None]).sum(axis=0) / np.maximum(coloured.sum(axis=0), 1)[..., None]
    code = np.round((a * measured).sum(axis=0) / np.maximum(m, 1))
    alpha = np.where(n == 0, 0, np.where(n - m > m, SNOW_MARK, code))
    return np.concatenate([np.round(255.0 * linear ** (1 / 2.2)), alpha[..., None]], axis=-1).astype(np.uint8)


def encoder(variant):
    """The encode of a tile of the four quarters: an archive per quarter, `q1` to `q4`."""
    from PIL import Image
    from pmtiles.tile import TileType

    codec, _, subsampling = VARIANTS[variant]

    def encode(texels):
        if not (texels[..., 3] > 0).any():
            return {}
        out = {}
        for q in range(4):
            stream = io.BytesIO()
            image = Image.fromarray(texels[q], "RGBA")
            if codec == "webp":
                image.save(stream, "WEBP", quality=QUALITY, alpha_quality=100)
                out[f"q{q + 1}"] = (TileType.WEBP, stream.getvalue())
            else:
                image.save(stream, "AVIF", quality=QUALITY, subsampling=subsampling, max_threads=1)
                out[f"q{q + 1}"] = (TileType.AVIF, stream.getvalue())
        return out

    return encode


def bake(mosaics, bounds, variants, output, metadata):
    """Write the archives of each of `variants`, which share a last zoom, in `output/<variant>/`; return
    their bytes per archive and zoom by variant."""
    top, = {VARIANTS[variant][1] for variant in variants}
    encoders = {variant: encoder(variant) for variant in variants}
    encode = lambda texels: {f"{variant}/{name}": data for variant, code in encoders.items() for name, data in code(texels).items()}
    stack = lambda z, x, y: np.stack([q for _, q in sorted(quarters(mosaics, tiles.transform(z, x, y), tiles.texel_m(z, y)).items())])
    sizes = tiles.pyramid(bounds, range(BOTTOM_ZOOM, top + 1), stack, average, encode, output,
                          {**metadata, "trailview_format": tiles.FORMAT})
    return {variant: {name.split("/")[1]: zooms for name, zooms in sizes.items() if name.startswith(f"{variant}/")} for variant in variants}


def step():
    request = step_request.read()
    options = request["options"]
    variant, output = options["variant"], Path(request["output"])
    sizes = bake(mosaics(step_request.files(request, "sentinel-2-mosaics")), options["bounds"], [variant], output / "variant",
                 {"attribution": options["attribution"].format(year=options["year"]), "variant": variant})[variant]
    for path in (output / "variant" / variant).iterdir():
        path.rename(output / path.name)
    for folder in (output / "variant" / variant, output / "variant"):
        folder.rmdir()
    step_request.metrics(request, {"bytes_per_zoom": sizes, "bytes_per_km2": per_km2(sizes, options["bounds"])})


def per_km2(sizes, bounds):
    return {name: round(sum(zooms.values()) / tiles.km2(bounds), 1) for name, zooms in sizes.items()}


def items(bounds, year):
    """The mosaic tiles of the year over the bounds: {id: {band: GDAL path}}."""
    from .. import planner_runtime as runtime

    query = urllib.parse.urlencode({"bbox": ",".join(map(str, bounds)), "limit": 100,
                                    "datetime": f"{year}-01-01T00:00:00Z/{year}-12-31T23:59:59Z"})
    page, out = runtime.read_url(f"{STAC}?{query}"), {}
    while True:
        for item in page["features"]:
            out[item["id"]] = {band: item["assets"][band]["href"].replace("s3://", "/vsis3/") for band in BANDS}
        following = next((link["href"] for link in page.get("links", []) if link.get("rel") == "next"), None)
        if not following:
            return out
        page = runtime.read_url(following)


def fetch(bounds, year, out):
    """The capture of `sentinel-2-mosaics`: `<mosaic tile>/<band>.tif`, the window of each band over the
    bounds, one pixel wider on each side. CDSE S3 allows 4 connections per user."""
    from concurrent.futures import ThreadPoolExecutor

    from .. import planner_snow

    planner_snow.cdse_credentials()
    found = items(bounds, year)
    for item in found:
        (out / item).mkdir(parents=True, exist_ok=True)
    with ThreadPoolExecutor(4) as pool:
        list(pool.map(lambda job: planner_snow.subset(job[1], bounds, out / job[0]),
                      [(item, path) for item, bands in found.items() for path in bands.values()]))
    print(f"sentinel-2-mosaics: {len(found)} mosaic tiles", file=sys.stderr, flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--bounds", type=geo.bounds, required=True)
    parser.add_argument("--year", type=int, required=True)
    parser.add_argument("--fetch", type=Path, help="write the capture of sentinel-2-mosaics here")
    parser.add_argument("--variants", type=Path, help="bake each variant from the capture in this folder")
    parser.add_argument("--output", type=Path, help="with --variants: the folder of the archives")
    args = parser.parse_args()
    if args.fetch:
        fetch(args.bounds, args.year, args.fetch)
        return
    if not (args.variants and args.output):
        parser.error("give --fetch, or --variants with --output")
    files = {path.relative_to(args.variants).as_posix(): path for path in args.variants.rglob("*.tif")}
    for top in sorted({zoom for _, zoom, _ in VARIANTS.values()}):
        start = time.monotonic()
        variants = [variant for variant, (_, zoom, _) in VARIANTS.items() if zoom == top]
        for variant, sizes in bake(mosaics(files), args.bounds, variants, args.output, {"attribution": ""}).items():
            print(json.dumps({"variant": variant, "bytes_per_km2": per_km2(sizes, args.bounds), "s": round(time.monotonic() - start)}))


if __name__ == "__main__":
    step() if sys.argv[1:] == ["--step"] else main()
