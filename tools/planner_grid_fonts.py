"""Join the label glyph ranges of a planner map for offline selection."""

import argparse
from contextlib import closing
import gzip
from pathlib import Path, PurePosixPath
import sqlite3
import tempfile

from . import planner_mvt as mvt, planner_grid_pack as pack, step_request

# A copy of the basemap text fields: protomaps `layers(..., {lang: "en"})` and `planner-poi-icons`
# in `map-style.ts`. A style, `lang` or protomaps change must update it. `planner-network-labels`
# draws the overlay `ref` that `glyph_ranges` reads.
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


def glyph_ranges(source):
    """Return the indices (code point // 256) of the glyph ranges that labels and route references use."""
    from pmtiles.reader import MmapSource, all_tiles
    texts = set()
    for name in ("basemap", "places"):
        with (source / "maps" / f"{name}.pmtiles").open("rb") as file:
            for _, tile in all_tiles(MmapSource(file)):
                texts.update(label_texts(gzip.decompress(tile) if tile[:2] == b"\x1f\x8b" else tile))
    with closing(sqlite3.connect(f"{(source / 'routing/overlays.sqlite').resolve().as_uri()}?mode=ro", uri=True)) as db:
        texts.update(ref for (ref,) in db.execute("SELECT json_extract(properties, '$.ref') FROM attributes "
                                                  "UNION SELECT json_extract(properties, '$.ref') FROM routes") if ref)
    # MapLibre requests the glyphs of the drawn text: the style upper-cases some labels, and Arabic
    # letters become presentation forms U+FB50-U+FEFF.
    ranges = {ord(character) >> 8 for text in texts for character in text + text.upper()}
    return ranges | ({251, 252, 253, 254} if ranges & {6, 7, 8} else set())


def offline_fonts(source, release, work):
    """Join the used glyph ranges of each font stack into one file for offline selections.

    MapLibre keeps only the glyphs of the requested range, so every range path of a stack can
    share this file. A missing range file stalls the labels of each tile that requests it."""
    ranges, stacks = glyph_ranges(source), {}
    start = lambda name: int(PurePosixPath(name).stem.split("-")[0])
    for name in release["files"]:
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
    output = Path(request["output"])
    inputs = {}
    for layer, selected in request["layers"].items():
        for name, source in selected.items():
            path = f"maps/{name}" if layer != "planner/routing" else name
            inputs[path] = source
    with tempfile.TemporaryDirectory(dir=output.parent) as temporary:
        work = Path(temporary)
        view = step_request.view(inputs, work / "source")
        joined = offline_fonts(view, {"files": inputs}, work)
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
