"""Commons facts and 500 px thumbnails for the photo candidates of the selected subjects.

Every request goes through `Capture` with its limits: four requests a second in series, `maxlag`,
`Retry-After`, three backoffs on 429 and 503, and media read at 3 MB/s. Capture retains each
response, so an interrupted run resumes without asking again.

A `file` fact keeps the thumbnail as its asset, and the landmark compiler converts it with
`photo::prepare`. A producer that converts earlier replaces the asset and its `file` fact here,
before the shards are written.
"""
from __future__ import annotations

import os
from pathlib import Path

from ..landmark_capture import Capture, batches, digest
from . import catalog
from .facts import category_members, commons_metadata, encoded, mediainfo

THUMBNAIL = "https://upload.wikimedia.org/wikipedia/commons/thumb/"
FIELDS = (("category", "categories", "Category:"), ("commons", "commons", "File:"), ("file", "files", "File:"))


def title(prefix: str, name: str) -> str:
    title = prefix + name.removeprefix(prefix).replace("_", " ")
    if title == prefix or "|" in title:
        raise ValueError(f"invalid Commons title: {name}")
    return title


def record(db, kind: str, key: str, value: dict | str):
    if isinstance(value, dict):
        catalog.put(db, value)
    else:
        catalog.fail(db, kind, key, value)


def acquire(db, work: Path, requests: dict):
    """`requests` maps each selected subject to the Commons `categories`, `commons` metadata and
    `files` that its compiler reads. Nothing outside it is asked for."""
    capture = Capture(work / "commons")
    capture.retry_failed()

    def fetch(url):
        return capture.json(f"api/{digest(url.encode())}.json", url)

    wanted = {kind: set() for kind, _, _ in FIELDS}
    for subject, request in requests.items():
        for kind, field, prefix in FIELDS:
            for name in request.get(field, []):
                key = title(prefix, name)
                wanted[kind].add(key)
                db.execute("INSERT OR IGNORE INTO uses VALUES(?,?,?)", (subject, kind, key))
                if kind == "file":
                    wanted["commons"].add(key)
                    db.execute("INSERT OR IGNORE INTO uses VALUES(?,?,?)", (subject, "commons", key))
    for key in sorted(wanted["category"]):
        if not catalog.fact(db, "category", key):
            record(db, "category", key, category_members(fetch, key))
    for key, value in commons_metadata(fetch, [key for key in sorted(wanted["commons"]) if not catalog.fact(db, "commons", key)]):
        record(db, "commons", key, value)
    media = set()
    for subject, key in db.execute("SELECT subject, key FROM uses WHERE kind='commons'").fetchall():
        value = catalog.fact(db, "commons", key)
        if value and value["status"] == "present":
            media.add(f"M{value['pageid']}")
            db.execute("INSERT OR IGNORE INTO uses VALUES(?,?,?)", (subject, "mediainfo", f"M{value['pageid']}"))
    for key, value in mediainfo(fetch, [key for key in sorted(media) if not catalog.fact(db, "mediainfo", key)]):
        record(db, "mediainfo", key, value)
    db.commit()
    photos(db, capture, work, sorted(wanted["file"]))


def same(pinned: dict, current) -> bool:
    return isinstance(current, dict) and all(pinned.get(field) == current.get(field) for field in ("identity", "revision", "file_revision"))


def witness(value: dict) -> dict:
    revision = dict(timestamp=value["file_revision"]["timestamp"], sha1=value["file_revision"]["sha1"],
                    description_revision=value["revision"], extmetadata=value["imageinfo"].get("extmetadata"))
    return {"query": {"pages": {"compact": {"title": value["key"], "imageinfo": [revision]}}}}


def photos(db, capture: Capture, work: Path, titles: list[str]):
    """Each thumbnail sits between two fresh metadata checks, fifty files to a check. A file whose
    metadata changed in between gets no photo in this version."""
    pending = []
    for key in titles:
        pinned = catalog.fact(db, "commons", key)
        kept = catalog.fact(db, "file", key)
        if not pinned or pinned["status"] != "present" or (kept and kept["identity"] == pinned["identity"] and kept["revision"] == pinned["file_revision"]):
            continue
        if not pinned["imageinfo"].get("thumburl", "").startswith(THUMBNAIL) or pinned["imageinfo"].get("thumbwidth") != 500:
            catalog.fail(db, "file", key, "Commons conversion input is not a standard 500 px thumbnail")
            continue
        pending.append(pinned)
    for chunk in batches(pending):
        prefix = "photos/" + digest(encoded([(pinned["key"], pinned["revision"]) for pinned in chunk]))

        def check(phase):
            fetch = lambda url: capture.json(f"{prefix}/{phase}-{digest(url.encode())}.json", url)
            return dict(commons_metadata(fetch, [pinned["key"] for pinned in chunk]))

        before = check("before")
        thumbnails = {pinned["key"]: capture.fetch(f"{prefix}/{digest(pinned['key'].encode())}", pinned["imageinfo"]["thumburl"])
                      for pinned in chunk if same(pinned, before.get(pinned["key"]))}
        after = check("after")
        for pinned in chunk:
            key, outcome = pinned["key"], thumbnails.get(pinned["key"])
            if not (same(pinned, before.get(key)) and same(pinned, after.get(key))):
                catalog.fail(db, "file", key, "Commons file or credits changed while acquiring thumbnail")
            elif outcome["status"] != "ok":
                catalog.fail(db, "file", key, f"thumbnail {outcome['status']}")
            else:
                asset = work / "assets" / outcome["sha256"]
                asset.parent.mkdir(parents=True, exist_ok=True)
                if not asset.exists():
                    os.link(capture.root / outcome["path"], asset)
                catalog.put(db, dict(kind="file", key=key, status="present", identity=pinned["identity"], revision=pinned["file_revision"],
                                     filename=pinned["filename"], revision_before=witness(before[key]), revision_after=witness(after[key]),
                                     asset=dict(path=f"assets/{outcome['sha256']}", sha256=outcome["sha256"], bytes=outcome["bytes"],
                                                url=pinned["imageinfo"]["thumburl"], input="thumbnail500")))
        db.commit()
