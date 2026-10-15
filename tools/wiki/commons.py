"""Commons facts and 500 px thumbnails for the photo candidates of the selected subjects.

Every request goes through `Capture` with its limits: four requests a second in series, `maxlag`,
`Retry-After`, three backoffs on 429 and 503, and media read at 3 MB/s. Capture retains each
response, so a run that stops, also on "Wikimedia is busy", resumes when it is called again.

The producer calls `acquire` once per round, each time with all requests so far: first the
categories, then the metadata of every photo candidate, then the files that the compilers
choose. A round asks only for what earlier rounds have not answered.
"""
from __future__ import annotations

import os
from pathlib import Path
import time

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


def keep(db, kind: str, results, total: int):
    """Each `(key, fact or reason)` of the `total` of `results`, committed and counted every thousand."""
    start = time.monotonic()
    for count, (key, value) in enumerate(results, 1):
        if isinstance(value, dict):
            catalog.put(db, value)
        else:
            catalog.fail(db, kind, key, value)
        if count % 1000 == 0:
            db.commit()
            print(f"commons {kind}: {count} of {total}{catalog.progress(start, count / total)}", flush=True)
    db.commit()


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
    db.commit()
    categories = [key for key in sorted(wanted["category"]) if not catalog.fact(db, "category", key)]
    keep(db, "category", ((key, category_members(fetch, key)) for key in categories), len(categories))
    files = [key for key in sorted(wanted["commons"]) if not catalog.fact(db, "commons", key)]
    keep(db, "commons", commons_metadata(fetch, files), len(files))
    media = set()
    for subject, key in db.execute("SELECT subject, key FROM uses WHERE kind='commons'").fetchall():
        value = catalog.fact(db, "commons", key)
        if value and value["status"] == "present":
            media.add(f"M{value['pageid']}")
            db.execute("INSERT OR IGNORE INTO uses VALUES(?,?,?)", (subject, "mediainfo", f"M{value['pageid']}"))
    media = [key for key in sorted(media) if not catalog.fact(db, "mediainfo", key)]
    keep(db, "mediainfo", mediainfo(fetch, media), len(media))
    photos(db, capture, work, sorted(wanted["file"]))


def same(one, other) -> bool:
    return isinstance(one, dict) and isinstance(other, dict) and all(one.get(field) == other.get(field) for field in ("identity", "revision", "file_revision"))


def thumbnail(value: dict) -> str | None:
    info = value.get("imageinfo") or {}
    url = info.get("thumburl") or ""
    return url if value.get("status") == "present" and url.startswith(THUMBNAIL) and info.get("thumbwidth") == 500 else None


def witness(value: dict) -> dict:
    revision = dict(timestamp=value["file_revision"]["timestamp"], sha1=value["file_revision"]["sha1"],
                    description_revision=value["revision"], extmetadata=value["imageinfo"].get("extmetadata"))
    return {"query": {"pages": {"compact": {"title": value["key"], "imageinfo": [revision]}}}}


def photos(db, capture: Capture, work: Path, titles: list[str]):
    """Each thumbnail sits between two fresh metadata checks, fifty files to a check. When the two
    checks agree they become the file's `commons` fact, even after a description edit; when they
    differ the file gets no photo in this version."""
    pending = {}
    for key in titles:
        pinned, kept = catalog.fact(db, "commons", key), catalog.fact(db, "file", key)
        if not pinned or pinned["status"] != "present" or (kept and kept["identity"] == pinned["identity"] and kept["revision"] == pinned["file_revision"]):
            continue
        if thumbnail(pinned):
            pending[key] = pinned
        else:
            catalog.fail(db, "file", key, "Commons file has no standard 500 px thumbnail")
    chunks, start = list(batches(sorted(pending))), time.monotonic()
    for number, chunk in enumerate(chunks, 1):
        prefix = "photos/" + digest(encoded([(key, pending[key]["revision"]) for key in chunk]))

        def check(phase):
            return dict(commons_metadata(lambda url: capture.json(f"{prefix}/{phase}-{digest(url.encode())}.json", url), chunk))

        before = check("before")
        thumbnails = {key: capture.fetch(f"{prefix}/{digest(key.encode())}", url)
                      for key, value in before.items() if isinstance(value, dict) and (url := thumbnail(value))}
        after = check("after")
        for key in chunk:
            current, outcome = before.get(key), thumbnails.get(key)
            if not same(current, after.get(key)):
                catalog.fail(db, "file", key, "Commons metadata changed or was unavailable while acquiring thumbnail")
            elif current["status"] == "missing":
                db.execute("DELETE FROM facts WHERE kind='commons' AND key=?", (key,))
                catalog.put(db, current)
                catalog.fail(db, "file", key, "Commons file was deleted")
            elif current["revision"] < pending[key]["revision"]:
                # A lagging replica: the newer stored fact stays, and the older checks are no witness.
                catalog.fail(db, "file", key, "Commons answered an older revision than the stored metadata")
            elif outcome is None:
                catalog.fail(db, "file", key, "Commons file has no standard 500 px thumbnail")
            elif outcome["status"] != "ok":
                catalog.fail(db, "file", key, f"thumbnail {outcome['status']}")
            else:
                if not same(current, pending[key]):
                    catalog.put(db, current)
                asset = work / "assets" / outcome["sha256"]
                asset.parent.mkdir(parents=True, exist_ok=True)
                if not asset.exists():
                    os.link(capture.root / outcome["path"], asset)
                catalog.put(db, dict(kind="file", key=key, status="present", identity=current["identity"], revision=current["file_revision"],
                                     filename=current["filename"], revision_before=witness(current), revision_after=witness(after[key]),
                                     asset=dict(path=f"assets/{outcome['sha256']}", sha256=outcome["sha256"], bytes=outcome["bytes"],
                                                url=thumbnail(current), input="thumbnail500")))
        db.commit()
        if number % 20 == 0:
            print(f"commons file: {number} of {len(chunks)} batches{catalog.progress(start, number / len(chunks))}", flush=True)
