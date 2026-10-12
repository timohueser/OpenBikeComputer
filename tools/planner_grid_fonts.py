"""Join the label glyph ranges of a planner map for offline selection."""

import argparse
import gzip
import json
from pathlib import Path, PurePosixPath
import tempfile

from . import planner_mvt as mvt, planner_grid_pack as pack, planner_offline as offline, step_request

# A copy of the basemap text fields: protomaps `layers(..., {lang: "en"})` and `planner-poi-icons`
# in `map-style.ts`. A style, `lang` or protomaps change must update it. `planner-network-labels`
# draws the `ref` of the overlay tiles.
LABEL_KEYS ={"name", "name:en", "pgf:name", "name2", "pgf:name2", "name3", "pgf:name3",
              "ref", "ref:en", "shield_text", "addr_housenumber"}


def label_texts(tile):
    """Yield the distinct label strings of each layer of one vector tile."""
    for field, layer in mvt.fields(tile):
        if field != 3: continue
        keys, values, labels = [], [], set()
        for field, value in mvt.fields(layer):
            if field == 3: keys.append(value.decode())
            elif field == 4: values.append(next((text.decode() for number, text in mvt.fields(value) if number == 1), None))
            elif field == 2:
                for packed in (packed for number, packed in mvt.fields(value) if number == 2):
                    tags = mvt.packed(packed)
                    labels.update(zip(tags[::2], tags[1::2]))
        yield from {values[value] for key, value in labels if keys[key] in LABEL_KEYS and values[value]}


def archive_texts(archives):
    """Yield the label strings of each tile of the vector tile `archives`."""
    from pmtiles.reader import MmapSource, all_tiles
    for archive in archives:
        with archive.open("rb") as file:
            for _, tile in all_tiles(MmapSource(file)):
                yield from label_texts(gzip.decompress(tile) if tile[:2] == b"\x1f\x8b" else tile)


def glyph_ranges(texts):
    """Return the indices (code point // 256) of the glyph ranges that `texts` use."""
    # MapLibre requests the glyphs of the drawn text: the style upper-cases some labels, and Arabic
    # letters become presentation forms U+FB50-U+FEFF.
    ranges = {ord(character) >> 8 for text in set(texts) for character in text + text.upper()}
    return ranges | ({251, 252, 253, 254} if ranges & {6, 7, 8} else set())


def offline_fonts(ranges, source, names, work):
    """Join the glyph `ranges` of each font stack of `names`, files below `source`, into one file.

    MapLibre keeps only the glyphs of the requested range, so every range path of a stack can
    share this file. A missing range file stalls the labels of each tile that requests it."""
    stacks = {}
    start = lambda name: int(PurePosixPath(name).stem.split("-")[0])
    for name in names:
        path = PurePosixPath(name)
        if path.parent.parent == PurePosixPath("maps/assets/fonts") and path.suffix == ".pbf":
            stacks.setdefault(path.parent.name, []).append(name)
    result = {}
    for stack, names in sorted(stacks.items()):
        target = work / "fonts" / f"{stack}.pbf"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(b"".join((source / name).read_bytes() for name in sorted(names, key=start)
                                    if start(name) >> 8 in ranges))
        result[target] = names
    return result


def step(request):
    with tempfile.TemporaryDirectory(dir=Path(request["output"]).parent) as temporary:
        work = Path(temporary)
        source = work / "source"
        for number, files in enumerate(request["layers"].values()):
            grid = step_request.view(files, work / f"grid-{number}")
            index = json.loads((grid / "index.json").read_bytes())
            offline.materialize(grid, source, index, ("maps/tiles/", "maps/assets/fonts/"))
        ranges = glyph_ranges(archive_texts(sorted((source / "maps/tiles").glob("*/*.pmtiles"))))
        names = [path.relative_to(source).as_posix() for path in (source / "maps/assets/fonts").rglob("*.pbf")]
        joined = offline_fonts(ranges, source, names, work)
        files = {f"offline/fonts/{path.name}": path for path in joined}
        aliases = {name: f"offline/fonts/{path.name}" for path, names in joined.items() for name in names}
        pack.pack_index(request, files, aliases=aliases)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--step", action="store_true", required=True)
    parser.parse_args()
    step(step_request.read())


if __name__ == "__main__":
    main()
