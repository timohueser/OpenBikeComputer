"""Publish immutable planner objects and materialize runtime files from the object pool."""

import gzip
import os
from pathlib import Path
import shutil
import tempfile

try:
    from . import planner_runtime as runtime
except ImportError:
    import planner_runtime as runtime


COMPRESSED = {".bin", ".pmtiles", ".webp", ".png"}
CHUNK = 1024 * 1024


def sync_directory(path):
    descriptor = os.open(path, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def atomic_write(path, data):
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as stream:
        temporary = Path(stream.name)
        try:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
            temporary.replace(path)
            sync_directory(path.parent)
        finally:
            temporary.unlink(missing_ok=True)


def verify(path, item):
    if path.stat().st_size != item["bytes"] or runtime.digest(path) != item["sha256"]:
        raise ValueError(f"Checksum mismatch: {path.name}")


def item(path):
    return {"bytes": path.stat().st_size, "sha256": runtime.digest(path)}


def pack_file(source, objects):
    """Publish one immutable object. Compression runs only during publication."""
    original = item(source)
    objects.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".pack-", dir=objects.parent) as temporary:
        compressed = Path(temporary) / "object"
        if source.suffix not in COMPRESSED:
            with source.open("rb") as reader, compressed.open("wb") as target:
                with gzip.GzipFile(filename="", fileobj=target, mode="wb", compresslevel=6, mtime=0) as encoder:
                    shutil.copyfileobj(reader, encoder, CHUNK)
        if compressed.exists() and compressed.stat().st_size < original["bytes"]:
            transport = {**item(compressed), "encoding": "gzip"}
            target = objects / transport["sha256"]
            if not target.exists(): compressed.rename(target)
        else:
            transport = {**original, "encoding": "identity"}
            target = objects / original["sha256"]
            if not target.exists():
                try: os.link(source, target)
                except OSError: shutil.copyfile(source, target)
        verify(target, transport)
        with target.open("rb") as stream: os.fsync(stream.fileno())
    return {**original, "transport": transport}


def materialize(source, destination, prefixes):
    """Install only the server's runtime files from the canonical object pool."""
    _, document = runtime.release(source, include_sources=False)
    if not document.get("grid"):
        raise ValueError("Expected a grid publication")
    for name, entry in document["files"].items():
        if not name.startswith(prefixes): continue
        path = destination / name
        if path.exists():
            verify(path, entry)
            continue
        path.parent.mkdir(parents=True, exist_ok=True)
        transport = entry["transport"]
        original = source / "objects" / transport["sha256"]
        temporary = path.with_suffix(path.suffix + ".partial")
        try:
            if transport["encoding"] == "identity":
                try: os.link(original, temporary)
                except OSError: shutil.copyfile(original, temporary)
            elif transport["encoding"] == "gzip":
                with gzip.open(original, "rb") as reader, temporary.open("wb") as writer:
                    remaining = entry["bytes"]
                    while chunk := reader.read(min(CHUNK, remaining + 1)):
                        remaining -= len(chunk)
                        if remaining < 0: raise ValueError("Decoded object exceeds the release size")
                        writer.write(chunk)
            else:
                raise ValueError("Unsupported object encoding")
            verify(temporary, entry)
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)
    return document
