"""Switzerland: swissALTI3D, published per square kilometre over STAC."""

import os
from pathlib import Path

from .base import Source, decode_tiff, http_get
from .stac import stac_items


class StacSource(Source):
    """A STAC collection of published rasters, fetched as published."""

    def __init__(self, *args, stac, gsd, **kw):
        super().__init__(*args, **kw)
        self.stac = stac
        self.gsd = gsd

    def assets(self, bbox) -> list[tuple[str, str]]:
        """The name and the URL of each raster of the box, sorted by name."""

        url = f"{self.stac}?bbox={bbox[0]},{bbox[1]},{bbox[2]},{bbox[3]}&limit=100"
        # A square is published again after each survey: the newest item holds today's ground,
        # and an older one would raise the maximum and move the mean.
        newest = {}
        for feature in stac_items(url):
            square = tuple(feature["bbox"])
            if square not in newest or feature["properties"]["datetime"] > newest[square]["properties"]["datetime"]:
                newest[square] = feature
        assets = {name: asset["href"] for feature in newest.values() for name, asset in feature["assets"].items()
                  if name.endswith(".tif") and f"_{self.gsd}_" in name}
        return sorted(assets.items())

    def fetch(self, bbox, workdir) -> list[Path]:
        assets = self.assets(bbox)
        workdir.mkdir(parents=True, exist_ok=True)
        paths = []
        for i, (name, href) in enumerate(assets, 1):
            path = workdir / name
            if not path.exists():
                # A half-written file in the cache would look complete to the next run, so
                # the bytes land beside the name and are moved onto it at the end.
                part = path.with_name(name + ".part")
                part.write_bytes(http_get(href, check=decode_tiff))
                os.replace(part, path)
                print(f"  fetch [{i}/{len(assets)}] {name}")
            paths.append(path)
        return paths


CH = StacSource(
    "ch", "Switzerland", "swissALTI3D 2 m", 2.0,
    "Open data, attribution required", "LN02/LHN95",
    stac="https://data.geo.admin.ch/api/stac/v0.9/collections/ch.swisstopo.swissalti3d/items",
    gsd="2",
)
