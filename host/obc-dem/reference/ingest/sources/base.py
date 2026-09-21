"""What every adapter is: the manifest facts, and a way to obtain rasters for a box."""

import time
import urllib.error
import urllib.request
from pathlib import Path

from ..lattice import Refuse

HTTP_TIMEOUT = 300


class Source:
    """One national product: the manifest facts, and a way to obtain rasters for a box.

    `vertical_datum` is the datum the product's heights are on, and it is required: the
    archive is orthometric metres, and a source whose datum nobody wrote down cannot be
    held to that. An adapter whose product is **ellipsoidal** must convert before the
    tail, because an ellipsoidal height differs from an orthometric one by tens of metres,
    which is the size of a lift.
    """

    def __init__(self, key, country, product, resolution_m, licence, attribution, vertical_datum, extent):
        if not vertical_datum:
            raise Refuse(f"{key}: a source without a vertical_datum cannot be registered")
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


class ManualSource(Source):
    """A registered product with no unattended fetch, and `why` in the refusal.

    A source is here when a download needs a credential, when it is a whole-country
    archive, or when the service it used to answer on has gone. The row stays in the
    registry either way, so the licence, the attribution and the datum are recorded and
    `--input` treats the hand-fetched rasters exactly like fetched ones.
    """

    def __init__(self, *args, why, **kw):
        super().__init__(*args, **kw)
        self.why = why

    def fetch(self, bbox, workdir) -> list[Path]:
        raise Refuse(f"{self.key}: {self.why}. Fetch the rasters by hand and pass --input")


def http_get(url: str) -> bytes:
    """A service drops the occasional request and one box pulls hundreds, so retry.

    A refused request is not retried: a 400 from a coverage that has been renamed answers
    the same way four times, and its body is the only thing that says what happened.
    """

    for delay in (2, 4, 8, None):
        try:
            with urllib.request.urlopen(url, timeout=HTTP_TIMEOUT) as response:
                return response.read()
        except urllib.error.HTTPError as error:
            body = error.read()[:400].decode("utf-8", "replace").replace("\n", " ").strip()
            raise Refuse(f"{url}: HTTP {error.code} — {body}") from error
        except Exception:
            if delay is None:
                raise
            time.sleep(delay)
    raise AssertionError("unreachable")
