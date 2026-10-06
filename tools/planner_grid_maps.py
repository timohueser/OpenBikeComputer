"""Partition a planner map into immutable PMTiles grid archives."""

from collections import OrderedDict
from contextlib import closing
import argparse
from pathlib import Path
import sqlite3
import tempfile

from . import planner_offline as offline, planner_runtime as runtime, step_request

MAP_ZOOM = 11


def map_kinds(maps):
    """The tile archives of a map bundle; optional layers exist only when requested."""
    return ["basemap", "places", "overlays", "terrain"] + [
        layer for layer in runtime.DATA_LAYERS if (maps / f"{layer}.pmtiles").exists()]


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
                    for (z, x, y), data in all_tiles(read):
                        level = min(z, MAP_ZOOM)
                        name = f"{level}-{x >> (z-level)}-{y >> (z-level)}"
                        stream = cache.pop(name, None)
                        if stream is None: stream = (Path(temporary) / name).open("ab")
                        stream.write(struct.pack("<QI", zxy_to_tileid(z, x, y), len(data)))
                        stream.write(data)
                        cache[name] = stream
                        if len(cache) > 16: cache.popitem(last=False)[1].close()
            finally:
                for stream in cache.values(): stream.close()
            for path in sorted(Path(temporary).iterdir()):
                with path.open("rb") as records, (target / f"{path.name}.pmtiles").open("wb") as destination:
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
    from pmtiles.convert import mbtiles_to_pmtiles, mbtiles_to_header_json
    from pmtiles.reader import Reader, MmapSource

    kind = request["options"]["kind"]
    output = Path(request["output"])
    layer, = request["layers"].values()
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
            with closing(sqlite3.connect(f"{Path(source).resolve().as_uri()}?mode=ro", uri=True)) as db:
                empty = db.execute("SELECT 1 FROM tiles LIMIT 1").fetchone() is None
                if empty:
                    header, metadata = mbtiles_to_header_json(dict(db.execute("SELECT name, value FROM metadata")))
            # The PMTiles writer requires at least one tile; sea-only terrain has none.
            if not empty:
                mbtiles_to_pmtiles(source, archive, None)
        else:
            archive.symlink_to(Path(source).absolute())
        if not empty:
            map_tiles(archives, work / "tiles", [kind])
            with archive.open("rb") as stream:
                reader = Reader(MmapSource(stream))
                header, metadata = reader.header(), reader.metadata()
        objects = output / "objects"
        files = {f"maps/tiles/{kind}/{path.name}": offline.pack_file(path, objects)
                 for path in sorted((work / "tiles" / kind).glob("*.pmtiles"))}
        metadata = {**metadata, "tilejson": "3.0.0", "minzoom": header["min_zoom"],
                    "maxzoom": header["max_zoom"], "bounds": request["options"]["bounds"]}
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
