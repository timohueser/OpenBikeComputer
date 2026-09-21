"""Switzerland: swissALTI3D, published per square kilometre over STAC."""

import json
import os
from pathlib import Path

from .base import Source, http_get


class StacSource(Source):
    """A STAC collection of published rasters, fetched as published."""

    def __init__(self, *args, stac, gsd, **kw):
        super().__init__(*args, **kw)
        self.stac = stac
        self.gsd = gsd

    def fetch(self, bbox, workdir) -> list[Path]:
        url = f"{self.stac}?bbox={bbox[0]},{bbox[1]},{bbox[2]},{bbox[3]}&limit=100"
        assets, seen = [], set()
        while url:
            page = json.loads(http_get(url))
            for feature in page["features"]:
                for name, asset in feature["assets"].items():
                    if name.endswith(".tif") and f"_{self.gsd}_" in name and name not in seen:
                        seen.add(name)
                        assets.append((name, asset["href"]))
            url = next((l["href"] for l in page.get("links", []) if l.get("rel") == "next"), None)
        workdir.mkdir(parents=True, exist_ok=True)
        paths = []
        for i, (name, href) in enumerate(sorted(assets), 1):
            path = workdir / name
            if not path.exists():
                # A half-written file in the cache would look complete to the next run, so
                # the bytes land beside the name and are moved onto it at the end.
                part = path.with_name(name + ".part")
                part.write_bytes(http_get(href))
                os.replace(part, path)
                print(f"  fetch [{i}/{len(assets)}] {name}")
            paths.append(path)
        return paths


CH = StacSource(
    "ch", "Switzerland", "swissALTI3D 2 m", 2.0,
    "Open data, attribution required", "© swisstopo", "LN02/LHN95", (5.9, 45.8, 10.5, 47.9),
    stac="https://data.geo.admin.ch/api/stac/v0.9/collections/ch.swisstopo.swissalti3d/items",
    gsd="2",
)
