"""Bake conservative terrain maxima for the sunlight worker."""

import argparse
from concurrent.futures import ThreadPoolExecutor
from functools import lru_cache
import hashlib
import io
import json
import math
import os
from pathlib import Path
import sys
import tempfile
from zoneinfo import ZoneInfo

import numpy as np
from PIL import Image
from pmtiles.reader import Reader
from pmtiles.tile import Compression, TileType, zxy_to_tileid
from pmtiles.writer import Writer

from . import planner_offline as offline, planner_runtime as runtime, step_pool, step_request
from .planner_grid_maps import MAP_ZOOM
from .planner_map_archive import tile_window

SIZE, DEM_ZOOM, INDEX_ZOOM, UNKNOWN = 512, 12, 10, 32767
BOUND_STEP = 16


def quantize(values):
    """Round bounds upward. This reduces bytes without changing native ray intersections."""
    rounded = (values.astype(np.int32) + BOUND_STEP - 1) // BOUND_STEP * BOUND_STEP
    return np.minimum(rounded, UNKNOWN).astype(np.int16)


def maxima(vertices, stride):
    """A block includes its far edge; an unknown vertex makes the bound unknown."""
    rows = (vertices.shape[0] - 1) // stride
    cols = (vertices.shape[1] - 1) // stride
    result = np.full((rows, cols), -32768, np.int16)
    for y in range(stride + 1):
        for x in range(stride + 1):
            np.maximum(result, vertices[y:y + rows * stride:stride, x:x + cols * stride:stride], out=result)
    return result


def encode(values):
    packed = values.astype(np.int32) + 32768
    rgba = np.zeros((*values.shape, 4), np.uint8)
    rgba[:, :, 0], rgba[:, :, 1], rgba[:, :, 3] = packed >> 8, packed & 255, 255
    output = io.BytesIO()
    Image.fromarray(rgba).save(output, format="WEBP", lossless=True, method=4, exact=True)
    return output.getvalue()


def terrain_coverage(header, bounds, distance_m):
    if header["tile_type"] != TileType.WEBP or header["max_zoom"] != DEM_ZOOM:
        raise ValueError("Use the planner's zoom-12 WebP terrain archive")
    coverage = [header[key] / 1e7 for key in ("min_lon_e7", "min_lat_e7", "max_lon_e7", "max_lat_e7")]
    latitude = distance_m / 110000
    longitude = latitude / math.cos(math.radians(max(abs(bounds[1]), abs(bounds[3]))))
    required = [bounds[0] - longitude, bounds[1] - latitude, bounds[2] + longitude, bounds[3] + latitude]
    if any(coverage[i] > required[i] + 1e-7 for i in (0, 1)) or any(coverage[i] < required[i] - 1e-7 for i in (2, 3)):
        raise ValueError("Terrain must cover the sun search distance beyond every visible edge")
    return coverage


class Tiles:
    """The header, metadata and tiles of one PMTiles archive, or of the grid archives of one: {`z-x-y`: path}.
    A grid archive holds its zoom-11 tile and the zoom-12 tiles in it; a tile of a lower zoom has its own."""

    def __init__(self, archives, header, metadata):
        self.archives, self.header, self.metadata = archives, header, metadata

    @classmethod
    def archive(cls, path):
        with path.open("rb") as stream:
            reader = Reader(lambda offset, length: os.pread(stream.fileno(), length, offset))
            return cls({None: path}, reader.header(), reader.metadata())

    def get(self, z, x, y):
        level = min(z, MAP_ZOOM)
        name = None if None in self.archives else f"{level}-{x >> (z - level)}-{y >> (z - level)}"
        if name not in self.archives:
            return None
        with self.archives[name].open("rb") as stream:
            return Reader(lambda offset, length: os.pread(stream.fileno(), length, offset)).get(z, x, y)


def bake(terrain, output, bounds, timezone, distance_m, horizon_samples=32, horizon_directions=72, *, terrain_grid_sha256=None):
    """Write the sun archive from the Tiles `terrain`."""
    if output.exists():
        raise ValueError("Output exists; choose a fresh path")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".sun-", dir=output.parent) as folder:
        header = dict(terrain.header)
        coverage = terrain_coverage(header, bounds, distance_m)

        # Each index tile that a thread bakes reads 25 terrain tiles, and its neighbours read some of them too.
        workers = step_pool.cores()

        @lru_cache(maxsize=25 * workers)
        def heights(x, y):
            data = terrain.get(DEM_ZOOM, x, y)
            if data is None:
                return np.full((SIZE, SIZE), UNKNOWN, np.int16)
            rgba = np.asarray(Image.open(io.BytesIO(data)).convert("RGBA"))
            if rgba.shape != (SIZE, SIZE, 4):
                raise ValueError("Expected 512-pixel terrain tiles")
            height = rgba[:, :, 0].astype(np.int32) * 256 + rgba[:, :, 1].astype(np.int32) - 32768
            # Round upward when a source has fractional Terrarium metres.
            height += rgba[:, :, 2] > 0
            return np.where(rgba[:, :, 3] == 255, height, UNKNOWN).clip(-32768, UNKNOWN).astype(np.int16)

        # Use every finest terrain tile, including the context outside the displayed region.
        stage = Path(folder)

        def index_tile(z, x, y):
            if z == INDEX_ZOOM:
                vertices = np.full((SIZE * 4 + 1, SIZE * 4 + 1), UNKNOWN, np.int16)
                for dy in range(5):
                    for dx in range(5):
                        h, w = min(SIZE, vertices.shape[0] - dy * SIZE), min(SIZE, vertices.shape[1] - dx * SIZE)
                        vertices[dy * SIZE:dy * SIZE + h, dx * SIZE:dx * SIZE + w] = heights(x * 4 + dx, y * 4 + dy)[:h, :w]
                values = maxima(vertices, 4)
            else:
                children = np.full((SIZE * 2, SIZE * 2), UNKNOWN, np.int16)
                for dy in range(2):
                    for dx in range(2):
                        child = stage / f"{z + 1}-{x * 2 + dx}-{y * 2 + dy}.npy"
                        if child.exists():
                            children[dy * SIZE:(dy + 1) * SIZE, dx * SIZE:(dx + 1) * SIZE] = np.load(child)
                values = children.reshape(SIZE, 2, SIZE, 2).max(axis=(1, 3))
            values = quantize(values)
            np.save(stage / f"{z}-{x}-{y}.npy", values)
            return encode(values)

        count, payload = 0, 0
        # WebP, zlib and numpy release the interpreter lock, so the tiles of a zoom bake on threads.
        with (stage / "sun.pmtiles").open("wb") as target, ThreadPoolExecutor(workers) as executor:
            writer = Writer(target)
            try:
                for z in range(INDEX_ZOOM, -1, -1):
                    west, north, east, south = tile_window(coverage, z)
                    tiles = [(x, y) for y in range(north, south) for x in range(west, east)]
                    for (x, y), data in zip(tiles, executor.map(lambda tile, z=z: index_tile(z, *tile), tiles)):
                        writer.write_tile(zxy_to_tileid(z, x, y), data)
                        count, payload = count + 1, payload + len(data)
                    print(f"Sun index zoom {z}: {count} tiles, {payload / 1e6:.2f} MB", flush=True)
                binding = ({"terrain_grid_sha256": terrain_grid_sha256} if terrain_grid_sha256 is not None
                           else {"terrain_sha256": runtime.digest(terrain.archives[None])})
                metadata = {"sun_format": 1, "dem_zoom": DEM_ZOOM, "index_zoom": INDEX_ZOOM,
                            "distance_m": distance_m, "timezone": timezone, "bound_step": BOUND_STEP,
                            **binding,
                            "attribution": terrain.metadata.get("attribution", ""),
                            "bounds": bounds, "coverage": coverage}
                header.update(min_zoom=0, max_zoom=INDEX_ZOOM, tile_type=TileType.WEBP, tile_compression=Compression.NONE)
                writer.finalize(header, metadata)
            finally:
                writer.tile_f.close()
        from .planner_sun_horizons import add_horizons
        add_horizons(terrain, stage / "sun.pmtiles", stage / "complete.pmtiles", bounds, horizon_samples, horizon_directions)
        (stage / "complete.pmtiles").replace(output)
    print(json.dumps({"index_tiles": count, "index_payload_bytes": payload, "archive_bytes": output.stat().st_size}))


def tiles_of(read, header, offset=None, length=None):
    """The z, x, y of every tile of a PMTiles archive, from its directories alone."""
    from pmtiles.tile import deserialize_directory, tileid_to_zxy

    if offset is None:
        offset, length = header["root_offset"], header["root_length"]
    for entry in deserialize_directory(read(offset, length)):
        if entry.run_length:
            yield from (tileid_to_zxy(tile_id) for tile_id in range(entry.tile_id, entry.tile_id + entry.run_length))
        else:
            yield from tiles_of(read, header, header["leaf_directory_offset"] + entry.offset, entry.length)


def terrain_grid(request, directory):
    """The terrain Tiles of the grid archives, each verified once; the grid manifest is their identity."""
    source = step_request.view(request["layers"]["planner/terrain/grid"], directory / "source")
    index = json.loads((source / "index.json").read_bytes())
    if index["format"] != 1 or index["kind"] != "terrain" or index["map_zoom"] != 11:
        raise ValueError("Use a terrain grid index")
    for name in index["files"]:
        runtime.relative_path(name)
        if name != "maps/terrain.json" and not (Path(name).parent.as_posix() == "maps/tiles/terrain" and name.endswith(".pmtiles")):
            raise ValueError("Terrain grid selects an unrelated file")
    offline.materialize(source, directory, index, ("maps/terrain.json",))
    metadata = json.loads((directory / "maps/terrain.json").read_bytes())
    if metadata != index["metadata"]:
        raise ValueError("Terrain grid metadata differs")
    digest = hashlib.sha256(runtime.encoded(index)).hexdigest()
    # A grid archive is stored as it is, so its object is the archive, and one hash verifies it.
    parts, entries = {}, []
    for name, entry in sorted(index["files"].items()):
        if name.endswith(".pmtiles"):
            if entry["transport"] != {"bytes": entry["bytes"], "sha256": entry["sha256"], "encoding": "identity"}:
                raise ValueError("Terrain grid archives must be stored as they are")
            parts[Path(name).stem] = source / "objects" / entry["sha256"]
            entries.append(entry)
    with ThreadPoolExecutor(step_pool.cores()) as pool:
        list(pool.map(offline.verify, parts.values(), entries))
    if not parts:
        from .planner_geo import bounds as parse_bounds
        bounds = parse_bounds(metadata["bounds"] if isinstance(metadata["bounds"], str)
                              else ",".join(map(str, metadata["bounds"])))
        if metadata.get("format") != "webp":
            raise ValueError("Use WebP terrain")
        header = {"tile_type": TileType.WEBP, "max_zoom": metadata["maxzoom"],
                  **dict(zip(("min_lon_e7", "min_lat_e7", "max_lon_e7", "max_lat_e7"),
                             (round(value * 1e7) for value in bounds)))}
        return None, header, metadata, digest
    headers = []
    for part, path in parts.items():
        with path.open("rb") as stream:
            read = lambda offset, length: os.pread(stream.fileno(), length, offset)
            headers.append(Reader(read).header())
            if headers[-1]["tile_type"] != TileType.WEBP or headers[-1]["tile_compression"] != Compression.NONE:
                raise ValueError("Use uncompressed WebP terrain")
            keys = ("tile_type", "tile_compression", "min_lon_e7", "min_lat_e7", "max_lon_e7", "max_lat_e7")
            if any(headers[-1][key] != headers[0][key] for key in keys):
                raise ValueError("Terrain grid archive headers differ")
            # A tile is read from the archive of its zoom-11 tile only, so one in another archive would be lost.
            for z, x, y in tiles_of(read, headers[-1]):
                level = min(z, MAP_ZOOM)
                if part != f"{level}-{x >> (z - level)}-{y >> (z - level)}":
                    raise ValueError("Terrain tile belongs to another grid archive")
    header = {**headers[-1], "min_zoom": min(h["min_zoom"] for h in headers), "max_zoom": max(h["max_zoom"] for h in headers)}
    return Tiles(parts, header, metadata), header, metadata, digest


def step(request=None):
    """Bake sunlight from the portable terrain grid, including empty unknown coverage."""
    request = request if request is not None else step_request.read()
    options = request["options"]
    output = Path(request["output"]) / "sun"
    output.mkdir()
    with tempfile.TemporaryDirectory(dir=output.parent) as temporary:
        terrain, header, metadata, digest = terrain_grid(request, Path(temporary))
        coverage = terrain_coverage(header, options["bounds"], options["distance_m"])
        if terrain is None:
            from .planner_sun_horizons import ANGLE_STEP
            metadata = {"sun_format": 3, "dem_zoom": DEM_ZOOM, "index_zoom": INDEX_ZOOM,
                        "bound_step": BOUND_STEP, "horizon_min_zoom": 8, "horizon_zoom": DEM_ZOOM,
                        "horizon_samples": options["horizon_samples"], "horizon_directions": options["horizon_directions"],
                        "horizon_step": ANGLE_STEP, "distance_m": options["distance_m"], "timezone": options["time_zone"],
                        "terrain_grid_sha256": digest, "attribution": metadata.get("attribution", ""),
                        "bounds": options["bounds"], "coverage": coverage}
            (output / "empty.json").write_bytes(runtime.encoded(metadata))
            return
        bake(terrain, output / "sun.pmtiles", options["bounds"], options["time_zone"], options["distance_m"],
             options["horizon_samples"], options["horizon_directions"], terrain_grid_sha256=digest)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("region")
    parser.add_argument("--terrain", type=Path, required=True)
    parser.add_argument("--bounds", required=True)
    parser.add_argument("--time-zone", required=True)
    parser.add_argument("--distance-m", type=int, default=30000)
    parser.add_argument("--horizon-samples", type=int, choices=(32, 64), default=32)
    parser.add_argument("--horizon-directions", type=int, choices=(36, 72), default=72)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    bounds = list(map(float, args.bounds.split(",")))
    if len(bounds) != 4 or not all(math.isfinite(value) for value in bounds) or not (-180 <= bounds[0] < bounds[2] <= 180 and -85 < bounds[1] < bounds[3] < 85) or not 0 < args.distance_m <= 30000:
        parser.error("Provide four bounds and a search distance up to 30000 metres")
    ZoneInfo(args.time_zone)
    bake(Tiles.archive(args.terrain), args.output, bounds, args.time_zone, args.distance_m, args.horizon_samples,
         args.horizon_directions)


if __name__ == "__main__":
    step() if sys.argv[1:] == ["--step"] else main()
