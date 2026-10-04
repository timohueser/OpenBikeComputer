"""Bake map-scale horizons. Point inspection keeps the native terrain calculation."""
import io
from itertools import islice
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


def add_overview(source, output):
    """A coarser level keeps wide map views within the same decoded cache."""
    from .planner_sun import BOUND_STEP
    with source.open("rb") as stream, output.open("wb") as target:
        read = lambda offset, length: os.pread(stream.fileno(), length, offset)
        reader = Reader(read)
        metadata, header = reader.metadata(), reader.header()
        size, directions = metadata["horizon_samples"], metadata["horizon_directions"]
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
                for y in range(north, south):
                    for x in range(west, east):
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
                        current[x, y] = values
                        data = reader.get(z, x, y) if z <= 10 else None
                        if z <= 10 and data is None:
                            raise ValueError("Overview requires every terrain bound tile in the coverage")
                        rgba = np.asarray(Image.open(io.BytesIO(data)).convert("RGBA"))[:TILE] if data else None
                        writer.write_tile(zxy_to_tileid(z, x, y), encode(values, rgba))
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


def load_arrays(terrain, index, folder):
    """File-backed arrays keep the offline bake's working memory reclaimable."""
    origins, count = np.zeros((13, 5), np.int64), 0
    with terrain.open("rb") as stream:
        header = Reader(lambda offset, length: os.pread(stream.fileno(), length, offset)).header()
        coverage = [header[key] / 1e7 for key in ("min_lon_e7", "min_lat_e7", "max_lon_e7", "max_lat_e7")]
    for level in range(13):
        if level == 1:
            continue
        west, north, east, south = tile_window(coverage, 12 - level)
        width, rows = (east - west) * TILE, (south - north) * TILE
        origins[level] = [west * TILE, north * TILE, width, rows, count]
        count += width * rows
    arrays = np.memmap(folder / "heights.bin", mode="w+", dtype=np.int16, shape=(count,))
    arrays[:] = UNKNOWN
    for level in range(13):
        if level == 1:
            continue
        source, zoom = (terrain, 12) if level == 0 else (index, 12 - level)
        with source.open("rb") as stream:
            reader = Reader(lambda offset, length: os.pread(stream.fileno(), length, offset))
            west, north, east, south = tile_window(coverage, zoom)
            width, rows, offset = origins[level, 2:]
            values = arrays[offset:offset + width * rows].reshape(rows, width)
            for y in range(north, south):
                for x in range(west, east):
                    data = reader.get(zoom, x, y)
                    if data is None:
                        continue
                    rgba = np.asarray(Image.open(io.BytesIO(data)).convert("RGBA"))
                    heights = rgba[:, :, 0].astype(np.int32) * 256 + rgba[:, :, 1] - 32768
                    values[(y - north) * TILE:(y - north + 1) * TILE, (x - west) * TILE:(x - west + 1) * TILE] = np.where(rgba[:, :, 3] == 255, heights, UNKNOWN)
            arrays.flush()
        print(f"Loaded horizon level {level}", flush=True)
    return arrays, origins


def add_horizons(terrain, index, output, bounds, samples=32, directions=72, workers=2):
    from concurrent.futures import ThreadPoolExecutor
    with tempfile.TemporaryDirectory(prefix=".horizon-", dir=output.parent) as directory:
        arrays, origins = load_arrays(terrain, index, Path(directory))
        detail = Path(directory) / "detail.pmtiles"
        with index.open("rb") as stream, detail.open("wb") as target:
            reader = Reader(lambda offset, length: os.pread(stream.fileno(), length, offset))
            metadata, header = reader.metadata(), reader.header()
            writer = Writer(target)
            try:
                for (z, x, y), data in all_tiles(lambda offset, length: os.pread(stream.fileno(), length, offset)):
                    writer.write_tile(zxy_to_tileid(z, x, y), data)
                west, north, east, south = tile_window(bounds, 12)
                tiles = [(x, y) for y in range(north, south) for x in range(west, east)]
                # Compile once before the bounded worker pool enters the same function.
                trace_tile(arrays, origins, west, north, 1, directions, 1.)
                def bake_tile(tile):
                    x, y = tile
                    return tile, encode(trace_tile(arrays, origins, x, y, samples, directions, float(metadata["distance_m"])))
                payload = 0
                with ThreadPoolExecutor(max_workers=workers) as executor:
                    iterator, count = iter(tiles), 0
                    while batch := list(islice(iterator, workers * 2)):
                        for (x, y), data in executor.map(bake_tile, batch):
                            writer.write_tile(zxy_to_tileid(12, x, y), data)
                            count, payload = count + 1, payload + len(data)
                            if count % 16 == 0:
                                print(f"Horizons {count}/{len(tiles)}: {payload / 1e6:.2f} MB", flush=True)
                metadata.update(sun_format=2, horizon_zoom=12, horizon_samples=samples,
                                horizon_directions=directions, horizon_step=ANGLE_STEP)
                header.update(max_zoom=12)
                writer.finalize(header, metadata)
            finally:
                writer.tile_f.close()
        add_overview(detail, output)
    print(f"Sunlight archive: {output.stat().st_size / 1e6:.2f} MB", flush=True)
