#!/usr/bin/env python3
"""Build Ride Assistant maps from verified fixture packages, without network access."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
from tools.fixtures import Catalog, FixtureError, Store, build_package, cache_root


def build(region: str, output: Path) -> None:
    catalog = Catalog(ROOT / "fixtures/catalog.toml")
    store = Store(catalog, cache_root())
    # This verifies the raw bytes and tracked recipes before starting expensive tools.
    for package in ("assistant-osm", "assistant-terrain", "assistant-replays"):
        try:
            store.verify(package)
        except (FixtureError, OSError) as error:
            raise FixtureError(f"{error}; run tools/obc fixtures sync assistant-inputs, then retry") from error
    boundaries = json.loads((ROOT / "fixtures/sources/ride-assistant/regions.geojson").read_text())
    polygon = next(f["geometry"]["coordinates"][0] for f in boundaries["features"] if f["properties"]["id"] == region)
    west, south = polygon[0]
    east, north = polygon[2]
    package = "sim-assistant-" + region
    stage = output / package
    if stage.exists():
        raise FixtureError(f"output already exists: {stage}; use a new OBC_FIXTURE_BUILD_DIR")
    stage.mkdir(parents=True)
    source = store.package_root("assistant-osm") / ("switzerland.osm.pbf" if region == "meiringen" else "west-cork.osm.pbf")
    subprocess.run(["cargo", "build", "--offline", "--release", "-p", "obc-dem", "-p", "obc-pack"], cwd=ROOT, check=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    if not target.is_absolute():
        target = ROOT / target
    terrain = stage / (region + ".obcd")
    subprocess.run([str(target / "release/obc-dem"), "bake", "--sources", str(store.package_root("assistant-terrain")), "--bbox", f"{south},{west},{north},{east}", "--cell-log2", "16", "--shard", str(terrain), "--quiet"], check=True)
    subprocess.run([str(target / "release/obc-pack"), str(source), str(ROOT / "builder/presets/schema.json"), str(stage / (region + ".obcm")), "--bbox", f"{west},{south},{east},{north}", "--terrain", str(terrain)], check=True)
    replay_root = store.package_root("assistant-replays")
    replays = json.loads((replay_root / "manifest.json").read_text())
    (stage / "replays").mkdir()
    for replay in replays["replays"]:
        if replay["region"] == region:
            shutil.copyfile(replay_root / replay["gpx"], stage / replay["gpx"])
    recipe = {"schema": 1, "region": region, "source_packages": {p: catalog.packages[p]["sha256"] for p in ("assistant-osm", "assistant-terrain", "assistant-replays")}, "source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()}
    (stage / "build.json").write_text(json.dumps(recipe, indent=2) + "\n")
    size, digest = build_package(package, stage, output / (package + ".tar.gz"))
    print(f"{package}: bytes={size} sha256={digest}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("region", choices=("meiringen", "west-cork", "all"))
    args = parser.parse_args()
    output = Path(os.environ.get("OBC_FIXTURE_BUILD_DIR", ROOT / "fixtures/build/maps")).resolve()
    try:
        for region in ("meiringen", "west-cork") if args.region == "all" else (args.region,):
            build(region, output)
    except (FixtureError, OSError, subprocess.CalledProcessError) as error:
        print(f"assistant fixture: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
