"""Bake map-scale horizons. Point inspection keeps the native terrain calculation."""
import io
import math
import os
from pathlib import Path
import tempfile

import numpy as np
from numba import njit
from PIL import Image
from pmtiles.reader import Reader, all_tiles
from pmtiles.tile import zxy_to_tileid
from pmtiles.writer import Writer

from . import step_pool
from .planner_map_archive import tile_window

CURVE, UNKNOWN, TILE = 1 / (2 * 6371008.8), 32767, 512
ANGLE_STEP = 90 / 254


@njit(cache=True)
def height(arrays, origins, level, x, y):
    xx, yy = x - origins[level, 0], y - origins[level, 1]
    width, rows, offset = origins[level, 2], origins[level, 3], origins[level, 4]
    if xx < 0 or yy < 0 or xx >= width or yy >= rows:
        return math.inf
    value = arrays[offset + yy * width + xx]
    return math.inf if value == UNKNOWN else float(value)


@njit(cache=True)
def horizon(arrays, origins, px, py, dx, dy, distance):
    x0, y0 = math.floor(px), math.floor(py)
    h00, h10 = height(arrays, origins, 0, x0, y0), height(arrays, origins, 0, x0 + 1, y0)
    h01, h11 = height(arrays, origins, 0, x0, y0 + 1), height(arrays, origins, 0, x0 + 1, y0 + 1)
    if not (math.isfinite(h00) and math.isfinite(h10) and math.isfinite(h01) and math.isfinite(h11)):
        return 255
    fx, fy = px - x0, py - y0
    observer = h00 * (1 - fx) * (1 - fy) + h10 * fx * (1 - fy) + h01 * (1 - fx) * fy + h11 * fx * fy + 1
    lo, best = 0., 0.
    while lo < distance:
        level = 12
        while True:
            size = 1 << level
            x, y = math.floor((px + dx * (lo + 1e-5)) / size), math.floor((py + dy * (lo + 1e-5)) / size)
            tx = ((x + 1 if dx > 0 else x) * size - px) / dx if dx != 0 else math.inf
            ty = ((y + 1 if dy > 0 else y) * size - py) / dy if dy != 0 else math.inf
            hi = min(tx, ty, distance)
            if level >= 2:
                if height(arrays, origins, level, x, y) < observer + best * lo + CURVE * lo * lo:
                    lo = hi
                    break
                level = 0 if level == 2 else level - 1
                continue
            a0, a1 = height(arrays, origins, 0, x, y), height(arrays, origins, 0, x + 1, y)
            a2, a3 = height(arrays, origins, 0, x, y + 1), height(arrays, origins, 0, x + 1, y + 1)
            if not (math.isfinite(a0) and math.isfinite(a1) and math.isfinite(a2) and math.isfinite(a3)):
                return 255
            else:
                e, n, cross = a1 - a0, a2 - a0, a3 - a2 - a1 + a0
                fx, fy = px - x, py - y
                a = cross * dx * dy - CURVE
                b = e * dx + n * dy + cross * (fx * dy + fy * dx)
                c = a0 + e * fx + n * fy + cross * fx * fy - observer
                if lo > 0:
                    best = max(best, a * lo + b + c / lo)
                best = max(best, a * hi + b + c / hi)
                if a < 0 and c < 0:
                    peak = math.sqrt(c / a)
                    if lo < peak < hi:
                        best = max(best, a * peak + b + c / peak)
            lo = hi
            break
    return min(254, math.ceil(math.degrees(math.atan(best)) / ANGLE_STEP))


@njit(nogil=True, cache=True)
def trace_tile(arrays, origins, x, y, samples, directions, distance):
    result = np.empty((samples, samples, directions), np.uint8)
    world = TILE * (1 << 12)
    for row in range(samples):
        py = (y + (row + .5) / samples) * TILE - .5
        lat = math.atan(math.sinh(math.pi * (1 - 2 * (py + .5) / world)))
        pitch = 2 * math.pi * 6378137 * math.cos(lat) / world
        for col in range(samples):
            px = (x + (col + .5) / samples) * TILE - .5
            for direction in range(directions):
                angle = 2 * math.pi * direction / directions
                result[row, col, direction] = horizon(arrays, origins, px, py, math.sin(angle) / pitch, -math.cos(angle) / pitch, distance)
    return result


def encode(values, bounds=None):
    size, _, directions = values.shape
    delta = values.copy()
    delta[:, :, 1:] = np.diff(values, axis=2)
    # Consecutive bearings share RGB; each group is a spatial image plane.
    rgb = delta.reshape(size, size, directions // 3, 3).transpose(2, 0, 1, 3).reshape(size * directions // 3, size, 3)
    if bounds is not None:
        tail = np.zeros((math.ceil(rgb.size / (TILE * 3)), TILE, 4), np.uint8)
        tail[:, :, 3] = 255
        tail[:, :, :3].reshape(-1, 3)[:rgb.size // 3] = rgb.reshape(-1, 3)
        rgb = np.concatenate((bounds, tail))
    output = io.BytesIO()
    Image.fromarray(rgb).save(output, format="WEBP", lossless=True, method=4, exact=True)
    return output.getvalue()


def decode(data, samples, directions):
    rgb = np.asarray(Image.open(io.BytesIO(data)).convert("RGB"))
    if rgb.shape[1] == TILE:
        rgb = rgb[TILE:].reshape(-1, 3)[:samples * samples * directions // 3]
    delta = rgb.reshape(directions // 3, samples, samples, 3).transpose(1, 2, 0, 3).reshape(samples, samples, directions)
    return np.cumsum(delta, axis=2, dtype=np.uint8)


def add_overview(source, output, executor):
    """A coarser level keeps wide map views within the same decoded cache. The tiles of a zoom run on `executor`."""
    from .planner_sun import BOUND_STEP
    with source.open("rb") as stream, output.open("wb") as target:
        read = lambda offset, length: os.pread(stream.fileno(), length, offset)
        reader = Reader(read)
        metadata, header = reader.metadata(), reader.header()
        size, directions = metadata["horizon_samples"], metadata["horizon_directions"]

        def overview(z, x, y, previous):
            """The values and the bytes of tile z/x/y from its children: `previous` below zoom 11."""
            children = np.full((size * 2, size * 2, directions), 255, np.uint8)
            for dy in range(2):
                for dx in range(2):
                    coordinate = (x * 2 + dx, y * 2 + dy)
                    child = previous.get(coordinate)
                    if z == 11:
                        data = reader.get(12, *coordinate)
                        child = decode(data, size, directions) if data else None
                    if child is not None:
                        children[dy * size:(dy + 1) * size, dx * size:(dx + 1) * size] = child
            values = reduce_grid(children)
            data = reader.get(z, x, y) if z <= 10 else None
            if z <= 10 and data is None:
                raise ValueError("Overview requires every terrain bound tile in the coverage")
            rgba = np.asarray(Image.open(io.BytesIO(data)).convert("RGBA"))[:TILE] if data else None
            return values, encode(values, rgba)

        writer = Writer(target)
        try:
            for (z, x, y), data in all_tiles(read):
                if 8 <= z <= 11:
                    continue
                writer.write_tile(zxy_to_tileid(z, x, y), data)
            previous = {}
            for z in range(11, 7, -1):
                current = {}
                west, north, east, south = tile_window(metadata["coverage"], z)
                tiles = [(x, y) for y in range(north, south) for x in range(west, east)]
                done = executor.map(lambda tile, z=z, previous=previous: overview(z, *tile, previous), tiles)
                for (x, y), (values, data) in zip(tiles, done):
                    current[x, y] = values
                    writer.write_tile(zxy_to_tileid(z, x, y), data)
                previous = current
            metadata.update(sun_format=3, horizon_min_zoom=8, bound_step=BOUND_STEP)
            writer.finalize(header, metadata)
        finally:
            writer.tile_f.close()


def reduce_grid(children):
    size, _, directions = children.shape
    groups = children.reshape(size // 2, 2, size // 2, 2, directions)
    values = np.ceil(groups.mean(axis=(1, 3))).astype(np.uint8)
    values[groups.max(axis=(1, 3)) == 255] = 255
    return values


def load_arrays(terrain, index, rows, distance, executor):
    """The heights that a ray of `distance` metres from the zoom 12 tile rows `rows` reaches, at every level, as
    `horizon` reads them: level 0 from the Tiles `terrain` at zoom 12, level L from the Tiles `index` at zoom 12 - L.
    A height outside the arrays is unknown, so the arrays reach every tile row that such a ray reaches."""
    coverage = [terrain.header[key] / 1e7 for key in ("min_lon_e7", "min_lat_e7", "max_lon_e7", "max_lat_e7")]
    world = TILE << 12

    def reach(y):
        """Level 0 pixels of `distance` at the edge of tile row y: the edge farther from the equator holds the
        smallest pixel of the band."""
        lat = math.atan(math.sinh(math.pi * (1 - 2 * y / (1 << 12))))
        return distance / (2 * math.pi * 6378137 * math.cos(lat) / world)

    # A ray reads the bilinear corners one pixel past its end, and its position rounds down.
    halo = math.ceil(max(reach(rows.start), reach(rows.stop))) + 2
    origins, count = np.zeros((13, 5), np.int64), 0
    for level in range(13):
        if level == 1:
            continue
        west, north, east, south = tile_window(coverage, 12 - level)
        top = max(north, (rows.start * TILE - halo) // (TILE << level))
        bottom = min(south, (rows.stop * TILE + halo) // (TILE << level) + 1)
        width, height = (east - west) * TILE, (bottom - top) * TILE
        origins[level] = [west * TILE, top * TILE, width, height, count]
        count += width * height
    arrays = np.full(count, UNKNOWN, np.int16)
    for level in range(13):
        if level == 1:
            continue
        source, zoom = (terrain, 12) if level == 0 else (index, 12 - level)
        x0, y0, width, height, offset = (int(value) for value in origins[level])
        values = arrays[offset:offset + width * height].reshape(height, width)

        def load(tile):
            x, y = tile
            data = source.get(zoom, x, y)
            if data is None:
                return
            rgba = np.asarray(Image.open(io.BytesIO(data)).convert("RGBA"))
            heights = rgba[:, :, 0].astype(np.int32) * 256 + rgba[:, :, 1] - 32768
            at = np.s_[y * TILE - y0:(y + 1) * TILE - y0, x * TILE - x0:(x + 1) * TILE - x0]
            values[at] = np.where(rgba[:, :, 3] == 255, heights, UNKNOWN)

        tiles = [(x, y) for y in range(y0 // TILE, (y0 + height) // TILE) for x in range(x0 // TILE, (x0 + width) // TILE)]
        list(executor.map(load, tiles))
    return arrays, origins


# Zoom 12 tile rows that trace together. A band also decodes the rows that its rays reach on each side, 5.4 rows at
# 30 km and 55.4° N, so a wider band decodes fewer rows twice and holds more.
BAND = 16


def add_horizons(terrain, index, output, bounds, samples=32, directions=72, workers=None):
    """Trace the horizons of the zoom 12 tiles of `bounds` from the Tiles `terrain` and the sun index `index`, in
    bands of BAND tile rows on `workers` threads, by default one per core that the process may use: the trace releases
    the interpreter lock."""
    from concurrent.futures import ThreadPoolExecutor
    from .planner_sun import Tiles
    with tempfile.TemporaryDirectory(prefix=".horizon-", dir=output.parent) as directory, \
            ThreadPoolExecutor(workers or step_pool.cores()) as executor:
        detail = Path(directory) / "detail.pmtiles"
        with index.open("rb") as stream, detail.open("wb") as target:
            reader = Reader(lambda offset, length: os.pread(stream.fileno(), length, offset))
            metadata, header = reader.metadata(), reader.header()
            writer = Writer(target)
            try:
                for (z, x, y), data in all_tiles(lambda offset, length: os.pread(stream.fileno(), length, offset)):
                    writer.write_tile(zxy_to_tileid(z, x, y), data)
                west, north, east, south = tile_window(bounds, 12)
                distance, total = float(metadata["distance_m"]), (south - north) * (east - west)
                count, payload, levels = 0, 0, Tiles.archive(index)
                for top in range(north, south, BAND):
                    rows = range(top, min(top + BAND, south))
                    arrays = None  # the last band goes before the next one loads
                    arrays, origins = load_arrays(terrain, levels, rows, distance, executor)
                    # Compile before the threads enter the same function.
                    trace_tile(arrays, origins, west, top, 1, directions, 1.)
                    tiles = [(x, y) for y in rows for x in range(west, east)]
                    trace = lambda tile: encode(trace_tile(arrays, origins, *tile, samples, directions, distance))
                    for (x, y), data in zip(tiles, executor.map(trace, tiles)):
                        writer.write_tile(zxy_to_tileid(12, x, y), data)
                        count, payload = count + 1, payload + len(data)
                        if count % 16 == 0:
                            print(f"Horizons {count}/{total}: {payload / 1e6:.2f} MB", flush=True)
                metadata.update(sun_format=2, horizon_zoom=12, horizon_samples=samples,
                                horizon_directions=directions, horizon_step=ANGLE_STEP)
                header.update(max_zoom=12)
                writer.finalize(header, metadata)
            finally:
                writer.tile_f.close()
        add_overview(detail, output, executor)
    print(f"Sunlight archive: {output.stat().st_size / 1e6:.2f} MB", flush=True)
