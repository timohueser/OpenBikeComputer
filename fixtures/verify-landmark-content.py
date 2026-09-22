#!/usr/bin/env python3
"""Exercise the offline landmark compiler with pinned Swiss and Irish source bytes."""
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
    try:
        store = Store(Catalog(ROOT / "fixtures/catalog.toml"), cache_root())
        store.verify("assistant-wiki")
        source = store.package_root("assistant-wiki")
        subprocess.run(["cargo", "build", "--offline", "--locked", "-p", "obc-bake"], cwd=ROOT, check=True)
        target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
        if not target.is_absolute():
            target = ROOT / target
        executable = target / "debug" / ("obc-bake.exe" if os.name == "nt" else "obc-bake")
        with tempfile.TemporaryDirectory(prefix="obc-landmark-fixture-") as temporary:
            outputs = [Path(temporary) / name for name in ("first", "second")]
            for output in outputs:
                subprocess.run([str(executable), "landmark-content", "--snapshot", str(source / "manifest.json"),
                                "--boundary", str(source / "regions.geojson"),
                                "--out", str(output)], cwd=ROOT, check=True)
            files = [{file.name: file.read_bytes() for file in output.iterdir()} for output in outputs]
            if files[0] != files[1]:
                raise FixtureError("offline landmark output differs between builds")
            content = json.loads(files[0]["content.json"])
            expected = {"Q301191", "Q183395", "Q5315471", "Q666668"}
            if {record["qid"] for record in content["records"]} != expected:
                raise FixtureError("captured Swiss/Irish landmark selection differs")
            for record in content["records"]:
                if not any(v["language"] == "en" for v in record["variants"]) or not all(1 <= len(v["text_pages"]) <= 4 for v in record["variants"]):
                    raise FixtureError(f"invalid extracted text: {record['qid']}")
                photo = record["photo"]
                if record["qid"] == "Q666668":
                    if photo is not None or not any(o["qid"] == record["qid"] and o["reason"] == "photo_creator_missing" for o in content["omissions"]):
                        raise FixtureError("captured photo without creator was not rejected")
                    continue
                if photo is None or len(files[0][photo["path"]]) != 216 * 240:
                    raise FixtureError(f"missing bounded source photo: {record['qid']}")
            if content["source_coverage"].get("country_complete") is not False:
                raise FixtureError("review examples claim country coverage")
            print("Verified four source-extracted sites and byte-identical offline content builds")
    except (FixtureError, OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f"landmark content: {error}; sync assistant-inputs and install host build dependencies", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
