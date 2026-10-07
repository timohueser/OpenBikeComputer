"""Checks the hand-written test set and prints its counts.

    .venv/bin/python testset/check.py

Each line of <lang>.jsonl must pass `schema.validate`, have at most 80 characters, a unique id with
the file's language as prefix, and scope "in" or "out". An out-of-scope line gives `none` or a
request with ignored words. owner.jsonl may hold any of the languages.
"""

from __future__ import annotations

import json
import sys
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

from schema import INTENTS, Invalid, validate  # noqa: E402

LANGS = ["en", "de", "fr", "it"]
KEYS = {"id", "lang", "text", "scope", "request", "note"}


def problems(row: dict, stem: str) -> list[str]:
    out = []
    if not KEYS - {"note"} <= set(row) <= KEYS:
        return [f"keys {sorted(row)}"]
    lang, text = row["lang"], row["text"]
    if lang not in LANGS or (stem != "owner" and lang != stem):
        out.append(f"lang {lang!r} in {stem}.jsonl")
    if stem != "owner" and not row["id"].startswith(f"{stem}-"):
        out.append(f"id {row['id']!r} needs the prefix {stem}-")
    if not isinstance(text, str) or not text or text != text.strip() or len(text) > 80:
        out.append(f"text must be 1..80 characters without outer spaces ({len(text)})")
    if row["scope"] not in ("in", "out"):
        out.append(f"scope {row['scope']!r}")
    req = row["request"]
    try:
        validate(req)
    except Invalid as e:
        out.append(f"request: {e}")
        return out
    if row["scope"] == "out" and req["type"] != "none" and not req.get("ignored"):
        out.append("out of scope needs none or ignored words")
    return out


def main() -> int:
    rows, bad, ids = [], 0, Counter()
    for f in sorted(HERE.glob("*.jsonl")):
        for n, line in enumerate(f.read_text(encoding="utf-8").splitlines(), 1):
            if not line.strip():
                continue
            try:
                row = json.loads(line)
            except json.JSONDecodeError as e:
                print(f"{f.name}:{n}: {e}")
                bad += 1
                continue
            for p in problems(row, f.stem):
                print(f"{f.name}:{n}: {row.get('id')}: {p}")
                bad += 1
            ids[row.get("id")] += 1
            rows.append(row)
    for i, c in ids.items():
        if c > 1:
            print(f"duplicate id {i} ({c} times)")
            bad += 1

    langs = [lang for lang in LANGS if any(r["lang"] == lang for r in rows)]
    types = Counter((r["lang"], r["request"]["type"]) for r in rows)
    print(f"\n{'type':14s}" + "".join(f"{lang:>6s}" for lang in langs) + f"{'all':>6s}")
    for t in INTENTS:
        cells = [types[(lang, t)] for lang in langs]
        print(f"{t:14s}" + "".join(f"{c:6d}" for c in cells) + f"{sum(cells):6d}")
    print(f"\n{'scope':14s}" + "".join(f"{lang:>6s}" for lang in langs) + f"{'all':>6s}")
    for scope in ("in", "out"):
        cells = [sum(r["lang"] == lang and r["scope"] == scope for r in rows) for lang in langs]
        print(f"{scope:14s}" + "".join(f"{c:6d}" for c in cells) + f"{sum(cells):6d}")
    totals = [sum(r["lang"] == lang for r in rows) for lang in langs]
    outs = [sum(r["lang"] == lang and r["scope"] == "out" for r in rows) for lang in langs]
    print(f"{'total':14s}" + "".join(f"{c:6d}" for c in totals) + f"{sum(totals):6d}")
    print(f"{'out share %':14s}" + "".join(f"{100 * o / t:6.1f}" for o, t in zip(outs, totals))
          + f"{100 * sum(outs) / max(sum(totals), 1):6.1f}")
    print(f"\n{bad} problems" if bad else "\nclean")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
