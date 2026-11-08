"""A product published as files rather than as a service.

There is no request to shape here. The adapter's whole job is to say which files a box
needs, and the shared part downloads each one, unpacks it if it is an archive, and hands
the rasters to the tail. Files are cached by name in the work directory, so a second box in
the same state or the same project downloads nothing.
"""

from functools import partial
from pathlib import Path

from ..lattice import Refuse
from .base import MOST_MISSING, READABLE, Source, each, http_download, placed, unpack
from .protocols import PARALLEL_REQUESTS


class BulkSource(Source):
    """A source whose adapter maps a box to files. `files` is the only thing it writes.

    `skip_missing` is for a source whose file names are arithmetic rather than an index: a
    name it does not publish answers 404, and that is a coverage edge, not a fault.

    `listed` is for a source whose file names come from a catalogue: a named file that still
    answers 404 after the retries goes in `missing`, which the index of the version records.
    """

    credential_style = "headers"
    skip_missing = False
    listed = False
    # Which archive members are the heights. A row whose zips carry the same grid twice
    # names the one format it takes.
    members = READABLE

    def files(self, bbox) -> list[tuple[str, str]]:
        """The `(name, url)` of every file the box needs, biggest unit first."""

        raise NotImplementedError

    def fetch(self, bbox, workdir) -> list[Path]:
        return self.download(self.files(bbox), workdir)

    def download(self, wanted, workdir, relink=None) -> list[Path]:
        """The rasters of the `(name, url)` files of `wanted`, downloaded and unpacked.

        `relink(name)` gives a new URL for each retry of a file, if the source gives one.
        """

        workdir.mkdir(parents=True, exist_ok=True)
        rasters, absent = [], 0

        def one(item):
            name, url = item
            return http_download(url, workdir / name, optional=self.skip_missing,
                                 headers=self.headers_for(url), what=f"{self.key} {name}", listed=self.listed,
                                 relink=relink and partial(relink, name))

        # The files of one box are independent, and a box is tens of them, so they arrive
        # a few at a time; unpacking stays in order below, because it is the disk's turn.
        downloaded = each(one, wanted, PARALLEL_REQUESTS)
        for i, ((name, _), path) in enumerate(zip(wanted, downloaded), 1):
            if path is None and self.listed:
                self.missing.append(name)
                print(f"  {name}: in the catalogue, but the server does not hold it", flush=True)
                continue
            if path is None:
                absent += 1
                continue
            print(f"  fetch [{i}/{len(wanted)}] {name}", flush=True)
            if path.suffix.lower() == ".zip":
                inside = unpack(path, workdir / f"{path.stem}.d", self.members, optional=self.skip_missing)
                absent += not inside
                rasters.extend(placed(raster, self, workdir) for raster in inside)
            elif path.suffix.lower() in READABLE:
                rasters.append(placed(path, self, workdir))
            else:
                raise Refuse(f"{path}: the registry expected a raster or a zip, not {path.suffix}")
        if len(self.missing) > MOST_MISSING:
            raise Refuse(f"{self.key}: the server does not hold {len(self.missing)} files of its catalogue "
                         f"in one fetch, such as {self.missing[0]}")
        if absent:
            print(f"  {absent} of {len(wanted)} square(s) are not published: a coverage edge")
        # A box the state publishes nothing for is a coverage edge like any other, and a
        # per-tile run over a country box meets one at every corner: it is empty, not wrong.
        return rasters
