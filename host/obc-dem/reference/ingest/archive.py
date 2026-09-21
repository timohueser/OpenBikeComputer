"""The archive: tiles, the priority merge, the manifests, the index, and what `check` holds.

`tile_problems` is the contract as code: everything one tile must be. `rebuild_index` is the
only writer of `index.json`, and it reads nothing but the manifests in `sources/`.
"""

import hashlib
import json
from pathlib import Path

import numpy as np
import rasterio
from rasterio.warp import transform_bounds

from .lattice import (NODATA, Refuse, TILE_PX, WGS84, Window, check_world, covering_window,
                      tile_id, tile_path, tile_window)
from .pool import open_raster, pool_onto_lattice, read_source, to_int16
from .sources.base import READABLE, Source, placed, unpack


# Priority, finest and best-maintained national product first. Where two sources cover the
# same pixel, the one earlier in this list keeps it. Keys with no adapter yet are listed so
# the ranking does not move when an adapter lands.
PRIORITY = (
    "dk", "nl", "it-tn",
    "de-nw", "de-he", "de-ni", "de-by", "de-sn", "de-th", "de-mv", "de-st", "de-bw",
    "fr", "at", "no", "uk", "se", "us", "ca", "nz",
    "ch", "fi", "it-bz", "es", "au",
)

def priority_rank(key: str) -> int:
    """Lower is better. An unlisted key ranks last, so it never displaces a listed one."""

    return PRIORITY.index(key) if key in PRIORITY else len(PRIORITY)



def cut_tile(window: Window, data, ti: int, tj: int):
    """The part of a pooled window that belongs in one tile, as a full 1024 × 1024 raster.

    A tile is always whole: a box that covers a corner of it still writes 1024 × 1024
    pixels, with `NODATA` everywhere the box did not reach.
    """

    row0 = max(window.row0, ti * TILE_PX)
    row1 = min(window.row0 + window.rows, (ti + 1) * TILE_PX)
    col0 = max(window.col0, tj * TILE_PX)
    col1 = min(window.col0 + window.cols, (tj + 1) * TILE_PX)
    if row1 <= row0 or col1 <= col0:
        return None
    top = window.row0 + window.rows
    patch = data[top - row1:top - row0, col0 - window.col0:col1 - window.col0]
    tile = np.full((TILE_PX, TILE_PX), NODATA, dtype="int16")
    tile[(ti + 1) * TILE_PX - row1:(ti + 1) * TILE_PX - row0,
         col0 - tj * TILE_PX:col1 - tj * TILE_PX] = patch
    return tile


def write_tile(path: Path, ti: int, tj: int, tile) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    profile = {
        "driver": "GTiff",
        "height": TILE_PX,
        "width": TILE_PX,
        "count": 1,
        "dtype": "int16",
        "nodata": NODATA,
        "crs": WGS84,
        "transform": tile_window(ti, tj).transform,
        "compress": "deflate",
        "tiled": True,
        "blockxsize": 256,
        "blockysize": 256,
    }
    with rasterio.open(path, "w", **profile) as dst:
        dst.write(tile, 1)
        dst.update_tags(AREA_OR_POINT="Area")


def merge_tiles(existing, incoming, held: set[str], key: str):
    """Fold an incoming tile into the one the archive already has, pixel by pixel.

    Coverage stops at country borders and at survey edges, so two sources over one tile is
    the normal case, not the exception, and the rule is per pixel:

    - Only this source has been here: the maximum, so a second box adds its footprint and
      raises the pixels both boxes cover.
    - This source ranks better than every source that has been here: its pixels win where
      it has them, and the others' pixels stay in its gaps.
    - Otherwise: the pixels already there stay, and this source fills the gaps only.

    A pixel does not record which source wrote it, so the comparison is against the tile's
    contributors as a set. One case loses by that: a source that has already been in a tile
    a worse source also reached falls into the third rule, so where its two boxes overlap
    the **earlier** value stays instead of the maximum. A per-pixel owner plane beside each
    tile would make all of this exact; it would also double the archive.
    """

    if not held - {key}:
        return np.maximum(existing, incoming)
    if all(priority_rank(key) < priority_rank(other) for other in held):
        return np.where(incoming != NODATA, incoming, existing)
    return np.where(existing != NODATA, existing, incoming)


def ingest_raster(path: Path, source: Source, root: Path, held: dict[str, set[str]]):
    """Pool one raster onto the lattice and fold it into the archive's tiles.

    Returns the tiles it wrote, the fraction of the source raster that was void, and the
    number of pixel centres that fell outside the window, which should be none.
    """

    values, src_transform, src_crs, bounds, voided = read_source(path)
    check_world(bounds, str(path))
    window = covering_window(bounds, pad=1)
    lattice, dropped = pool_onto_lattice(values, src_transform, src_crs, window)
    pooled = to_int16(lattice)
    touched = []
    for ti, tj in window.tiles():
        tile = cut_tile(window, pooled, ti, tj)
        if tile is None or not (tile != NODATA).any():
            continue
        out = tile_path(root, ti, tj)
        contributors = held.setdefault(tile_id(ti, tj), set())
        if out.exists():
            if not contributors:
                raise Refuse(
                    f"{out} is in the archive but no source manifest holds it, so a run was cut "
                    "short; delete the tile, or ingest the source that wrote it again"
                )
            with open_raster(out) as src:
                existing = src.read(1)
            merged = merge_tiles(existing, tile, contributors, source.key)
            if not np.any((merged == existing) & (existing != NODATA)):
                contributors.clear()  # nothing the older sources wrote is left in this tile
            tile = merged
        write_tile(out, ti, tj, tile)
        contributors.add(source.key)
        touched.append(tile_id(ti, tj))
    return touched, voided, dropped


def local_rasters(source: Source, directory: Path, bbox, into: Path) -> list[Path]:
    """The files `--input` points at, as rasters that touch the box, in any CRS.

    A portal delivers a zip as often as a bare raster, and an ESRI ASCII grid arrives
    without the CRS the tail needs to place it, so the directory is opened and placed
    first. A zip's members land in `into`, which is the work directory, so a second run of
    the same delivery unpacks nothing and the input directory is left as the portal left it.
    """

    delivered = sorted(path for path in directory.rglob("*")
                       if path.suffix.lower() in READABLE | {".zip"})
    if not delivered:
        raise Refuse(f"{directory}: holds no .tif, .asc or .zip, so it is not what "
                     f"`{source.key}` is delivered as; README.md names the format")
    paths = []
    for path in delivered:
        if path.suffix.lower() == ".zip":
            paths.extend(unpack(path, into / f"{path.stem}.d", READABLE))
        else:
            paths.append(path)
    keep = []
    for path in paths:
        placed(path, source)
        with open_raster(path) as src:
            if src.crs is None:
                raise Refuse(f"{path}: the raster has no CRS, so it cannot be placed")
            west, south, east, north = transform_bounds(src.crs, WGS84, *src.bounds)
        if not (east < bbox[0] or west > bbox[2] or north < bbox[1] or south > bbox[3]):
            keep.append(path)
    return keep


def manifest_dir(root: Path) -> Path:
    return root / "sources"


def load_manifests(root: Path) -> dict[str, dict]:
    directory = manifest_dir(root)
    manifests = {}
    for path in sorted(directory.glob("*.json")) if directory.is_dir() else []:
        manifests[path.stem] = json.loads(path.read_text(encoding="utf-8"))
    return manifests


def write_manifest(root: Path, manifest: dict) -> None:
    manifest_dir(root).mkdir(parents=True, exist_ok=True)
    path = manifest_dir(root) / f"{manifest['key']}.json"
    path.write_text(json.dumps(manifest, indent=2, sort_keys=True, ensure_ascii=False) + "\n", encoding="utf-8")


def source_facts(manifests: dict[str, dict], key: str) -> dict:
    """The four contract fields plus the datum, or a refusal naming the manifest."""

    manifest = manifests[key]
    missing = [name for name in ("product", "attribution", "licence", "vertical_datum", "fetched")
               if not manifest.get(name)]
    if missing:
        raise Refuse(f"sources/{key}.json has no {', '.join(missing)}; ingest `{key}` again to write it")
    return {name: manifest[name] for name in ("product", "attribution", "licence", "vertical_datum", "fetched")}


def contributors(manifests: dict[str, dict]) -> dict[str, set[str]]:
    """Tile id to every source key that has written pixels into it."""

    held: dict[str, set[str]] = {}
    for key in sorted(manifests):
        for tile in manifests[key].get("tiles", []):
            held.setdefault(tile, set()).add(key)
    return held


def best_contributor(keys: set[str]) -> str:
    return min(keys, key=priority_rank)


def tile_digest(path: Path) -> str:
    """The sha256 of a tile's pixels: row-major, little-endian `int16`.

    Not of the file. Two GDAL or zlib builds deflate the same pixels into different bytes,
    and the terrain bakery uses this digest as a skip key, so it has to mean "these heights"
    and not "this compressed stream".
    """

    with open_raster(path) as src:
        pixels = src.read(1)
    return hashlib.sha256(np.ascontiguousarray(pixels.astype("<i2")).tobytes()).hexdigest()


def rebuild_index(root: Path) -> dict:
    """`index.json` from the manifests. A tile not in the index is not in the archive."""

    manifests = load_manifests(root)
    if not manifests:
        raise Refuse(f"{root}: no source manifests, so there is nothing to index")
    held = contributors(manifests)
    tiles, digests, credits = {}, {}, {}
    for tile in sorted(held):
        ti, tj = (int(part) for part in tile.split("/"))
        path = tile_path(root, ti, tj)
        if not path.is_file():
            raise Refuse(f"a source manifest names tile {tile}, which is not in the archive")
        credits[tile] = sorted(held[tile], key=priority_rank)
        tiles[tile] = credits[tile][0]
        digests[tile] = tile_digest(path)
    index = {
        "schema": 1,
        "step_log2": 6,
        "tile_log2": 16,
        "sources": {
            key: source_facts(manifests, key)
            for key in sorted(manifests)
            if any(key in keys for keys in held.values())
        },
        "tiles": tiles,
        "contributors": credits,
        "sha256": digests,
    }
    (root / "index.json").write_text(
        json.dumps(index, indent=2, sort_keys=True, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    return index


def read_index(root: Path) -> dict:
    path = root / "index.json"
    if not path.is_file():
        raise Refuse(f"{path}: no index; run `ingest.py index --archive {root}`")
    return json.loads(path.read_text(encoding="utf-8"))

def tile_problems(path: Path, ti: int, tj: int) -> list[str]:
    """Everything one tile must be, including how the bytes are laid out.

    The layout is part of the contract because the reader streams tiles: a tile that is not
    deflated in 256 x 256 blocks, or is big-endian, reads correctly but not the way the
    baker was measured against.
    """

    problems = []
    with open_raster(path) as src:
        if (src.width, src.height) != (TILE_PX, TILE_PX):
            problems.append(f"{path}: {src.width}x{src.height}, not {TILE_PX}x{TILE_PX}")
        if src.dtypes[0] != "int16":
            problems.append(f"{path}: dtype {src.dtypes[0]}, not int16")
        if src.nodata != NODATA:
            problems.append(f"{path}: nodata {src.nodata}, not {NODATA}")
        if src.crs != WGS84:
            problems.append(f"{path}: CRS {src.crs}, not EPSG:4326")
        if src.count != 1:
            problems.append(f"{path}: {src.count} bands, not 1")
        if src.tags().get("AREA_OR_POINT") != "Area":
            problems.append(f"{path}: AREA_OR_POINT is {src.tags().get('AREA_OR_POINT')}, not Area")
        compression = src.compression.value.lower() if src.compression else "none"
        if compression != "deflate":
            problems.append(f"{path}: compression {compression}, not deflate")
        if src.block_shapes[0] != (256, 256):
            problems.append(f"{path}: internal blocks {src.block_shapes[0]}, not (256, 256)")
        expected = tile_window(ti, tj).transform
        if max(abs(a - b) for a, b in zip(src.transform[:6], expected[:6])) > 1e-9:
            problems.append(f"{path}: transform {src.transform!r} is not the lattice {expected!r}")
    with path.open("rb") as handle:
        if handle.read(2) != b"II":
            problems.append(f"{path}: the TIFF header is not little-endian")
    return problems
