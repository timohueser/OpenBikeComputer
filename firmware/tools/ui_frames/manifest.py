"""The committed digest manifest, `firmware/ui-snapshots.sha256`.

One `SHA256  basename` row per frame, sorted by basename, and nothing else. The PNGs themselves are
not committed — they are reproducible from the table, and a few hundred binaries per refactor is not
a repository.

`obc shot --check` compares a sweep against these rows. It fails on a changed digest, a row the
sweep did not produce, a frame the manifest does not name, a duplicated basename, and **two frames
with different names and identical pixels** unless the pair is declared below.

`obc shot --accept` writes the rows and prints what moved. The review rule the manifest exists to
enforce: an intentional pixel change is a change you have *looked at*. Render the sweep, open the
changed frames, and only then accept — the printed basename list belongs in the pull request.
"""

from __future__ import annotations

import hashlib
import sys
from pathlib import Path

SNAPSHOT_SUFFIX = ".png"

# Frames that are **supposed** to be pixel-identical to another frame, as sorted name groups.
#
# Two names over one image is normally a recipe rendering the wrong state under the right name,
# which every other check passes. Where identity is the assertion instead, declare it here.
IDENTICAL_BY_DESIGN: list[set[str]] = [
    # Pan mode owns the Map's chrome, not the Statistics grid's: entering it changes no pixel here.
    {"statistics-pan.png", "statistics.png"},
    # Back from a peak article restores the selected summit and Browse heading exactly.
    {"peak-article-back.png", "peak-article-indicator.png"},
    # The held shortcut opens the same Assistant question list from either base.
    {"assistant.png", "quick-assistant.png"},
]


class ManifestError(Exception):
    """A manifest that cannot be read, or a sweep that disagrees with one."""


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as fh:
        for block in iter(lambda: fh.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def read(path: Path) -> dict[str, str]:
    """Parse a manifest into ``{basename: digest}``.

    Strict on purpose. A malformed row, a path instead of a basename, a digest that is not 64 hex
    characters, or the same basename twice are all rejected rather than merged — a manifest with two
    opinions about one frame has no opinion about it.
    """
    rows: dict[str, str] = {}
    try:
        text = Path(path).read_text()
    except OSError as exc:
        raise ManifestError(f"cannot read manifest {path}: {exc}") from exc
    for number, line in enumerate(text.splitlines(), start=1):
        if not line.strip():
            continue
        parts = line.split()
        if len(parts) != 2:
            raise ManifestError(f"{path}:{number}: expected 'SHA256  basename', got {line!r}")
        digest, name = parts
        if len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest):
            raise ManifestError(f"{path}:{number}: {digest!r} is not a lowercase sha256 digest")
        if "/" in name or name in (".", ".."):
            raise ManifestError(f"{path}:{number}: {name!r} must be a bare file name")
        if name in rows:
            raise ManifestError(f"{path}:{number}: duplicate entry for {name}")
        rows[name] = digest
    return rows


def scan(directory: Path) -> dict[str, str]:
    """Digest every PNG in a sweep's output directory, keyed by basename."""
    directory = Path(directory)
    if not directory.is_dir():
        raise ManifestError(f"{directory} is not a directory — run `obc shot --all` first")
    found = {
        path.name: sha256_file(path)
        for path in sorted(directory.iterdir())
        if path.is_file() and path.suffix == SNAPSHOT_SUFFIX
    }
    if not found:
        raise ManifestError(f"{directory} holds no {SNAPSHOT_SUFFIX} files — the sweep produced nothing")
    return found


def render(rows: dict[str, str]) -> str:
    return "".join(f"{rows[name]}  {name}\n" for name in sorted(rows))


def diff(expected: dict[str, str], actual: dict[str, str]) -> tuple[list[str], list[str], list[str]]:
    """``(changed, missing, extra)`` basenames, each sorted."""
    changed = sorted(name for name in expected.keys() & actual.keys() if expected[name] != actual[name])
    return changed, sorted(expected.keys() - actual.keys()), sorted(actual.keys() - expected.keys())


def undeclared_twins(rows: dict[str, str]) -> list[list[str]]:
    """Name groups that share one digest and are not declared in [`IDENTICAL_BY_DESIGN`]."""
    by_digest: dict[str, list[str]] = {}
    for name, digest in rows.items():
        by_digest.setdefault(digest, []).append(name)
    return sorted(
        sorted(names) for names in by_digest.values() if len(names) > 1 and set(names) not in IDENTICAL_BY_DESIGN
    )


def check(manifest: Path, directory: Path) -> int:
    expected = read(manifest)
    actual = scan(directory)
    changed, missing, extra = diff(expected, actual)
    twins = undeclared_twins(actual)
    if twins:
        for names in twins:
            print(f"identical: {' == '.join(names)}", file=sys.stderr)
        print(
            f"\nui-frames: {len(twins)} group(s) of frames are pixel-identical under different names.\n"
            "A recipe that renders the wrong state under the right name looks exactly like this.\n"
            "Fix the recipe, or — if the identity IS the assertion — declare the group in\n"
            "  IDENTICAL_BY_DESIGN in ui_frames/manifest.py",
            file=sys.stderr,
        )
        return 1
    if not (changed or missing or extra):
        print(f"ui-frames: {len(actual)} frames match {manifest}")
        return 0
    for name in changed:
        print(f"changed: {name}", file=sys.stderr)
    for name in missing:
        print(f"missing: {name} (the manifest names it; the sweep did not render it)", file=sys.stderr)
    for name in extra:
        print(f"extra:   {name} (the sweep rendered it; the manifest does not name it)", file=sys.stderr)
    print(
        f"\nui-frames: {len(changed)} changed, {len(missing)} missing, {len(extra)} extra.\n"
        "Look at the frames above, then record them with:\n"
        f"  obc shot --accept {' '.join(name.removesuffix('.png') for name in changed) or '[names]'}",
        file=sys.stderr,
    )
    return 1


def accept(manifest: Path, directory: Path, names: list[str] | None = None) -> int:
    """Record the sweep's digests. With names, only those rows move; the rest keep their digest."""
    actual = scan(directory)
    expected = read(manifest) if Path(manifest).exists() else {}
    if names:
        wanted = {f"{name.removesuffix('.png')}.png" for name in names}
        unknown = sorted(wanted - actual.keys())
        if unknown:
            raise ManifestError(f"{directory} holds no frame named {', '.join(unknown)}")
        actual = expected | {name: actual[name] for name in wanted}
    changed, missing, extra = diff(expected, actual)
    Path(manifest).write_text(render(actual))
    if not (changed or missing or extra):
        print(f"ui-frames: {manifest} already matched {len(actual)} frames")
        return 0
    for name in changed:
        print(f"changed: {name}")
    for name in missing:
        print(f"removed: {name}")
    for name in extra:
        print(f"added:   {name}")
    print(f"\nui-frames: wrote {len(actual)} rows to {manifest}")
    return 0


def stale(frames: list[str], manifest: Path) -> list[str]:
    """The names a table and a manifest disagree about — a frame with no digest row, and a row with
    no frame. This is the half of `--check` that needs no render, so `obc suites check` runs it."""
    recorded = set(read(manifest))
    rendered = {f"{name}.png" for name in frames}
    return [f"{name}: no digest row in {Path(manifest).name}" for name in sorted(rendered - recorded)] + [
        f"{name}: a digest row with no frame in the table" for name in sorted(recorded - rendered)
    ]
