"""Fetches real place names from Wikidata into data/names/<group>.json.

    .venv/bin/python gen/fetch_names.py            # fetch the missing groups, clean the cached ones
    .venv/bin/python gen/fetch_names.py --force    # fetch all again

The generator samples these names so that the tagger learns where a name is, not which names
exist. Each file is a sorted list of unique labels. Hand-made names (businesses, names that are
common words) live in templates/names.yaml.
"""

from __future__ import annotations

import argparse
import json
import re
import time
import urllib.parse
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent
OUT = HERE / "data" / "names"
ENDPOINT = "https://query.wikidata.org/sparql"
AGENT = "OpenBikeComputer-query-spike/0.1 (https://github.com/timohueser/OpenBikeComputer)"

ALPINE = "wd:Q142 wd:Q38 wd:Q39 wd:Q40 wd:Q183 wd:Q29 wd:Q228"  # FR IT CH AT DE ES AD

# group -> list of (instance-of classes, countries, label languages, limit)
TOWNS = [
    ("wd:Q262166", "wd:Q183", "de", 700),              # municipality of Germany
    ("wd:Q667509", "wd:Q40", "de", 400),               # municipality of Austria
    ("wd:Q70208", "wd:Q39", "de fr it", 400),          # municipality of Switzerland
    ("wd:Q484170", "wd:Q142", "fr", 700),              # commune of France
    ("wd:Q747074", "wd:Q38", "it", 700),               # comune of Italy
    ("wd:Q2039348 wd:Q532 wd:Q3957", "wd:Q55", "nl", 300),
    ("wd:Q493522", "wd:Q31", "nl fr", 300),            # municipality of Belgium
    ("wd:Q2074737", "wd:Q29", "es", 400),              # municipality of Spain
    ("wd:Q532 wd:Q3957", "wd:Q145", "en", 500),        # village, town in the UK
]
GROUPS = {
    "towns": TOWNS,
    "passes": [("wd:Q133056", ALPINE, "de fr it en", 1500)],
    "peaks": [("wd:Q8502", "wd:Q142 wd:Q38 wd:Q39 wd:Q40 wd:Q183", "de fr it en", 1500)],
    "huts": [("wd:Q182676", ALPINE, "de fr it en", 800)],
    "lakes": [("wd:Q23397", "wd:Q183 wd:Q40 wd:Q39 wd:Q142 wd:Q38", "de fr it en", 800)],
}

# A label a rider could type: Latin letters, spaces, hyphens, apostrophes and dots only.
_OK = re.compile(r"^[^\W\d_](?:[^\W\d_]|[ '’.-])*$")


def query(classes: str, countries: str, langs: str, limit: int) -> list[str]:
    lang_list = ", ".join(f'"{lang}"' for lang in langs.split())
    sparql = f"""
SELECT DISTINCT ?label WHERE {{
  VALUES ?class {{ {classes} }}
  VALUES ?country {{ {countries} }}
  ?item wdt:P31 ?class ; wdt:P17 ?country ; rdfs:label ?label .
  FILTER(LANG(?label) IN ({lang_list}))
}} LIMIT {limit}"""
    url = ENDPOINT + "?" + urllib.parse.urlencode({"query": sparql, "format": "json"})
    req = urllib.request.Request(url, headers={"User-Agent": AGENT})
    with urllib.request.urlopen(req, timeout=90) as resp:
        rows = json.load(resp)["results"]["bindings"]
    return [r["label"]["value"] for r in rows]


def clean(labels: list[str]) -> list[str]:
    """Typeable labels with an upper-case first letter. French and Italian Wikidata labels start
    lower case ("col du Galibier", "rifugio Bonatti"), but each one is a proper name."""
    out = set()
    for s in labels:
        s = s.strip()
        if 2 <= len(s) <= 32 and len(s.split()) <= 4 and _OK.match(s):
            out.add(s[0].upper() + s[1:])
    return sorted(out)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--force", action="store_true")
    args = ap.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    for group, specs in GROUPS.items():
        path = OUT / f"{group}.json"
        if path.exists() and not args.force:
            names = clean(json.loads(path.read_text(encoding="utf-8")))
            path.write_text(json.dumps(names, ensure_ascii=False, indent=0) + "\n", encoding="utf-8")
            print(f"{group}: cached, {len(names)} names")
            continue
        labels: list[str] = []
        for spec in specs:
            got = query(*spec)
            print(f"{group} {spec[1]}: {len(got)}")
            labels += got
            time.sleep(2)
        names = clean(labels)
        path.write_text(json.dumps(names, ensure_ascii=False, indent=0) + "\n", encoding="utf-8")
        print(f"{group}: {len(names)} names")


if __name__ == "__main__":
    main()
