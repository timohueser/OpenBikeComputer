"""Partition a planner map into immutable PMTiles grid archives."""

from collections import OrderedDict
from itertools import islice
import argparse
import json
from pathlib import Path
import tempfile

from . import planner_offline as offline, planner_runtime as runtime, step_pool, step_request
from .planner_map_archive import empty_mbtiles, lossless

MAP_ZOOM = 11
# Only TrailView reads terrain of this zoom: its own archives keep it out of the planner and the offline areas.
FINE_ZOOM = 13


def map_kinds(maps):
    """The tile archives of a map bundle; optional layers exist only when requested."""
    return ["basemap", "places", "overlays", "terrain"] + [
        layer for layer in runtime.DATA_LAYERS if (maps / f"{layer}.pmtiles").exists()]


def recoded(tiles):
    """Terrain tiles encoded again by libwebp: the same pixels in fewer bytes than `planner-dem` writes. Pillow holds
    the GIL while it encodes, so the tiles go to processes."""
    workers = step_pool.worker_count()
    with step_pool.pool(workers) as pool:
        while batch := list(islice(tiles, 4 * workers)):
            yield from zip((zxy for zxy, _ in batch), pool.map(smaller, (data for _, data in batch)))


def smaller(data):
    return lossless(data)[0]


def map_tiles(source, output, kinds=None):
    from pmtiles.reader import Reader, MmapSource, all_tiles
    from pmtiles.tile import zxy_to_tileid
    from pmtiles.writer import Writer
    import struct
    import tempfile

    output.mkdir(parents=True, exist_ok=True)
    for kind in kinds or map_kinds(source):
        target = output / kind
        target.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(prefix=".tiles-", dir=output) as temporary:
            cache = OrderedDict()
            try:
                with (source / f"{kind}.pmtiles").open("rb") as file:
                    read = MmapSource(file)
                    reader = Reader(read)
                    header, metadata = reader.header(), reader.metadata()
                    terrain = kind == "terrain"
                    for (z, x, y), data in recoded(all_tiles(read)) if terrain else all_tiles(read):
                        level = min(z, MAP_ZOOM)
                        name = f"{level}-{x >> (z-level)}-{y >> (z-level)}"
                        if terrain and z >= FINE_ZOOM: name = f"{FINE_ZOOM}/{name}"
                        stream = cache.pop(name, None)
                        if stream is None:
                            (Path(temporary) / name).parent.mkdir(exist_ok=True)
                            stream = (Path(temporary) / name).open("ab")
                        stream.write(struct.pack("<QI", zxy_to_tileid(z, x, y), len(data)))
                        stream.write(data)
                        cache[name] = stream
                        if len(cache) > 16: cache.popitem(last=False)[1].close()
            finally:
                for stream in cache.values(): stream.close()
            for path in sorted(path for path in Path(temporary).rglob("*") if path.is_file()):
                archive = target / f"{path.relative_to(temporary)}.pmtiles"
                archive.parent.mkdir(exist_ok=True)
                with path.open("rb") as records, archive.open("wb") as destination:
                    writer = Writer(destination)
                    try:
                        while record := records.read(12):
                            tile_id, length = struct.unpack("<QI", record)
                            data = records.read(length)
                            if len(data) != length: raise ValueError("Incomplete tile staging file")
                            writer.write_tile(tile_id, data)
                        writer.finalize(dict(header), dict(metadata))
                    finally:
                        writer.tile_f.close()
                path.unlink()


def step(request):
    from pmtiles.convert import mbtiles_to_pmtiles
    from pmtiles.reader import Reader, MmapSource

    kind = request["options"]["kind"]
    output = Path(request["output"])
    # A step that writes more than one archive names itself; its grid selects one archive.
    layer = request["layers"][request["options"].get("step", f"planner/{kind}")]
    if len(layer) != 1:
        raise ValueError("A map grid reads one archive")
    name, source = next(iter(layer.items()))
    with tempfile.TemporaryDirectory(dir=output.parent) as directory:
        work = Path(directory)
        archives = work / "archives"
        archives.mkdir()
        archive = archives / f"{kind}.pmtiles"
        empty = False
        if name.endswith(".mbtiles"):
            empty_metadata = empty_mbtiles(source)
            empty = empty_metadata is not None
            if empty:
                header, metadata = empty_metadata
            # The PMTiles writer requires at least one tile; sea-only terrain has none.
            if not empty:
                mbtiles_to_pmtiles(source, archive, None)
        elif kind == "sun" and name == "sun/empty.json":
            metadata = json.loads(Path(source).read_bytes())
            if metadata.get("sun_format") != 3:
                raise ValueError("Unsupported empty sunlight metadata")
            header = {"min_zoom": 0, "max_zoom": 12}
            empty = True
        else:
            archive.symlink_to(Path(source).absolute())
        if not empty:
            map_tiles(archives, work / "tiles", [kind])
            with archive.open("rb") as stream:
                reader = Reader(MmapSource(stream))
                header, metadata = reader.header(), reader.metadata()
        objects = output / "objects"
        tiles = work / "tiles" / kind
        files = {f"maps/tiles/{kind}/{path.relative_to(tiles).as_posix()}": offline.pack_file(path, objects)
                 for path in sorted(tiles.rglob("*.pmtiles"))}
        metadata = {**metadata, "tilejson": "3.0.0", "minzoom": header["min_zoom"],
                    "maxzoom": header["max_zoom"], "bounds": request["options"]["bounds"]}
        if any(name.startswith(f"maps/tiles/terrain/{FINE_ZOOM}/") for name in files):
            # The tile service and an offline area stop at zoom 12; TrailView reads the zoom 13 packs.
            metadata.update(maxzoom=FINE_ZOOM - 1, trailview_maxzoom=FINE_ZOOM)
        if kind == "overlays":
            routing = json.loads(Path(request["layers"]["planner/routing/grid"]["index.json"]).read_bytes())
            if metadata.get("routing_package") != routing["graph"]["source"]:
                raise ValueError("Overlay tiles use another routing package")
            metadata["routing_package"] = routing["files"]["routing/blocks.json"]["sha256"]
        tilejson = work / f"{kind}.json"
        tilejson.write_bytes(runtime.encoded(metadata))
        files[f"maps/{kind}.json"] = offline.pack_file(tilejson, objects)
        index = {"format": 1, "kind": kind, "map_zoom": MAP_ZOOM, "files": files,
                 "source": offline.item(Path(source) if empty else archive), "metadata": metadata}
        (output / "index.json").write_bytes(runtime.encoded(index))
        step_request.metrics(request, {"archives": len(files) - 1})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--step", action="store_true", required=True)
    parser.parse_args()
    step(step_request.read())


if __name__ == "__main__":
    main()
