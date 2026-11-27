"""Partition a planner map into immutable PMTiles grid archives."""

from collections import OrderedDict
from itertools import islice
import argparse
import json
from pathlib import Path
import struct
import tempfile

from . import planner_offline as offline, planner_runtime as runtime, step_pool, step_request
from .planner_map_archive import empty_mbtiles, lossless

# The largest pack file: specs/planner-offline.md#grid-publication-and-selection gives the reason.
PACK_BYTES = 15 * 1024 * 1024
# Only TrailView reads terrain of this zoom: its own archives keep it out of the planner and the offline areas.
FINE_ZOOM = 13


def recoded(tiles):
    """Terrain tiles encoded again by libwebp: the same pixels in fewer bytes than `planner-dem` writes. Pillow holds
    the GIL while it encodes, so the tiles go to processes."""
    workers = step_pool.worker_count()
    with step_pool.pool(workers) as pool:
        while batch := list(islice(tiles, 4 * workers)):
            yield from zip((zxy for zxy, _ in batch), pool.map(smaller, (data for _, data in batch)))


def smaller(data):
    return lossless(data)[0]


def staged(path):
    """The (tile ID, bytes) records of a staging file, in the order they were written."""
    with path.open("rb") as records:
        while record := records.read(12):
            tile_id, length = struct.unpack("<QI", record)
            data = records.read(length)
            if len(data) != length: raise ValueError("Incomplete tile staging file")
            yield tile_id, data


def stage(stream, tile_id, data):
    stream.write(struct.pack("<QI", tile_id, len(data)))
    stream.write(data)


def split(path):
    """Moves the tiles deeper than the zoom of the pack staged at `path` into the staging files of its four
    squares one zoom deeper, and keeps its one tile at its own zoom, if it has one, at `path`."""
    from pmtiles.tile import tileid_to_zxy

    level = int(path.name.split("-")[0])
    own, children = None, {}
    try:
        for count, (tile_id, data) in enumerate(staged(path), 1):
            z, x, y = tileid_to_zxy(tile_id)
            if z == level:
                own = tile_id, data
                continue
            child = path.with_name(f"{level + 1}-{x >> (z - level - 1)}-{y >> (z - level - 1)}")
            if child not in children: children[child] = child.open("ab")
            stage(children[child], tile_id, data)
    finally:
        for stream in children.values(): stream.close()
    if count == 1: raise ValueError(f"The pack {path.name} holds one tile of more than {PACK_BYTES} bytes")
    path.unlink()
    if own:
        with path.open("ab") as stream: stage(stream, *own)
        return [path, *children]
    return list(children)


def map_tiles(source, output, kinds):
    """Writes the tiles of each archive into packs: a pack holds the tiles of one square from its zoom down to
    the end of the level range of its base level. A pack file of more than `PACK_BYTES` splits into its one
    tile at its own zoom and the packs of its four squares one zoom deeper."""
    from pmtiles.reader import Reader, MmapSource, all_tiles
    from pmtiles.tile import zxy_to_tileid
    from pmtiles.writer import Writer

    output.mkdir(parents=True, exist_ok=True)
    for kind in kinds:
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
                        level = runtime.base_level(z)
                        name = f"{level}-{x >> (z-level)}-{y >> (z-level)}"
                        if terrain and z >= FINE_ZOOM: name = f"{FINE_ZOOM}/{name}"
                        stream = cache.pop(name, None)
                        if stream is None:
                            (Path(temporary) / name).parent.mkdir(exist_ok=True)
                            stream = (Path(temporary) / name).open("ab")
                        stage(stream, zxy_to_tileid(z, x, y), data)
                        cache[name] = stream
                        if len(cache) > 16: cache.popitem(last=False)[1].close()
            finally:
                for stream in cache.values(): stream.close()
            pending = [path for path in Path(temporary).rglob("*") if path.is_file()]
            while pending:
                path = pending.pop()
                archive = target / f"{path.relative_to(temporary)}.pmtiles"
                archive.parent.mkdir(exist_ok=True)
                with archive.open("wb") as destination:
                    writer = Writer(destination)
                    try:
                        for tile_id, data in staged(path): writer.write_tile(tile_id, data)
                        writer.finalize(dict(header), dict(metadata))
                    finally:
                        writer.tile_f.close()
                if archive.stat().st_size > PACK_BYTES:
                    archive.unlink()
                    pending += split(path)
                else:
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
        # The PMTiles writer requires at least one tile; sea-only terrain has none.
        empty = empty_mbtiles(source) if name.endswith(".mbtiles") else None
        if empty:
            header, metadata = empty
        elif name.endswith(".mbtiles"):
            mbtiles_to_pmtiles(source, archive, None)
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
            cells = json.loads(Path(request["layers"]["planner/routing/cells/grid"]["index.json"]).read_bytes())
            if metadata.get("routing_package") != cells["files"]["routing/directory.bin"]["sha256"]:
                raise ValueError("Overlay tiles use another routing package")
        tilejson = work / f"{kind}.json"
        tilejson.write_bytes(runtime.encoded(metadata))
        files[f"maps/{kind}.json"] = offline.pack_file(tilejson, objects)
        index = {"format": 1, "kind": kind, "files": files,
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
