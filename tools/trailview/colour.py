"""The TrailView colour: the colour tiles of one region from the prepared texels of `colour`, and those
texels of one zoom 10 block from the Sentinel-2 quarterly mosaics of CDSE (specs/obc-data.md#colour,
specs/trailview-tiles.md#colour).

The `obc data` step is `planner/trailview-colour` (`--step`). `prepare --block X-Y --year YEAR` writes the
file of one block of `colour`: it reads the window over the block of each band of each mosaic tile of the
year, over CDSE S3 with the keys of `tools/planner_snow.py`.
"""

import argparse
import hashlib
import io
import os
import re
import shutil
import sys
import threading
import urllib.parse
from pathlib import Path

import numpy as np

from . import tiles
from .. import planner_geo as geo, step_request

STAC = "https://stac.dataspace.copernicus.eu/v1/collections/sentinel-2-global-mosaics/items"
BANDS = ("B02", "B03", "B04", "B08", "observations")
# The zoom of a file of `colour`, and of its texels.
BLOCK_ZOOM, ZOOM = 10, 12
# A look count of `colour` stops here, so MIN_OBSERVATIONS can be 1 to LOOKS.
LOOKS = 15
# Darker than this in red plus near-infrared reflectance, a texel is water or deep shadow, as most
# reflectance there is noise or clipped below 0: its NDVI is −1.
DARK_NDVI = 0.001
# A pixel with fewer valid looks in its quarter falls back to the palette.
MIN_OBSERVATIONS = 3
# `trailview_format` of the colour archives, which the readers check.
FORMAT = 2
# Below this visible reflectance a winter pixel is mostly terrain shadow, so its chroma comes from summer.
DARK = 0.025
# The alpha code of a quarter's texel that saw snow (snow_free); a measured NDVI never takes it.
SNOW_MARK = 1
# The mark of a measured texel in the NDVI tile; the other marks are the codes 0 and SNOW_MARK.
MEASURED = 2
# The prototype's colour reaches 156 m texels; zoom 9 has 0.17 km texels at 47° N.
BOTTOM_ZOOM = 9
# Variant: codec, last zoom and chroma subsampling. 4:2:0 halves the chroma resolution of the texels:
# at zoom 12 the texels are 13 m at 47° N, and the chroma and the NDVI 26 m.
VARIANTS = {
    "webp-12": ("webp", 12, "4:2:0"), "avif-12": ("avif", 12, "4:2:0"), "avif444-12": ("avif", 12, "4:4:4"),
}
# Pillow's default WebP quality is 80; AVIF uses the same number.
QUALITY = 80
# The step of a stored NDVI code: 0.031 NDVI, a twentieth of the span the cover ramps take.
NDVI_STEP = 4


def smoothstep(low, high, x):
    t = np.clip((x - low) / (high - low), 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


def ndvi(code):
    """The NDVI of an alpha code of 2 to 255."""
    return (np.asarray(code, np.float32) - 1.0) / 127 - 1


def mosaics(files):
    """{quarter: [{band: path}]}: the band windows of `fetch`, by mosaic tile."""
    out = {}
    for name, path in sorted(files.items()):
        item, band = name.rsplit("/", 1)
        quarter = int(re.search(r"_Q([1-4])_", item)[1])
        out.setdefault(quarter, {}).setdefault(item, {})[band.removesuffix(".tif")] = path
    return {quarter: list(items.values()) for quarter, items in sorted(out.items())}


_open = threading.local()


def dataset(path):
    """The open raster `path`. A worker keeps it open, so that GDAL's block cache keeps the strips of a
    window that the next tile reads too; a thread has its own, as a dataset is not thread-safe."""
    import rasterio

    opened = _open.__dict__.setdefault("datasets", {})
    if path not in opened:
        opened[path] = rasterio.open(path)
    return opened[path]


def measure(items, transform, texel):
    """The texels of `colour` of one quarter on the EPSG:3857 grid `transform`: the red, green, blue and
    near-infrared codes (H, W, 4), and the look counts (H, W)."""
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
    # The floor keeps "looks ≥ n" for a whole n.
    looks = np.where(np.isfinite(bands["B04"]), np.minimum(np.floor(np.nan_to_num(bands["observations"])), LOOKS), 0)
    refl = np.clip(np.nan_to_num(np.stack([bands[band] for band in ("B04", "B03", "B02", "B08")], axis=-1)) / 10000.0, 0.0, 1.0)
    return np.round(255.0 * refl ** (1 / 2.2)).astype(np.uint8), looks.astype(np.uint8)


def seen(rgbn, looks):
    """{quarter: (visible, NDVI, valid)} for `texels` from the texels of `colour` of one tile: (4, H, W, 4)
    codes and (H, W, 4) looks."""
    out = {}
    for q in range(1, 5):
        refl = (rgbn[q - 1] / 255.0) ** 2.2
        red, nir = refl[..., 0], refl[..., 3]
        ndvi_ = np.where(nir + red < DARK_NDVI, -1.0, np.clip((nir - red) / np.maximum(nir + red, 1e-4), -1.0, 1.0))
        out[q] = (np.moveaxis(refl[..., :3], -1, 0), ndvi_, looks[..., q - 1] >= MIN_OBSERVATIONS)
    return out


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


def block_path(year, x, y):
    """The path of the file of block x, y of `colour` of `year`."""
    return f"{year}/{x}/{y}.pmtiles"


_readers = threading.local()


class Prepared:
    """The texels of `colour` of `year` in `files` ({path: file}), as `specs/obc-data.md#colour` defines
    them. A worker thread opens a file once."""

    def __init__(self, files, year):
        self.files, self.year = files or {}, year

    def tile(self, x, y):
        """The codes (4, H, W, 4) and the looks (H, W, 4) of zoom 12 tile x, y; 0 where it has no data."""
        from pmtiles.reader import MmapSource, Reader

        path = self.files.get(block_path(self.year, x >> (ZOOM - BLOCK_ZOOM), y >> (ZOOM - BLOCK_ZOOM)))
        opened = _readers.__dict__.setdefault("readers", {})
        if path is not None and path not in opened:
            # A block without data is an empty file.
            opened[path] = Path(path).stat().st_size and Reader(MmapSource(open(path, "rb")))
        body = opened[path].get(ZOOM, x, y) if path is not None and opened[path] else None
        if body is None:
            return np.zeros((4, tiles.TILE, tiles.TILE, 4), np.uint8), np.zeros((tiles.TILE, tiles.TILE, 4), np.uint8)
        images = []
        while body:
            end = 8 + int.from_bytes(body[4:8], "little")
            images.append(lossless_texels(body[:end]))
            body = body[end:]
        return np.stack(images[:4]), images[4]


def lossless_texels(image):
    """The RGBA texels of a lossless WebP image."""
    from PIL import Image

    return np.asarray(Image.open(io.BytesIO(image)).convert("RGBA"))


def quarters(prepared, z, x, y):
    """The four quarters of tile z/x/y, zoom 12 or finer, RGBA8 after the look minimum, the NDVI, the shadow
    rule and the snow rule. A finer texel is the zoom 12 texel that holds its centre."""
    levels = z - ZOOM
    key = (prepared, x >> levels, y >> levels)
    # A thread keeps the last zoom 12 tile, which the finer tiles under it read in turn.
    if getattr(_readers, "last", (None,))[0] != key:
        _readers.last = key, snow_free(texels(seen(*prepared.tile(key[1], key[2]))))
    clean = _readers.last[1]
    size, mask = tiles.TILE >> levels, (1 << levels) - 1
    rows, cols = slice((y & mask) * size, ((y & mask) + 1) * size), slice((x & mask) * size, ((x & mask) + 1) * size)
    return {q: t[rows, cols].repeat(1 << levels, axis=0).repeat(1 << levels, axis=1) for q, t in clean.items()}


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


def stored_ndvi(alpha):
    """The alpha of a quarter as the archives store it: a measured texel takes the mean NDVI code of the
    measured texels of its aligned 2 × 2 block, rounded half to even, in steps of NDVI_STEP; codes 0 and
    SNOW_MARK stay. The step buckets meet at code 128, NDVI 0, which the engine's water test reads."""
    measured = alpha > SNOW_MARK
    blocks = lambda texels: texels.reshape(tiles.TILE // 2, 2, tiles.TILE // 2, 2).sum(axis=(1, 3))
    mean = np.round(blocks(np.where(measured, alpha, 0).astype(np.float32)) / np.maximum(blocks(measured), 1))
    code = 128 + NDVI_STEP * np.floor_divide(mean.astype(np.int32) - 128, NDVI_STEP) + NDVI_STEP // 2
    return np.where(measured, code.repeat(2, axis=0).repeat(2, axis=1), alpha).astype(np.uint8)


def ndvi_tile(alpha):
    """The NDVI tile of a quarter's alpha (specs/trailview-tiles.md#colour): per aligned 2 × 2 block,
    the code of `stored_ndvi` that its measured texels share, 0 where none is measured, and the marks of
    its four texels, 2 bits each in reading order from bit 0."""
    stored = stored_ndvi(alpha)
    blocks = lambda texels: texels.reshape(tiles.TILE // 2, 2, tiles.TILE // 2, 2)
    code = blocks(np.where(stored > SNOW_MARK, stored, 0)).max(axis=(1, 3))
    mark = blocks(np.where(stored > SNOW_MARK, MEASURED, stored))
    marks = mark[:, 0, :, 0] | mark[:, 0, :, 1] << 2 | mark[:, 1, :, 0] << 4 | mark[:, 1, :, 1] << 6
    return np.stack([code, marks], axis=-1).astype(np.uint8)


def encoder(variant):
    """The encode of a tile of the four quarters: an archive per quarter, `q1` to `q4`. A body is the
    NDVI tile as lossless WebP, then the colour without alpha in the codec of the variant."""
    from PIL import Image
    from pmtiles.tile import TileType

    codec, _, subsampling = VARIANTS[variant]

    def encode(texels):
        if not (texels[..., 3] > 0).any():
            return {}
        out = {}
        for q in range(4):
            stream = io.BytesIO()
            image = Image.fromarray(np.ascontiguousarray(texels[q][..., :3]), "RGB")
            if codec == "webp":
                image.save(stream, "WEBP", quality=QUALITY)
            else:
                image.save(stream, "AVIF", quality=QUALITY, subsampling=subsampling, max_threads=1)
            out[f"q{q + 1}"] = (TileType.UNKNOWN, tiles.lossless(ndvi_tile(texels[q][..., 3]))[1] + stream.getvalue())
        return out

    return encode


def masked(prepared, bounds, z, x, y):
    """The quarters (4, H, W, 4) of tile z/x/y of the archives: 0 where a texel centre is outside the bounds."""
    out = np.stack([q for _, q in sorted(quarters(prepared, z, x, y).items())])
    out[:, ~tiles.inside(bounds, z, x, y)] = 0
    return out


def bake(prepared, bounds, variant, output, metadata):
    """Write the archives of `variant` in `output`; return their bytes per archive and zoom."""
    stack = lambda z, x, y: masked(prepared, bounds, z, x, y)
    return tiles.pyramid(bounds, range(BOTTOM_ZOOM, VARIANTS[variant][1] + 1), stack, average, encoder(variant), output,
                         {**metadata, "trailview_format": FORMAT})


def step():
    request = step_request.read()
    options = request["options"]
    variant = options["variant"]
    prepared = Prepared(step_request.files(request, "colour"), options["year"])
    sizes = bake(prepared, options["bounds"], variant, Path(request["output"]),
                 {"attribution": options["attribution"].format(year=options["year"]), "variant": variant})
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
    """`<mosaic tile>/<band>.tif` in `out`: the window of each band over the bounds, one pixel wider on
    each side, one at a time. A window that `out` holds stays."""
    from .. import planner_snow

    planner_snow.cdse_credentials()
    # A transient 5xx of CDSE S3 must not end a run of days.
    os.environ.update(GDAL_HTTP_MAX_RETRY="8", GDAL_HTTP_RETRY_DELAY="3")
    for item, bands in items(bounds, year).items():
        (out / item).mkdir(parents=True, exist_ok=True)
        for path in bands.values():
            planner_snow.subset(path, bounds, out / item)


def prepare(block, year, work, out):
    """Write the file of `block` (x, y) of `colour` of `year` into `out`, then its mark `<x>-<y>.sha256`, the
    SHA-256 of the file. `work` holds the windows of the mosaic tiles; it stays when the run fails, so the
    next run downloads no window again."""
    bx, by = block
    fetch(geo.tile_bounds(BLOCK_ZOOM, bx, by), year, work)
    target = out / block_path(year, bx, by)
    write(mosaics({str(path.relative_to(work)): path for path in work.glob("*/*.tif")}), block, target)
    with open(target, "rb") as stream:
        (out / f"{bx}-{by}.sha256").write_text(hashlib.file_digest(stream, "sha256").hexdigest() + "\n")
    shutil.rmtree(work, ignore_errors=True)


def write(found, block, target):
    """Write the file `target` of `block` (x, y) of `colour` from the band windows `found` of `mosaics`: an
    empty file where no tile of the block has a look."""
    from pmtiles.tile import Compression, TileType, zxy_to_tileid
    from pmtiles.writer import Writer

    bx, by = block
    span = range(1 << (ZOOM - BLOCK_ZOOM))
    bodies = {}
    for x, y in ((bx * len(span) + dx, by * len(span) + dy) for dy in span for dx in span):
        measured = [measure(found.get(q, []), tiles.transform(ZOOM, x, y), tiles.texel_m(ZOOM, y)) for q in range(1, 5)]
        looks = np.stack([look for _, look in measured], axis=-1)
        if looks.any():
            bodies[zxy_to_tileid(ZOOM, x, y)] = b"".join(tiles.lossless(image)[1] for image in [*(c for c, _ in measured), looks])
    target.parent.mkdir(parents=True, exist_ok=True)
    with open(target, "wb") as stream:
        if bodies:
            writer = Writer(stream)
            for tile_id in sorted(bodies):
                writer.write_tile(tile_id, bodies[tile_id])
            e7 = lambda value: round(value * 1e7)
            west, south, east, north = geo.tile_bounds(BLOCK_ZOOM, bx, by)
            writer.finalize({
                "tile_type": TileType.UNKNOWN, "tile_compression": Compression.NONE,
                "min_lon_e7": e7(west), "min_lat_e7": e7(south), "max_lon_e7": e7(east), "max_lat_e7": e7(north),
                "center_zoom": ZOOM, "center_lon_e7": e7((west + east) / 2), "center_lat_e7": e7((south + north) / 2),
            }, {})


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    command = commands.add_parser("prepare", help="the file of one block of `colour`")
    command.add_argument("--block", type=lambda text: tuple(map(int, text.split("-"))), required=True, help="X-Y")
    command.add_argument("--year", type=int, required=True)
    command.add_argument("--work", type=Path, required=True, help="the directory of the windows, removed at success")
    command.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    prepare(args.block, args.year, args.work, args.out)


if __name__ == "__main__":
    step() if sys.argv[1:] == ["--step"] else main()
