"""Turn a national DTM into reference archive tiles, and move the archive to and from R2.

The archive is one format: max-pooled bare-earth height on the OBCT lattice at 2^6 µdeg,
`int16` metres, one GeoTIFF per 2^16 µdeg tile. Every country branch is an adapter in
`ingest/sources/`, and the tool runs once per source release. The baker knows the tiles
only, and `README.md` beside this file holds the contract both sides read.

    python3 ingest.py ingest ch --bbox 8.30,46.75,8.60,46.95 --archive ref/
    python3 ingest.py check --archive ref/
    python3 ingest.py publish --archive ref/

A source adapter has one job: give the shared tail rasters that cover the box, in any CRS
and any dtype. The tail pools each one onto the lattice by the contract's rule — a lattice
pixel keeps the maximum of the source pixels whose centres lie in it — so a rock tower one
source pixel wide survives the 7 m step, and writes whole tiles.
"""

import argparse
import json
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

from . import publish, wizard
from .archive import (contributors, ingest_raster, load_manifests, local_rasters, rebuild_index,
                      read_index, tile_path, tile_problems, tile_digest, write_manifest)
from .lattice import Refuse, box_tiles, check_world, tile_id
from .pool import open_raster
from .sources import SOURCES


def registered(key: str):
    if key not in SOURCES:
        raise Refuse(f"unknown source `{key}`; this tool implements {', '.join(sorted(SOURCES))}")
    return SOURCES[key]


def source_step(path: Path) -> str:
    """The step one delivered raster is on, in the units of its own CRS."""

    with open_raster(path) as src:
        step = abs(src.transform.a)
        unit = "°" if src.crs and src.crs.is_geographic else "m"
    return f"{step:.3g} {unit}"


def require_datum(source, given) -> None:
    """Refuse a delivery whose datum nobody has read.

    One portal publishes orthometric and ellipsoidal products side by side, and the two
    stand tens of metres apart, which is the size of a lift. No row can tell which an
    order held, so the owner reads the order's metadata and says so.
    """

    if source.confirm_datum is None or given == source.confirm_datum:
        return
    raise Refuse(
        f"{source.key} delivers more than one vertical datum, and the archive is "
        f"orthometric metres on {source.vertical_datum}. Read the order's metadata and "
        f"pass --datum {source.confirm_datum} when it says so; an order on an ellipsoidal "
        f"height has to be converted before the ingest, which nothing here does"
    )


def command_ingest(args) -> int:
    bbox = parse_bbox(args.bbox)
    check_world(bbox, "--bbox")
    source = registered(args.source)
    if not source.covers(bbox):
        print(f"warning: {args.bbox} looks outside {source.country}", file=sys.stderr)
    root = Path(args.archive)
    work = Path(args.work) if args.work else Path(tempfile.gettempdir()) / f"obc-reference-{source.key}"
    if args.input:
        require_datum(source, getattr(args, "datum", None))
        rasters = local_rasters(source, Path(args.input), bbox, work)
    else:
        source.require_credential()
        rasters = source.fetch(bbox, work)
    if not rasters:
        raise Refuse("no rasters cover that box")

    manifests = load_manifests(root)
    held = contributors(manifests)
    touched: set[str] = set()
    outside = 0
    for i, path in enumerate(rasters, 1):
        written, voided, dropped = ingest_raster(path, source, root, held)
        touched.update(written)
        outside += dropped
        # A row that states no step gets the delivered one printed, because the step of
        # an order is a fact about the delivery and not about the row.
        step = "" if source.resolution_m else f", {source_step(path)}"
        print(f"  [{i}/{len(rasters)}] {path.name}: {len(written)} tile(s), "
              f"{voided:.1%} void{step}")

    fetched = datetime.now(timezone.utc).date().isoformat()
    mine = sorted(tile for tile, keys in held.items() if source.key in keys)
    write_manifest(root, {
        "key": source.key,
        "country": source.country,
        "product": source.product,
        "resolution_m": source.resolution_m,
        "licence": source.licence,
        "attribution": source.credit(fetched),
        "vertical_datum": source.vertical_datum,
        "fetched": fetched,
        "tiles": mine,
    })
    for key, manifest in manifests.items():
        if key == source.key:
            continue
        kept = [tile for tile in manifest.get("tiles", []) if key in held.get(tile, set())]
        if kept != manifest.get("tiles", []):
            write_manifest(root, {**manifest, "tiles": kept})
    index = rebuild_index(root)
    total = sum(tile_path(root, *(int(p) for p in tile.split("/"))).stat().st_size for tile in index["tiles"])
    print(f"{root}: {len(index['tiles'])} tile(s), {total} bytes, {len(touched)} written this run")
    print(f"  {outside} source pixel centre(s) fell outside their lattice window")
    print(f"Attribution: {source.credit(fetched)} ({source.licence})")
    return 0


def command_wizard(args, ask=input, say=print) -> int:
    """Walk one portal's download, then ingest what it delivered."""

    source = registered(args.source)
    if not source.steps:
        raise Refuse(f"{source.key} needs no account: run `ingest {source.key}` directly")
    if not wizard.walk(source, ask, say):
        return 1
    if wizard.take_credential(source, ask, say):
        args.input = None
        return command_ingest(args)
    directory = wizard.input_directory(source, args.input, ask, say)
    if directory is None:
        return 1
    args.datum = wizard.confirmed_datum(source, ask, say)
    if source.confirm_datum and args.datum is None:
        return 1
    args.input = directory
    return command_ingest(args)


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
        credits = index.get("contributors", {}).get(tile, [])
        if key not in index.get("sources", {}):
            problems.append(f"{tile}: source `{key}` is not in the index's sources")
        if not credits or credits[0] != key:
            problems.append(f"{tile}: contributors {credits} do not start with the tile's source `{key}`")
        for other in credits:
            if other not in index.get("sources", {}):
                problems.append(f"{tile}: contributor `{other}` is not in the index's sources")
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



def command_publish(args) -> int:
    root = Path(args.archive)
    local = read_index(root)
    remote = publish.r2_remote()
    with tempfile.TemporaryDirectory() as directory:
        staging = Path(directory)
        upload, listing, fetch, send = publish.publish_plan(root, remote, staging)
        print(f"  rclone {' '.join(upload)}")
        publish.run_rclone(upload, remote.env)
        if publish.run_rclone(listing, remote.env, capture=True).strip():
            print(f"  rclone {' '.join(fetch)}")
            publish.run_rclone(fetch, remote.env)
        published = staging / "index.json"
        already = json.loads(published.read_text(encoding="utf-8")) if published.is_file() else None
        merged = publish.merge_index(already, local)
        published.write_text(
            json.dumps(merged, indent=2, sort_keys=True, ensure_ascii=False) + "\n", encoding="utf-8"
        )
        print(f"  rclone {' '.join(send)}")
        publish.run_rclone(send, remote.env)
    print(f"published {len(local['tiles'])} tile(s) to {remote.path}; the index names {len(merged['tiles'])}")
    return 0


def command_mirror(args) -> int:
    """Copy the tiles a box needs out of R2, for a bakery run that is about to start."""

    root = Path(args.archive)
    bbox = parse_bbox(args.bbox)
    check_world(bbox, "--bbox")
    remote = publish.r2_remote()
    root.mkdir(parents=True, exist_ok=True)
    publish.run_rclone(["copyto", f"{remote.path}/index.json", str(root / "index.json")], remote.env)
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
        publish.run_rclone(publish.mirror_plan(root, remote, listing), remote.env)
    finally:
        listing.unlink(missing_ok=True)
    missing = [tile for tile in needed if tile not in index.get("tiles", {})]
    if missing:
        shown = ", ".join(missing[:20])
        more = f", and {len(missing) - 20} more" if len(missing) > 20 else ""
        print(f"  the archive does not hold {len(missing)} of the {len(needed)} tile(s) the box and "
              f"its halo need: {shown}{more}")
    print(f"mirrored {len(wanted)} tile(s) into {root}")
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

    def for_source(name, help_text, run):
        sub = with_archive(name, help_text)
        sub.add_argument("source", help=f"source key: {', '.join(sorted(SOURCES))}")
        sub.add_argument("--bbox", required=True, help="min_lon,min_lat,max_lon,max_lat")
        sub.add_argument("--input", help="a directory of hand-fetched files, instead of the service")
        sub.add_argument("--work", help="where fetched rasters are cached (default: the system temp dir)")
        sub.add_argument("--datum", help="the vertical datum the delivery states, for a source "
                                         "whose portal publishes more than one")
        sub.set_defaults(run=run)

    for_source("ingest", "warp a source's rasters onto the lattice and write tiles", command_ingest)
    for_source("wizard", "walk the account and download steps of a source behind a login",
               command_wizard)

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

