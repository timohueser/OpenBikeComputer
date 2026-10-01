"""Portable planner release identity and file verification."""

import hashlib
import json
from pathlib import Path
import re
from urllib.request import Request, urlopen


def encoded(value):
    return (json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n").encode()


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def open_url(url, timeout=60):
    request = Request(url) if isinstance(url, str) else url
    request.add_header("User-Agent", "OpenBikeComputer/1.0")
    return urlopen(request, timeout=timeout)


def release(data, include_sources=True):
    path = data / "release.json"
    document = json.loads(path.read_bytes())
    identity = digest(path)
    if document["format"] != 1 or not re.fullmatch(r"[a-z][a-z0-9-]{0,63}", document["region"]):
        raise ValueError("Unsupported planner release")
    files = {**document["files"], **(document.get("source_files", {}) if include_sources else {})}
    for name, item in files.items():
        file = data / name
        if not file.resolve().is_relative_to(data.resolve()) or not re.fullmatch(r"[a-f0-9]{64}", item["sha256"]):
            raise ValueError("Invalid release file")
        if file.stat().st_size != item["bytes"] or digest(file) != item["sha256"]:
            raise ValueError(f"Release checksum mismatch: {name}")
    return identity, document
