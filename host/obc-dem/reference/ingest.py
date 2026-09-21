#!/usr/bin/env python3
"""Turn a national DTM into reference archive tiles, and move the archive to and from R2.

The archive is one format: max-pooled bare-earth height on the OBCT lattice at 2^6 µdeg,
`int16` metres, one GeoTIFF per 2^16 µdeg tile. Every country branch is here, in one
offline tool that runs once per source release. The baker knows the tiles only, and
`README.md` beside this file holds the contract both sides read.

    python3 ingest.py ingest ch --bbox 8.30,46.75,8.60,46.95 --archive ref/
    python3 ingest.py check --archive ref/
    python3 ingest.py publish --archive ref/

A source adapter has one job: give the shared tail rasters that cover the box, in any CRS
and any dtype. The tail pools each one onto the lattice by the contract's rule — a lattice
pixel keeps the maximum of the source pixels whose centres lie in it — so a rock tower one
source pixel wide survives the 7 m step, and writes whole tiles.
"""

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.request
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

import numpy as np
import rasterio
from rasterio.crs import CRS
from rasterio.errors import RasterioIOError
from rasterio.transform import Affine
from rasterio.warp import transform_bounds
from pyproj import Transformer

# ── the lattice ─────────────────────────────────────────────────────────────
# All lattice arithmetic is exact integer microdegrees. Degrees appear only in the
# GeoTIFF transform, which is built from these integers and read back into them.

GRID_ORIGIN = -268435456  # −2^28 µdeg, the OBCT grid origin on both axes
STEP = 1 << 6  # µdeg per pixel: about 7.1 m of latitude
TILE = 1 << 16  # µdeg per tile side
TILE_PX = TILE // STEP  # 1024 pixels per tile side
NODATA = -32768
DEGREE = 1_000_000  # µdeg per degree

# The world box, in µdeg. A tile outside it is a bug in the caller, not a coverage edge.
WORLD = (-180 * DEGREE, -90 * DEGREE, 180 * DEGREE, 90 * DEGREE)

WGS84 = CRS.from_epsg(4326)

# Absence while pooling. Every real height is far above it, so a lattice pixel that no
# source pixel centre reached still reads as absent after the maximum.
VOID = -1e30
POOL_BLOCK = 256  # source rows per coordinate transform

# The range a real orthometric height is in. A value outside it is a sentinel that the
# source did not declare — −9999 is the common one — and not ground.
PLAUSIBLE_M = (-500.0, 9000.0)

# Priority, finest and best-maintained national product first. Where two sources cover the
# same pixel, the one earlier in this list keeps it. Keys with no adapter yet are listed so
# the ranking does not move when an adapter lands.
PRIORITY = ("nl", "de-nw", "fr", "no", "us", "ch", "es")

ARCHIVE_PREFIX = "reference/v1"
STAC_TIMEOUT = 300


def pixel_index(udeg: int) -> int:
    """The lattice index of the pixel that contains a microdegree coordinate."""

    return (udeg - GRID_ORIGIN) // STEP


def tile_index(pixel: int) -> int:
    return pixel // TILE_PX


def tile_id(ti: int, tj: int) -> str:
    return f"{ti:04d}/{tj:04d}"


def tile_path(root: Path, ti: int, tj: int) -> Path:
    return root / "16" / f"{ti:04d}" / f"{tj:04d}.tif"


@dataclass(frozen=True)
class Window:
    """A rectangle of lattice pixels. Rows count north from the grid origin."""

    row0: int
    col0: int
    rows: int
    cols: int

    @property
    def transform(self) -> Affine:
        """The exact lattice transform, north-up: row 0 of the raster is the northernmost."""

        lon0 = (GRID_ORIGIN + self.col0 * STEP) / DEGREE
        lat_top = (GRID_ORIGIN + (self.row0 + self.rows) * STEP) / DEGREE
        return Affine(STEP / DEGREE, 0, lon0, 0, -STEP / DEGREE, lat_top)

    def tiles(self):
        for ti in range(tile_index(self.row0), tile_index(self.row0 + self.rows - 1) + 1):
            for tj in range(tile_index(self.col0), tile_index(self.col0 + self.cols - 1) + 1):
                yield ti, tj


def covering_window(bounds: tuple[float, float, float, float], pad: int = 0) -> Window:
    """The smallest lattice window that covers a degree box, with `pad` pixels of margin.

    An ingest pads by one pixel. The pooling below drops any centre that lands outside the
    window, and a reprojected boundary is an approximation of a curve, so the margin is
    what makes that drop a safety net rather than a way to lose an edge pixel.
    """

    west, south, east, north = bounds
    col0 = pixel_index(int(np.floor(west * DEGREE))) - pad
    col1 = pixel_index(int(np.ceil(east * DEGREE)) - 1) + 1 + pad
    row0 = pixel_index(int(np.floor(south * DEGREE))) - pad
    row1 = pixel_index(int(np.ceil(north * DEGREE)) - 1) + 1 + pad
    return Window(row0, col0, max(row1 - row0, 1), max(col1 - col0, 1))


def tile_window(ti: int, tj: int) -> Window:
    return Window(ti * TILE_PX, tj * TILE_PX, TILE_PX, TILE_PX)


class Refuse(Exception):
    """A condition the tool refuses to guess about, reported to the caller by name."""


def check_world(bounds: tuple[float, float, float, float], what: str) -> None:
    """Refuse a box outside the world or across the antimeridian.

    The lattice has no wrap. A box whose east edge is west of its west edge crosses the
    antimeridian, and one beyond ±180° or ±90° would land on tiles that cannot exist, so
    both are refused here rather than producing tiles nobody can read.
    """

    west, south, east, north = bounds
    if east <= west or north <= south:
        raise Refuse(
            f"{what}: {west},{south},{east},{north} is empty or crosses the antimeridian; "
            "the archive lattice does not wrap, so split the box at ±180°"
        )
    box = (int(np.floor(west * DEGREE)), int(np.floor(south * DEGREE)),
           int(np.ceil(east * DEGREE)), int(np.ceil(north * DEGREE)))
    if box[0] < WORLD[0] or box[1] < WORLD[1] or box[2] > WORLD[2] or box[3] > WORLD[3]:
        raise Refuse(f"{what}: {west},{south},{east},{north} reaches outside the world box")


# ── sources ─────────────────────────────────────────────────────────────────


class Source:
    """One national product: the manifest facts, and a way to obtain rasters for a box.

    `vertical_datum` is the datum the product's heights are on. An adapter whose product is
    ellipsoidal must convert before the tail: the archive is orthometric.
    """

    def __init__(self, key, country, product, resolution_m, licence, attribution, vertical_datum, extent):
        self.key = key
        self.country = country
        self.product = product
        self.resolution_m = resolution_m
        self.licence = licence
        self.attribution = attribution
        self.vertical_datum = vertical_datum
        self.extent = extent

    def covers(self, bbox) -> bool:
        west, south, east, north = bbox
        a, b, c, d = self.extent
        return not (east < a or west > c or north < b or south > d)

    def fetch(self, bbox, workdir) -> list[Path]:
        raise Refuse(f"{self.key} has no adapter; fetch the rasters by hand and pass --input")


class StacSource(Source):
    """A STAC collection of published rasters, fetched as published."""

    def __init__(self, *args, stac, gsd, **kw):
        super().__init__(*args, **kw)
        self.stac = stac
        self.gsd = gsd

    def fetch(self, bbox, workdir) -> list[Path]:
        url = f"{self.stac}?bbox={bbox[0]},{bbox[1]},{bbox[2]},{bbox[3]}&limit=100"
        assets, seen = [], set()
        while url:
            page = json.loads(http_get(url))
            for feature in page["features"]:
                for name, asset in feature["assets"].items():
                    if name.endswith(".tif") and f"_{self.gsd}_" in name and name not in seen:
                        seen.add(name)
                        assets.append((name, asset["href"]))
            url = next((l["href"] for l in page.get("links", []) if l.get("rel") == "next"), None)
        workdir.mkdir(parents=True, exist_ok=True)
        paths = []
        for i, (name, href) in enumerate(sorted(assets), 1):
            path = workdir / name
            if not path.exists():
                # A half-written file in the cache would look complete to the next run, so
                # the bytes land beside the name and are moved onto it at the end.
                part = path.with_name(name + ".part")
                part.write_bytes(http_get(href))
                os.replace(part, path)
                print(f"  fetch [{i}/{len(assets)}] {name}")
            paths.append(path)
        return paths


SOURCES = {
    "ch": StacSource(
        "ch", "Switzerland", "swissALTI3D 2 m", 2.0,
        "Open data, attribution required", "© swisstopo", "LN02/LHN95", (5.9, 45.8, 10.5, 47.9),
        stac="https://data.geo.admin.ch/api/stac/v0.9/collections/ch.swisstopo.swissalti3d/items",
        gsd="2",
    ),
}


def http_get(url: str) -> bytes:
    """A service drops the occasional request and one box pulls hundreds, so retry."""

    for delay in (2, 4, 8, None):
        try:
            with urllib.request.urlopen(url, timeout=STAC_TIMEOUT) as response:
                return response.read()
        except Exception:
            if delay is None:
                raise
            time.sleep(delay)
    raise AssertionError("unreachable")


def priority_rank(key: str) -> int:
    """Lower is better. An unlisted key ranks last, so it never displaces a listed one."""

    return PRIORITY.index(key) if key in PRIORITY else len(PRIORITY)


# ── the shared tail ─────────────────────────────────────────────────────────


def open_raster(path: Path):
    """`rasterio.open`, with a refusal that names the file and where to delete it from."""

    try:
        return rasterio.open(path)
    except RasterioIOError as exc:
        raise Refuse(f"{path}: cannot be opened ({exc}); delete it from {path.parent} and run again") from exc


def source_xy(transform: Affine, rows, cols):
    """Source-CRS coordinates of pixel centres. Rotation and shear fall out of the affine."""

    return (transform.c + transform.a * cols + transform.b * rows,
            transform.f + transform.d * cols + transform.e * rows)


def source_envelope(transform: Affine, width: int, height: int):
    """The axis-aligned box of a raster's four corners, in its own CRS."""

    cols = np.array([0.0, width, 0.0, width])
    rows = np.array([0.0, 0.0, height, height])
    x, y = source_xy(transform, rows, cols)
    return float(x.min()), float(y.min()), float(x.max()), float(y.max())


def read_source(path: Path):
    """One raster as float32 heights with voids marked, plus its CRS, transform and box.

    A void arrives as a declared sentinel, as a non-finite value, or undeclared. All three
    become `VOID` here, so the pooling below has one convention and no adapter has to know
    what its service sends. Anything outside `PLAUSIBLE_M` is an undeclared sentinel.

    The fraction of the raster that is void comes back with it, because that number is how a
    source that answered with a mostly empty raster is noticed.
    """

    with open_raster(path) as src:
        if src.crs is None:
            raise Refuse(f"{path}: the raster has no CRS, so it cannot be placed")
        if src.scales != (1.0,) or src.offsets != (0.0,):
            raise Refuse(
                f"{path}: the band has scale {src.scales} and offset {src.offsets}, but the tail "
                "reads raw metres; undo them with `gdal_translate -unscale` first"
            )
        values = src.read(1).astype("float32")
        void = ~np.isfinite(values) | (values < PLAUSIBLE_M[0]) | (values > PLAUSIBLE_M[1])
        if src.nodata is not None and np.isfinite(src.nodata):
            void |= values == np.float32(src.nodata)
        values[void] = VOID
        envelope = source_envelope(src.transform, src.width, src.height)
        bounds = transform_bounds(src.crs, WGS84, *envelope)
        return values, src.transform, src.crs, bounds, float(void.mean())


def pool_onto_lattice(values, src_transform, src_crs, window: Window):
    """The contract's pooling rule, done directly.

    Every source pixel goes to the one lattice pixel that contains its centre, and a
    lattice pixel keeps the maximum of the pixels that reached it. `Resampling.max` cannot
    do this: it pools by area overlap, so a source pixel that merely touches a lattice
    pixel raises it. Over the Engelberg box that read 51.7 % of the pixels too high and
    none too low, by up to 286 m, it filled 2 659 pixels no source centre reached, and it
    spread a one-pixel tower over two to four archive pixels.

    A centre is a point, so a rotated or sheared source needs no special case, and a source
    coarser than the lattice simply reaches fewer lattice pixels: the pooling never invents
    ground where no centre landed.
    """

    dest = np.full(window.rows * window.cols, VOID, dtype="float32")
    to_wgs84 = Transformer.from_crs(src_crs, WGS84, always_xy=True)
    top = window.row0 + window.rows
    for start in range(0, values.shape[0], POOL_BLOCK):
        band = values[start:start + POOL_BLOCK]
        rows, cols = np.nonzero(band > VOID / 2)
        if rows.size == 0:
            continue
        x, y = source_xy(src_transform, rows + start + 0.5, cols + 0.5)
        lon, lat = to_wgs84.transform(x, y)
        row = pixel_index(np.floor(lat * DEGREE).astype("int64"))
        col = pixel_index(np.floor(lon * DEGREE).astype("int64"))
        inside = ((row >= window.row0) & (row < top)
                  & (col >= window.col0) & (col < window.col0 + window.cols))
        flat = (top - 1 - row[inside]) * window.cols + (col[inside] - window.col0)
        np.maximum.at(dest, flat, band[rows[inside], cols[inside]])
    return dest.reshape(window.rows, window.cols)


def to_int16(values):
    """Metres as `int16`, rounded half away from zero, with every void at `NODATA`."""

    valid = np.isfinite(values) & (values > VOID / 2)
    finite = np.where(valid, values, 0.0).astype("float64")
    rounded = np.clip(np.sign(finite) * np.floor(np.abs(finite) + 0.5), NODATA + 1, 32767)
    out = np.full(values.shape, NODATA, dtype="int16")
    out[valid] = rounded[valid].astype("int16")
    return out


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
    best contributor. The one case that loses is a source that ranks best and ingests two
    overlapping boxes into a tile a worse source also reached: those two boxes then take
    the later value instead of the maximum.
    """

    if not held - {key}:
        return np.maximum(existing, incoming)
    if all(priority_rank(key) < priority_rank(other) for other in held):
        return np.where(incoming != NODATA, incoming, existing)
    return np.where(existing != NODATA, existing, incoming)


def ingest_raster(path: Path, source: Source, root: Path, held: dict[str, set[str]]):
    """Pool one raster onto the lattice and fold it into the archive's tiles.

    Returns the tiles it wrote and the fraction of the source raster that was void.
    """

    values, src_transform, src_crs, bounds, voided = read_source(path)
    check_world(bounds, str(path))
    window = covering_window(bounds, pad=1)
    pooled = to_int16(pool_onto_lattice(values, src_transform, src_crs, window))
    touched = []
    for ti, tj in window.tiles():
        tile = cut_tile(window, pooled, ti, tj)
        if tile is None or not (tile != NODATA).any():
            continue
        out = tile_path(root, ti, tj)
        contributors = held.setdefault(tile_id(ti, tj), set())
        if out.exists():
            with open_raster(out) as src:
                tile = merge_tiles(src.read(1), tile, contributors, source.key)
        write_tile(out, ti, tj, tile)
        contributors.add(source.key)
        touched.append(tile_id(ti, tj))
    return touched, voided


def local_rasters(directory: Path, bbox) -> list[Path]:
    """Hand-fetched rasters from `--input` that touch the box, in any CRS."""

    paths = sorted(p for p in directory.rglob("*") if p.suffix.lower() in {".tif", ".tiff"})
    if not paths:
        raise Refuse(f"{directory}: no .tif files")
    keep = []
    for path in paths:
        with open_raster(path) as src:
            if src.crs is None:
                raise Refuse(f"{path}: the raster has no CRS, so it cannot be placed")
            west, south, east, north = transform_bounds(src.crs, WGS84, *src.bounds)
        if not (east < bbox[0] or west > bbox[2] or north < bbox[1] or south > bbox[3]):
            keep.append(path)
    return keep


# ── manifests and the index ─────────────────────────────────────────────────


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
    tiles, digests = {}, {}
    for tile in sorted(held):
        ti, tj = (int(part) for part in tile.split("/"))
        path = tile_path(root, ti, tj)
        if not path.is_file():
            raise Refuse(f"a source manifest names tile {tile}, which is not in the archive")
        tiles[tile] = best_contributor(held[tile])
        digests[tile] = tile_digest(path)
    index = {
        "schema": 1,
        "step_log2": 6,
        "tile_log2": 16,
        "sources": {
            key: {
                "product": manifests[key]["product"],
                "attribution": manifests[key]["attribution"],
                "licence": manifests[key]["licence"],
                "vertical_datum": manifests[key]["vertical_datum"],
                "fetched": manifests[key]["fetched"],
            }
            for key in sorted(manifests)
            if any(key in keys for keys in held.values())
        },
        "tiles": tiles,
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


# ── rclone ──────────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class Remote:
    """The archive's place on R2, and the child environment that defines it."""

    path: str
    env: dict[str, str]


def r2_remote() -> Remote:
    """The `RCLONE_CONFIG_OBCR2_*` remote, from the `OBC_R2_*` variables of `obc.local`.

    The secret reaches rclone through the child's environment only: it is never an
    argument, because argv is readable by every process on the box.
    """

    def need(name: str) -> str:
        value = os.environ.get(name)
        if not value:
            raise Refuse(f"{name} is not set; source tools/obc.local, which holds the R2 credential")
        return value

    endpoint = os.environ.get("OBC_R2_ENDPOINT") or f"https://{need('OBC_R2_ACCOUNT_ID')}.r2.cloudflarestorage.com"
    bucket = need("OBC_R2_BUCKET")
    prefix = os.environ.get("OBC_R2_PREFIX", "").strip("/")
    env = {
        "RCLONE_CONFIG_OBCR2_TYPE": "s3",
        "RCLONE_CONFIG_OBCR2_PROVIDER": "Cloudflare",
        "RCLONE_CONFIG_OBCR2_REGION": "auto",
        "RCLONE_CONFIG_OBCR2_ENDPOINT": endpoint,
        "RCLONE_CONFIG_OBCR2_ACCESS_KEY_ID": need("OBC_R2_ACCESS_KEY_ID"),
        "RCLONE_CONFIG_OBCR2_SECRET_ACCESS_KEY": need("OBC_R2_SECRET_ACCESS_KEY"),
        "RCLONE_CONFIG_OBCR2_NO_CHECK_BUCKET": "true",
    }
    key_root = "/".join(part for part in (bucket, prefix, ARCHIVE_PREFIX) if part)
    return Remote(f"OBCR2:{key_root}", env)


def run_rclone(argv: list[str], env: dict[str, str], capture: bool = False) -> str:
    """Spawn rclone with the remote in its environment. The one seam the tests replace."""

    try:
        done = subprocess.run(["rclone", *argv], env={**os.environ, **env}, check=True,
                              capture_output=capture, text=capture)
        return done.stdout if capture else ""
    except FileNotFoundError as exc:
        raise Refuse("rclone is not on PATH — the publish and mirror steps need it "
                     "(https://rclone.org/install/)") from exc
    except subprocess.CalledProcessError as exc:
        raise Refuse(f"rclone {argv[0]} failed with status {exc.returncode}") from exc


def publish_plan(root: Path, remote: Remote, staging: Path) -> list[list[str]]:
    """The four rclone calls of a publish, in order: upload, ask, fetch, publish.

    Every transfer is `copy`, so a publish only ever adds: an archive of one box cannot
    delete another region's tiles. The index is the only file a consumer reads before it
    knows what exists, so it goes last, and it goes up as the merge of what is already on
    R2 with this archive. The `lsf` call is how a first publish is told from a later one,
    so an empty bucket needs no failed download. `--checksum` makes a re-run idempotent: a
    tile whose bytes are already there is skipped whatever its timestamp says.
    """

    return [
        ["copy", str(root), remote.path, "--checksum", "--exclude", "/index.json"],
        ["lsf", remote.path, "--include", "index.json"],
        ["copy", remote.path, str(staging), "--include", "/index.json"],
        ["copy", str(staging / "index.json"), remote.path, "--checksum"],
    ]


def merge_index(published: dict | None, local: dict) -> dict:
    """The index to publish: what is on R2, plus this archive, this archive winning.

    A publish adds tiles, so the index it uploads must describe every tile on R2 and not
    only the box that was ingested last. Where both hold a tile, this archive wins: its
    bytes are the ones the first call just uploaded.
    """

    if not published:
        return local
    if (published.get("schema"), published.get("step_log2"), published.get("tile_log2")) != (1, 6, 16):
        raise Refuse("the index on R2 is not this contract, so publish refuses to merge with it")
    tiles = dict(sorted({**published.get("tiles", {}), **local.get("tiles", {})}.items()))
    digests = dict(sorted({**published.get("sha256", {}), **local.get("sha256", {})}.items()))
    sources = {**published.get("sources", {}), **local.get("sources", {})}
    held = set(tiles.values())
    return {
        **local,
        "sources": {key: sources[key] for key in sorted(sources) if key in held},
        "tiles": tiles,
        "sha256": digests,
    }


def mirror_plan(root: Path, remote: Remote, listing: Path) -> list[str]:
    return ["copy", remote.path, str(root), "--files-from", str(listing), "--checksum"]


def box_tiles(bbox, halo: int = 1) -> list[str]:
    """The tiles a box needs, with a ring of `halo` tiles around it.

    The baker reads a cell's nodes plus a two-node halo, and a node at the edge of a cell
    takes its pixels from the tile next door, so the box alone is one tile short on every
    side.
    """

    window = covering_window(bbox)
    rows = range(max(tile_index(window.row0) - halo, 0), tile_index(window.row0 + window.rows - 1) + halo + 1)
    cols = range(max(tile_index(window.col0) - halo, 0), tile_index(window.col0 + window.cols - 1) + halo + 1)
    return [tile_id(ti, tj) for ti in rows for tj in cols]


# ── commands ────────────────────────────────────────────────────────────────


def command_ingest(args) -> int:
    bbox = parse_bbox(args.bbox)
    check_world(bbox, "--bbox")
    if args.source not in SOURCES:
        raise Refuse(f"unknown source `{args.source}`; this tool implements {', '.join(sorted(SOURCES))}")
    source = SOURCES[args.source]
    if not source.covers(bbox):
        print(f"warning: {args.bbox} looks outside {source.country}", file=sys.stderr)
    root = Path(args.archive)
    if args.input:
        rasters = local_rasters(Path(args.input), bbox)
    else:
        work = Path(args.work) if args.work else Path(tempfile.gettempdir()) / f"obc-reference-{source.key}"
        rasters = source.fetch(bbox, work)
    if not rasters:
        raise Refuse("no rasters cover that box")

    manifests = load_manifests(root)
    held = contributors(manifests)
    touched: set[str] = set()
    for i, path in enumerate(rasters, 1):
        written, voided = ingest_raster(path, source, root, held)
        touched.update(written)
        print(f"  [{i}/{len(rasters)}] {path.name}: {len(written)} tile(s), {voided:.1%} void")

    mine = sorted(tile for tile, keys in held.items() if source.key in keys)
    write_manifest(root, {
        "key": source.key,
        "country": source.country,
        "product": source.product,
        "resolution_m": source.resolution_m,
        "licence": source.licence,
        "attribution": source.attribution,
        "vertical_datum": source.vertical_datum,
        "fetched": datetime.now(timezone.utc).date().isoformat(),
        "tiles": mine,
    })
    index = rebuild_index(root)
    total = sum(tile_path(root, *(int(p) for p in tile.split("/"))).stat().st_size for tile in index["tiles"])
    print(f"{root}: {len(index['tiles'])} tile(s), {total} bytes, {len(touched)} written this run")
    print(f"Attribution: {source.attribution} ({source.licence})")
    return 0


def command_index(args) -> int:
    index = rebuild_index(Path(args.archive))
    print(f"index.json: {len(index['tiles'])} tile(s) from {', '.join(sorted(index['sources']))}")
    return 0


def command_check(args) -> int:
    """Open every tile and hold it against the contract, then against the index."""

    root = Path(args.archive)
    index = read_index(root)
    problems = []
    if (index.get("schema"), index.get("step_log2"), index.get("tile_log2")) != (1, 6, 16):
        problems.append("index.json: schema 1, step_log2 6 and tile_log2 16 are the contract")
    for tile, key in sorted(index.get("tiles", {}).items()):
        ti, tj = (int(part) for part in tile.split("/"))
        path = tile_path(root, ti, tj)
        if key not in index.get("sources", {}):
            problems.append(f"{tile}: source `{key}` is not in the index's sources")
        if not path.is_file():
            problems.append(f"{tile}: the index names it, but {path} is missing")
            continue
        problems.extend(tile_problems(path, ti, tj))
        if tile_digest(path) != index.get("sha256", {}).get(tile):
            problems.append(f"{tile}: the sha256 in the index is not this tile's pixels")
    for path in sorted((root / "16").rglob("*.tif")) if (root / "16").is_dir() else []:
        try:
            ti, tj = int(path.parent.name), int(path.stem)
        except ValueError:
            problems.append(f"{path}: not a tile id; a tile is 16/<ti:04>/<tj:04>.tif")
            continue
        if tile_path(root, ti, tj) != path:
            problems.append(f"{path}: a tile id is zero padded to four digits")
        elif tile_id(ti, tj) not in index.get("tiles", {}):
            problems.append(f"{path}: not in the index, so no consumer can see it")
    for problem in problems:
        print(f"check: {problem}", file=sys.stderr)
    if problems:
        return 1
    print(f"{root}: {len(index['tiles'])} tile(s) hold the contract")
    return 0


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


def command_publish(args) -> int:
    root = Path(args.archive)
    local = read_index(root)
    remote = r2_remote()
    with tempfile.TemporaryDirectory() as directory:
        staging = Path(directory)
        upload, listing, fetch, publish = publish_plan(root, remote, staging)
        print(f"  rclone {' '.join(upload)}")
        run_rclone(upload, remote.env)
        if run_rclone(listing, remote.env, capture=True).strip():
            print(f"  rclone {' '.join(fetch)}")
            run_rclone(fetch, remote.env)
        published = staging / "index.json"
        already = json.loads(published.read_text(encoding="utf-8")) if published.is_file() else None
        merged = merge_index(already, local)
        published.write_text(
            json.dumps(merged, indent=2, sort_keys=True, ensure_ascii=False) + "\n", encoding="utf-8"
        )
        print(f"  rclone {' '.join(publish)}")
        run_rclone(publish, remote.env)
    print(f"published {len(local['tiles'])} tile(s) to {remote.path}; the index names {len(merged['tiles'])}")
    return 0


def command_mirror(args) -> int:
    """Copy the tiles a box needs out of R2, for a bakery run that is about to start."""

    root = Path(args.archive)
    bbox = parse_bbox(args.bbox)
    check_world(bbox, "--bbox")
    remote = r2_remote()
    root.mkdir(parents=True, exist_ok=True)
    run_rclone(["copyto", f"{remote.path}/index.json", str(root / "index.json")], remote.env)
    index = read_index(root)
    needed = box_tiles(bbox)
    wanted = [tile for tile in needed if tile in index.get("tiles", {})]
    if not wanted:
        print(f"{args.bbox}: the archive holds no tile for that box")
        return 0
    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as handle:
        handle.write("".join(f"16/{tile}.tif\n" for tile in wanted))
        listing = Path(handle.name)
    try:
        run_rclone(mirror_plan(root, remote, listing), remote.env)
    finally:
        listing.unlink(missing_ok=True)
    print(f"mirrored {len(wanted)} of the {len(needed)} tile(s) the box and its halo need into {root}; "
          f"the archive does not hold {len(needed) - len(wanted)}")
    return 0


def parse_bbox(text: str):
    try:
        values = tuple(float(part) for part in text.split(","))
    except ValueError as exc:
        raise Refuse("--bbox is min_lon,min_lat,max_lon,max_lat") from exc
    if len(values) != 4:
        raise Refuse("--bbox is min_lon,min_lat,max_lon,max_lat")
    return values


def glue_bbox(argv: list[str]) -> list[str]:
    """A western longitude starts with a minus, which argparse reads as the next option."""

    out, rest = [], iter(argv)
    for word in rest:
        out.append(f"--bbox={next(rest, '')}" if word == "--bbox" else word)
    return out


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)

    def with_archive(name, help_text):
        sub = commands.add_parser(name, help=help_text)
        sub.add_argument("--archive", required=True, help="the archive root")
        return sub

    ingest = with_archive("ingest", "warp a source's rasters onto the lattice and write tiles")
    ingest.add_argument("source", help=f"source key: {', '.join(sorted(SOURCES))}")
    ingest.add_argument("--bbox", required=True, help="min_lon,min_lat,max_lon,max_lat")
    ingest.add_argument("--input", help="a directory of hand-fetched rasters, instead of the service")
    ingest.add_argument("--work", help="where fetched rasters are cached (default: the system temp dir)")
    ingest.set_defaults(run=command_ingest)

    with_archive("index", "rebuild index.json from the source manifests").set_defaults(run=command_index)
    with_archive("check", "hold every tile against the contract").set_defaults(run=command_check)
    with_archive("publish", "rclone sync the archive to R2, index.json last").set_defaults(run=command_publish)

    mirror = with_archive("mirror", "copy the tiles a box needs from R2")
    mirror.add_argument("--bbox", required=True, help="min_lon,min_lat,max_lon,max_lat")
    mirror.set_defaults(run=command_mirror)

    args = parser.parse_args(glue_bbox(list(sys.argv[1:] if argv is None else argv)))
    try:
        return args.run(args)
    except Refuse as refusal:
        print(f"ingest: {refusal}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
