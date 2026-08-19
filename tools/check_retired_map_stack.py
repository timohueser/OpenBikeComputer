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
    # The volume set (#1420 FS7.5b2 producers, FS7.5c3b readers). A map is one OBCM file: no
    # manifest, no shards, no roles, no archive to bundle several files into, and no 32-slot sink
    # to write them through. The reader side went with the USB cutover — `obc-formats/src/obcs.rs`,
    # the board's set machinery, `SetPart` and the three wire kinds — so both halves are listed now
    # and neither may come back.
    re.compile(r"\bsendAssembled" + r"SetFile\b"),
    re.compile(r"\babandonAssembled" + r"Set\b"),
    re.compile(r"\bopenShard" + r"Sink\b"),
    re.compile(r"\bSET_SHARD_" + r"CEILING\b"),
    re.compile(r"\bstore" + r"Zip\b"),
    re.compile(r"\bzip" + r"Layout\b"),
    re.compile(r"\bforce_" + r"split\b"),
    # The reader side, retired by FS7.5-c3b with the USB v4 cutover.
    re.compile(r"\bobc_formats::" + r"obcs\b"),
    re.compile(r"\bSet" + r"Part\b"),
    re.compile(r"\bset_shard_" + r"begin\b"),
    re.compile(r"\bset_manifest_" + r"commit\b"),
    re.compile(r"\bvalidate_committed_" + r"manifest\b"),
    re.compile(r"\bsweep_aborted_" + r"sets\b"),
    re.compile(r"\bSD_SET_MAX_" + r"SHARDS\b"),
    # The v1 USB selector envelope, retired by FS7.5-c3b: the control bulk pair is wholly v4, and
    # the non-object surface is one EP0 vendor request (`FLAT_Store_Protocol.md` §5.2.1).
    re.compile(r"\bCARD_FREE_" + r"READ\b"),
    re.compile(r"\bDEVICE_INFO_" + r"READ\b"),
    re.compile(r"\bIDENTITY_" + r"READ\b"),
]

# Spellings deliberately **not** listed, so the next reader does not add them: `transferControl`,
# `mapSet` and `terrainShard` are wire names this repo still explains in prose — the retirement note
# in `specs/vectors/README.md`, the BLE interface spec's no-reuse table, the docs' companion-link
# page — and a guard that banned the word would ban the explanation with it. What may not come back
# is the *code*, and the identifiers above are what the code was called.


# **One file is exempt, and the reason is what the file is.** `firmware/tools/resource_baseline.json`
# is a *measurement log*: every `_resident_note_*` records what a past slice moved and why, and
# several of them name the very symbols this guard bans because those symbols are what was measured.
# Rewriting those notes to dodge a grep would be falsifying the record — the whole point of keeping
# them is that a reader can reconstruct how the board's RAM got where it is. The guard exists to stop
# the *code* coming back; a note saying "`SD_SET_MAX_SHARDS` cost this many bytes, and here is when it
# stopped" is the opposite of that code coming back.
#
# Deliberately one path and not a glob: the exemption should be uncomfortable enough to notice if
# someone tries to widen it.
EXEMPT = {Path("firmware/tools/resource_baseline.json")}


def main() -> int:
    failures: list[str] = []
    for directory, child_dirs, files in os.walk(ROOT):
        child_dirs[:] = sorted(name for name in child_dirs if name not in SKIP_PARTS)
        for name in sorted(files):
            path = Path(directory, name)
            if path == Path(__file__).resolve() or path.suffix not in TEXT_SUFFIXES:
                continue
            rel = path.relative_to(ROOT)
            if rel in EXEMPT:
                continue
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
