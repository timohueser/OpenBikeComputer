#!/usr/bin/env python3
"""OBC's content-addressed developer-fixture registry.

The tracked catalog describes immutable packages, runnable simulator scenarios,
and convenience profiles. Package bytes live outside Git; this tool downloads,
verifies, safely extracts, caches, resolves, packs, and prunes them.
"""

from __future__ import annotations

import argparse
import contextlib
import fcntl
import gzip
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
from typing import Iterable
from urllib.error import HTTPError, URLError
from urllib.parse import urljoin, urlparse
from urllib.request import Request, urlopen


CATALOG_SCHEMA = 1
PACKAGE_SCHEMA = 1
MANIFEST_NAME = ".obc-package.json"
BUFFER_BYTES = 1024 * 1024


class FixtureError(RuntimeError):
    pass


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(BUFFER_BYTES), b""):
            digest.update(chunk)
    return digest.hexdigest()


def repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


def cache_root() -> Path:
    override = os.environ.get("OBC_FIXTURE_CACHE")
    if override:
        return Path(override).expanduser().resolve()
    xdg = os.environ.get("XDG_CACHE_HOME")
    base = Path(xdg).expanduser() if xdg else Path.home() / ".cache"
    return (base / "openbikecomputer" / "fixtures").resolve()


def _table(document: dict, key: str) -> dict:
    value = document.get(key, {})
    if not isinstance(value, dict):
        raise FixtureError(f"catalog {key!r} must be a table")
    return value


class Catalog:
    def __init__(self, path: Path):
        self.path = path
        try:
            self.document = tomllib.loads(path.read_text(encoding="utf-8"))
        except (OSError, tomllib.TOMLDecodeError) as error:
            raise FixtureError(f"cannot read fixture catalog {path}: {error}") from error
        if self.document.get("schema") != CATALOG_SCHEMA:
            raise FixtureError(f"catalog schema must be {CATALOG_SCHEMA}")
        base_url = self.document.get("base_url")
        if not isinstance(base_url, str) or not base_url.endswith("/"):
            raise FixtureError("catalog base_url must be a string ending in '/'")
        parsed = urlparse(base_url)
        if parsed.scheme != "https" or not parsed.netloc:
            raise FixtureError("catalog base_url must be an absolute HTTPS URL")
        self.base_url = base_url
        self.packages = _table(self.document, "packages")
        self.scenarios = _table(self.document, "scenarios")
        self.profiles = _table(self.document, "profiles")
        self._validate()

    def _validate(self) -> None:
        digests: dict[str, str] = {}
        for package_id, package in self.packages.items():
            _identifier("package", package_id)
            if not isinstance(package, dict):
                raise FixtureError(f"package {package_id!r} must be a table")
            digest = package.get("sha256")
            if not isinstance(digest, str) or len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest):
                raise FixtureError(f"package {package_id!r} has an invalid sha256")
            size = package.get("bytes")
            if not isinstance(size, int) or size <= 0:
                raise FixtureError(f"package {package_id!r} bytes must be positive")
            archive = package.get("archive")
            if not isinstance(archive, str) or _unsafe_relpath(archive) or ":" in archive or "\\" in archive:
                raise FixtureError(f"package {package_id!r} archive must be a safe relative path")
            if archive != f"packages/{digest}.tar.gz":
                raise FixtureError(f"package {package_id!r} archive must be the content-addressed key packages/{digest}.tar.gz")
            for required in ("summary", "provenance", "license"):
                if not isinstance(package.get(required), str) or not package[required].strip():
                    raise FixtureError(f"package {package_id!r} requires non-empty {required}")
            tracked_sources = package.get("tracked_sources", {})
            if not isinstance(tracked_sources, dict):
                raise FixtureError(f"package {package_id!r} tracked_sources must be a table")
            for packaged_path, source_path in tracked_sources.items():
                if _unsafe_relpath(packaged_path) or not isinstance(source_path, str) or _unsafe_relpath(source_path):
                    raise FixtureError(f"package {package_id!r} has an unsafe tracked source mapping")
            if digest in digests:
                raise FixtureError(
                    f"packages {digests[digest]!r} and {package_id!r} share a digest; "
                    "one immutable package must have one identity"
                )
            digests[digest] = package_id

        for scenario_id, scenario in self.scenarios.items():
            _identifier("scenario", scenario_id)
            if not isinstance(scenario, dict):
                raise FixtureError(f"scenario {scenario_id!r} must be a table")
            if not isinstance(scenario.get("summary"), str) or not scenario["summary"].strip():
                raise FixtureError(f"scenario {scenario_id!r} requires a summary")
            package_ids = scenario.get("packages")
            if not isinstance(package_ids, list) or not package_ids:
                raise FixtureError(f"scenario {scenario_id!r} requires packages")
            for package_id in package_ids:
                if package_id not in self.packages:
                    raise FixtureError(f"scenario {scenario_id!r} names unknown package {package_id!r}")
            if ("map" in scenario) == ("map_set" in scenario):
                raise FixtureError(f"scenario {scenario_id!r} requires exactly one of map or map_set")
            for field in ("map", "map_set", "gpx", "weather", "routes_dir", "tracks_dir"):
                if field in scenario:
                    self.resolve_ref_shape(scenario_id, package_ids, field, scenario[field])
            args = scenario.get("args", [])
            if not isinstance(args, list) or not all(isinstance(arg, str) for arg in args):
                raise FixtureError(f"scenario {scenario_id!r} args must be an array of strings")

        for profile_id, profile in self.profiles.items():
            _identifier("profile", profile_id)
            if not isinstance(profile, dict):
                raise FixtureError(f"profile {profile_id!r} must be a table")
            if not isinstance(profile.get("summary"), str) or not profile["summary"].strip():
                raise FixtureError(f"profile {profile_id!r} requires a summary")
            scenarios = profile.get("scenarios", [])
            packages = profile.get("packages", [])
            if not isinstance(scenarios, list) or not all(isinstance(item, str) for item in scenarios):
                raise FixtureError(f"profile {profile_id!r} scenarios must be an array of strings")
            if not isinstance(packages, list) or not all(isinstance(item, str) for item in packages):
                raise FixtureError(f"profile {profile_id!r} packages must be an array of strings")
            if not scenarios and not packages:
                raise FixtureError(f"profile {profile_id!r} must contain scenarios or packages")
            for scenario_id in scenarios:
                if scenario_id not in self.scenarios:
                    raise FixtureError(f"profile {profile_id!r} names unknown scenario {scenario_id!r}")
            for package_id in packages:
                if package_id not in self.packages:
                    raise FixtureError(f"profile {profile_id!r} names unknown package {package_id!r}")

    def resolve_ref_shape(self, scenario_id: str, package_ids: list[str], field: str, value: object) -> None:
        if not isinstance(value, str) or ":" not in value:
            raise FixtureError(f"scenario {scenario_id!r} {field} must be PACKAGE:PATH")
        package_id, relative = value.split(":", 1)
        if package_id not in package_ids:
            raise FixtureError(f"scenario {scenario_id!r} {field} uses undeclared package {package_id!r}")
        if _unsafe_relpath(relative):
            raise FixtureError(f"scenario {scenario_id!r} {field} contains an unsafe path")

    def package_ids_for(self, targets: Iterable[str]) -> list[str]:
        result: list[str] = []

        def add(package_id: str) -> None:
            if package_id not in result:
                result.append(package_id)

        for target in targets:
            if target in self.packages:
                add(target)
            elif target in self.scenarios:
                for package_id in self.scenarios[target]["packages"]:
                    add(package_id)
            elif target in self.profiles:
                profile = self.profiles[target]
                for package_id in profile.get("packages", []):
                    add(package_id)
                for scenario_id in profile.get("scenarios", []):
                    for package_id in self.scenarios[scenario_id]["packages"]:
                        add(package_id)
            else:
                raise FixtureError(f"unknown fixture package, scenario, or profile: {target}")
        return result


def _identifier(kind: str, value: str) -> None:
    if not value or not value.isascii() or any(not ("a" <= c <= "z" or "0" <= c <= "9" or c in "-_") for c in value):
        raise FixtureError(f"{kind} id {value!r} must use lowercase letters, digits, '-' or '_'")


def _unsafe_relpath(value: str) -> bool:
    path = PurePosixPath(value)
    return not value or "\\" in value or path.is_absolute() or ".." in path.parts or "." in path.parts


class Store:
    def __init__(self, catalog: Catalog, root: Path):
        self.catalog = catalog
        self.root = root
        self.archives = root / "archives"
        self.packages = root / "packages"
        self.by_id = root / "by-id"
        self.locks = root / "locks"

    def package_root(self, package_id: str) -> Path:
        return self.packages / self.catalog.packages[package_id]["sha256"]

    def archive_path(self, package_id: str) -> Path:
        return self.archives / f"{self.catalog.packages[package_id]['sha256']}.tar.gz"

    def package_ready(self, package_id: str) -> bool:
        package = self.catalog.packages[package_id]
        root = self.package_root(package_id)
        try:
            marker = json.loads((root / ".obc-ready.json").read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return False
        return marker == {"package": package_id, "sha256": package["sha256"]}

    @contextlib.contextmanager
    def lock(self, digest: str):
        self.locks.mkdir(parents=True, exist_ok=True)
        with (self.locks / f"{digest}.lock").open("a+b") as handle:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX)
            yield

    def sync(self, package_id: str) -> bool:
        package = self.catalog.packages[package_id]
        digest = package["sha256"]
        with self.lock(digest):
            if self.package_ready(package_id):
                try:
                    self.verify(package_id)
                except FixtureError:
                    shutil.rmtree(self.package_root(package_id), ignore_errors=True)
                    archive = self.archive_path(package_id)
                    package = self.catalog.packages[package_id]
                    if not (
                        archive.is_file()
                        and archive.stat().st_size == package["bytes"]
                        and sha256_file(archive) == package["sha256"]
                    ):
                        archive.unlink(missing_ok=True)
                else:
                    self._materialize_id(package_id)
                    return False
            archive = self._ensure_archive(package_id)
            self._extract(package_id, archive)
            self._materialize_id(package_id)
        return True

    def _materialize_id(self, package_id: str) -> None:
        self.by_id.mkdir(parents=True, exist_ok=True)
        destination = self.by_id / package_id
        temporary = self.by_id / f".{package_id}.{os.getpid()}"
        temporary.unlink(missing_ok=True)
        temporary.symlink_to(self.package_root(package_id), target_is_directory=True)
        if destination.is_symlink() or destination.is_file():
            destination.unlink()
        elif destination.exists():
            shutil.rmtree(destination)
        os.replace(temporary, destination)

    def _ensure_archive(self, package_id: str) -> Path:
        package = self.catalog.packages[package_id]
        expected_digest = package["sha256"]
        expected_bytes = package["bytes"]
        destination = self.archive_path(package_id)
        destination.parent.mkdir(parents=True, exist_ok=True)
        if destination.is_file() and destination.stat().st_size == expected_bytes and sha256_file(destination) == expected_digest:
            return destination
        destination.unlink(missing_ok=True)
        url = urljoin(self.catalog.base_url, package["archive"])
        parsed = urlparse(url)
        if parsed.scheme != "https":
            raise FixtureError(f"refusing non-HTTPS fixture URL: {url}")
        temporary = destination.with_name(f".{destination.name}.{os.getpid()}.part")
        temporary.unlink(missing_ok=True)
        digest = hashlib.sha256()
        written = 0
        try:
            request = Request(url, headers={"User-Agent": "OpenBikeComputer-fixtures/1"})
            with urlopen(request, timeout=60) as response, temporary.open("wb") as output:
                while chunk := response.read(BUFFER_BYTES):
                    output.write(chunk)
                    digest.update(chunk)
                    written += len(chunk)
                output.flush()
                os.fsync(output.fileno())
        except (HTTPError, URLError, TimeoutError, OSError) as error:
            temporary.unlink(missing_ok=True)
            raise FixtureError(f"download failed for {package_id} ({url}): {error}") from error
        if written != expected_bytes or digest.hexdigest() != expected_digest:
            temporary.unlink(missing_ok=True)
            raise FixtureError(
                f"downloaded {package_id} failed verification: got {written} bytes/{digest.hexdigest()}, "
                f"expected {expected_bytes}/{expected_digest}"
            )
        os.replace(temporary, destination)
        return destination

    def _extract(self, package_id: str, archive: Path) -> None:
        destination = self.package_root(package_id)
        destination.parent.mkdir(parents=True, exist_ok=True)
        temporary = Path(tempfile.mkdtemp(prefix=f".{destination.name}.", dir=destination.parent))
        try:
            extract_package_archive(archive, temporary, package_id)
            (temporary / ".obc-ready.json").write_text(
                json.dumps({"package": package_id, "sha256": self.catalog.packages[package_id]["sha256"]}) + "\n",
                encoding="utf-8",
            )
            if destination.exists():
                shutil.rmtree(destination)
            os.replace(temporary, destination)
        except Exception:
            shutil.rmtree(temporary, ignore_errors=True)
            raise

    def verify(self, package_id: str) -> None:
        package = self.catalog.packages[package_id]
        archive = self.archive_path(package_id)
        if not archive.is_file():
            raise FixtureError(f"{package_id}: archive is not cached")
        if archive.stat().st_size != package["bytes"] or sha256_file(archive) != package["sha256"]:
            raise FixtureError(f"{package_id}: cached archive digest or size differs from the catalog")
        verify_package_tree(self.package_root(package_id), package_id)
        for packaged_path, source_path in package.get("tracked_sources", {}).items():
            packaged = self.package_root(package_id) / packaged_path
            source = self.catalog.path.parent / source_path
            if not source.is_file():
                raise FixtureError(f"{package_id}: tracked source is missing: {source_path}")
            if packaged.stat().st_size != source.stat().st_size or sha256_file(packaged) != sha256_file(source):
                raise FixtureError(
                    f"{package_id}: tracked source {source_path} differs from packaged file {packaged_path}; "
                    "rebuild and republish the package"
                )

    def prune(self, apply: bool) -> tuple[list[Path], int]:
        reachable = {package["sha256"] for package in self.catalog.packages.values()}
        stale: list[Path] = []
        total = 0
        for parent in (self.archives, self.packages):
            if not parent.is_dir():
                continue
            for path in parent.iterdir():
                digest = path.name.removesuffix(".tar.gz")
                if digest not in reachable:
                    stale.append(path)
                    total += _disk_bytes(path)
        if apply:
            for path in stale:
                shutil.rmtree(path) if path.is_dir() else path.unlink(missing_ok=True)
            if self.by_id.is_dir():
                for path in self.by_id.iterdir():
                    package_id = path.name
                    current = package_id in self.catalog.packages
                    correctly_linked = (
                        current
                        and path.is_symlink()
                        and path.resolve(strict=False) == self.package_root(package_id).resolve(strict=False)
                        and self.package_ready(package_id)
                    )
                    if not correctly_linked:
                        shutil.rmtree(path) if path.is_dir() and not path.is_symlink() else path.unlink(missing_ok=True)
        return stale, total


def verify_package_tree(root: Path, expected_package: str) -> None:
    try:
        manifest = json.loads((root / MANIFEST_NAME).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise FixtureError(f"{expected_package}: missing or invalid {MANIFEST_NAME}: {error}") from error
    if manifest.get("schema") != PACKAGE_SCHEMA or manifest.get("package") != expected_package:
        raise FixtureError(f"{expected_package}: package manifest identity does not match")
    files = manifest.get("files")
    if not isinstance(files, list):
        raise FixtureError(f"{expected_package}: package manifest files must be an array")
    expected_paths = {MANIFEST_NAME, ".obc-ready.json"}
    for entry in files:
        if not isinstance(entry, dict):
            raise FixtureError(f"{expected_package}: malformed file record")
        relative = entry.get("path")
        if not isinstance(relative, str) or _unsafe_relpath(relative):
            raise FixtureError(f"{expected_package}: unsafe file record {relative!r}")
        path = root / relative
        if not path.is_file() or path.is_symlink():
            raise FixtureError(f"{expected_package}: missing regular file {relative}")
        if path.stat().st_size != entry.get("bytes") or sha256_file(path) != entry.get("sha256"):
            raise FixtureError(f"{expected_package}: file verification failed for {relative}")
        expected_paths.add(relative)
    actual_paths = {
        path.relative_to(root).as_posix()
        for path in root.rglob("*")
        if path.is_file() and not path.is_symlink()
    }
    if actual_paths != expected_paths and actual_paths != expected_paths - {".obc-ready.json"}:
        extra = sorted(actual_paths - expected_paths)
        missing = sorted((expected_paths - {".obc-ready.json"}) - actual_paths)
        raise FixtureError(f"{expected_package}: package file set differs (extra={extra}, missing={missing})")


def extract_package_archive(archive: Path, destination: Path, expected_package: str) -> None:
    try:
        with tarfile.open(archive, "r:gz") as package_tar:
            members = package_tar.getmembers()
            for member in members:
                if _unsafe_relpath(member.name) or not (member.isfile() or member.isdir()):
                    raise FixtureError(f"package {expected_package} contains unsafe member {member.name!r}")
            package_tar.extractall(destination, members=members, filter="data")
    except (OSError, tarfile.TarError) as error:
        raise FixtureError(f"cannot read package archive {archive}: {error}") from error
    verify_package_tree(destination, expected_package)


def build_package(package_id: str, source: Path, output: Path) -> tuple[int, str]:
    if not source.is_dir():
        raise FixtureError(f"package source is not a directory: {source}")
    files = []
    for path in sorted(source.rglob("*")):
        if path.is_symlink():
            raise FixtureError(f"package source contains a symlink: {path}")
        if path.is_file() and path.name not in {MANIFEST_NAME, ".obc-ready.json"}:
            files.append(
                {
                    "path": path.relative_to(source).as_posix(),
                    "bytes": path.stat().st_size,
                    "sha256": sha256_file(path),
                }
            )
    if not files:
        raise FixtureError("refusing to build an empty package")
    manifest = json.dumps({"schema": PACKAGE_SCHEMA, "package": package_id, "files": files}, indent=2) + "\n"
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_name(f".{output.name}.{os.getpid()}.part")
    try:
        with temporary.open("wb") as raw:
            with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as zipped:
                with tarfile.open(fileobj=zipped, mode="w") as package_tar:
                    manifest_bytes = manifest.encode()
                    info = tarfile.TarInfo(MANIFEST_NAME)
                    info.size = len(manifest_bytes)
                    _normalize_tar_info(info)
                    package_tar.addfile(info, io.BytesIO(manifest_bytes))
                    for entry in files:
                        path = source / entry["path"]
                        info = package_tar.gettarinfo(str(path), arcname=entry["path"])
                        _normalize_tar_info(info)
                        with path.open("rb") as contents:
                            package_tar.addfile(info, contents)
            raw.flush()
            os.fsync(raw.fileno())
        os.replace(temporary, output)
    except Exception:
        temporary.unlink(missing_ok=True)
        raise
    return output.stat().st_size, sha256_file(output)


def _normalize_tar_info(info: tarfile.TarInfo) -> None:
    info.uid = info.gid = 0
    info.uname = info.gname = ""
    info.mtime = 0
    info.mode = 0o644


def _disk_bytes(path: Path) -> int:
    if path.is_file():
        return path.stat().st_size
    return sum(item.stat().st_size for item in path.rglob("*") if item.is_file())


def _human_bytes(value: int) -> str:
    for unit in ("B", "KiB", "MiB", "GiB"):
        if value < 1024 or unit == "GiB":
            return f"{value:.0f} {unit}" if unit == "B" else f"{value:.1f} {unit}"
        value /= 1024
    raise AssertionError


def resolve_path(catalog: Catalog, store: Store, reference: str, field: str) -> Path:
    package_id, relative = reference.split(":", 1)
    path = store.package_root(package_id) / relative
    wants_directory = field in {"routes_dir", "tracks_dir"}
    if wants_directory and not path.is_dir():
        raise FixtureError(f"resolved fixture {field} is not a directory: {reference} ({path})")
    if not wants_directory and not path.is_file():
        raise FixtureError(f"resolved fixture {field} is not a file: {reference} ({path})")
    return path


def command_list(catalog: Catalog, store: Store, _args: argparse.Namespace) -> None:
    print("SCENARIOS\nID                           STATE      DOWNLOAD  DESCRIPTION")
    for scenario_id, scenario in catalog.scenarios.items():
        package_ids = scenario["packages"]
        ready = all(store.package_ready(package_id) for package_id in package_ids)
        missing_bytes = sum(catalog.packages[p]["bytes"] for p in package_ids if not store.package_ready(p))
        print(f"{scenario_id:<28} {'ready' if ready else 'missing':<10} {_human_bytes(missing_bytes):>9}  {scenario['summary']}")
    print("\nPROFILES")
    for profile_id, profile in catalog.profiles.items():
        package_ids = catalog.package_ids_for([profile_id])
        missing_bytes = sum(catalog.packages[p]["bytes"] for p in package_ids if not store.package_ready(p))
        print(f"{profile_id:<28} {_human_bytes(missing_bytes):>20}  {profile['summary']}")
    print("\nPACKAGES")
    for package_id, package in catalog.packages.items():
        state = "ready" if store.package_ready(package_id) else "missing"
        print(f"{package_id:<28} {state:<10} {_human_bytes(package['bytes']):>9}  {package['summary']}")


def command_show(catalog: Catalog, store: Store, args: argparse.Namespace) -> None:
    target = args.target
    if target in catalog.scenarios:
        scenario = catalog.scenarios[target]
        print(f"scenario: {target}\n{scenario['summary']}\n")
        print("packages:")
        for package_id in scenario["packages"]:
            package = catalog.packages[package_id]
            state = "ready" if store.package_ready(package_id) else "missing"
            print(f"  {package_id:<24} {_human_bytes(package['bytes']):>9}  {state:<7}  {package['summary']}")
        print("\ninputs:")
        for field in ("map", "map_set", "gpx", "weather", "routes_dir", "tracks_dir"):
            if field in scenario:
                print(f"  {field:<12} {scenario[field]}")
        for value in scenario.get("args", []):
            print(f"  arg          {value}")
        print(f"\nrun: obc sim {target}")
    elif target in catalog.packages:
        package = catalog.packages[target]
        print(f"package: {target}\n{package['summary']}")
        print(f"archive: {package['archive']}\nbytes: {package['bytes']}\nsha256: {package['sha256']}")
        print(f"provenance: {package['provenance']}\nlicense: {package['license']}")
    elif target in catalog.profiles:
        profile = catalog.profiles[target]
        print(f"profile: {target}\n{profile.get('summary', '')}")
        print("scenarios: " + ", ".join(profile.get("scenarios", [])))
        print("packages: " + ", ".join(profile.get("packages", [])))
    else:
        raise FixtureError(f"unknown fixture package, scenario, or profile: {target}")


def command_sync(catalog: Catalog, store: Store, args: argparse.Namespace) -> None:
    package_ids = catalog.package_ids_for(args.targets)
    for package_id in package_ids:
        package = catalog.packages[package_id]
        cached = store.package_ready(package_id)
        if not cached:
            print(f"↓ {package_id} ({_human_bytes(package['bytes'])})", file=sys.stderr)
        changed = store.sync(package_id)
        print(f"✓ {package_id}{'' if changed else ' (cached, verified)'}")


def command_verify(catalog: Catalog, store: Store, args: argparse.Namespace) -> None:
    targets = args.targets or ["all"]
    package_ids = list(catalog.packages) if targets == ["all"] else catalog.package_ids_for(targets)
    for package_id in package_ids:
        store.verify(package_id)
        print(f"✓ {package_id}")


def command_resolve(catalog: Catalog, store: Store, args: argparse.Namespace) -> None:
    scenario = catalog.scenarios.get(args.scenario)
    if scenario is None:
        raise FixtureError(f"unknown fixture scenario: {args.scenario}")
    if not args.no_sync:
        for package_id in scenario["packages"]:
            store.sync(package_id)
    for field in ("map", "map_set", "gpx", "weather", "routes_dir", "tracks_dir"):
        if field in scenario:
            print(f"{field}\t{resolve_path(catalog, store, scenario[field], field)}")
    for value in scenario.get("args", []):
        print(f"arg\t{value}")


def command_pack(_catalog: Catalog, _store: Store, args: argparse.Namespace) -> None:
    _identifier("package", args.package)
    output = args.output or Path(f"{args.package}.tar.gz")
    size, digest = build_package(args.package, args.source.resolve(), output.resolve())
    print(f"wrote {output.resolve()}\nbytes = {size}\nsha256 = \"{digest}\"")


def command_prune(_catalog: Catalog, store: Store, args: argparse.Namespace) -> None:
    stale, total = store.prune(args.apply)
    for path in stale:
        print(("deleted " if args.apply else "would delete ") + str(path))
    suffix = "removed" if args.apply else "reclaimable (repeat with --apply)"
    print(f"{_human_bytes(total)} {suffix}")


def command_publish(catalog: Catalog, _store: Store, args: argparse.Namespace) -> None:
    package = catalog.packages.get(args.package)
    if package is None:
        raise FixtureError(f"unknown fixture package: {args.package}")
    archive = args.archive.resolve()
    if not archive.is_file():
        raise FixtureError(f"archive does not exist: {archive}")
    actual_bytes = archive.stat().st_size
    actual_digest = sha256_file(archive)
    if actual_bytes != package["bytes"] or actual_digest != package["sha256"]:
        raise FixtureError(
            f"archive does not match catalog package {args.package}: got {actual_bytes}/{actual_digest}, "
            f"expected {package['bytes']}/{package['sha256']}"
        )
    with tempfile.TemporaryDirectory(prefix="obc-fixture-publish-") as scratch:
        extract_package_archive(archive, Path(scratch), args.package)
    public_url = urljoin(catalog.base_url, package["archive"])
    try:
        _verify_public_object(public_url, actual_bytes, actual_digest)
    except FixtureError:
        pass
    else:
        print(f"✓ {args.package} already exists and is verified through {catalog.base_url}")
        return
    required = (
        "OBC_FIXTURE_R2_BUCKET",
        "OBC_FIXTURE_R2_ACCESS_KEY_ID",
        "OBC_FIXTURE_R2_SECRET_ACCESS_KEY",
    )
    missing = [name for name in required if not os.environ.get(name)]
    endpoint = os.environ.get("OBC_FIXTURE_R2_ENDPOINT")
    account = os.environ.get("OBC_FIXTURE_R2_ACCOUNT_ID")
    if not endpoint and not account:
        missing.append("OBC_FIXTURE_R2_ENDPOINT or OBC_FIXTURE_R2_ACCOUNT_ID")
    if missing:
        raise FixtureError("missing publish configuration: " + ", ".join(missing))
    if shutil.which("rclone") is None:
        raise FixtureError("rclone is required to publish fixture packages")
    endpoint = endpoint or f"https://{account}.r2.cloudflarestorage.com"
    prefix = urlparse(catalog.base_url).path.strip("/")
    key = "/".join(part for part in (prefix, package["archive"]) if part)
    remote = f"obcfixtures:{os.environ['OBC_FIXTURE_R2_BUCKET']}/{key}"
    environment = os.environ.copy()
    environment.update(
        {
            "RCLONE_CONFIG_OBCFIXTURES_TYPE": "s3",
            "RCLONE_CONFIG_OBCFIXTURES_PROVIDER": "Cloudflare",
            "RCLONE_CONFIG_OBCFIXTURES_REGION": "auto",
            "RCLONE_CONFIG_OBCFIXTURES_ENDPOINT": endpoint,
            "RCLONE_CONFIG_OBCFIXTURES_ACCESS_KEY_ID": os.environ["OBC_FIXTURE_R2_ACCESS_KEY_ID"],
            "RCLONE_CONFIG_OBCFIXTURES_SECRET_ACCESS_KEY": os.environ["OBC_FIXTURE_R2_SECRET_ACCESS_KEY"],
            "RCLONE_CONFIG_OBCFIXTURES_NO_CHECK_BUCKET": "true",
        }
    )
    print(f"uploading {args.package} -> {key}", file=sys.stderr)
    result = subprocess.run(
        [
            "rclone",
            "copyto",
            "--no-traverse",
            "--immutable",
            "--header-upload",
            "Cache-Control: public, max-age=31536000, immutable",
            str(archive),
            remote,
        ],
        env=environment,
        check=False,
    )
    if result.returncode:
        raise FixtureError(f"rclone failed while publishing {args.package}")
    _verify_public_object(public_url, actual_bytes, actual_digest)
    print(f"✓ {args.package} uploaded and verified through {catalog.base_url}")


def _verify_public_object(url: str, expected_bytes: int, expected_digest: str) -> None:
    digest = hashlib.sha256()
    received = 0
    try:
        with urlopen(Request(url, headers={"User-Agent": "OpenBikeComputer-fixtures/1"}), timeout=60) as response:
            while chunk := response.read(BUFFER_BYTES):
                digest.update(chunk)
                received += len(chunk)
    except (HTTPError, URLError, TimeoutError, OSError) as error:
        raise FixtureError(f"public verification failed for {url}: {error}") from error
    if received != expected_bytes or digest.hexdigest() != expected_digest:
        raise FixtureError(f"public object failed verification: {url}")


def command_exists(catalog: Catalog, _store: Store, args: argparse.Namespace) -> None:
    if args.scenario not in catalog.scenarios:
        raise SystemExit(1)


def command_root(_catalog: Catalog, store: Store, _args: argparse.Namespace) -> None:
    print(store.by_id)


def command_complete(catalog: Catalog, _store: Store, args: argparse.Namespace) -> None:
    values = {
        "scenarios": catalog.scenarios,
        "profiles": catalog.profiles,
        "packages": catalog.packages,
        "targets": {**catalog.scenarios, **catalog.profiles, **catalog.packages},
    }[args.kind]
    print("\n".join(values))


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(prog="obc fixtures", description=__doc__)
    parser.add_argument("--catalog", type=Path, default=Path(os.environ.get("OBC_FIXTURE_CATALOG", repo_root() / "fixtures/catalog.toml")))
    parser.add_argument("--cache", type=Path, default=cache_root())
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("list", help="list runnable scenarios")
    show = commands.add_parser("show", help="explain a scenario, package, or profile")
    show.add_argument("target")
    sync = commands.add_parser("sync", help="download and verify packages for targets")
    sync.add_argument("targets", nargs="+")
    verify = commands.add_parser("verify", help="fully rehash cached packages")
    verify.add_argument("targets", nargs="*")
    resolve = commands.add_parser("resolve", help=argparse.SUPPRESS)
    resolve.add_argument("scenario")
    resolve.add_argument("--no-sync", action="store_true")
    exists = commands.add_parser("exists", help=argparse.SUPPRESS)
    exists.add_argument("scenario")
    commands.add_parser("root", help=argparse.SUPPRESS)
    complete = commands.add_parser("complete", help=argparse.SUPPRESS)
    complete.add_argument("kind", choices=("scenarios", "profiles", "packages", "targets"))
    pack = commands.add_parser("pack", help="build a deterministic package archive")
    pack.add_argument("package")
    pack.add_argument("source", type=Path)
    pack.add_argument("--output", type=Path)
    prune = commands.add_parser("prune", help="show or remove unreferenced cached data")
    prune.add_argument("--apply", action="store_true")
    publish = commands.add_parser("publish", help="immutably upload a cataloged package (maintainers)")
    publish.add_argument("package")
    publish.add_argument("archive", type=Path)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv if argv is not None else sys.argv[1:])
    try:
        if args.command == "pack":
            command_pack(None, None, args)
            return 0
        catalog = Catalog(args.catalog.resolve())
        store = Store(catalog, args.cache.expanduser().resolve())
        {
            "list": command_list,
            "show": command_show,
            "sync": command_sync,
            "verify": command_verify,
            "resolve": command_resolve,
            "pack": command_pack,
            "prune": command_prune,
            "publish": command_publish,
            "exists": command_exists,
            "root": command_root,
            "complete": command_complete,
        }[args.command](catalog, store, args)
        return 0
    except FixtureError as error:
        print(f"obc fixtures: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
