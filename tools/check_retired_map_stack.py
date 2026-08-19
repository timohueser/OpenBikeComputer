#!/usr/bin/env python3
"""Fail when a deleted map-distribution API or version marker grows back."""

from __future__ import annotations

import os
import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SKIP_PARTS = {".git", ".claude", ".codex", ".venv", "dist", "node_modules", "target"}
TEXT_SUFFIXES = {"", ".json", ".md", ".py", ".rs", ".sh", ".svelte", ".toml", ".ts", ".yml", ".yaml"}

# Keep the spellings split in this guard's own source so it audits itself too.
RETIRED = [
    re.compile(r"\bfetch" + r"Artifact\b"),
    re.compile(r"\bRegion" + r"Step\b"),
    re.compile(r"--allow" + r"-shrink\b"),
    re.compile(r"\bcatalog\." + r"v2\b"),
    re.compile(r"\bparse" + r"Catalog\b"),
    re.compile(r"\bobc-web-" + r"preview\b"),
    re.compile(r"\bcatalog/" + r"v2\b"),
    re.compile(r"\bCatalog" + r"V2\b"),
    re.compile(r"--catalog-" + r"v2\b"),
    re.compile(r"--" + r"v2\b"),
    re.compile(r"\bcatalog" + r"Root\b"),
    re.compile(r"maps\.openbikecomputer\." + r"org/catalog\.json"),
    # The volume set (#1420 FS7.5b2). A map is one OBCM file: no manifest, no shards, no roles,
    # no archive to bundle several files into, and no 32-slot sink to write them through. These
    # are the producer-side spellings, so none of them may come back — the *reader* side
    # (`obc_formats::obcs`, the board's mount, the three wire kinds) is deliberately alive until
    # the board cutover and is therefore not listed here.
    re.compile(r"\bsendAssembled" + r"SetFile\b"),
    re.compile(r"\babandonAssembled" + r"Set\b"),
    re.compile(r"\bopenShard" + r"Sink\b"),
    re.compile(r"\bSET_SHARD_" + r"CEILING\b"),
    re.compile(r"\bstore" + r"Zip\b"),
    re.compile(r"\bzip" + r"Layout\b"),
    re.compile(r"\bforce_" + r"split\b"),
]


def main() -> int:
    failures: list[str] = []
    for directory, child_dirs, files in os.walk(ROOT):
        child_dirs[:] = sorted(name for name in child_dirs if name not in SKIP_PARTS)
        for name in sorted(files):
            path = Path(directory, name)
            if path == Path(__file__).resolve() or path.suffix not in TEXT_SUFFIXES:
                continue
            rel = path.relative_to(ROOT)
            try:
                lines = path.read_text(encoding="utf-8").splitlines()
            except (OSError, UnicodeDecodeError):
                continue
            for line_no, line in enumerate(lines, 1):
                for retired in RETIRED:
                    match = retired.search(line)
                    if match:
                        failures.append(f"{rel}:{line_no}: retired `{match.group(0)}`")

    if failures:
        print("Deleted map-stack identifiers returned:")
        print("\n".join(failures))
        return 1
    print("retired map-stack identifiers remain absent")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
