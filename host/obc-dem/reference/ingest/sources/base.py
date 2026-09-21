"""What every adapter is: the manifest facts, and a way to obtain rasters for a box."""

import time
import urllib.request
from pathlib import Path

from ..lattice import Refuse


STAC_TIMEOUT = 300

class Source:
    """One national product: the manifest facts, and a way to obtain rasters for a box.

    `vertical_datum` is the datum the product's heights are on. An adapter whose product is
    ellipsoidal must convert before the tail: the archive is orthometric.
    """

    def __init__(self, key, country, product, resolution_m, licence, attribution, vertical_datum, extent):
        self.key = key
        self.country = country
        self.product = product
        self.resolution_m = resolution_m
        self.licence = licence
        self.attribution = attribution
        self.vertical_datum = vertical_datum
        self.extent = extent

    def covers(self, bbox) -> bool:
        west, south, east, north = bbox
        a, b, c, d = self.extent
        return not (east < a or west > c or north < b or south > d)

    def fetch(self, bbox, workdir) -> list[Path]:
        raise Refuse(f"{self.key} has no adapter; fetch the rasters by hand and pass --input")


def http_get(url: str) -> bytes:
    """A service drops the occasional request and one box pulls hundreds, so retry."""

    for delay in (2, 4, 8, None):
        try:
            with urllib.request.urlopen(url, timeout=STAC_TIMEOUT) as response:
                return response.read()
        except Exception:
            if delay is None:
                raise
            time.sleep(delay)
    raise AssertionError("unreachable")
