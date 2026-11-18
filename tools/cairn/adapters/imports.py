"""Dependency edges between web source files, read from their relative imports.

Many systems share one npm package, so an edge joins two tracked files, not two packages. An
import of a package or through an alias names no file here, and is not an edge.
"""

from __future__ import annotations

import os
import re
import subprocess
from pathlib import Path

#: `from "./x"`, `import "./x"`, `import("./x")`, and `new URL("./x", import.meta.url)`, which starts a worker.
IMPORT = re.compile(r"""\b(?:from|import|new URL)\s*\(?\s*["'](\.[^"'?]*)""")
#: An import can leave out the extension, name a folder, or name the `.js` that a `.ts` file becomes.
ENDINGS = ("", ".ts", ".js", ".mjs", ".svelte", ".svelte.ts", "/index.ts", "/index.js")


def target(path: str, name: str, tracked: set[str]) -> str | None:
    base = os.path.normpath(os.path.join(os.path.dirname(path), name))
    bases = [base, base.removesuffix(".js")] if base.endswith(".js") else [base]
    return next((b + ending for b in bases for ending in ENDINGS if b + ending in tracked), None)


def collect(texts: dict[str, str], tracked: set[str]) -> list[tuple[str, str, bool]]:
    """The edges of the files in `texts`, as (importer, imported, False). The check marks the test edges."""
    found = {(path, hit, False) for path, text in texts.items() for name in IMPORT.findall(text)
             if (hit := target(path, name, tracked)) and hit != path}
    return sorted(found)


def load(root: Path, settings: dict) -> tuple[list[dict], list[tuple[str, str, bool]]]:
    out = subprocess.run(["git", "-C", str(root), "ls-files"], capture_output=True, text=True, check=True).stdout
    tracked = {path for path in out.splitlines() if (root / path).is_file()}
    extensions = tuple(settings["extensions"])
    texts = {path: (root / path).read_text(encoding="utf-8", errors="replace") for path in tracked if path.endswith(extensions)}
    return [], collect(texts, tracked)
