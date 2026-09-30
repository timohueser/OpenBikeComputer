"""Pack and install verified planner releases with resumable immutable objects."""

import argparse
from contextlib import contextmanager
import fcntl
import gzip
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import sys
import tempfile
import time
from urllib.parse import urljoin, urlparse
from urllib.request import Request

try:
    from . import planner_runtime as runtime
except ImportError:
    import planner_runtime as runtime


COMPRESSED = {".bin", ".pmtiles", ".webp", ".png", ".pbf"}
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


def size_report(manifest, manifest_bytes):
    unique = {entry["sha256"]: entry for entry in manifest["files"].values()}
    wire = {entry["transport"]["sha256"]: entry["transport"]["bytes"] for entry in unique.values()}
    return {"transfer_bytes": sum(wire.values()) + manifest["release"]["bytes"] + manifest_bytes,
            "installed_bytes": sum(entry["bytes"] for entry in manifest["files"].values()) + manifest["release"]["bytes"],
            "unique_installed_bytes": sum(entry["bytes"] for entry in unique.values()) + manifest["release"]["bytes"],
            "objects": len(wire)}


def pack(data, destination):
    identity, document = runtime.release(data, include_sources=False)
    destination.mkdir(parents=True, exist_ok=True)
    if (destination / "bundle.json").exists():
        raise ValueError("Bundle already exists; choose a fresh output directory")
    objects = destination / "objects"
    objects.mkdir(exist_ok=True)
    files, cache = {}, {}
    for name, original in sorted(document["files"].items()):
        checksum = original["sha256"]
        if checksum not in cache:
            source = data / name
            if original["bytes"] >= 64 * CHUNK:
                print(f"Packing {name}", file=sys.stderr, flush=True)
            with tempfile.TemporaryDirectory(prefix=".pack-", dir=destination) as temporary:
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
                    target = objects / checksum
                    if not target.exists():
                        try:
                            os.link(source, target)
                        except OSError:
                            shutil.copyfile(source, target)
                verify(target, transport)
                with target.open("rb") as stream:
                    os.fsync(stream.fileno())
                cache[checksum] = transport
        files[name] = {**original, "transport": cache[checksum]}
    release_bytes = (data / "release.json").read_bytes()
    manifest = {"format": 1, "release": {"sha256": identity, "bytes": len(release_bytes)}, "files": files}
    encoded = runtime.encoded(manifest)
    sync_directory(objects)
    atomic_write(destination / "release.json", release_bytes)
    atomic_write(destination / "bundle.json", encoded)
    return {"release": identity, **size_report(manifest, len(encoded))}


def metadata(source, name):
    if urlparse(str(source)).scheme in {"https", "http"}:
        with runtime.open_url(urljoin(str(source).rstrip("/") + "/", name)) as response:
            data = response.read(16 * CHUNK + 1)
    else:
        with (Path(source) / name).open("rb") as stream:
            data = stream.read(16 * CHUNK + 1)
    if len(data) > 16 * CHUNK:
        raise ValueError("Bundle metadata is too large")
    return data


def validate(manifest, release_bytes):
    expected = manifest["release"]
    if manifest["format"] != 1 or len(release_bytes) != expected["bytes"] or hashlib.sha256(release_bytes).hexdigest() != expected["sha256"]:
        raise ValueError("Invalid bundle release identity")
    document = json.loads(release_bytes)
    if document["format"] != 1 or not re.fullmatch(r"[a-z][a-z0-9-]{0,63}", document["region"]) or set(document["files"]) != set(manifest["files"]):
        raise ValueError("Bundle does not contain the complete runtime release")
    for name, entry in manifest["files"].items():
        path = PurePosixPath(name)
        if not path.parts or path.is_absolute() or ".." in path.parts or str(path) != name or name == "release.json":
            raise ValueError("Invalid release path")
        if {key: entry[key] for key in ("bytes", "sha256")} != document["files"][name]:
            raise ValueError("Bundle file does not match the release")
        for record in (entry, entry["transport"]):
            if not re.fullmatch(r"[a-f0-9]{64}", record["sha256"]) or type(record["bytes"]) is not int or record["bytes"] < 0:
                raise ValueError("Invalid bundle object")
        if entry["transport"]["encoding"] not in {"identity", "gzip"}:
            raise ValueError("Unsupported object encoding")
        if entry["transport"]["encoding"] == "identity" and any(entry[key] != entry["transport"][key] for key in ("bytes", "sha256")):
            raise ValueError("Identity transport differs from the release")
    return document


def verify_bundle(source):
    started = time.monotonic()
    bundle_bytes = metadata(source, "bundle.json")
    manifest = json.loads(bundle_bytes)
    validate(manifest, metadata(source, "release.json"))
    entries = {entry["sha256"]: entry for entry in manifest["files"].values()}
    for entry in entries.values():
        transport = entry["transport"]
        path = source / "objects" / transport["sha256"]
        verify(path, transport)
        if transport["encoding"] == "gzip":
            checksum, size = hashlib.sha256(), 0
            with gzip.open(path, "rb") as reader:
                while chunk := reader.read(CHUNK):
                    checksum.update(chunk)
                    size += len(chunk)
                    if size > entry["bytes"]:
                        raise ValueError("Decoded object exceeds the release size")
            if size != entry["bytes"] or checksum.hexdigest() != entry["sha256"]:
                raise ValueError("Decoded object does not match the release")
    return {"release": manifest["release"]["sha256"], **size_report(manifest, len(bundle_bytes)),
            "verified_objects": len(entries), "verified_files": len(manifest["files"]),
            "verify_seconds": time.monotonic() - started}


def fetch_object(source, transport, partial):
    offset = partial.stat().st_size if partial.exists() else 0
    if offset > transport["bytes"]:
        raise ValueError("Partial download exceeds the object size")
    received = 0
    if transport["bytes"] == 0:
        partial.touch(exist_ok=True)
    if offset < transport["bytes"]:
        name = "objects/" + transport["sha256"]
        if urlparse(str(source)).scheme in {"https", "http"}:
            headers = {"Accept-Encoding": "identity"}
            if offset: headers["Range"] = f"bytes={offset}-"
            reader = runtime.open_url(Request(urljoin(str(source).rstrip("/") + "/", name), headers=headers))
            if reader.status == 206:
                expected = f"bytes {offset}-{transport['bytes'] - 1}/{transport['bytes']}"
                if reader.headers.get("Content-Range") != expected:
                    reader.close()
                    raise ValueError("Invalid download range")
            elif reader.status == 200:
                offset = 0
            else:
                reader.close()
                raise ValueError("Unexpected download response")
        else:
            reader = (Path(source) / name).open("rb")
            reader.seek(offset)
        with reader, partial.open("ab" if offset else "wb") as output:
            while chunk := reader.read(CHUNK):
                if output.tell() + len(chunk) > transport["bytes"]:
                    raise ValueError("Download exceeds the object size")
                output.write(chunk)
                received += len(chunk)
            output.flush()
            os.fsync(output.fileno())
    if partial.stat().st_size < transport["bytes"]:
        raise ValueError("Incomplete object download; rerun to resume")
    try:
        verify(partial, transport)
    except ValueError:
        partial.unlink(missing_ok=True)
        raise
    return received


def stored_bytes(root):
    files = {}
    for path in root.rglob("*"):
        if path.is_file():
            stat = path.stat()
            files[(stat.st_dev, stat.st_ino)] = stat.st_size
    return sum(files.values())


class StorageUsage:
    """Count unique inodes while the installer holds its exclusive lock."""

    def __init__(self, root):
        self.paths, self.inodes = {}, {}
        self.size = self.peak = 0
        for path in root.rglob("*"):
            if path.is_file(): self.refresh(path)

    def refresh(self, *paths):
        for path in paths:
            previous = self.paths.pop(path, None)
            if previous is not None:
                record = self.inodes[previous]
                record[1] -= 1
                if not record[1]:
                    self.size -= record[0]
                    del self.inodes[previous]
            if path.is_file():
                stat = path.stat()
                identity = (stat.st_dev, stat.st_ino)
                record = self.inodes.setdefault(identity, [0, 0])
                self.size += stat.st_size - record[0]
                record[0] = stat.st_size
                record[1] += 1
                self.paths[path] = identity
        self.reserve(0)

    def reserve(self, size):
        # Atomic replacement retains the old inode while the new bytes are written.
        self.peak = max(self.peak, self.size + size)


@contextmanager
def install_lock(root):
    root.mkdir(parents=True, exist_ok=True)
    with (root / ".lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        yield


def install(source, root):
    bundle_bytes = metadata(source, "bundle.json")
    release_bytes = metadata(source, "release.json")
    manifest = json.loads(bundle_bytes)
    document = validate(manifest, release_bytes)
    identity = manifest["release"]["sha256"]
    with install_lock(root):
        objects, downloads, releases_dir = (root / name for name in ("objects", "downloads", "releases"))
        for path in (objects, downloads, releases_dir): path.mkdir(exist_ok=True)
        usage = StorageUsage(root)
        initial = usage.size
        received = len(bundle_bytes) + len(release_bytes)
        for entry in {entry["sha256"]: entry for entry in manifest["files"].values()}.values():
            target = objects / entry["sha256"]
            if target.exists():
                verify(target, entry)
                continue
            transport = entry["transport"]
            partial = downloads / transport["sha256"]
            received += fetch_object(source, transport, partial)
            usage.refresh(partial)
            if transport["encoding"] == "identity":
                verify(partial, entry)
                partial.replace(target)
                usage.refresh(partial, target)
            else:
                decoded = objects / (entry["sha256"] + ".part")
                try:
                    with gzip.open(partial, "rb") as reader, decoded.open("wb") as output:
                        while chunk := reader.read(CHUNK):
                            if output.tell() + len(chunk) > entry["bytes"]:
                                raise ValueError("Decoded object exceeds the release size")
                            output.write(chunk)
                        output.flush()
                        os.fsync(output.fileno())
                    usage.refresh(decoded)
                    verify(decoded, entry)
                    decoded.replace(target)
                    partial.unlink()
                    usage.refresh(decoded, target, partial)
                finally:
                    decoded.unlink(missing_ok=True)
            sync_directory(objects)
        destination = releases_dir / identity
        if not destination.exists():
            with tempfile.TemporaryDirectory(prefix=".install-", dir=releases_dir) as temporary:
                stage = Path(temporary)
                for name, entry in manifest["files"].items():
                    target = stage / name
                    target.parent.mkdir(parents=True, exist_ok=True)
                    os.link(objects / entry["sha256"], target)
                # Release paths hard-link existing objects; only its metadata adds bytes.
                usage.reserve(len(release_bytes))
                atomic_write(stage / "release.json", release_bytes)
                usage.refresh(stage / "release.json")
                for directory in sorted((p for p in stage.rglob("*") if p.is_dir()), reverse=True):
                    sync_directory(directory)
                sync_directory(stage)
                stage.rename(destination)
                usage.refresh(stage / "release.json", destination / "release.json")
                sync_directory(releases_dir)
        installed_id, _ = runtime.release(destination, include_sources=False)
        if installed_id != identity:
            raise ValueError("Installed release identity mismatch")
        active = runtime.encoded({"release": identity, "region": document["region"]})
        usage.reserve(len(active))
        atomic_write(root / "active.json", active)
        final = stored_bytes(root)
        return {"release": identity, **size_report(manifest, len(bundle_bytes)), "downloaded_bytes": received,
                "retained_before_bytes": initial, "stored_bytes": final,
                "peak_install_bytes": max(usage.peak, final), "peak_added_bytes": max(usage.peak, final) - initial,
                "added_stored_bytes": final - initial, "download_cache_bytes": stored_bytes(downloads)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    build = commands.add_parser("pack")
    build.add_argument("release", type=Path)
    build.add_argument("output", type=Path)
    load = commands.add_parser("install")
    load.add_argument("bundle", help="Local bundle directory or HTTP(S) base URL")
    load.add_argument("output", type=Path)
    check = commands.add_parser("verify")
    check.add_argument("bundle", type=Path)
    args = parser.parse_args()
    try:
        if args.command == "pack":
            result = pack(args.release, args.output)
        elif args.command == "verify":
            result = verify_bundle(args.bundle)
        else:
            result = install(args.bundle, args.output)
        print(json.dumps(result, sort_keys=True))
    except (OSError, ValueError, KeyError) as error:
        parser.exit(1, f"planner offline: {error}\n")


if __name__ == "__main__":
    main()
