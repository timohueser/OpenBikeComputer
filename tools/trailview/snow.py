"""Bake the TrailView snow days of one region from the planner snow archive, as specs/trailview-tiles.md
defines them: per band of day steps an archive whose tiles hold, per pixel, the seasons with data and
the seasons with snow on the band's day step. A tile of zoom z holds the four snow tiles of zoom z + 1,
so it covers the material tile of zoom z at full detail. A view reads the band of its date only.

The `obc data` step is `planner/trailview-snow` (`--step`). Without `--step`, it bakes `--snow` into
`--output` and prints the bytes per band.
"""

import argparse
import gzip
import itertools
import json
import os
import sys
import tempfile
from pathlib import Path

import numpy as np

from . import tiles
from .. import step_pool, step_request

# `trailview_format` of the snow day archives.
FORMAT = 3
# The day steps of a season (specs/planner-snow-tiles.md), the steps per band, and the bands.
DAY_STEPS = 183
BAND_STEPS = 4
BANDS = -(-DAY_STEPS // BAND_STEPS)
# Snow sentinels of specs/planner-snow-tiles.md.
NO_SNOW, WHOLE_SEASON, NO_DATA = 253, 254, 255
# The pixels of a snow tile, and the pixels a side of a snow day tile.
AREA, SIZE = 256 * 256, 512
# Snow day tiles per batch of the workers: the bands of a batch wait in memory.
BATCH = 64


def day_step(band):
    """The day step that band `band` shows: the third of its four steps, or the last of the season."""
    return min(BAND_STEPS * band + 2, DAY_STEPS - 1)


def days(body, seasons):
    """The snow day of each band from the planar bytes of one snow tile: per pixel, the seasons with data
    × 16 + the seasons with snow on the band's day step. Shape (BANDS, AREA)."""
    planes = np.frombuffer(body, np.uint8).reshape(seasons, 2, AREA)
    onset, melt = planes[:, 0], planes[:, 1]
    steps = np.array([day_step(band) for band in range(BANDS)], np.uint8)[:, None, None]
    snowy = (onset == WHOLE_SEASON) | ((onset <= DAY_STEPS - 1) & (onset <= steps) & (steps <= melt))
    known = (onset != NO_DATA).sum(axis=0, dtype=np.uint8)
    return (16 * known + snowy.sum(axis=1, dtype=np.uint8)).astype(np.uint8)


def encode(entry):
    """The lossless WebP bodies of the bands of one snow day tile from its four snow tiles (north-west
    first, None where the snow archive has none); none where no season has data."""
    tile_id, bodies, seasons = entry
    planes, half = np.zeros((BANDS, SIZE, SIZE), np.uint8), SIZE // 2
    for q, body in enumerate(bodies):
        if body is not None:
            row, col = (q >> 1) * half, (q & 1) * half
            planes[:, row:row + half, col:col + half] = days(gzip.decompress(body), seasons).reshape(BANDS, half, half)
    if not planes.any():
        return tile_id, None
    return tile_id, [tiles.lossless(plane)[1] for plane in planes]


def parents(snow_tiles, seasons):
    """Per parent of a snow tile, in tile order, its tile id and the bodies of its four children, north-west
    first, from the snow tiles in tile order. Tile ids follow a Hilbert curve per zoom, so the four
    children of a parent are consecutive and the parents come in tile order."""
    from pmtiles.tile import zxy_to_tileid

    parent, bodies = None, None
    for (z, x, y), body in snow_tiles:
        if z == 0:
            continue
        tile_id = zxy_to_tileid(z - 1, x >> 1, y >> 1)
        if tile_id != parent:
            if parent is not None:
                yield parent, bodies, seasons
            parent, bodies = tile_id, [None] * 4
        bodies[(y & 1) << 1 | (x & 1)] = body
    if parent is not None:
        yield parent, bodies, seasons


def bake(snow, output, workers=None):
    """Write `output/<band>.pmtiles` for each band from the snow archive `snow`; return the bytes per band."""
    from pmtiles.reader import MmapSource, Reader, all_tiles
    from pmtiles.tile import Compression, TileType
    from pmtiles.writer import Writer

    output.mkdir(parents=True, exist_ok=True)
    with snow.open("rb") as stream:
        source = MmapSource(stream)
        reader = Reader(source)
        header, metadata = reader.header(), reader.metadata()
        if header["tile_compression"] != Compression.GZIP or metadata.get("step_days") != 2:
            raise ValueError("The snow archive is not the planner snow archive")
        seasons = metadata["seasons"]
        if seasons > 15:
            raise ValueError(f"A snow day holds 15 seasons, not {seasons}")
        streams = [tempfile.NamedTemporaryFile(dir=output, prefix=f".{band:02}-", delete=False) for band in range(BANDS)]
        writers = [Writer(stream) for stream in streams]
        sizes = [0] * BANDS
        try:
            entries = parents(all_tiles(source), seasons)
            pool = step_pool.pool(workers)
            try:
                # The tiles go in batches, so the archive is never in memory, and `map` keeps the tile order of the writers.
                while batch := list(itertools.islice(entries, BATCH)):
                    for tile_id, bodies in pool.map(encode, batch):
                        for band, data in enumerate(bodies or []):
                            writers[band].write_tile(tile_id, data)
                            sizes[band] += len(data)
            finally:
                pool.shutdown(cancel_futures=True)
            if not any(sizes):
                raise ValueError("The snow archive has no tile with data")
            keys = ("min_lon_e7", "min_lat_e7", "max_lon_e7", "max_lat_e7", "center_zoom", "center_lon_e7", "center_lat_e7")
            for band, (writer, stream) in enumerate(zip(writers, streams)):
                writer.finalize({"tile_type": TileType.WEBP, "tile_compression": Compression.NONE, **{key: header[key] for key in keys}}, {
                    "trailview_format": FORMAT, "archive": f"snow-{band:02}", "band": band, "band_steps": BAND_STEPS, "day_step": day_step(band),
                    "first_season": metadata["first_season"], "seasons": seasons, "source": metadata["source"],
                    "attribution": metadata["attribution"],
                })
                stream.close()
                os.replace(stream.name, output / f"{band:02}.pmtiles")
        finally:
            for stream in streams:
                stream.close()
                if os.path.exists(stream.name):
                    os.unlink(stream.name)
    return sizes


def step():
    request = step_request.read()
    sizes = bake(Path(request["layers"]["planner/snow"]["snow.pmtiles"]), Path(request["output"]))
    step_request.metrics(request, {"bytes_per_band": sizes, "bytes_per_km2": round(sum(sizes) / tiles.km2(request["options"]["bounds"]), 1)})


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--snow", type=Path, required=True, help="the planner snow archive")
    parser.add_argument("--output", type=Path, required=True, help="the folder of the band archives")
    args = parser.parse_args()
    print(json.dumps({"bytes_per_band": bake(args.snow, args.output)}))


if __name__ == "__main__":
    step() if sys.argv[1:] == ["--step"] else main()
