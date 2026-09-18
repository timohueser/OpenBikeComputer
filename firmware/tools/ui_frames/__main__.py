"""`obc shot` — render the frames of `firmware/ui-frames.toml`.

One frame, a family, the whole sweep and the digest check are modes of one renderer, so they cannot
disagree. The simulator and the build are quiet unless they fail: a successful render prints the
path it wrote and nothing else.
"""

from __future__ import annotations

import argparse
import difflib
import fnmatch
import functools
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from . import compare, environments, manifest, table

ROOT = Path(__file__).resolve().parents[3]
TABLE = ROOT / "firmware" / "ui-frames.toml"
MANIFEST = ROOT / "firmware" / "ui-snapshots.sha256"
OUT = ROOT / "ui-snapshots"
BASE = ROOT / "target" / "shot-base"
#: The scenarios the frames draw from. `sync` is content-addressed and offline once cached.
FIXTURES = ("sim", "sim-assistant-west-cork")


class Failure(Exception):
    """Something the caller has to fix: a bad name, a failed render, a stale manifest."""


def say(message: str) -> None:
    print(message, file=sys.stderr)


def run(command: list[str], cwd: Path = ROOT) -> str:
    done = subprocess.run(command, cwd=cwd, capture_output=True, text=True)
    if done.returncode:
        raise Failure(f"{' '.join(command)}\n{(done.stderr or done.stdout).strip()}")
    return done.stdout.strip()


@functools.cache
def fixture_root() -> Path:
    run(["python3", str(ROOT / "tools" / "fixtures.py"), "sync", *FIXTURES])
    return Path(run(["python3", str(ROOT / "tools" / "fixtures.py"), "root"]))


def simulator() -> Path:
    """The simulator this tree builds, or the one `SIM` names (CI passes its debug build)."""
    named = os.environ.get("SIM")
    if named:
        return Path(named)
    run(["cargo", "build", "--release", "-p", "obc-sim", "--quiet"])
    return ROOT / "target" / "release" / "obc-sim"


def base_simulator(ref: str) -> Path:
    """The simulator as of `ref`, built once and kept, so a second `--vs` does not build again."""
    sha = run(["git", "rev-parse", ref])
    binary = BASE / sha / "obc-sim"
    if binary.exists():
        return binary
    source = BASE / sha / "src"
    say(f"building obc-sim at {ref} ({sha[:8]}) — once for this reference")
    run(["git", "worktree", "add", "--detach", "--quiet", str(source), sha])
    try:
        run(["cargo", "build", "--release", "-p", "obc-sim", "--quiet"], cwd=source)
        shutil.copy(source / "target" / "release" / "obc-sim", binary)
    finally:
        run(["git", "worktree", "remove", "--force", str(source)])
    return binary


class Renderer:
    """Renders frames, staging each environment once and sharing it, as the frames expect."""

    def __init__(self, sim: Path):
        self.sim = sim
        self._fragments = table.fragments(TABLE)
        self._staged: dict[str, environments.Staging] = {}
        self._root = Path(tempfile.mkdtemp(prefix="ui-frames-"))

    def close(self) -> None:
        shutil.rmtree(self._root, ignore_errors=True)

    def _stage(self, name: str) -> environments.Staging:
        if name not in self._staged:
            recipe = environments.ENVIRONMENTS.get(name)
            if recipe is None:
                known = sorted(environments.ENVIRONMENTS)
                raise Failure(f"unknown environment `{name}`; the table can name only {known}")
            stage = environments.Stage(
                repo=ROOT,
                fixtures=fixture_root(),
                root=self._root,
                run=lambda args: run([str(self.sim), *args]),
                script=lambda text: table.expand_script(text, self._fragments),
            )
            self._staged[name] = recipe(stage)
        return self._staged[name]

    def command(self, frame: table.Frame, png: Path, sim: Path | None = None) -> list[str]:
        args, staged_map = [], None
        for name in frame.envs:
            staging = self._stage(name)
            args += list(staging.args)
            staged_map = staging.map or staged_map
        if staged_map and frame.map:
            raise Failure(f"{frame.name}: its environment stages a map, so the row must not name one")
        command = [str(sim or self.sim)]
        if staged_map or frame.map:
            command.append(staged_map or frame.map)
        if frame.boot:
            command.append("--boot")
        command += args + list(frame.args)
        if frame.script:
            command += ["--script", frame.script]
        if frame.lang:
            command += ["--lang", frame.lang]
        return command + ["--expect-screen", frame.expect, "--png", str(png)]

    def render(self, frame: table.Frame, directory: Path, sim: Path | None = None) -> Path:
        directory.mkdir(parents=True, exist_ok=True)
        png = directory / f"{frame.name}.png"
        run(self.command(frame, png, sim))
        return png


def frames(paths: bool = True) -> list[table.Frame]:
    """Every frame. Without `paths` the fixture roots stay unresolved, which is all a listing or a
    manifest comparison needs — and it keeps those two off the fixture registry."""
    return table.load(TABLE, fixtures=fixture_root() if paths else "", repo=ROOT)


def select(every: list[table.Frame], patterns: list[str]) -> list[table.Frame]:
    chosen: list[table.Frame] = []
    for pattern in patterns:
        matched = [frame for frame in every if fnmatch.fnmatch(frame.name, pattern)]
        if not matched:
            close = difflib.get_close_matches(pattern, [frame.name for frame in every], n=3, cutoff=0.0)
            raise Failure(f"no frame named `{pattern}`. Closest: {', '.join(close)}")
        chosen += [frame for frame in matched if frame not in chosen]
    return chosen


def command_list(pattern: str) -> int:
    every = frames(paths=False)
    matched = [frame for frame in every if fnmatch.fnmatch(frame.name, pattern)]
    if not matched:
        raise Failure(f"no frame matches `{pattern}`")
    width = max(len(frame.name) for frame in matched)
    for frame in matched:
        print(f"{frame.name.ljust(width)}  {frame.expect}")
    return 0


def command_render(patterns: list[str], directory: Path) -> int:
    renderer = Renderer(simulator())
    try:
        for frame in select(frames(), patterns):
            print(renderer.render(frame, directory))
    finally:
        renderer.close()
    return 0


def command_all(directory: Path) -> int:
    every = frames()
    renderer = Renderer(simulator())
    try:
        for frame in every:
            renderer.render(frame, directory)
    finally:
        renderer.close()
    print(f"ui-frames: {len(every)} frames rendered into {directory}")
    return 0


def command_check(directory: Path | None) -> int:
    problems = manifest.stale([frame.name for frame in frames(paths=False)], MANIFEST)
    if problems:
        for problem in problems:
            say(problem)
        raise Failure("the table and the digest manifest disagree")
    if directory is None:
        directory = Path(tempfile.mkdtemp(prefix="ui-frames-sweep-"))
        try:
            command_all(directory)
            return manifest.check(MANIFEST, directory)
        finally:
            shutil.rmtree(directory, ignore_errors=True)
    return manifest.check(MANIFEST, directory)


def command_vs(patterns: list[str], ref: str, directory: Path) -> int:
    base = base_simulator(ref)
    renderer = Renderer(simulator())
    try:
        for frame in select(frames(), patterns):
            head = renderer.render(frame, directory)
            try:
                before = renderer.render(frame, base.parent / "frames", base)
            except Failure as exc:
                say(f"{frame.name}: {ref} cannot render this frame — showing the head frame alone\n{exc}")
                print(head)
                continue
            count, total = compare.changed(head, before)
            print(f"{frame.name}: {100 * count / total:.2f}% changed ({count} px)")
            print(head)
            print(before)
    finally:
        renderer.close()
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="obc shot", description=__doc__.splitlines()[0])
    parser.add_argument("names", nargs="*", metavar="NAME", help="frame names or globs; a directory for --all")
    parser.add_argument("--list", nargs="?", const="*", metavar="PATTERN", help="the frames and their screens")
    parser.add_argument("--all", action="store_true", help="render every frame")
    parser.add_argument("--check", action="store_true", help="compare a sweep against the digest manifest")
    parser.add_argument("--accept", action="store_true", help="record the digests of a sweep you have looked at")
    parser.add_argument("--vs", metavar="REF", help="render the frame here and at REF, and compare")
    parser.add_argument("--out", type=Path, default=OUT, help=f"where the frames land (default {OUT})")
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        if args.list is not None:
            return command_list(args.list)
        if args.check or args.all:
            if len(args.names) > 1:
                parser.error("--all and --check take one directory")
            where = Path(args.names[0]) if args.names else None
            return command_check(where) if args.check else command_all(where or args.out)
        if args.accept:
            return manifest.accept(MANIFEST, args.out, args.names)
        if not args.names:
            parser.error("name a frame, or use --list, --all, --check or --accept")
        return command_vs(args.names, args.vs, args.out) if args.vs else command_render(args.names, args.out)
    except (Failure, table.TableError, manifest.ManifestError) as exc:
        say(f"obc shot: {exc}")
        return 1


if __name__ == "__main__":
    sys.exit(main())
