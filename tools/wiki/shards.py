"""The `wiki` output: compact facts and photos in geographic shards.

`cells/<i>-<j>.tar.zst` holds the facts of each subject whose box lies in that 2^20 µdeg cell of
the OBCA grid. `common.tar.zst` holds the classes and the facts of every other subject. A fact
that two shards need is in both, with the same bytes. Members are `content/<kind>/<sha256>.json`
and `content/assets/<sha256>`, as `obc_pack::landmarks::shared` reads them. `index.json` lists
each shard with its SHA-256 and size, and counts the subjects and the unresolved work.
"""
from __future__ import annotations

from collections import Counter
import hashlib
import io
from itertools import groupby
import json
from pathlib import Path
import subprocess
import tarfile

from ..landmark_capture import LANGUAGES, write_json
from . import catalog, dumps


def needs(db, targets, wikidata: str | None, wikipedia: str | None) -> set[tuple[str, str]]:
    """Every fact that the compilers read for one OSM subject, except the classes."""
    keys, identities = set(), []
    for kind, tag in (("wikidata", wikidata), ("wikipedia", wikipedia)):
        if tag:
            keys.add(("link", f"{kind}:{tag}"))
            link = catalog.fact(db, "link", f"{kind}:{tag}")
            if link and link["status"] == "present":
                identities.append(link["identity"])
                keys.update(("article", f"{site.removesuffix('wiki')}:{value['title']}") for site, value in link.get("sitelinks", {}).items())
    for identity in identities + ([wikidata] if wikidata else []):
        names, value = {identity}, catalog.fact(db, "entity", identity)
        if value and value["status"] == "present":
            names.add(value["identity"])
            keys.update(("entity", qid) for qid in catalog.locales(targets, value["identity"]))
            keys.update(("article", f"{language}:{link['title']}") for language in LANGUAGES
                        if (link := value["entity"]["sitelinks"].get(language + "wiki")))
        for name in names:
            keys.add(("entity", name))
            keys.update(db.execute("SELECT kind, key FROM uses WHERE subject=?", (name,)).fetchall())
    return keys


def archive(path: Path, members: dict[str, bytes | Path]):
    """A reproducible tar of `members`, compressed by zstd."""
    path.parent.mkdir(parents=True, exist_ok=True)
    partial = path.with_suffix(".partial")
    with partial.open("wb") as output:
        process = subprocess.Popen(["zstd", "-q", "-19", "-T0", "-c"], stdin=subprocess.PIPE, stdout=output)
        with tarfile.open(fileobj=process.stdin, mode="w|", format=tarfile.PAX_FORMAT) as tar:
            for name, data in sorted(members.items()):
                info = tarfile.TarInfo(name)
                info.size, info.mode = (data.stat().st_size if isinstance(data, Path) else len(data)), 0o644
                with (data.open("rb") if isinstance(data, Path) else io.BytesIO(data)) as source:
                    tar.addfile(info, source)
        process.stdin.close()
        if process.wait():
            raise ValueError(f"zstd could not write {path.name}")
    partial.replace(path)


def write(db, work: Path, out: Path):
    targets = catalog.graph(db)
    shards, counts = [], Counter()
    groups = {None: {("entity", qid) for qid in catalog.classes(db, targets)}}
    rows = db.execute("SELECT cell, wikidata, wikipedia FROM subjects ORDER BY cell").fetchall()
    for cell, subjects in groupby(rows, key=lambda row: row[0]):
        keys = groups.setdefault(cell, set())
        for _, wikidata, wikipedia in subjects:
            counts["subjects" if cell else "subjects_in_common"] += 1
            keys |= needs(db, targets, wikidata, wikipedia)
        if cell is not None:
            shards.append(shard(db, work, out, f"cells/{cell}.tar.zst", groups.pop(cell)))
    shards.insert(0, shard(db, work, out, "common.tar.zst", groups.pop(None)))
    counts.update({f"unresolved_{kind}": count for kind, count in db.execute("SELECT kind, COUNT(*) FROM failures GROUP BY kind")})
    counts["subjects_without_link"] = db.execute("""SELECT COUNT(*) FROM subjects WHERE
        (wikidata IS NOT NULL AND NOT EXISTS (SELECT 1 FROM facts WHERE kind='link' AND key='wikidata:'||wikidata)) OR
        (wikipedia IS NOT NULL AND NOT EXISTS (SELECT 1 FROM facts WHERE kind='link' AND key='wikipedia:'||wikipedia))""").fetchone()[0]
    paths = {shard["path"] for shard in shards}
    for stale in out.glob("**/*.tar.zst"):
        if stale.relative_to(out).as_posix() not in paths:
            stale.unlink()
    write_json(out / "index.json", dict(shards=shards, counts=dict(sorted(counts.items()))))


def shard(db, work: Path, out: Path, name: str, keys: set[tuple[str, str]]) -> dict:
    members = {}
    for kind, key in keys:
        row = db.execute("SELECT data FROM facts WHERE kind=? AND key=?", (kind, key)).fetchone()
        if row:
            members[f"content/{kind}/{hashlib.sha256(row[0]).hexdigest()}.json"] = row[0]
            if kind == "file":
                asset = json.loads(row[0])["asset"]
                members[f"content/assets/{asset['sha256']}"] = work / asset["path"]
    archive(out / name, members)
    return dict(path=name, sha256=dumps.sha(out / name), bytes=(out / name).stat().st_size,
                facts=sum(name.endswith(".json") for name in members))
