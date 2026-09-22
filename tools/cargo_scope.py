#!/usr/bin/env python3
"""Answer a question about the packages a Cargo scope selects.

Usage: python3 tools/cargo_scope.py has-library [CARGO ARGS...]

`has-library` exits 0 when at least one selected package has a library target, and 1 when
none has. `cargo test --doc` needs this: a scope with no library is a hard error there, not
an empty run, so the doctest step in `obc test` would fail a binary-only package after its
own tests passed.

The arguments are a Cargo command line, and the selection follows Cargo's own rules:
`-p`/`--package` and `--exclude` take glob patterns, `--workspace`/`--all` claims every
member, and without any of them the scope is the default members of the manifest, which is
the one package when `--manifest-path` names a member. Every other flag is ignored, so a
test filter can pass through unread.
"""

from __future__ import annotations

import json
import subprocess
import sys
from fnmatch import fnmatchcase

LIBRARY_KINDS = {"lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"}

def scope(arguments: list[str]) -> dict:
    """The package selection a Cargo argument list expresses."""

    chosen: dict = {"packages": [], "exclude": [], "workspace": False, "manifest": None}
    pending = ""
    for argument in arguments:
        if pending:
            chosen[pending] = chosen[pending] + [argument] if pending != "manifest" else argument
            pending = ""
        elif argument in ("-p", "--package"):
            pending = "packages"
        elif argument == "--exclude":
            pending = "exclude"
        elif argument == "--manifest-path":
            pending = "manifest"
        elif argument in ("--workspace", "--all"):
            chosen["workspace"] = True
        elif argument.startswith("--package="):
            chosen["packages"].append(argument.split("=", 1)[1])
        elif argument.startswith("--exclude="):
            chosen["exclude"].append(argument.split("=", 1)[1])
        elif argument.startswith("--manifest-path="):
            chosen["manifest"] = argument.split("=", 1)[1]
    return chosen

def selected(metadata: dict, chosen: dict) -> list[dict]:
    """The metadata packages a scope selects, the way Cargo selects them."""

    packages = metadata.get("packages", [])
    if chosen["packages"]:
        wanted = [
            package
            for package in packages
            if any(fnmatchcase(package["name"], pattern) for pattern in chosen["packages"])
        ]
    elif chosen["workspace"]:
        wanted = packages
    else:
        # A manifest that names one member defaults to that member alone. Cargo fails the
        # doctest run on it even when a sibling in the same workspace has a library.
        members = set(metadata["workspace_default_members"])
        wanted = [package for package in packages if package["id"] in members]
    return [
        package
        for package in wanted
        if not any(fnmatchcase(package["name"], pattern) for pattern in chosen["exclude"])
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
    chosen = scope(argv[1:])
    return 0 if has_library(selected(metadata(chosen["manifest"]), chosen)) else 1

if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
