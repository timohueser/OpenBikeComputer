"""A product published as files rather than as a service.

There is no request to shape here. The adapter's whole job is to say which files a box
needs, and the shared part downloads each one, unpacks it if it is an archive, and hands
the rasters to the tail. Files are cached by name in the work directory, so a second box in
the same state or the same project downloads nothing.
"""

from pathlib import Path

from ..lattice import Refuse
from .base import READABLE, Source, http_download, placed, unpack


class BulkSource(Source):
    """A source whose adapter maps a box to files. `files` is the only thing it writes.

    `skip_missing` is for a source whose file names are arithmetic rather than an index: a
    name it does not publish answers 404, and that is a coverage edge, not a fault.
    """

    skip_missing = False

    def files(self, bbox) -> list[tuple[str, str]]:
        """The `(name, url)` of every file the box needs, biggest unit first."""

        raise NotImplementedError

    def fetch(self, bbox, workdir) -> list[Path]:
        wanted = self.files(bbox)
        if not wanted:
            raise Refuse(f"{self.key}: nothing published covers {bbox}")
        workdir.mkdir(parents=True, exist_ok=True)
        rasters, absent = [], 0
        for i, (name, url) in enumerate(wanted, 1):
            path = http_download(url, workdir / name, optional=self.skip_missing,
                                 headers=self.headers())
            if path is None:
                absent += 1
                continue
            print(f"  fetch [{i}/{len(wanted)}] {name}")
            if path.suffix.lower() == ".zip":
                rasters.extend(placed(raster, self)
                               for raster in unpack(path, workdir / f"{path.stem}.d", READABLE))
            elif path.suffix.lower() in READABLE:
                rasters.append(placed(path, self))
            else:
                raise Refuse(f"{path}: the registry expected a raster or a zip, not {path.suffix}")
        if absent:
            print(f"  {absent} of {len(wanted)} square(s) are not published: a coverage edge")
        if not rasters:
            raise Refuse(f"{self.key}: none of the {len(wanted)} file(s) the box needs is published")
        return rasters
