#!/usr/bin/env python3
"""Fail when code fetches from a host that is neither a declared source nor a listed non-bake host.

The registry is the one list of external data, and its licence and attribution are only as
complete as that list. URL literals in justfile, host/, planner/, sim/, tools/, fixtures/ and
builder/server/ name a host. That host is the host of a source's `fetch.url`, one of its
`hosts`, or one of the non-bake hosts in `NOT_SOURCES`. The check is per host, not per URL: a
new download from a host that a source already declares (github.com, for one) passes. Tests
and reserved example domains are not checked. A host built at run time (`https://{site}/…`)
cannot be checked; a placeholder in front of a fixed domain counts as `*.domain`.
"""

from __future__ import annotations

GOVERNS = ['justfile', 'host/**', 'planner/**', 'sim/**', 'tools/**', 'fixtures/**', 'builder/server/**', 'data/sources.toml']
RULE = 'Every host that code in justfile, host/, planner/, sim/, tools/, fixtures/ and builder/server/ fetches from is a source in data/sources.toml or a listed non-bake host.'

import re
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCOPE = ("justfile", "host/", "planner/", "sim/", "tools/", "fixtures/", "builder/server/")
CODE = (".rs", ".py", ".sh", ".mjs", ".js", ".ts", ".toml", "justfile")
TEST = re.compile(r"(^|/)(tests?/|test_[^/]*\.py$|[^/]*_tests?\.(rs|py)$|tests\.rs$)")
URL = re.compile(r"https?://((?:\{[^{}\s]*\}|[A-Za-z0-9.-])+)")
# RFC 2606 and RFC 6761 names, which never reach a real server.
RESERVED = re.compile(r"(^|\.)(example|test|invalid|localhost)$|(^|\.)example\.(com|net|org)$|^127\.0\.0\.1$")

NOT_SOURCES = {
    # Our own services and storage.
    "openbikecomputer.com": "our site",
    "*.openbikecomputer.com": "our services and published data",
    "*.r2.cloudflarestorage.com": "our R2 bucket",
    # Links a person reads, licence texts and schema identifiers; nothing fetches them.
    "creativecommons.org": "licence text",
    "opendatacommons.org": "licence text",
    "www.openstreetmap.org": "copyright page",
    "docs.rs": "documentation link",
    "rclone.org": "install hint",
    "rustup.rs": "install hint",
    "json-schema.org": "schema identifier",
    "www.w3.org": "XML namespace",
    "cdn.jsdelivr.net": "search lexicon setup instructions",
    "writewithharper.com": "prose checker documentation",
    # Toolchain installers: build tools, not bake inputs.
    "download.osgeo.org": "GEOS for obc-pack",
    "nodejs.org": "Node for the verification console",
    # A service a host app queries while it runs; no bake reads it.
    "nominatim.openstreetmap.org": "obc-usb-host geocoder",
}


def declared_hosts(registry: dict) -> set[str]:
    hosts = set()
    for source in registry["source"]:
        url = source["fetch"].get("url")
        if url:
            hosts.add(normalise(URL.match(url)[1]))
        hosts.update(source.get("hosts", []))
    return hosts


def normalise(host: str) -> str:
    """`{language}.wikipedia.org` is `*.wikipedia.org`; a host made only of placeholders is ''."""
    host = re.sub(r"^(\{[^{}]*\}\.?)+", "*.", host.lower()).rstrip(".")
    return "" if host in ("*.", "*") or "{" in host else host


def covered(host: str, allowed: set[str]) -> bool:
    if host in allowed:
        return True
    return any(pattern.startswith("*.") and (host.endswith(pattern[1:]) or host == pattern)
               for pattern in allowed)


def hosts_in(text: str) -> set[str]:
    """Public hosts only: a name without a dot (`https://github\\.com` in a regex) is not one."""
    return {h for h in (normalise(m[1]) for m in URL.finditer(text)) if "." in h and not RESERVED.search(h)}


def undeclared(files: dict[str, str], allowed: set[str]) -> dict[str, list[str]]:
    """Each host that is not allowed, with the files that name it."""
    found: dict[str, list[str]] = {}
    for path, text in files.items():
        for host in hosts_in(text):
            if not covered(host, allowed):
                found.setdefault(host, []).append(path)
    return found


def code_files() -> dict[str, str]:
    paths = subprocess.check_output(["git", "-C", str(ROOT), "ls-files", *SCOPE], text=True).split("\n")
    return {path: (ROOT / path).read_text(errors="replace") for path in paths
            if path.endswith(CODE) and not TEST.search(path) and (ROOT / path).is_file()}


def main() -> int:
    registry = tomllib.loads((ROOT / "data/sources.toml").read_text())
    missing = undeclared(code_files(), declared_hosts(registry) | set(NOT_SOURCES))
    if missing:
        print("data sources: code fetches from hosts that data/sources.toml does not declare.", file=sys.stderr)
        print("Add the source, or its host to `hosts` of the source it belongs to:", file=sys.stderr)
        for host, paths in sorted(missing.items()):
            print(f"  {host}: {', '.join(sorted(paths))}", file=sys.stderr)
        return 1
    print("data sources: every fetched host is declared")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
