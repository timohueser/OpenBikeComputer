"""The request of an `obc data` step, for a Python step: specs/obc-data.md, "The step contract"."""

import json
import sys
from pathlib import Path


def read():
    """The request that the engine writes to standard input."""
    return json.load(sys.stdin)


def files(request, source):
    """The files of snapshot `source` ({name: object path}). A captured file is named
    `#<query>/<path>`; here its name is the path."""
    return {name.split("/", 1)[1] if name.startswith("#") else name: Path(path)
            for name, path in request["snapshots"][source].items()}


def metrics(request, values):
    """Write the JSON object `values` as the metrics of the step."""
    Path(request["metrics"]).write_text(json.dumps(values, sort_keys=True))


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
