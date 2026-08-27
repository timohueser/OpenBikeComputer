#!/usr/bin/env python3
"""The committed digest manifest for the UI snapshot sweep (`firmware/ui-snapshots.sh`).

The sweep renders every on-device screen headlessly. Until now its only reader was a human diffing
two output directories, which means a pixel change was caught exactly when somebody remembered to
keep the previous run around. `firmware/ui-snapshots.sha256` is that previous run, committed: one
`SHA256  basename` row per rendered PNG, sorted by basename, and nothing else. The PNGs themselves
are not committed — they are reproducible from the script, and a few hundred binaries per refactor
is not a repository.

Two commands, deliberately separate:

* ``check MANIFEST DIR`` — compare a fresh sweep against the manifest. It fails on a changed digest,
  a file the manifest names that the sweep did not produce, a file the sweep produced that the
  manifest does not name, a duplicated basename inside the manifest, and **two frames with different
  names and identical pixels** unless the pair is declared below. This is the CI command.
* ``update MANIFEST DIR`` — rewrite the manifest from a sweep and print what moved. This is a
  developer command and must never run in CI: a manifest that regenerates itself records nothing.

The review rule the manifest exists to enforce: an intentional pixel change is a change you have
*looked at*. Run the sweep, open the changed frames, and only then run ``update`` — the printed
basename list belongs in the pull request. An update with no reviewed output is not evidence.
"""

from __future__ import annotations

import argparse
import hashlib
import sys
from pathlib import Path

# The sweep writes PNGs and only PNGs; anything else in the directory is not this manifest's
# business (an OUT_DIR is allowed to be somebody's scratch folder).
SNAPSHOT_SUFFIX = ".png"

# Frames that are **supposed** to be pixel-identical to another frame, as sorted name groups.
#
# Two names over one image is normally a recipe rendering the wrong state under the right name,
# which every other check passes. Where identity is the assertion instead, declare it here.
IDENTICAL_BY_DESIGN: list[set[str]] = [
    # The expiry row is absent in both cases, so both frames are the plain overview (epic #638 S3).
    {"routeoverview-expiry-far-absent.png", "routeoverview-expiry-unstarted-absent.png", "routeoverview.png"},
    # Pan mode owns the Map's chrome, not the Statistics grid's: entering it changes no pixel here.
    {"statistics-pan.png", "statistics.png"},
]


class ManifestError(Exception):
    """A manifest that cannot be read, or a sweep that disagrees with one."""


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as fh:
        for block in iter(lambda: fh.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def read_manifest(path: Path) -> dict[str, str]:
    """Parse a manifest into ``{basename: digest}``.

    Strict on purpose. A malformed row, a path instead of a basename, a digest that is not 64 hex
    characters, or the same basename twice are all rejected rather than merged — a manifest with two
    opinions about one frame has no opinion about it.
    """
    rows: dict[str, str] = {}
    try:
        text = path.read_text()
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


def scan_snapshots(directory: Path) -> dict[str, str]:
    """Digest every PNG in a sweep's output directory, keyed by basename."""
    if not directory.is_dir():
        raise ManifestError(f"{directory} is not a directory — run firmware/ui-snapshots.sh first")
    found = {p.name: sha256_file(p) for p in sorted(directory.iterdir()) if p.is_file() and p.suffix == SNAPSHOT_SUFFIX}
    if not found:
        raise ManifestError(f"{directory} holds no {SNAPSHOT_SUFFIX} files — the sweep produced nothing")
    return found


def render_manifest(rows: dict[str, str]) -> str:
    return "".join(f"{rows[name]}  {name}\n" for name in sorted(rows))


def diff(expected: dict[str, str], actual: dict[str, str]) -> tuple[list[str], list[str], list[str]]:
    """``(changed, missing, extra)`` basenames, each sorted."""
    changed = sorted(name for name in expected.keys() & actual.keys() if expected[name] != actual[name])
    missing = sorted(expected.keys() - actual.keys())
    extra = sorted(actual.keys() - expected.keys())
    return changed, missing, extra


def undeclared_twins(rows: dict[str, str]) -> list[list[str]]:
    """Name groups that share one digest and are not declared in [`IDENTICAL_BY_DESIGN`]."""
    by_digest: dict[str, list[str]] = {}
    for name, digest in rows.items():
        by_digest.setdefault(digest, []).append(name)
    return sorted(
        sorted(names) for names in by_digest.values() if len(names) > 1 and set(names) not in IDENTICAL_BY_DESIGN
    )


def check(manifest: Path, directory: Path) -> int:
    expected = read_manifest(manifest)
    actual = scan_snapshots(directory)
    changed, missing, extra = diff(expected, actual)
    twins = undeclared_twins(actual)
    if twins:
        for names in twins:
            print(f"identical: {' == '.join(names)}", file=sys.stderr)
        print(
            f"\nui-snapshots: {len(twins)} group(s) of frames are pixel-identical under different names.\n"
            "A recipe that renders the wrong state under the right name looks exactly like this.\n"
            "Fix the recipe, or — if the identity IS the assertion — declare the group in\n"
            f"  IDENTICAL_BY_DESIGN in {Path(__file__).name}",
            file=sys.stderr,
        )
        return 1
    if not (changed or missing or extra):
        print(f"ui-snapshots: {len(actual)} frames match {manifest}")
        return 0
    for name in changed:
        print(f"changed: {name}", file=sys.stderr)
    for name in missing:
        print(f"missing: {name} (the manifest names it; the sweep did not render it)", file=sys.stderr)
    for name in extra:
        print(f"extra:   {name} (the sweep rendered it; the manifest does not name it)", file=sys.stderr)
    print(
        f"\nui-snapshots: {len(changed)} changed, {len(missing)} missing, {len(extra)} extra.\n"
        "Look at the frames above, then record them with:\n"
        f"  python3 {Path(__file__).name} update {manifest} {directory}",
        file=sys.stderr,
    )
    return 1


def update(manifest: Path, directory: Path) -> int:
    actual = scan_snapshots(directory)
    expected = read_manifest(manifest) if manifest.exists() else {}
    changed, missing, extra = diff(expected, actual)
    manifest.write_text(render_manifest(actual))
    if not (changed or missing or extra):
        print(f"ui-snapshots: {manifest} already matched {len(actual)} frames")
        return 0
    for name in changed:
        print(f"changed: {name}")
    for name in missing:
        print(f"removed: {name}")
    for name in extra:
        print(f"added:   {name}")
    print(f"\nui-snapshots: wrote {len(actual)} rows to {manifest}")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    for name, help_text in (
        ("check", "fail if a sweep disagrees with the manifest (the CI command)"),
        ("update", "rewrite the manifest from a sweep and print what moved (never in CI)"),
    ):
        cmd = sub.add_parser(name, help=help_text)
        cmd.add_argument("manifest", type=Path, help="the committed manifest, firmware/ui-snapshots.sha256")
        cmd.add_argument("directory", type=Path, help="a sweep's output directory")
    args = parser.parse_args(argv)
    try:
        return check(args.manifest, args.directory) if args.command == "check" else update(args.manifest, args.directory)
    except ManifestError as exc:
        print(f"ui-snapshots: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
