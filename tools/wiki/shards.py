"""The `wiki` output: compact facts and photos in geographic shards.

`cells/<i>-<j>.tar.zst` holds the facts of each subject whose box lies in that 2^20 µdeg cell of
the OBCA grid. `common.tar.zst` holds the classes and the facts of every other subject. A fact
that two shards need is in both, with the same bytes. Members are `content/<kind>/<sha256>.json`
and `content/assets/<sha256>`, as `obc_pack::landmarks::shared` reads them. `index.json` lists
each shard with its SHA-256 and size, and counts the subjects and the unresolved work. A version
that extends the version before holds each of its shards too, merged with the facts of this run.
"""
from __future__ import annotations

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


def published(kind: str, data: bytes, text: bool) -> bytes:
    """The bytes of a fact as a version holds it. Without `text`, an article is a fact with its
    key and the status `missing`: the compilers take it as no article, so a lead image is no
    photo, and the producer chooses the photos from the facts that the readers get."""
    if text or kind != "article":
        return data
    stub = dict(kind="article", key=json.loads(data)["key"], status="missing")
    return json.dumps(stub, ensure_ascii=False, separators=(",", ":")).encode()


def export(db, out: Path, ids: list[str], links: list[list[str | None]], text: bool = True):
    """The facts that the compilers read of the subjects `ids` and of the peaks with the
    `[wikidata, wikipedia]` tags of `links`, with the classes of their types, as
    `content/<kind>/<sha256>.json` under `out`; without `text`, as [`published`] gives them."""
    targets = catalog.graph(db)
    keys = set()
    for qid in ids:
        keys |= needs(db, targets, qid, None)
    for wikidata, wikipedia in links:
        keys |= needs(db, targets, wikidata, wikipedia)
    found, pending = set(), set().union(*(targets(31, key) for kind, key in keys if kind == "entity"))
    while pending:
        found |= pending
        pending = set().union(*(targets(279, qid) for qid in pending)) - found
    for kind, key in keys | {("entity", qid) for qid in found}:
        if row := db.execute("SELECT data FROM facts WHERE kind=? AND key=?", (kind, key)).fetchone():
            data = published(kind, row[0], text)
            path = out / "content" / kind / f"{hashlib.sha256(data).hexdigest()}.json"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)


def archive(path: Path, members):
    """A reproducible tar of the `(name, bytes or file)` members, compressed by zstd."""
    path.parent.mkdir(parents=True, exist_ok=True)
    partial = path.with_suffix(".partial")
    with partial.open("wb") as output:
        # Most bytes are JPEG thumbnails, which do not compress. Level 10 keeps the JSON facts
        # small at a fraction of the time of level 19.
        process = subprocess.Popen(["zstd", "-q", "-10", "-T0", "-c"], stdin=subprocess.PIPE, stdout=output)
        with tarfile.open(fileobj=process.stdin, mode="w|", format=tarfile.PAX_FORMAT) as tar:
            for name, data in members:
                info = tarfile.TarInfo(name)
                info.size, info.mode = (data.stat().st_size if isinstance(data, Path) else len(data)), 0o644
                with (data.open("rb") if isinstance(data, Path) else io.BytesIO(data)) as source:
                    tar.addfile(info, source)
        process.stdin.close()
        if process.wait():
            raise ValueError(f"zstd could not write {path.name}")
    partial.replace(path)


def write(db, work: Path, out: Path, done: dict | None = None, text: bool = True, counts: dict | None = None,
          keep: dict | None = None):
    """`done` gives the SHA-256 and the size of each shard that is published already: it is not
    written again, and the index lists it. Without `text`, the shards hold no article text. `counts`
    are the counts of the producer, which the index adds to its own. `keep` gives `before`, the
    index entries of the shards of the version before, each with its local `file`: the version
    holds each of those shards, merged as `shard` says."""
    if not catalog.complete(db, "wikidata:2"):
        raise ValueError("the Wikidata import has not finished; run the import step first")
    targets = catalog.graph(db)
    db.execute("CREATE TEMP TABLE IF NOT EXISTS shard(kind TEXT, key TEXT, PRIMARY KEY(kind,key)) WITHOUT ROWID")
    db.execute("CREATE TEMP TABLE IF NOT EXISTS earlier(kind TEXT, key TEXT, data BLOB, PRIMARY KEY(kind,key)) WITHOUT ROWID")
    db.execute("DELETE FROM earlier")
    before = {entry["path"]: entry for entry in (keep or {}).get("before", [])}
    for entry in before.values():
        prefer(db, work, Path(entry["file"]))
    written = (out, done or {}, text)
    classes = [("entity", qid) for qid in catalog.classes(db, targets)]
    common = db.execute("SELECT wikidata, wikipedia FROM subjects WHERE cell=''")
    shards = [shard(db, work, written, "common.tar.zst", targets, classes, common, before.get("common.tar.zst"))]
    rows = db.execute("SELECT cell, wikidata, wikipedia FROM subjects WHERE cell!='' ORDER BY cell")
    for cell, subjects in groupby(rows, key=lambda row: row[0]):
        name = f"cells/{cell}.tar.zst"
        shards.append(shard(db, work, written, name, targets, [], (row[1:] for row in subjects), before.get(name)))
    made = {entry["path"] for entry in shards}
    shards += [shard(db, work, written, name, targets, [], [], entry) for name, entry in before.items() if name not in made]
    shards[1:] = sorted(shards[1:], key=lambda entry: entry["path"])
    found = {f"unresolved_{kind}": count for kind, count in db.execute("SELECT kind, COUNT(*) FROM failures GROUP BY kind")}
    found.update(counts or {})
    found.update(subjects=db.execute("SELECT COUNT(DISTINCT osm) FROM subjects").fetchone()[0],
                  subjects_in_common=db.execute("SELECT COUNT(DISTINCT osm) FROM subjects WHERE cell=''").fetchone()[0],
                  subjects_without_link=db.execute("""SELECT COUNT(DISTINCT osm) FROM subjects WHERE
                      (wikidata IS NOT NULL AND NOT EXISTS (SELECT 1 FROM facts WHERE kind='link' AND key='wikidata:'||wikidata)) OR
                      (wikipedia IS NOT NULL AND NOT EXISTS (SELECT 1 FROM facts WHERE kind='link' AND key='wikipedia:'||wikipedia))""").fetchone()[0])
    paths = {entry["path"] for entry in shards}
    for stale in out.glob("**/*.tar.zst"):
        if stale.relative_to(out).as_posix() not in paths:
            stale.unlink()
    write_json(out / "index.json", dict(shards=shards, counts=dict(sorted(found.items()))))


def prefer(db, work: Path, path: Path):
    """Record in `earlier` each fact of the earlier shard `path` that the version holds in its
    earlier form: one that the catalog lacks, one of a newer revision than the catalog holds, and
    one that the catalog has only as an article of a language that this run did not prepare. The
    asset of such a `file` fact goes into `work` at its path."""
    assets = {}
    for member, data in unpacked(path):
        if not member.endswith(".json"):
            continue
        value = json.loads(data)
        row = db.execute("SELECT revision, data FROM facts WHERE kind=? AND key=?", (value["kind"], value["key"])).fetchone()
        revision = value.get("revision") if isinstance(value.get("revision"), int) else 0
        if row and revision <= row[0] and json.loads(row[1]).get("reason") != "language_not_prepared":
            continue
        db.execute("INSERT OR IGNORE INTO earlier VALUES(?,?,?)", (value["kind"], value["key"], data))
        if value["kind"] == "file":
            asset = Path(value["asset"]["path"])
            if asset.is_absolute() or ".." in asset.parts:
                raise ValueError(f"{path.name}: the asset path {asset} leaves the work directory")
            if not (work / asset).is_file():
                assets[f"content/assets/{value['asset']['sha256']}"] = work / asset
    for member, data in unpacked(path) if assets else ():
        if target := assets.get(member):
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)


def shard(db, work: Path, written: tuple, name: str, targets, keys: list[tuple[str, str]], subjects,
          before: dict | None = None) -> dict:
    """Write `keys` and the facts of `subjects` to `out/name`, unless `done` names it. The keys
    collect in a temporary table, so a shard of any size streams from the catalog.

    With `before`, the index entry of the earlier shard of that name with its `file`, the shard
    also holds each earlier fact. A fact takes the form that `earlier` records, else the form of
    the catalog, so it has the same bytes in every shard of the version; without `text`, no
    article has text. An earlier shard whose members this run does not change stays as it is: its
    entry is returned and nothing is written."""
    out, done, text = written
    db.execute("DELETE FROM shard")
    db.executemany("INSERT OR IGNORE INTO shard VALUES(?,?)", keys)
    for wikidata, wikipedia in subjects:
        db.executemany("INSERT OR IGNORE INTO shard VALUES(?,?)", needs(db, targets, wikidata, wikipedia))
    held = set()
    for member, data in unpacked(Path(before["file"])) if before else ():
        held.add(member)
        if member.endswith(".json"):
            value = json.loads(data)
            db.execute("INSERT OR IGNORE INTO shard VALUES(?,?)", (value["kind"], value["key"]))

    def members():
        assets = set()
        for kind, data in db.execute("""SELECT kind, COALESCE(earlier.data, facts.data) FROM shard
                LEFT JOIN earlier USING(kind, key) LEFT JOIN facts USING(kind, key)
                WHERE earlier.data IS NOT NULL OR facts.data IS NOT NULL ORDER BY kind, key"""):
            data = published(kind, data, text)
            yield f"content/{kind}/{hashlib.sha256(data).hexdigest()}.json", data
            if kind == "file" and (asset := json.loads(data)["asset"])["sha256"] not in assets:
                assets.add(asset["sha256"])
                yield f"content/assets/{asset['sha256']}", work / asset["path"]

    names = [member for member, _ in members()]
    if before and set(names) == held:
        return {key: value for key, value in before.items() if key != "file"}
    facts = sum(1 for member in names if member.endswith(".json"))
    if name in done:
        return dict(path=name, sha256=done[name]["sha256"], bytes=done[name]["bytes"], facts=facts)
    archive(out / name, members())
    return dict(path=name, sha256=dumps.sha(out / name), bytes=(out / name).stat().st_size, facts=facts)


def unpacked(path: Path):
    """The `(name, bytes)` of each file of a shard."""
    process = subprocess.Popen(["zstd", "-q", "-dc", str(path)], stdout=subprocess.PIPE)
    with tarfile.open(fileobj=process.stdout, mode="r|") as tar:
        for member in tar:
            if member.isfile():
                yield member.name, tar.extractfile(member).read()
    process.stdout.close()
    if process.wait():
        raise ValueError(f"zstd could not read {path.name}")
