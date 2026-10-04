"""Immutable planner component receipts and resumable build selection."""

from contextlib import contextmanager
import fcntl
import hashlib
import inspect
import json
from pathlib import Path, PurePosixPath
import resource
import re
import tempfile
import time
import tomllib

from .planner_runtime import digest, encoded


def implementation(functions=(), paths=()):
    """Only the producer's code enters its identity; runtime code does not."""
    code = [inspect.getsource(function) for function in functions]
    repository = Path(__file__).resolve().parent.parent
    files = {path.relative_to(repository).as_posix(): digest(path) for path in sorted(paths)}
    return hashlib.sha256(encoded({"code": code, "files": files})).hexdigest()


def rust_sources(*roots):
    """Include local build dependencies without running Cargo or fetching packages."""
    repository = Path(__file__).resolve().parent.parent
    workspace = tomllib.loads((repository / "Cargo.toml").read_text()).get("workspace", {}).get("dependencies", {})
    paths = {repository / name for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml")}
    pending = [repository / root for root in roots]
    while pending:
        root = pending.pop().resolve()
        manifest = root / "Cargo.toml"
        if manifest in paths:
            continue
        paths.add(manifest)
        paths.update(path for path in (root / "src").rglob("*") if path.is_file())
        if (root / "build.rs").exists(): paths.add(root / "build.rs")
        package = tomllib.loads(manifest.read_text())
        for group in [package, *package.get("target", {}).values()]:
            for table in ("dependencies", "build-dependencies"):
                for name, dependency in group.get(table, {}).items():
                    if not isinstance(dependency, dict): continue
                    base = root
                    if dependency.get("workspace"):
                        dependency, base = workspace.get(name, {}), repository
                    if isinstance(dependency, dict) and "path" in dependency:
                        pending.append(base / dependency["path"])
    return sorted(paths)


def specification(name, producer, inputs, options, coverage, dependencies=()):
    return {"format": 1, "name": name, "producer": producer, "inputs": inputs,
            "options": options, "coverage": coverage, "dependencies": list(dependencies)}


def identity(spec):
    return hashlib.sha256(encoded(spec)).hexdigest()


def safe_name(name):
    path = PurePosixPath(name)
    if not path.parts or path.is_absolute() or ".." in path.parts or str(path) != name or "\\" in name:
        raise ValueError("Invalid component file name")
    return name


def verify(root, receipt, full=True):
    if receipt.get("format") != 1 or identity(receipt["spec"]) != receipt.get("key") or not receipt.get("files"):
        raise ValueError("Invalid component receipt")
    for name, entry in receipt["files"].items():
        path = root / safe_name(name)
        if not path.resolve().is_relative_to(root.resolve()) or path.stat().st_size != entry["bytes"]:
            raise ValueError(f"Missing or changed component file: {name}")
        if full and digest(path) != entry["sha256"]:
            raise ValueError(f"Component checksum mismatch: {name}")


@contextmanager
def lock(root):
    root.mkdir(parents=True, exist_ok=True)
    with (root / ".lock").open("a") as stream:
        fcntl.flock(stream, fcntl.LOCK_EX)
        yield


class Cache:
    def __init__(self, root):
        self.root = root

    def path(self, spec):
        if not re.fullmatch(r"[a-z][a-z0-9-]{0,127}", spec["name"]):
            raise ValueError("Invalid component name")
        return self.root / "components" / spec["name"] / identity(spec)

    def read(self, spec, full=True):
        root = self.path(spec)
        receipt = json.loads((root / "receipt.json").read_bytes())
        if receipt["spec"] != spec:
            raise ValueError("Component identity differs")
        verify(root, receipt, full)
        return root, receipt

    def status(self, spec):
        root = self.path(spec)
        if not root.exists():
            return "missing", "No completed receipt for these inputs, options and producer"
        try:
            _, receipt = self.read(spec, full=False)
            return "reused", f"{sum(file['bytes'] for file in receipt['files'].values())} bytes; checksums verified when applied"
        except (OSError, ValueError, KeyError, TypeError) as error:
            return "invalid", str(error)

    def build(self, spec, producer):
        root = self.path(spec)
        root.parent.mkdir(parents=True, exist_ok=True)
        with lock(root.parent):
            if root.exists():
                return self.read(spec)
            start = time.monotonic()
            before = resource.getrusage(resource.RUSAGE_CHILDREN)
            cpu = time.process_time()
            with tempfile.TemporaryDirectory(prefix=".building-", dir=root.parent) as directory:
                stage = Path(directory)
                producer(stage)
                files = {path.relative_to(stage).as_posix(): {"bytes": path.stat().st_size, "sha256": digest(path)}
                         for path in sorted(stage.rglob("*")) if path.is_file()}
                if not files:
                    raise ValueError(f"Producer returned no files: {spec['name']}")
                after = resource.getrusage(resource.RUSAGE_CHILDREN)
                receipt = {"format": 1, "key": identity(spec), "spec": spec, "files": files,
                           "cost": {"elapsed_seconds": time.monotonic() - start,
                                    "cpu_seconds": time.process_time() - cpu + after.ru_utime + after.ru_stime - before.ru_utime - before.ru_stime,
                                    "output_bytes": sum(file["bytes"] for file in files.values()), "peak_ram_bytes": None}}
                (stage / "receipt.json").write_bytes(encoded(receipt))
                for path in stage.rglob("*"):
                    if path.is_file(): path.chmod(0o444)
                stage.rename(root)
            return root, receipt

    def inventory(self):
        for path in sorted((self.root / "components").glob("*/*/receipt.json")):
            receipt = json.loads(path.read_bytes())
            verify(path.parent, receipt, full=False)
            yield {"component": receipt["spec"]["name"], "key": receipt["key"], "path": str(path.parent),
                   "coverage": receipt["spec"]["coverage"], "cost": receipt["cost"]}


def plan(specs, selected=None, previous=None):
    """Select requested producers and their dependencies; retain explicit unselected receipts."""
    selected = set(specs) if selected is None else set(selected)
    if selected - set(specs):
        raise ValueError(f"Unknown components: {', '.join(sorted(selected - set(specs)))}")
    drivers = set(selected)
    while True:
        active = set(drivers)
        pending = list(active)
        while pending:
            for dependency in specs[pending.pop()]["dependencies"]:
                if dependency not in active:
                    active.add(dependency); pending.append(dependency)
        # Retain unchanged ancestors. Shared input changes select every consumer.
        changed_inputs = {(key, encoded(value)) for name in active for key, value in specs[name]["inputs"].items()
                          if previous and previous.get(name, {}).get("spec", {}).get("inputs", {}).get(key) != value}
        derived = {name for name, spec in specs.items() if set(spec["dependencies"]) & drivers or
                   any((key, encoded(value)) in changed_inputs for key, value in spec["inputs"].items())}
        if derived <= drivers:
            break
        drivers.update(derived)
    if previous is None and active != set(specs):
        raise ValueError("A component-only update needs --input-release; prepare all components for the first release")
    return {name: specs[name] if name in active else previous[name]["spec"] for name in specs}, active
