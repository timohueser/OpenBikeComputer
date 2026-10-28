#!/usr/bin/env python3
"""Inventory and remove old OpenBikeComputer test scratch and stale shared cargo artifacts."""

from __future__ import annotations

import argparse
import contextlib
import fcntl
import os
import shutil
import subprocess
import sys
import tempfile
import time
from collections.abc import Iterator
from pathlib import Path
from stat import S_ISDIR


SECONDS_PER_DAY = 24 * 60 * 60


class CleanupError(RuntimeError):
    pass


def git(repo: Path, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", "-C", str(repo), *args],
        check=check,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def registered_worktrees(repo: Path) -> set[Path]:
    output = git(repo, "worktree", "list", "--porcelain", "-z").stdout
    return {
        Path(field.removeprefix("worktree ")).resolve()
        for field in output.split("\0")
        if field.startswith("worktree ")
    }


def directory_sizes(paths: list[Path]) -> dict[Path, int]:
    """Use the platform's optimized walker; fall back to Python where du is absent."""
    existing = [path for path in paths if path.exists()]
    if not existing:
        return {}
    try:
        sizes: dict[Path, int] = {}
        # Bound argv even after thousands of test runs have accumulated scratch.
        for offset in range(0, len(existing), 512):
            result = subprocess.run(
                ["du", "-sk", *map(str, existing[offset : offset + 512])],
                check=True,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            for line in result.stdout.splitlines():
                blocks, name = line.split("\t", 1)
                sizes[Path(name)] = int(blocks) * 1024
        return sizes
    except (FileNotFoundError, subprocess.CalledProcessError, ValueError):
        sizes = {}
        for path in existing:
            total = 0
            for root, dirs, files in os.walk(path, onerror=lambda _: None):
                dirs[:] = [name for name in dirs if not (Path(root) / name).is_symlink()]
                for name in files:
                    try:
                        total += (Path(root) / name).stat().st_size
                    except OSError:
                        pass
            sizes[path] = total
        return sizes


def shallow_activity(path: Path) -> float:
    """Latest activity of a path and its direct children, without walking a whole tree."""
    candidates = [path]
    if path.is_dir():
        try:
            candidates.extend(path.iterdir())
        except OSError:
            pass
    newest = 0.0
    for candidate in candidates:
        try:
            newest = max(newest, candidate.lstat().st_mtime)
        except OSError:
            pass
    return newest


def format_size(size: int) -> str:
    value = float(size)
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if value < 1024 or unit == "TiB":
            return f"{value:.1f} {unit}" if unit != "B" else f"{int(value)} B"
        value /= 1024
    raise AssertionError("unreachable")


def remove_path(path: Path) -> None:
    if path.is_symlink() or path.is_file():
        path.unlink()
    elif path.is_dir():
        shutil.rmtree(path)


def is_git_repository(path: Path) -> bool:
    """Recognize worktrees, ordinary clones, and bare repositories."""
    return git(path, "rev-parse", "--git-dir", check=False).returncode == 0


def temp_candidates(
    now: float,
    days: int,
    root: Path | None = None,
    excluded: set[Path] | None = None,
) -> list[Path]:
    cutoff = now - days * SECONDS_PER_DAY
    root = (root or Path(tempfile.gettempdir())).resolve()
    excluded = {path.resolve() for path in (excluded or set())}
    result: list[Path] = []
    for path in root.iterdir():
        # These namespaces are created by this repository's Rust/Python test helpers.
        if not (path.name.startswith("obc-") or path.name.startswith("obcm-")):
            continue
        # Never follow a temp-name symlink. Keeping the direct child path is
        # essential: resolving it here would turn cleanup into deletion of its
        # target outside the temp directory.
        if path.is_symlink():
            continue
        resolved = path.resolve()
        # Review clones and registered worktrees can also use these prefixes.
        if any(worktree.is_relative_to(resolved) for worktree in excluded) or is_git_repository(path):
            continue
        try:
            if shallow_activity(path) <= cutoff:
                result.append(path)
        except OSError:
            pass
    return sorted(result)


def cargo_target(value: str | None) -> Path | None:
    if not value:
        return None
    target = Path(value).resolve()
    if not target.is_dir():
        return None
    if target in (Path("/"), Path.home().resolve()) or (target / ".git").exists():
        raise CleanupError(f"refusing to prune CARGO_TARGET_DIR={target}")
    # Cargo writes this tag; without it the globs below could match a real project folder.
    if not (target / "CACHEDIR.TAG").is_file():
        print(f"warning: CARGO_TARGET_DIR={target} has no CACHEDIR.TAG; not pruning it")
        return None
    return target


def last_use(entry: Path) -> tuple[float, int]:
    """Return (newest use, bytes) of a file or a whole directory tree.

    Use is the later of atime and mtime: reading a file updates atime (daily under relatime).
    A directory counts by mtime only, because this walk updates its atime.
    """
    info = entry.lstat()
    if not S_ISDIR(info.st_mode):
        return max(info.st_atime, info.st_mtime), info.st_size
    newest, size = info.st_mtime, 0
    for root, dirs, files in os.walk(entry):
        for name in dirs:
            newest = max(newest, (Path(root) / name).lstat().st_mtime)
        for name in files:
            stat = (Path(root) / name).lstat()
            newest = max(newest, stat.st_atime, stat.st_mtime)
            size += stat.st_size
    return newest, size


def unit_hash(entry: Path) -> str:
    """`serde-<hash>`, `libserde-<hash>.rlib` and `serde-<hash>.d` share one unit hash."""
    return entry.name.split(".", 1)[0].rpartition("-")[2]


@contextlib.contextmanager
def build_lock(lock: Path, apply: bool) -> Iterator[bool]:
    """Hold the lock cargo holds for a build in this profile; yield False if a build has it.

    A running build only stats an output it judged fresh and reads it later, so a removal
    under that build breaks it. Between builds cargo rebuilds a missing unit.
    A dry run takes no lock and writes nothing.
    """
    if not apply:
        yield True
        return
    fd = os.open(lock, os.O_RDONLY)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            yield False
            return
        yield True
    finally:
        os.close(fd)


def prune_cargo_target(
    target: Path, cutoff: float, apply: bool
) -> tuple[dict[Path, tuple[int, int]], list[Path]]:
    """Return ({artifact dir: (entries, bytes)} unused since cutoff, busy profiles).

    A unit's files share one hash in `.fingerprint/`, `build/` and `deps/`. Cargo reads the
    fingerprint on every build that includes the unit but only stats the other outputs, so the
    fingerprint dates the whole unit, and the unit goes whole: a lost part rebuilds the unit
    and everything downstream. An incremental crate dir is dated by its newest file.
    """
    pruned: dict[Path, tuple[int, int]] = {}
    busy: list[Path] = []
    # Cargo creates `.cargo-lock` before it writes a profile; a dir without one is not a profile.
    for lock in sorted([*target.glob("*/.cargo-lock"), *target.glob("*/*/.cargo-lock")]):
        profile = lock.parent
        fingerprints = profile / ".fingerprint"
        # A symlink anywhere in the path could lead out of the target.
        if profile.resolve() != profile or fingerprints.is_symlink() or not fingerprints.is_dir():
            continue
        with build_lock(lock, apply) as locked:
            if not locked:
                busy.append(profile)
                continue
            alive: set[str] = set()
            for name in (".fingerprint", "build", "deps", "incremental"):
                top = profile / name
                if top.is_symlink() or not top.is_dir():
                    continue
                entries = size = 0
                for entry in sorted(top.iterdir()):
                    try:
                        used, entry_size = last_use(entry)
                    except OSError:
                        alive.add(unit_hash(entry))
                        continue
                    if name in ("build", "deps"):
                        keep = unit_hash(entry) in alive
                    else:
                        keep = used > cutoff
                        if keep and name == ".fingerprint":
                            alive.add(unit_hash(entry))
                    if keep:
                        continue
                    if apply:
                        try:
                            remove_path(entry)
                        except OSError:
                            continue
                    entries += 1
                    size += entry_size
                if entries:
                    pruned[top] = (entries, size)
    return pruned, busy


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(
        description=(
            "Remove old OBC test scratch under the system temp directory and old artifacts in "
            "$CARGO_TARGET_DIR (dry-run by default)."
        )
    )
    result.add_argument("--apply", action="store_true", help="remove eligible entries")
    result.add_argument("--days", type=int, default=7, help="minimum inactivity age (default: 7)")
    result.add_argument("--repo", type=Path, help=argparse.SUPPRESS)
    return result


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    if args.days < 0:
        raise CleanupError("--days must be zero or greater")

    repo = (args.repo or Path.cwd()).resolve()
    root = Path(tempfile.gettempdir()).resolve()
    scratch = temp_candidates(time.time(), args.days, root, registered_worktrees(repo))
    sizes = directory_sizes(scratch)
    print(f"OBC test scratch cleanup ({'APPLY' if args.apply else 'DRY RUN'})")
    print(f"age>={args.days}d  temp={root}")
    for path in scratch:
        print(f"  REMOVE {format_size(sizes.get(path, 0)):>10}  {path}")
    target = cargo_target(os.environ.get("CARGO_TARGET_DIR"))
    cutoff = time.time() - args.days * SECONDS_PER_DAY
    artifacts, busy = prune_cargo_target(target, cutoff, args.apply) if target else ({}, [])
    if target:
        print(f"cargo target={target}")
    for path, (entries, size) in artifacts.items():
        print(f"  REMOVE {format_size(size):>10}  {entries} entries in {path}")
    for profile in busy:
        print(f"  SKIP   {profile}: a cargo build holds its lock")
    # An upper bound: a hard link outside the removed entries keeps its inode.
    reclaimable = sum(sizes.values()) + sum(size for _, size in artifacts.values())
    print(f"Potentially reclaimable: up to {format_size(reclaimable)}")

    if not args.apply:
        print("Dry run only; pass --apply to remove the listed entries.")
        return 0

    eligible = set(temp_candidates(time.time(), args.days, root, registered_worktrees(repo)))
    for path in scratch:
        if path in eligible:
            remove_path(path)
        else:
            print(f"skipped {path}: eligibility changed after planning")
    print("Cleanup applied.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except CleanupError as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(2)
