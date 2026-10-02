"""Extract planner tiles and preserve every selected vector byte or terrain pixel."""

import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import io
from itertools import islice
import json
import math
import os
from pathlib import Path
import resource
import struct
import sys
import tempfile
import time


def tile_window(region, zoom, halo=0):
    """Return an exclusive XYZ rectangle. X can cross the world seam."""
    west, south, east, north = region
    count = 1 << zoom

    def row(latitude):
        return (1 - math.asinh(math.tan(math.radians(latitude))) / math.pi) / 2 * count

    return (math.floor((west + 180) / 360 * count) - halo,
            max(0, math.floor(row(north)) - halo),
            math.ceil((east + 180) / 360 * count) + halo,
            min(count, math.ceil(row(south)) + halo))


def selected(zxy, region, terrain=False):
    zoom, x, y = zxy
    west, north, east, south = tile_window(region, zoom, int(terrain))
    count = 1 << zoom
    return north <= y < south and any(west <= wrapped < east for wrapped in (x - count, x, x + count))


def pixels(data):
    from PIL import Image
    with Image.open(io.BytesIO(data)) as image:
        rgba = image.convert("RGBA")
        return struct.pack("<II", *rgba.size) + rgba.tobytes()


def lossless(data):
    from PIL import Image
    with Image.open(io.BytesIO(data)) as image:
        output = io.BytesIO()
        image.save(output, format="WEBP", lossless=True, method=6, quality=75, exact=True)
    encoded = output.getvalue()
    original = pixels(data)
    if pixels(encoded) != original:
        raise ValueError("Lossless WebP encoding changed terrain pixels")
    return (encoded if len(encoded) < len(data) else data), original


def extract(source, destination, region, terrain=False, workers=2, recompress=True):
    from pmtiles.reader import Reader, all_tiles
    from pmtiles.tile import Compression, TileType, zxy_to_tileid
    from pmtiles.writer import Writer

    if destination.exists():
        raise ValueError(f"{destination} already exists")
    start = progress = time.monotonic()
    digest = hashlib.sha256()
    counts, input_bytes = {}, 0
    destination.parent.mkdir(parents=True, exist_ok=True)
    with source.open("rb") as stream, tempfile.TemporaryDirectory(prefix=".tiles-", dir=destination.parent) as directory:
        read = lambda offset, length: os.pread(stream.fileno(), length, offset)
        reader = Reader(read)
        header, metadata = reader.header(), reader.metadata()
        expected = TileType.WEBP if terrain else TileType.MVT
        if header["tile_type"] != expected or terrain and header["tile_compression"] != Compression.NONE:
            raise ValueError("Expected uncompressed WebP terrain or MVT basemap")
        available = [header[key] / 1e7 for key in ("min_lon_e7", "min_lat_e7", "max_lon_e7", "max_lat_e7")]
        if any(region[i] < available[i] - 1e-7 for i in (0, 1)) or any(region[i] > available[i] + 1e-7 for i in (2, 3)):
            raise ValueError("Requested bounds exceed the source archive coverage")
        tiles = ((zxy, data) for zxy, data in all_tiles(read) if selected(zxy, region, terrain))
        output = Path(directory) / "archive.pmtiles"

        def encode(item):
            zxy, data = item
            encoded, exact = lossless(data) if terrain and recompress else (data, pixels(data) if terrain else data)
            return zxy, len(data), encoded, exact

        with output.open("wb") as target, ThreadPoolExecutor(max_workers=workers) as pool:
            writer = Writer(target)
            try:
                # Bound both decoded pixels and encoded payloads independently of region size.
                while batch := list(islice(tiles, workers * 2)):
                    for zxy, size, encoded, exact in pool.map(encode, batch):
                        tile_id = zxy_to_tileid(*zxy)
                        writer.write_tile(tile_id, encoded)
                        digest.update(struct.pack("<QQ", tile_id, len(exact)))
                        digest.update(exact)
                        counts[zxy[0]] = counts.get(zxy[0], 0) + 1
                        input_bytes += size
                    if time.monotonic() - progress >= 10:
                        print(f"Verified {sum(counts.values())} tiles", file=sys.stderr, flush=True)
                        progress = time.monotonic()
                if not counts:
                    raise ValueError("No tiles intersect the requested bounds")
                for key, value in zip(("min_lon_e7", "min_lat_e7", "max_lon_e7", "max_lat_e7"), region):
                    header[key] = round(value * 1e7)
                header["center_lon_e7"] = round((region[0] + region[2]) * 0.5e7)
                header["center_lat_e7"] = round((region[1] + region[3]) * 0.5e7)
                if "bounds" in metadata:
                    metadata["bounds"] = ",".join(map(str, region))
                writer.finalize(header, metadata)
            finally:
                writer.tile_f.close()
        encoded_seconds = time.monotonic() - start
        verified = hashlib.sha256()
        verified_count = 0
        with output.open("rb") as target:
            read_output = lambda offset, length: os.pread(target.fileno(), length, offset)
            for zxy, data in all_tiles(read_output):
                exact = pixels(data) if terrain else data
                verified.update(struct.pack("<QQ", zxy_to_tileid(*zxy), len(exact)))
                verified.update(exact)
                verified_count += 1
            output_header = Reader(read_output).header()
        if verified.digest() != digest.digest() or verified_count != sum(counts.values()):
            raise ValueError("Output archive does not match every selected source tile")
        output.rename(destination)
    return {"source": str(source), "output": str(destination), "bounds": region,
            "terrain": terrain, "recompressed": terrain and recompress, "input_bytes": source.stat().st_size,
            "selected_addressed_bytes": input_bytes, "output_bytes": destination.stat().st_size,
            "output_unique_payload_bytes": output_header["tile_data_length"],
            "tiles_by_zoom": counts, "verified_tiles": verified_count,
            "content_sha256": digest.hexdigest(), "encode_seconds": encoded_seconds,
            "total_seconds": time.monotonic() - start,
            "peak_rss_bytes": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss * (1 if sys.platform == "darwin" else 1024)}


def main():
    try:
        from .planner_maps import bounds
    except ImportError:
        from planner_maps import bounds
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--bbox", type=bounds, required=True)
    parser.add_argument("--terrain", action="store_true")
    parser.add_argument("--no-recompress", action="store_true")
    parser.add_argument("--workers", type=int, choices=range(1, 9), default=2)
    args = parser.parse_args()
    print(json.dumps(extract(args.source, args.output, args.bbox, args.terrain, args.workers,
                             not args.no_recompress), sort_keys=True))


if __name__ == "__main__":
    main()
