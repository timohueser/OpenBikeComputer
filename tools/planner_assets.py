"""The planner map assets: the fonts and sprites of the Protomaps assets archive."""

import io
from pathlib import Path
import sys
import zipfile

from . import step_request


def install_assets(data, destination):
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        for entry in archive.infolist():
            parts = Path(entry.filename).parts[1:]
            if entry.is_dir() or not parts or ".." in parts:
                continue
            if parts[0] not in {"fonts", "sprites"}:
                continue
            path = destination.joinpath(*parts)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(archive.read(entry))
    for name in ["fonts/OFL.txt", "fonts/Noto Sans Regular/0-255.pbf",
                 "sprites/v4/light.json", "sprites/v4/dark@2x.png"]:
        if not (destination / name).is_file():
            raise ValueError(f"Map assets are missing {name}")


def step():
    """The `obc data` step `planner/assets`: the fonts and sprites of the Protomaps assets, with the
    MIT notice of the Mapzen icons beside the sprites."""
    request = step_request.read()
    (archive,) = step_request.files(request, "protomaps-assets").values()
    (notice,) = step_request.files(request, "tangrams-icons").values()
    output = Path(request["output"]) / "assets"
    install_assets(archive.read_bytes(), output)
    (output / "sprites/LICENSE.txt").write_bytes(notice.read_bytes())


if __name__ == "__main__":
    if sys.argv[1:] != ["--step"]:
        sys.exit("usage: python -m tools.planner_assets --step, with the request of the step on standard input")
    step()
