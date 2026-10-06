# SPDX-License-Identifier: MIT OR Apache-2.0
"""Check the downloads archive before loading its known installer module."""
import hashlib
from pathlib import Path, PurePosixPath
import platform
import shutil
import sys
import tarfile

archive, expected, output, python = sys.argv[1:]
if platform.python_implementation() != "CPython" or platform.python_version() != python:
    raise ValueError("Prepare the exact configured CPython on the publication owner")
with open(archive, "rb") as source:
    if hashlib.file_digest(source, "sha256").hexdigest() != expected:
        raise ValueError("Installer archive checksum differs")
seen = set()
with tarfile.open(archive, "r:gz") as bundle:
    for entry in bundle:
        path = PurePosixPath(entry.name)
        if not entry.isfile() or path.is_absolute() or any(part in (".", "..") for part in path.parts) or path.as_posix() != entry.name or entry.name in seen:
            raise ValueError("Installer archive needs unique regular relative paths")
        seen.add(entry.name)
        target = Path(output) / entry.name
        target.parent.mkdir(parents=True, exist_ok=True)
        with bundle.extractfile(entry) as source, target.open("xb") as destination:
            shutil.copyfileobj(source, destination)
        target.chmod(entry.mode & 0o755)
if "tools/planner_install.py" not in seen or "tools/planner_activation.py" not in seen:
    raise ValueError("Downloads runtime lacks the known installation helper")
