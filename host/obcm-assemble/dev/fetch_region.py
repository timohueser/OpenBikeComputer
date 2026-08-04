#!/usr/bin/env python3
"""Fetch one published region's cells into the sidecar layout `obcm-assemble` reads.

This is the *input half* of the memory harness: `obcm-assemble --features mem-profile` can only
report what a country-scale assembly costs if a country-scale selection is on disk, and the only
reproducible source of one is the published catalog. Nothing here packs, cuts or assembles — it
downloads content-addressed objects and writes the cutter's `cells.json` sidecar over them.

    # 1. fetch (once; resumable — a present file with the right digest is skipped)
    python3 host/obcm-assemble/dev/fetch_region.py \\
        europe/germany/baden-wuerttemberg/freiburg-regbez /tmp/obca/freiburg

    # 2. measure
    cargo run --release -p obcm-assemble --features mem-profile -- \\
        --cells /tmp/obca/freiburg/cells.json \\
        --skin  /tmp/obca/freiburg/skin.json \\
        --out   /tmp/obca/freiburg/out --accept-holes

The written tree is exactly what `obc-pack cut` produces, because that is what the CLI parses
(`main.rs::parse_sidecar`): cell artifacts under `cells/<band>/<i>/<j>.obcm`, and a `cells.json`
naming every cell's `id`, `band`, `path` and OBCA §3.7 `partial` flag plus the schema they were
baked at. `schema.json` (an OBCC v2 root) and `skin.json` are written beside it for the CLI's
`--schema` / `--skin` arguments; `--schema` is optional since the sidecar carries one.

Terrain is deliberately **not** fetched. The harness measures the nav rewrite, which is where an
assembly's memory goes; the terrain shard streams cell-by-cell and would only add gigabytes of
download. Assemble without `--terrain` and the set simply has no raster.

Catalog shape (host/obc-pack/schema/catalog.schema.json): the root lists `regions[]` — each with a
`cells_url` naming that region's cells *by id, grouped by band* — and `cell_index[]`, one document
per band carrying every cell's `url`, `sha256`, `bytes` and `partial`. A region fetch is therefore
the intersection: read the region's id lists, then look each id up in its band index.

python3 stdlib only (urllib, json, hashlib) — no pip dependencies, so it runs anywhere the repo does.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

DEFAULT_CATALOG = "https://maps.openbikecomputer.com/cell-catalog/catalog.json"

# R2 refuses urllib's default agent, and an anonymous fetcher should say what it is anyway.
USER_AGENT = "obcm-assemble-fetch-region/1 (+https://github.com/timohueser/OpenBikeComputer)"


def fetch(url: str) -> bytes:
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(req, timeout=120) as r:
        return r.read()


def fetch_json(url: str) -> dict:
    return json.loads(fetch(url))


def digest(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def download_cell(entry: dict, dest: Path) -> tuple[str, int]:
    """Download one cell unless it is already there with the right digest. Returns (state, bytes)."""
    want = entry["sha256"]
    if dest.exists() and digest(dest) == want:
        return ("kept", dest.stat().st_size)
    dest.parent.mkdir(parents=True, exist_ok=True)
    raw = fetch(entry["url"])
    got = hashlib.sha256(raw).hexdigest()
    if got != want:
        raise SystemExit(f"error: {entry['url']} hashes to {got}, the catalog says {want}")
    # Write through a temporary so an interrupted run never leaves a short file that a later
    # resume would have to distrust — the digest check above would catch it, but only after
    # re-hashing gigabytes.
    tmp = dest.with_suffix(dest.suffix + ".part")
    tmp.write_bytes(raw)
    os.replace(tmp, dest)
    return ("fetched", len(raw))


def main() -> int:
    ap = argparse.ArgumentParser(
        description="Fetch a published region's cells into the layout obcm-assemble reads.",
        epilog="Terrain cells are never fetched: the harness measures the nav rewrite, so assemble "
        "without --terrain.",
    )
    # Both optional only so `--list-regions` can be asked on its own; checked below.
    ap.add_argument("region", nargs="?", help="region id, e.g. europe/germany/baden-wuerttemberg/freiburg-regbez")
    ap.add_argument("out", nargs="?", type=Path, help="output directory (created; re-runs resume)")
    ap.add_argument("--catalog", default=DEFAULT_CATALOG, help=f"catalog root URL (default: {DEFAULT_CATALOG})")
    ap.add_argument("--skin", default="default", help="which catalog skin to write as skin.json (default: default)")
    ap.add_argument("--band", action="append", default=[], help="only fetch these bands (repeatable)")
    ap.add_argument("--jobs", type=int, default=8, help="parallel downloads (default: 8)")
    ap.add_argument("--list-regions", action="store_true", help="list the catalog's regions and exit")
    args = ap.parse_args()

    catalog = fetch_json(args.catalog)
    regions = catalog.get("regions", [])
    if args.list_regions:
        for r in regions:
            print(f"{r['id']:60} {r.get('bytes', 0) / 1e6:10.1f} MB")
        return 0

    if args.region is None or args.out is None:
        ap.error("region and out are required (unless --list-regions)")

    region = next((r for r in regions if r["id"] == args.region), None)
    if region is None:
        known = "\n  ".join(r["id"] for r in regions)
        raise SystemExit(f"error: no region {args.region!r} in the catalog. Known:\n  {known}")

    # Every band index the region's selection touches, fetched once.
    listed: dict[str, list[str]] = fetch_json(region["cells_url"]).get("cells", {})
    bands = [b for b in listed if not args.band or b in args.band]
    index_url = {e["band"]: e["url"] for e in catalog.get("cell_index", [])}

    jobs: list[tuple[dict, Path, str]] = []
    for band in bands:
        if band not in index_url:
            raise SystemExit(f"error: the catalog has no cell index for band {band!r}")
        by_id = {c["id"]: c for c in fetch_json(index_url[band]).get("cells", [])}
        for cid in listed[band]:
            entry = by_id.get(cid)
            if entry is None:
                # A listed-but-unindexed cell is a broken catalog, not a hole: holes are stated as
                # `known_empty` ranges and are simply absent from the region's id list.
                raise SystemExit(f"error: region lists {band} cell {cid}, the band index does not carry it")
            _, i, j = cid.split("/")
            jobs.append((entry, args.out / "cells" / band / i / f"{j}.obcm", band))

    total = sum(e["bytes"] for e, _, _ in jobs)
    print(f"{args.region}: {len(jobs)} cell(s) over {len(bands)} band(s), {total / 1e6:.1f} MB", file=sys.stderr)

    done = {"fetched": 0, "kept": 0, "bytes": 0}
    with ThreadPoolExecutor(max_workers=max(1, args.jobs)) as pool:
        futures = [pool.submit(download_cell, e, p) for e, p, _ in jobs]
        for n, f in enumerate(futures, 1):
            state, size = f.result()
            done[state] += 1
            done["bytes"] += size
            print(f"\r  {n}/{len(jobs)} cells · {done['bytes'] / 1e6:8.1f} MB", end="", file=sys.stderr)
    print(f"\n  {done['fetched']} fetched, {done['kept']} already present", file=sys.stderr)

    schema = catalog["schema"]
    sidecar = {
        "cutter": f"fetch_region.py ({args.catalog})",
        "region": args.region,
        "schema": schema,
        "cells": [
            {
                "id": e["id"],
                "band": band,
                "path": str(p.relative_to(args.out)),
                "bytes": e["bytes"],
                "sha256": e["sha256"],
                "partial": bool(e.get("partial", False)),
            }
            for e, p, band in jobs
        ],
    }
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "cells.json").write_text(json.dumps(sidecar, indent=2) + "\n")
    # An OBCC v2 root, which is one of the two shapes `Schema::parse` accepts.
    (args.out / "schema.json").write_text(json.dumps({"schema": schema}, indent=2) + "\n")

    skins = catalog.get("skins", [])
    skin = next((s for s in skins if s["id"] == args.skin), None)
    if skin is None:
        raise SystemExit(f"error: no skin {args.skin!r} in the catalog (have: {', '.join(s['id'] for s in skins)})")
    (args.out / "skin.json").write_text(json.dumps(skin, indent=2) + "\n")

    print(
        f"wrote {args.out / 'cells.json'} (+ schema.json, skin.json)\n"
        f"  measure with: cargo run --release -p obcm-assemble --features mem-profile -- "
        f"--cells {args.out / 'cells.json'} --skin {args.out / 'skin.json'} --out {args.out / 'out'}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except urllib.error.HTTPError as e:
        raise SystemExit(f"error: {e.url}: HTTP {e.code} {e.reason}")
