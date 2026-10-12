"""Prepare the `wiki` source: compact Wikimedia facts and photos for the OSM subjects.

Each step resumes in its work directory. The producer runs them in this order:

    python -m tools.wiki subjects   --osm PLANET.osm.pbf --work W --out subjects.ndjson [--bbox W,S,E,N]
    python -m tools.wiki import     --work W --inputs inputs.json --subjects subjects.ndjson
    python -m tools.wiki candidates --work W --out candidates.jsonl
    python -m tools.wiki articles   --work W --inputs inputs.json --selected selected.json
    python -m tools.wiki facts      --work W --selected ids.json --requests links.json --out DIR [--no-text]
    python -m tools.wiki commons    --work W --requests requests.json
    python -m tools.wiki shards     --work W --out OUT [--done done.json] [--counts counts.json] [--no-text]

`inputs.json` gives each dump that the producer reads in place, as `{"path": PATH, "sha256": HEX}`:
`{"wikidata": DUMP, "wikidata_pages": DUMP, "wikidata_redirects": DUMP, "wikipedia": {LANG: DUMP},
"langlinks": {LANG: DUMP}}`. `wikidata` is the JSON dump, `wikipedia` the Enterprise HTML, the
others are SQL tables. A path may have no suffix: the first bytes name a bz2, gzip or tar file.
`candidates.jsonl` is what the landmark rule of the compilers reads, and `selected.json` lists the
QIDs whose articles the compilers read. `done.json` gives the `sha256` and `bytes` of each shard
that is published already, and `counts.json` the counts of the producer for the index.
`requests.json` maps each subject that the compilers select to its Commons `categories`,
`commons` and `files`; see `commons.acquire`. When Wikimedia is busy, `commons` stops and leaves
the file `busy` in the work directory: the next call resumes. `facts` writes the facts that the
compilers read of the subjects `ids.json` and of the peaks with the tags of `links.json`.

The steps need Osmium (or `OBC_OSMIUM`) and zstd; lbzip2 and pigz decompress in parallel when
present.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path

from ..landmark_capture import LANGUAGES
from . import catalog, commons, shards, subjects


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("step", choices=["subjects", "import", "candidates", "articles", "facts", "commons", "shards"])
    parser.add_argument("--work", type=Path, required=True)
    for name in ("osm", "out", "inputs", "subjects", "selected", "requests", "done", "counts"):
        parser.add_argument(f"--{name}", type=Path)
    parser.add_argument("--bbox", type=lambda text: [float(value) for value in text.split(",")])
    parser.add_argument("--no-text", action="store_true")
    args = parser.parse_args()
    if args.step == "subjects":
        subjects.extract(args.osm, args.out, args.work, args.bbox)
        return 0
    db = catalog.connect(args.work)
    inputs = json.loads(args.inputs.read_bytes()) if args.inputs else {}

    def source(name: str, dump: dict) -> tuple[Path, str]:
        print(f"{name}: {dump['path']} sha256 {dump['sha256']}", flush=True)
        return Path(dump["path"]), dump["sha256"]

    if args.step == "import":
        subjects.load(db, args.work, args.subjects)
        if missing := {"wikidata", "wikidata_pages", "wikidata_redirects"} - set(inputs):
            raise ValueError(f"inputs.json names no {', '.join(sorted(missing))}")
        for name in ("wikidata_pages", "wikidata_redirects"):
            catalog.sql(db, args.work, name, *source(name, inputs[name]))
        catalog.wikidata(db, args.work, *source("wikidata", inputs["wikidata"]))
    elif args.step == "candidates":
        catalog.candidates(db, args.out)
    elif args.step == "articles":
        catalog.want(db, args.work, json.loads(args.selected.read_bytes()))
        for language, spec in inputs.get("langlinks", {}).items():
            catalog.sql(db, args.work, "langlinks:" + language, *source("langlinks:" + language, spec))
        for language in reversed(LANGUAGES):
            if spec := inputs.get("wikipedia", {}).get(language):
                catalog.articles(db, args.work, language, *source("wikipedia:" + language, spec))
        catalog.links(db)
    elif args.step == "facts":
        shards.export(db, args.out, json.loads(args.selected.read_bytes()), json.loads(args.requests.read_bytes()),
                      text=not args.no_text)
    elif args.step == "commons":
        try:
            commons.acquire(db, args.work, json.loads(args.requests.read_bytes()))
        except RuntimeError as error:
            if "busy" in str(error):
                (args.work / "busy").touch()
            raise
    else:
        done, counts = (json.loads(path.read_bytes()) if path else None for path in (args.done, args.counts))
        shards.write(db, args.work, args.out, done, text=not args.no_text, counts=counts)
    db.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
