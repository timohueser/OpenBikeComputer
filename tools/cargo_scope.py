#!/usr/bin/env python3
"""Answer a question about the packages a Cargo scope selects.

Usage: python3 tools/cargo_scope.py has-library [CARGO ARGS...]

`has-library` exits 0 when at least one selected package has a library target, and 1 when
none has. `cargo test --doc` needs this: a scope with no library is a hard error there, not
an empty run, so the doctest step in `obc test` would fail a binary-only package after its
own tests passed.

The arguments are a Cargo command line. Everything that is not `-p`/`--package` or
`--manifest-path` is ignored, so a test filter or any other flag can pass through unread.
"""

from __future__ import annotations

import json
import subprocess
import sys
from fnmatch import fnmatchcase

LIBRARY_KINDS = {"lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"}

def scope(arguments: list[str]) -> tuple[list[str], str | None]:
    """The package patterns and the manifest a Cargo argument list selects."""

    patterns: list[str] = []
    manifest: str | None = None
    pending = ""
    for argument in arguments:
        if pending == "package":
            patterns.append(argument)
            pending = ""
        elif pending == "manifest":
            manifest = argument
            pending = ""
        elif argument in ("-p", "--package"):
            pending = "package"
        elif argument == "--manifest-path":
            pending = "manifest"
        elif argument.startswith("--package="):
            patterns.append(argument.split("=", 1)[1])
        elif argument.startswith("--manifest-path="):
            manifest = argument.split("=", 1)[1]
    return patterns, manifest

def selected(metadata: dict, patterns: list[str]) -> list[dict]:
    """The metadata packages the patterns select. No pattern means every package."""

    if not patterns:
        return metadata.get("packages", [])
    return [
        package
        for package in metadata.get("packages", [])
        if any(fnmatchcase(package["name"], pattern) for pattern in patterns)
    ]

def has_library(packages: list[dict]) -> bool:
    return any(
        LIBRARY_KINDS & set(target.get("kind", ()))
        for package in packages
        for target in package.get("targets", [])
    )

def metadata(manifest: str | None) -> dict:
    command = ["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"]
    if manifest is not None:
        command += ["--manifest-path", manifest]
    return json.loads(subprocess.run(command, check=True, capture_output=True, text=True).stdout)

def main(argv: list[str]) -> int:
    if not argv or argv[0] != "has-library":
        print(__doc__.splitlines()[2], file=sys.stderr)
        return 2
    patterns, manifest = scope(argv[1:])
    return 0 if has_library(selected(metadata(manifest), patterns)) else 1

if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
