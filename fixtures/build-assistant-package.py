#!/usr/bin/env python3
"""Bake and package regional Ride Assistant maps from verified cached inputs."""
from __future__ import annotations

import argparse
from datetime import datetime
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
from tools.fixtures import Catalog, FixtureError, Store, build_package, cache_root, sha256_file

REGIONS = {"meiringen": "europe/switzerland", "west-cork": "europe/ireland/west-cork"}
# The one OBCM version literal in the tree is the format crate's constant.
OBCM_VERSION = int(re.search(r"pub const VERSION: u8 = (\d+);",
                             (ROOT / "firmware/obc-formats/src/obcm.rs").read_text())[1])


def input_packages(region: str) -> tuple[str, ...]:
    content = "assistant-switzerland-content" if region == "meiringen" else "assistant-wiki"
    packages = ("assistant-osm", "assistant-terrain", "assistant-replays", content)
    return packages + (("peak-content",) if region == "meiringen" else ())


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, indent=2) + "\n")


def checked(path: Path, digest: str) -> Path:
    if sha256_file(path) != digest:
        raise FixtureError(f"source hash mismatch: {path}")
    return path


def assembly_inputs(tree: Path, region_id: str, native: Path, out: Path) -> None:
    """Translate the normal local catalog selection to the native driver's sidecars."""
    catalog = json.loads((tree / "catalog.json").read_text())
    region = next(r for r in catalog["regions"] if r["id"] == region_id)
    selection_path = tree / "regions" / region_id / "cells.json"
    selection = json.loads(checked(selection_path, region["cells_sha256"]).read_text())
    cells = []
    for index in catalog["cell_index"]:
        band = index["band"]
        index_path = tree / "cells" / band / "index.json"
        entries = json.loads(checked(index_path, index["sha256"]).read_text())["cells"]
        by_id = {entry["id"]: entry for entry in entries}
        for cell_id in selection["cells"].get(band, []):
            entry = by_id[cell_id]
            _, i, j = cell_id.split("/")
            path = checked(tree / "cells" / band / i / (j + ".obcm"), entry["sha256"])
            cells.append({"id": cell_id, "band": band, "path": str(path), "bytes": path.stat().st_size,
                          "sha256": entry["sha256"], "partial": bool(entry.get("partial", False))})
    out.mkdir(parents=True, exist_ok=True)
    write_json(out / "cells.json", {"schema": catalog["schema"], "cells": cells})
    write_json(out / "skin.json", next(s for s in catalog["skins"] if s["id"] == "default"))
    terrain = []
    for path in sorted(native.glob("*.obcd")):
        scale, i, j = path.stem.split("_")
        terrain.append({"id": f"{scale}/{int(i):04d}/{int(j):04d}", "path": str(path), "sha256": sha256_file(path)})
    if not terrain:
        raise FixtureError("native terrain bake produced no cells")
    write_json(out / "native-terrain.json", {"posting_log2": 9, "cell_log2": 19, "cells": terrain})


def bake(region: str, work: Path, store: Store, bin_dir: Path, landmarks: Path | None, peaks: Path | None) -> tuple[Path, dict]:
    work.mkdir(parents=True)
    commands = []
    recipe_commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    executables = {name: {"sha256": sha256_file(bin_dir / name)}
                   for name in ("obc-bake", "obc-dem", "obcm-assemble")}

    def run(*args: object) -> None:
        argv = [str(arg) for arg in args]
        commands.append(argv)
        subprocess.run(argv, cwd=ROOT, check=True)

    boundaries = json.loads((ROOT / "fixtures/sources/ride-assistant/regions.geojson").read_text())
    polygon = next(f["geometry"]["coordinates"][0] for f in boundaries["features"] if f["properties"]["id"] == region)
    if region == "meiringen":
        # Map selection bounds differ from the wider acquisition/replay boundary.
        polygon = [[8.1, 46.5], [8.4, 46.5], [8.4, 46.8], [8.1, 46.8], [8.1, 46.5]]
    west, south = polygon[0]
    east, north = polygon[2]
    region_id = REGIONS[region]
    local_source = work / "input"
    local_source.mkdir()
    prefix = region_id.replace("/", "_")
    source = store.package_root("assistant-osm") / ("switzerland.osm.pbf" if region == "meiringen" else "west-cork.osm.pbf")
    pbf = local_source / (prefix + "-latest.osm.pbf")
    shutil.copyfile(source, pbf)
    manifest = json.loads((store.package_root("assistant-osm") / "manifest.json").read_text())
    source_timestamp = next(s["source_timestamp"] for s in manifest["sources"] if s["path"] == source.name)
    epoch = datetime.fromisoformat(source_timestamp.replace("Z", "+00:00")).timestamp()
    os.utime(pbf, (epoch, epoch))
    (local_source / (prefix + ".poly")).write_text(region + " validation crop\n1\n" + "".join(f" {lon} {lat}\n" for lon, lat in polygon) + "END\nEND\n")
    regions = work / "regions.toml"
    regions.write_text(f'[[regions]]\nid = "{region_id}"\nname = "{region} validation crop"\n')
    if landmarks is None and region == "meiringen":
        landmarks = store.package_root("assistant-switzerland-content") / "content.json"
    if landmarks is None:
        wiki = store.package_root("assistant-wiki")
        run(bin_dir / "obc-bake", "landmarks", "--snapshot", wiki / "manifest.json", "--boundary", wiki / "regions.geojson", "--out", work / "landmarks")
        landmarks = work / "landmarks/content.json"
    if peaks is None and region == "meiringen":
        peaks = store.package_root("peak-content") / "peaks.json"
    peak_args = ["--peaks", peaks] if peaks else []
    tree = work / "tree"
    run(bin_dir / "obc-bake", "bake", region_id, "--regions", regions, "--source", local_source,
        "--dem-sources", store.package_root("assistant-terrain"), "--landmarks", landmarks, *peak_args,
        "--presets-dir", ROOT / "builder/presets", "--skin", "default", "--out", tree, "--base-url", "http://localhost/assistant",
        "--generated-at", "2026-09-15T00:00:00Z", "--summary-json", work / "summary.json", "--fail-fast")
    native = work / "native-terrain"
    run(bin_dir / "obc-dem", "bake", "--sources", store.package_root("assistant-terrain"),
        "--bbox", f"{south},{west},{north},{east}", "--posting-log2", "9", "--cell-log2", "19", "--out", native)
    assembled = work / "assembled"
    assembly_inputs(tree, region_id, native, assembled)
    result = assembled / (region + ".obcm")
    run(bin_dir / "obcm-assemble", "--cells", assembled / "cells.json", "--skin", assembled / "skin.json",
        "--terrain", assembled / "native-terrain.json", "--out", result, "--accept-partial", "--json")
    content = json.loads(landmarks.read_text())
    return result, {"recipe_commit": recipe_commit,
                    "executables": executables, "commands": commands, "bounds_lon_lat": [west, south, east, north],
                    "content_manifest_sha256": sha256_file(landmarks), "content_counts": content["counts"],
                    "source_coverage": content["source_coverage"],
                    "peak_content_sha256": sha256_file(peaks) if peaks else None, "summary": json.loads((work / "summary.json").read_text())}


def package(region: str, output: Path, map_path: Path, provenance: dict, catalog: Catalog, store: Store) -> None:
    with map_path.open("rb") as source:
        header = source.read(5)
    if len(header) != 5 or header[:4] != b"OBCM" or header[4] != OBCM_VERSION:
        raise FixtureError(f"the scenario requires an OBCM v{OBCM_VERSION} map")
    digest = sha256_file(map_path)
    expected = provenance.get("map")
    if expected and (expected["sha256"] != digest or expected["bytes"] != map_path.stat().st_size):
        raise FixtureError("completed map does not match its retained provenance")
    package_id = "sim-assistant-" + region
    stage = output / package_id
    stage.mkdir(parents=True)
    shutil.copyfile(map_path, stage / (region + ".obcm"))
    replay_root = store.package_root("assistant-replays")
    replays = json.loads((replay_root / "manifest.json").read_text())
    (stage / "replays").mkdir()
    for replay in replays["replays"]:
        if replay["region"] == region:
            shutil.copyfile(replay_root / replay["gpx"], stage / replay["gpx"])
    write_json(stage / "build.json", {"schema": 2, "region": region, "coverage": "regional crop",
        "source_packages": {p: catalog.packages[p]["sha256"] for p in input_packages(region)}, "provenance": provenance,
        "map": {"bytes": map_path.stat().st_size, "sha256": digest, "obcm_version": OBCM_VERSION}})
    size, digest = build_package(package_id, stage, output / (package_id + ".tar.gz"))
    print(f"{package_id}: bytes={size} sha256={digest}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("region", choices=(*REGIONS, "all"))
    parser.add_argument("--bin-dir", type=Path, default=ROOT / "target/release", help="prebuilt shipping host tools; no implicit Cargo build")
    parser.add_argument("--landmarks", type=Path, help="optional compiled content.json from the normal compiler")
    parser.add_argument("--peaks", type=Path, help="optional compiled peaks.json; Meiringen defaults to verified peak-content")
    parser.add_argument("--assembled-map", type=Path, help="package an already completed shipping bake without repeating it")
    parser.add_argument("--provenance", type=Path, help="retained build evidence JSON, required with --assembled-map")
    args = parser.parse_args()
    if bool(args.assembled_map) != bool(args.provenance) or (args.assembled_map and args.region == "all"):
        parser.error("--assembled-map and --provenance require each other and one region")
    output = Path(os.environ.get("OBC_FIXTURE_BUILD_DIR", ROOT / "fixtures/build/maps")).resolve()
    try:
        catalog = Catalog(ROOT / "fixtures/catalog.toml")
        store = Store(catalog, cache_root())
        regions = REGIONS if args.region == "all" else (args.region,)
        for package_id in sorted({p for region in regions for p in input_packages(region)}):
            store.verify(package_id)
        for region in regions:
            if args.assembled_map:
                result, provenance = args.assembled_map.resolve(), json.loads(args.provenance.read_text())
            else:
                result, provenance = bake(region, output / (region + "-work"), store, args.bin_dir.resolve(), args.landmarks.resolve() if args.landmarks else None, args.peaks.resolve() if args.peaks else None)
            package(region, output, result, provenance, catalog, store)
    except (FixtureError, OSError, ValueError, KeyError, StopIteration, subprocess.CalledProcessError) as error:
        print(f"assistant fixture: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
