#!/usr/bin/env python3
"""Safely inventory and remove stale OpenBikeComputer development state."""

from __future__ import annotations

import argparse
import fcntl
import os
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path


SECONDS_PER_DAY = 24 * 60 * 60
STANDALONE_TARGETS = (
    Path("firmware/obc-fw-nrf54l/target"),
    Path("firmware/obc-boot/target"),
    Path("apps/obc-desktop/target"),
)
# Per-profile directories whose entries cargo recreates on demand. Cargo never removes
# superseded entries itself, so every dependency bump, feature set, and toolchain update
# leaves a generation behind.
ARTIFACT_DIRS = ("deps", "examples", "build", "incremental", ".fingerprint")


class CleanupError(RuntimeError):
    pass


@dataclass(frozen=True)
class Worktree:
    path: Path
    head: str | None = None
    branch: str | None = None
    detached: bool = False
    locked: bool = False
    prunable: bool = False


@dataclass(frozen=True)
class WorktreeDecision:
    worktree: Worktree
    eligible: bool
    reason: str
    age_days: int | None = None


def git(repo: Path, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", "-C", str(repo), *args],
        check=check,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def parse_worktrees(output: str) -> list[Worktree]:
    records: list[Worktree] = []
    fields: dict[str, str | bool] = {}

    def finish() -> None:
        if "worktree" not in fields:
            return
        records.append(
            Worktree(
                path=Path(str(fields["worktree"])).resolve(),
                head=str(fields["HEAD"]) if "HEAD" in fields else None,
                branch=str(fields["branch"]) if "branch" in fields else None,
                detached=bool(fields.get("detached", False)),
                locked=bool(fields.get("locked", False)),
                prunable=bool(fields.get("prunable", False)),
            )
        )

    for line in output.splitlines() + [""]:
        if not line:
            finish()
            fields = {}
            continue
        key, _, value = line.partition(" ")
        fields[key] = value if value else True
    return records


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


def candidate_targets(root: Path) -> list[Path]:
    return [root / "target", *(root / relative for relative in STANDALONE_TARGETS)]


def profile_dirs(target: Path) -> list[Path]:
    """Cargo profile directories: target/<profile> and target/<triple>/<profile>."""
    result: list[Path] = []
    for child in sorted(target.iterdir()):
        if child.is_symlink() or not child.is_dir():
            continue
        if (child / ".cargo-lock").exists():
            result.append(child)
            continue
        result.extend(
            grandchild
            for grandchild in sorted(child.iterdir())
            if not grandchild.is_symlink() and (grandchild / ".cargo-lock").exists()
        )
    return result


def stale_artifacts(target: Path, cutoff: float) -> dict[Path, list[Path]]:
    """Artifacts cargo has not rewritten since the cutoff, grouped by profile directory.

    A no-op build touches nothing, so an entry's activity is its last compile. Cargo
    rebuilds whatever is missing, so removing an entry costs compile time, never
    correctness: a removed dependency recompiles and its dependents follow.
    """
    result: dict[Path, list[Path]] = {}
    for profile in profile_dirs(target):
        stale: list[Path] = []
        for entry in profile.iterdir():
            if entry.name in ARTIFACT_DIRS and entry.is_dir():
                stale.extend(child for child in entry.iterdir() if shallow_activity(child) <= cutoff)
            elif entry.is_file() and entry.name != ".cargo-lock" and shallow_activity(entry) <= cutoff:
                stale.append(entry)
        if stale:
            result[profile] = sorted(stale)
    return result


def remove_stale_artifacts(profile: Path, entries: list[Path], cutoff: float) -> bool:
    """Remove entries under one profile directory; False when a running build holds its lock."""
    with open(profile / ".cargo-lock", "a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            return False
        for entry in entries:
            if shallow_activity(entry) <= cutoff:
                remove_path(entry)
        return True


def worktree_activity_time(worktree: Worktree, commit_time: int | None) -> int | None:
    """Best local activity signal: checkout creation/metadata or the tip commit."""
    candidates = [commit_time] if commit_time is not None else []
    for path in (worktree.path, worktree.path / ".git"):
        try:
            candidates.append(int(path.lstat().st_mtime))
        except OSError:
            pass
    return max(candidates) if candidates else None


def classify_worktree(
    worktree: Worktree,
    *,
    main_path: Path,
    current_path: Path,
    dirty: bool,
    merged: bool,
    activity_time: int | None,
    now: float,
    days: int,
) -> WorktreeDecision:
    if worktree.path == main_path:
        return WorktreeDecision(worktree, False, "main checkout")
    if worktree.path == current_path:
        return WorktreeDecision(worktree, False, "current checkout")
    if worktree.locked:
        return WorktreeDecision(worktree, False, "locked")
    if worktree.prunable or not worktree.path.exists():
        return WorktreeDecision(worktree, False, "missing; Git metadata is prunable")
    if dirty:
        return WorktreeDecision(worktree, False, "dirty")
    if not merged:
        return WorktreeDecision(worktree, False, "unmerged")
    if activity_time is None:
        return WorktreeDecision(worktree, False, "unknown activity age")
    age = int(max(0, (now - activity_time) // SECONDS_PER_DAY))
    if age < days:
        return WorktreeDecision(worktree, False, f"recent ({age}d < {days}d)", age)
    return WorktreeDecision(worktree, True, f"clean, merged, and {age}d old", age)


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
        # A review clone/worktree may also have an obc-* name. Git-owned paths only
        # leave through the worktree classifier above, never through this prefix rule.
        if resolved in excluded or is_git_repository(path):
            continue
        try:
            if shallow_activity(path) <= cutoff:
                result.append(path)
        except OSError:
            pass
    return sorted(result)


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(
        description="Inventory stale worktrees, build outputs, and OBC test scratch (dry-run by default)."
    )
    result.add_argument("--apply", action="store_true", help="remove eligible entries")
    result.add_argument("--days", type=int, default=7, help="minimum inactivity age (default: 7)")
    result.add_argument("--base", default="develop", help="branch work must be merged into (default: develop)")
    result.add_argument(
        "--include-builds",
        action="store_true",
        help="also remove build artifacts cargo has not rewritten within --days (the next build recompiles what it still needs)",
    )
    result.add_argument("--repo", type=Path, help=argparse.SUPPRESS)
    return result


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    if args.days < 0:
        raise CleanupError("--days must be zero or greater")

    start = (args.repo or Path.cwd()).resolve()
    top = git(start, "rev-parse", "--show-toplevel").stdout.strip()
    if not top:
        raise CleanupError(f"not in a Git worktree: {start}")
    current = Path(top).resolve()
    worktrees = parse_worktrees(git(current, "worktree", "list", "--porcelain").stdout)
    if not worktrees:
        raise CleanupError("Git reported no worktrees")
    main_path = worktrees[0].path
    if args.base.startswith("-") or git(main_path, "check-ref-format", "--branch", args.base, check=False).returncode:
        raise CleanupError(f"invalid base branch: {args.base}")
    base_check = git(main_path, "rev-parse", "--verify", args.base, check=False)
    if base_check.returncode:
        raise CleanupError(f"base ref does not exist: {args.base}")

    now = time.time()
    decisions: list[WorktreeDecision] = []
    for worktree in worktrees:
        exists = worktree.path.exists()
        if exists:
            status = git(worktree.path, "status", "--porcelain", check=False)
            dirty = status.returncode != 0 or bool(status.stdout)
        else:
            dirty = False
        merged = False
        commit_time: int | None = None
        if worktree.head:
            merged = git(main_path, "merge-base", "--is-ancestor", worktree.head, args.base, check=False).returncode == 0
            timestamp = git(main_path, "show", "-s", "--format=%ct", worktree.head, check=False)
            if timestamp.returncode == 0 and timestamp.stdout.strip().isdigit():
                commit_time = int(timestamp.stdout.strip())
        decisions.append(
            classify_worktree(
                worktree,
                main_path=main_path,
                current_path=current,
                dirty=dirty,
                merged=merged,
                activity_time=worktree_activity_time(worktree, commit_time),
                now=now,
                days=args.days,
            )
        )

    print(f"OBC development cleanup ({'APPLY' if args.apply else 'DRY RUN'})")
    print(f"base={args.base}  age>={args.days}d  main={main_path}")
    print("\nWorktrees")
    reclaimable = 0
    eligible_worktrees: list[Worktree] = []
    all_targets = [
        target
        for decision in decisions
        for target in candidate_targets(decision.worktree.path)
        if target.is_dir()
    ]
    target_sizes = directory_sizes(all_targets)
    for decision in decisions:
        size = sum(target_sizes.get(path, 0) for path in candidate_targets(decision.worktree.path))
        marker = "REMOVE" if decision.eligible else "keep"
        print(f"  {marker:6} {format_size(size):>10}  {decision.worktree.path} — {decision.reason}")
        if decision.eligible:
            eligible_worktrees.append(decision.worktree)
            reclaimable += size

    cutoff = now - args.days * SECONDS_PER_DAY
    sweeps: list[dict[Path, list[Path]]] = []
    print("\nBuild artifacts")
    for decision in decisions:
        if decision.eligible:
            continue
        for target in candidate_targets(decision.worktree.path):
            if not target.is_dir():
                continue
            stale = stale_artifacts(target, cutoff)
            entries = [entry for profile_entries in stale.values() for entry in profile_entries]
            size = sum(directory_sizes(entries).values())
            marker = "SWEEP" if args.include_builds else "keep"
            suffix = "" if args.include_builds else ", pass --include-builds"
            print(f"  {marker:6} {format_size(size):>10}  {target} — {len(entries)} artifacts older than {args.days}d{suffix}")
            if args.include_builds:
                sweeps.append(stale)
                reclaimable += size

    scratch = temp_candidates(now, args.days, excluded={worktree.path for worktree in worktrees})
    scratch_size = sum(directory_sizes(scratch).values())
    reclaimable += scratch_size
    print("\nTest scratch")
    print(f"  REMOVE {format_size(scratch_size):>10}  {len(scratch)} OBC temp entries at least {args.days}d old")

    prunable = any(worktree.prunable or not worktree.path.exists() for worktree in worktrees)
    print("\nGit metadata")
    print(f"  {'PRUNE ' if prunable else 'keep  '} registered metadata for missing worktrees")
    print(f"\nPotentially reclaimable: {format_size(reclaimable)}")

    if not args.apply:
        print("Dry run only; pass --apply to remove the entries marked REMOVE/PRUNE.")
        return 0

    for worktree in eligible_worktrees:
        status = git(worktree.path, "status", "--porcelain", check=False)
        head = git(worktree.path, "rev-parse", "HEAD", check=False)
        still_merged = (
            head.returncode == 0
            and git(main_path, "merge-base", "--is-ancestor", head.stdout.strip(), args.base, check=False).returncode == 0
        )
        if status.returncode or status.stdout or head.returncode or head.stdout.strip() != worktree.head or not still_merged:
            print(f"error: {worktree.path} changed after planning; refusing to remove it", file=sys.stderr)
            return 1
    for stale in sweeps:
        for profile, entries in stale.items():
            if not remove_stale_artifacts(profile, entries, cutoff):
                print(f"skipped {profile}: a build holds its lock")
    for path in scratch:
        remove_path(path)
    for worktree in eligible_worktrees:
        result = git(main_path, "worktree", "remove", str(worktree.path), check=False)
        if result.returncode:
            print(f"error: could not remove {worktree.path}: {result.stderr.strip()}", file=sys.stderr)
            return 1
    if prunable:
        git(main_path, "worktree", "prune")
    print("Cleanup applied. Local branches and fixture packages were left untouched.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except CleanupError as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(2)
