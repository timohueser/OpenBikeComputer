"""The request of an `obc data` step, for a Python step: specs/obc-data.md, "The step contract"."""

import json
import sys
from pathlib import Path


def read():
    """The request that the engine writes to standard input."""
    return json.load(sys.stdin)


def view(files, directory):
    """Link each file of `files` ({path: object}) into the new `directory`, for a tool that reads a
    directory. Make it beside the output: the engine removes that directory when the step ends."""
    directory = Path(directory)
    directory.mkdir()
    for name, target in files.items():
        if Path(name).is_absolute() or ".." in Path(name).parts:
            raise ValueError(f"{name} is not a relative path")
        link = directory / name
        link.parent.mkdir(parents=True, exist_ok=True)
        link.symlink_to(target)
    return directory
