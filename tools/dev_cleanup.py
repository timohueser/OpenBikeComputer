#!/usr/bin/env python3
"""Inventory and remove old OpenBikeComputer test scratch under the system temp directory."""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path


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


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(
        description="Remove old OBC test scratch under the system temp directory (dry-run by default)."
    )
    result.add_argument("--apply", action="store_true", help="remove eligible scratch entries")
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
    print(f"Potentially reclaimable: {format_size(sum(sizes.values()))}")

    if not args.apply:
        print("Dry run only; pass --apply to remove the listed scratch entries.")
        return 0

    eligible = set(temp_candidates(time.time(), args.days, root, registered_worktrees(repo)))
    for path in scratch:
        if path in eligible:
            remove_path(path)
        else:
            print(f"skipped {path}: eligibility changed after planning")
    print("Scratch cleanup applied.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except CleanupError as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(2)
