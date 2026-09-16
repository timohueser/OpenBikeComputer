#!/usr/bin/env python3
"""Compile the bounded Swiss peak sources twice, without network access."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
from tools.fixtures import Catalog, FixtureError, Store, cache_root


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, help="local captured package before publication")
    parser.add_argument("--out", type=Path, help="retain the first compiled catalogue in this empty directory")
    args = parser.parse_args()
    try:
        source = args.source
        if source is None:
            store = Store(Catalog(ROOT / "fixtures/catalog.toml"), cache_root())
            store.verify("peak-wiki")
            source = store.package_root("peak-wiki")
        subprocess.run(["cargo", "build", "--offline", "--locked", "-p", "obc-bake"], cwd=ROOT, check=True)
        target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
        if not target.is_absolute():
            target = ROOT / target
        executable = target / "debug" / ("obc-bake.exe" if os.name == "nt" else "obc-bake")
        with tempfile.TemporaryDirectory(prefix="obc-peak-fixture-") as temporary:
            temporary = Path(temporary)
            discovered = temporary / "summits.json"
            subprocess.run([str(executable), "peak-candidates", "--osm", str(source / "peaks.osm.pbf"), "--boundary", str(source / "boundary.geojson"), "--out", str(discovered)], cwd=ROOT, check=True)
            if discovered.read_bytes() != (source / "summits.json").read_bytes():
                raise FixtureError("captured summit discovery differs from the source PBF")
            outputs = [args.out or temporary / "first", temporary / "second"]
            for output in outputs:
                subprocess.run([str(executable), "peaks", "--snapshot", str(source / "manifest.json"), "--boundary", str(source / "boundary.geojson"), "--out", str(output)], cwd=ROOT, check=True)
            files = [{file.name: file.read_bytes() for file in output.iterdir()} for output in outputs]
            if files[0] != files[1]:
                raise FixtureError("offline peak output differs between builds")
            content = json.loads(files[0]["peaks.json"])
            expected = {"Q136829", "Q1374", "Q15138", "Q16525", "Q2970842", "Q4425", "Q7199902"}
            if content["collection"] != "peaks" or {record["id"] for record in content["records"]} != expected:
                raise FixtureError("captured peak article selection differs")
            counts = content["counts"]
            if (counts["captured"], counts["candidates"], counts["texts"], counts["images"]) != (9, 8, 7, 6):
                raise FixtureError("captured peak counts differ")
            links = {item["node_id"]: item["article_id"] for item in content["associations"]}
            if len(links) != 8 or links.get(26864310) != "Q7199902" or links.get(8736685488) != "Q7199902":
                raise FixtureError("duplicate article lost an OSM summit association")
            if links.get(7165008398) != "Q136829" or links.get(13848863734) != "Q2970842":
                raise FixtureError("direct Wikipedia association differs")
            languages = {variant["language"] for record in content["records"] for variant in record["variants"]}
            if languages != {"en", "de", "fr", "es"}:
                raise FixtureError("supported captured languages were dropped")
            for record in content["records"]:
                if not all(1 <= len(v["text_pages"]) <= 4 for v in record["variants"]):
                    raise FixtureError("invalid peak excerpt bounds")
                photo = record["photo"]
                if photo and len(files[0][photo["path"]]) != 216 * 240:
                    raise FixtureError("invalid shared peak photo bounds")
            omissions = {(o["qid"], o["reason"]) for o in content["omissions"]}
            if not {("Q7199902", "attribution_bytes"), ("osm-node-1244930329", "no_explicit_link")}.issubset(omissions):
                raise FixtureError("captured omissions differ")
            if content["source_coverage"]["country_complete"]:
                raise FixtureError("bounded source claims country coverage")
            print("Verified nine OSM nodes, seven articles, eight associations, six photos and deterministic offline output")
    except (FixtureError, OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f"peak content: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
