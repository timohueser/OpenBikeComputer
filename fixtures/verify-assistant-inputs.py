#!/usr/bin/env python3
"""Verify captured source manifests and recorded review identities offline."""
import hashlib
import json
import re
from pathlib import Path
import subprocess
import sys
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
from tools.fixtures import Catalog, FixtureError, Store, cache_root, sha256_file


def main() -> int:
    catalog = Catalog(ROOT / "fixtures/catalog.toml")
    store = Store(catalog, cache_root())
    try:
        for package in catalog.package_ids_for(["assistant-inputs"]):
            store.verify(package)
            root = store.package_root(package)
            manifest = json.loads((root / "manifest.json").read_text())
            for source in manifest.get("sources", []):
                path = root / source["path"]
                if path.stat().st_size != source["bytes"] or sha256_file(path) != source["sha256"]:
                    raise FixtureError(f"{package}: source manifest differs for {source['path']}")
        osm = store.package_root("assistant-osm")
        examples = json.loads((osm / "examples.json").read_text())["examples"]
        for source in sorted({example["source"] for example in examples}):
            selected = [example for example in examples if example["source"] == source]
            ids = [example["osm_type"][0] + str(example["osm_id"]) for example in selected]
            raw = subprocess.check_output(["osmium", "getid", str(osm / source.split(":", 1)[1]), *ids, "--add-referenced", "-f", "osm"])
            elements = {(e.tag, int(e.attrib["id"])): e for e in ET.fromstring(raw) if "id" in e.attrib}
            for example in selected:
                element = elements.get((example["osm_type"], example["osm_id"]))
                if element is None:
                    raise FixtureError(f"missing source identity: {example['osm_type']}/{example['osm_id']}")
                tags = {tag.attrib["k"]: tag.attrib["v"] for tag in element.findall("tag")}
                if int(element.attrib["version"]) != example["version"] or element.attrib["timestamp"] != example["timestamp"] or tags != example["tags"]:
                    raise FixtureError(f"source facts differ: {example['osm_type']}/{example['osm_id']}")
                node = element if element.tag == "node" else elements[("node", int(element.find("nd").attrib["ref"]))]
                if [float(node.attrib["lon"]), float(node.attrib["lat"])] != example["coordinate"]:
                    raise FixtureError(f"source coordinate differs: {example['osm_type']}/{example['osm_id']}")
        wiki = store.package_root("assistant-wiki")
        manifest = json.loads((wiki / "manifest.json").read_text())
        for place in manifest["places"]:
            entity = json.loads((wiki / "entities" / (place["qid"] + ".json")).read_text())["entities"][place["qid"]]
            if entity["lastrevid"] != place["entity_revision"] or entity["claims"]["P625"][0]["mainsnak"]["datavalue"]["value"] != place["coordinate"]:
                raise FixtureError(f"entity revision differs: {place['qid']}")
            for article in place["articles"]:
                response = json.loads((wiki / article["path"]).read_text())
                revision = next(iter(response["query"]["pages"].values()))["revisions"][0]
                if revision["revid"] != article["revision"] or revision["timestamp"] != article["timestamp"]:
                    raise FixtureError(f"article revision differs: {article['path']}")
                html = (wiki / article["html_path"]).read_text()
                rendered_revision = re.search(r'"wgRevisionId":(\d+)', html)
                if not rendered_revision or int(rendered_revision[1]) != article["revision"]:
                    raise FixtureError(f"rendered revision differs: {article['html_path']}")
            for image in place["images"]:
                response = json.loads((wiki / image["metadata_path"]).read_text())
                info = next(iter(response["query"]["pages"].values()))["imageinfo"][0]
                with (wiki / image["path"]).open("rb") as source:
                    if hashlib.file_digest(source, "sha1").hexdigest() != info["sha1"]:
                        raise FixtureError(f"Commons original differs: {image['path']}")
        print(f"Verified {len(examples)} OSM review identities and {len(manifest['places'])} Wiki sites")
    except (FixtureError, OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f"assistant inputs: {error}; run tools/obc fixtures sync assistant-inputs, then retry", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
