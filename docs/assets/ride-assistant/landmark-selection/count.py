"""Recount the captured Wikidata candidates after editing selection.json. No network I/O."""

import gzip
import json
from pathlib import Path


def main():
    folder = Path(__file__).resolve().parent
    selection = json.loads((folder / "selection.json").read_text())
    types = json.loads((folder / "types.json").read_text())
    items = json.loads(gzip.decompress((folder / "switzerland.json.gz").read_bytes()))
    roots = {
        qid: {root for t in item["types"] for root in types.get(t, {}).get("roots", [])}
        for qid, item in items.items()
    }
    excluded = {qid for qid, value in roots.items() if value.intersection(selection["exclude_roots"])}
    included = set()
    groups = {}

    def stats(qids):
        images = sum(items[qid]["has_image"] for qid in qids)
        return {
            "items": len(qids),
            "with_p18_image": images,
            "with_english_article": sum("en" in items[qid]["languages"] for qid in qids),
            "raw_photo_bytes": images * selection["image_dimensions"][0] * selection["image_dimensions"][1],
        }

    for name, group in selection["groups"].items():
        matches = {qid for qid, value in roots.items() if value.intersection(group["roots"])} - excluded
        groups[name] = stats(matches)
        if group["include"]:
            included.update(matches)
    pending_roots = {
        root for name in selection["pending_groups"] for root in selection["groups"][name]["roots"]
    }
    pending = {qid for qid, value in roots.items() if value.intersection(pending_roots)} - excluded
    print(json.dumps({
        "date": selection["date"],
        "source_items": len(items),
        "groups_overlap": True,
        "groups": groups,
        "core_distinct": stats(included),
        "core_plus_glaciers_and_passes_distinct": stats(included | pending),
    }, indent=2))


if __name__ == "__main__":
    main()
