#!/usr/bin/env python3
"""Refuse a return to large, scattered fixture blobs in Git."""

from __future__ import annotations

from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parent.parent
LIMIT = 256 * 1024
ALLOWED = {
    # Shipped wasm demo payload, not a developer fixture.
    "apps/obc-sim/assets/grimsel-demo.obcm",
    # Skin-preview product input/golden, owned and rendered by obc-bake.
    "host/obc-bake/assets/teningen-preview.obcm",
    # Authored source rides whose textual diffs remain reviewable.
    "fixtures/sources/sim-grimsel/tracks/grimsel-climb.gpx",
    "companion-ios/Packages/OBCKit/Sources/OBCMock/Fixtures/website-import.gpx",
}
FIXTURE_SUFFIXES = (
    ".obcm",
    ".obcd",
    ".grib2",
    ".grib2.gz",
    ".grib2.bz2",
    ".tar",
    ".tar.gz",
    ".osm.pbf",
    ".gpx",
)


def tracked_files() -> list[str]:
    output = subprocess.check_output(["git", "ls-files", "-z"], cwd=ROOT)
    return [item.decode() for item in output.split(b"\0") if item]


def main() -> int:
    violations = []
    for relative in tracked_files():
        if relative in ALLOWED or not relative.endswith(FIXTURE_SUFFIXES):
            continue
        path = ROOT / relative
        if path.is_file() and path.stat().st_size > LIMIT:
            violations.append((relative, path.stat().st_size))
    if violations:
        print("large generated/captured fixture data belongs in fixtures/catalog.toml:", file=sys.stderr)
        for relative, size in violations:
            print(f"  {size:>10}  {relative}", file=sys.stderr)
        return 1
    print("fixture policy: no unregistered large blobs")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
