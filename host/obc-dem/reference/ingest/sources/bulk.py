"""A product published as files rather than as a service.

There is no request to shape here. The adapter's whole job is to say which files a box
needs, and the shared part downloads each one, unpacks it if it is an archive, and hands
the rasters to the tail. Files are cached by name in the work directory, so a second box in
the same state or the same project downloads nothing.
"""

from pathlib import Path

from ..lattice import Refuse
from .base import RASTER_SUFFIXES, Source, http_download, unpack


class BulkSource(Source):
    """A source whose adapter maps a box to files. `files` is the only thing it writes."""

    def files(self, bbox) -> list[tuple[str, str]]:
        """The `(name, url)` of every file the box needs, biggest unit first."""

        raise NotImplementedError

    def fetch(self, bbox, workdir) -> list[Path]:
        wanted = self.files(bbox)
        if not wanted:
            raise Refuse(f"{self.key}: nothing published covers {bbox}")
        workdir.mkdir(parents=True, exist_ok=True)
        rasters = []
        for i, (name, url) in enumerate(wanted, 1):
            print(f"  fetch [{i}/{len(wanted)}] {name}")
            path = http_download(url, workdir / name)
            if path.suffix.lower() == ".zip":
                rasters.extend(unpack(path, workdir / f"{path.stem}.d"))
            elif path.suffix.lower() in RASTER_SUFFIXES:
                rasters.append(path)
            else:
                raise Refuse(f"{path}: the registry expected a raster or a zip, not {path.suffix}")
        return rasters
