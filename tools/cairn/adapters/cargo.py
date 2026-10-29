"""Rust units and dependency edges, read from `cargo metadata`.

A Cargo root is a tracked `Cargo.toml` with a `[workspace]` table, outside the `ignore` globs. One
unit is one member package of a root. Its entry is the file of its library
target, or of its main binary when it has no library. An edge is a path dependency from one unit
to another unit of any root. Every other tracked `Cargo.toml` outside `ignore` must give a unit. A `dev` dependency gives a test edge; a normal or `build`
dependency gives a production edge.
"""

from __future__ import annotations

import json
import subprocess
import sys
import tomllib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from test_plan import glob_matches  # noqa: E402

LIBRARY_KINDS = {"lib", "rlib", "staticlib", "cdylib", "dylib", "proc-macro"}


class AdapterError(RuntimeError):
    pass


def metadata(manifest: Path) -> dict:
    command = [
        "cargo", "metadata", "--format-version", "1", "--no-deps", "--locked",
        "--manifest-path", str(manifest),
    ]
    try:
        return json.loads(subprocess.run(command, check=True, capture_output=True, text=True).stdout)
    except (OSError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
        detail = getattr(error, "stderr", "") or str(error)
        raise AdapterError(f"cargo metadata failed for {manifest}: {detail.strip()}") from error


def entry(package: dict) -> str:
    targets = package["targets"]
    library = [t for t in targets if LIBRARY_KINDS & set(t["kind"])]
    binary = sorted(
        (t for t in targets if "bin" in t["kind"]),
        key=lambda t: not t["src_path"].endswith("/src/main.rs"),
    )
    return (library or binary or targets)[0]["src_path"]


def collect(root: Path, metadatas: list[dict]) -> tuple[list[dict], list[tuple[str, str, bool]]]:
    """Units as {name, dir, entry} with repository-relative paths, and (from, to, test) edges."""

    def relative(path: str) -> str | None:
        resolved = Path(path).resolve()
        return resolved.relative_to(root).as_posix() if resolved.is_relative_to(root) else None

    packages = []
    for data in metadatas:
        members = set(data["workspace_members"])
        packages += [p for p in data["packages"] if p["id"] in members]
    units = {
        relative(str(Path(p["manifest_path"]).parent)): {
            "name": p["name"],
            "dir": relative(str(Path(p["manifest_path"]).parent)),
            "entry": relative(entry(p)),
        }
        for p in packages
    }
    test_only: dict[tuple[str, str], bool] = {}
    for package in packages:
        source = package["name"]
        for dependency in package["dependencies"]:
            target = units.get(relative(dependency["path"])) if dependency.get("path") else None
            if target is None or target["name"] == source:
                continue
            key = (source, target["name"])
            # Cargo writes `None` for a normal dependency. A production edge hides a test edge.
            test_only[key] = test_only.get(key, True) and dependency.get("kind") == "dev"
    edges = [(a, b, test) for (a, b), test in sorted(test_only.items())]
    return sorted(units.values(), key=lambda u: u["dir"]), edges


def manifests(root: Path, ignore: list[str]) -> list[str]:
    out = subprocess.run(["git", "-C", str(root), "ls-files", "Cargo.toml", "*/Cargo.toml"],
                         capture_output=True, text=True, check=True).stdout
    return [f for f in out.splitlines() if not any(glob_matches(f, glob) for glob in ignore)]


def load(root: Path, settings: dict) -> tuple[list[dict], list[tuple[str, str, bool]]]:
    root = root.resolve()
    found = manifests(root, settings.get("ignore", []))
    roots = [f for f in found if "workspace" in tomllib.loads((root / f).read_text())]
    units, edges = collect(root, [metadata(root / f) for f in roots])
    covered = {f"{u['dir']}/Cargo.toml" for u in units} | set(roots)
    missing = sorted(set(found) - covered)
    if missing:
        raise AdapterError("no Cargo root holds " + ", ".join(missing))
    return units, edges
