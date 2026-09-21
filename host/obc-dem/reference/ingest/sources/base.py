"""What every adapter is: the manifest facts, and a way to obtain rasters for a box.

Every request in the registry goes through `with_retry`, so one rule covers all of them: a
dropped connection, a 429 and a 5xx are retried; every other 4xx is the server's final
answer and is refused at once, with its body, because that body is the only thing that
says what happened.
"""

import os
import time
import urllib.error
import urllib.request
import zipfile
from pathlib import Path

from ..lattice import Refuse

HTTP_TIMEOUT = 300

# What a bulk archive is worth unpacking. A `.asc` grid carries no CRS of its own and the
# tail cannot place a raster whose CRS nobody named, so it is not taken out of a zip: a
# source that ships one has to state its grid, and none in the registry does.
RASTER_SUFFIXES = {".tif", ".tiff"}

# How long to wait before each retry. A service drops the occasional request and one box
# pulls hundreds, so the first failure is never the answer.
RETRY_DELAYS = (2, 4, 8)

# The orthometric height systems the registry knows. A row has to name one of them: the
# archive is orthometric metres and the bakery holds the reference against Copernicus on
# EGM2008, so a datum nobody recognised is a refusal and not a footnote. Add a name here
# when a new source's agency documents an orthometric system.
ORTHOMETRIC = (
    "LN02", "LHN95", "NGF-IGN69", "NAVD88", "NN2000", "REDNAP", "NAP",
    "Ordnance Datum Newlyn", "EVRF2000", "CGVD2013", "NZVD2016", "DHHN2016", "DHHN92",
    "m s.l.m.", "EGM2008",
)

# What an ellipsoidal height is called. It stands tens of metres from an orthometric one,
# which is the size of a lift, so a row that names one is refused rather than converted.
ELLIPSOIDAL = ("ellipsoid", "wgs84 h", "wgs 84 h", "nad83 h", "grs80 h")


def with_retry(attempt, what: str, absent=()):
    """Run one request, retrying what is worth retrying and refusing what is not.

    `absent` is the set of status codes that mean "there is nothing here", which for a
    grid of tiles is a coverage edge rather than a fault; those come back as `None`.
    """

    for delay in (*RETRY_DELAYS, None):
        try:
            return attempt()
        except urllib.error.HTTPError as error:
            if error.code in absent:
                return None
            body = error.read()[:400].decode("utf-8", "replace").replace("\n", " ").strip()
            if error.code != 429 and error.code < 500:
                raise Refuse(f"{what}: HTTP {error.code} — {body}") from error
            if delay is None:
                raise Refuse(f"{what}: HTTP {error.code} after {len(RETRY_DELAYS)} "
                             f"retries — {body}") from error
        except (urllib.error.URLError, TimeoutError, OSError) as error:
            if delay is None:
                raise Refuse(f"{what}: {type(error).__name__}: {error}") from error
        time.sleep(delay)
    raise AssertionError("unreachable")


def http_get(url: str) -> bytes:
    """One response body, whole."""

    def once():
        with urllib.request.urlopen(url, timeout=HTTP_TIMEOUT) as response:
            return response.read()

    return with_retry(once, url)


def http_download(url: str, path: Path, optional: bool = False) -> Path | None:
    """Stream one file to disk, which is how a bulk product of several gigabytes arrives.

    The bytes land beside the name and are moved onto it at the end, so a download that
    was cut short is never mistaken for a complete one by the next run. `optional` turns a
    404 into absence, which is what a grid square outside its state is.
    """

    if path.exists():
        return path
    path.parent.mkdir(parents=True, exist_ok=True)
    part = path.with_name(path.name + ".part")

    def once():
        with urllib.request.urlopen(url, timeout=HTTP_TIMEOUT) as response:
            total = int(response.headers.get("Content-Length") or 0)
            done = 0
            with part.open("wb") as handle:
                while chunk := response.read(1 << 20):
                    handle.write(chunk)
                    done += len(chunk)
                    if total:
                        print(f"\r    {path.name}: {done / total:.0%} of {total / 1e6:.0f} MB",
                              end="", flush=True)
        if total:
            print()
        os.replace(part, path)
        return path

    try:
        return with_retry(once, url, absent=(404,) if optional else ())
    finally:
        part.unlink(missing_ok=True)


def unpack(archive: Path, into: Path) -> list[Path]:
    """The rasters inside a downloaded archive, extracted once.

    A bulk product arrives as a zip of tiles. Only rasters are taken out of it, each at
    the path it has inside the archive so two tiles of the same name cannot collide, and a
    member that names a path outside the directory is refused rather than written.
    """

    rasters = []
    with zipfile.ZipFile(archive) as bundle:
        for member in bundle.namelist():
            if Path(member).suffix.lower() not in RASTER_SUFFIXES:
                continue
            if Path(member).is_absolute() or ".." in Path(member).parts:
                raise Refuse(f"{archive}: the member `{member}` reaches outside {into}")
            target = into / member
            if not target.exists():
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(bundle.read(member))
            rasters.append(target)
    if not rasters:
        raise Refuse(f"{archive}: holds no raster; it is not what the registry expected")
    return sorted(rasters)


class Source:
    """One national product: the manifest facts, and a way to obtain rasters for a box.

    `vertical_datum` is the datum the product's heights are on, and it has to name a
    height system `ORTHOMETRIC` recognises. The archive is orthometric metres, and an
    ellipsoidal height differs from one by tens of metres, which is the size of a lift.
    """

    def __init__(self, key, country, product, resolution_m, licence, attribution,
                 vertical_datum, extent):
        self.check_datum(key, vertical_datum)
        self.key = key
        self.country = country
        self.product = product
        self.resolution_m = resolution_m
        self.licence = licence
        self.attribution = attribution
        self.vertical_datum = vertical_datum
        self.extent = extent

    @staticmethod
    def check_datum(key: str, datum: str) -> None:
        if not datum:
            raise Refuse(f"{key}: a source without a vertical_datum cannot be registered")
        if any(word in datum.lower() for word in ELLIPSOIDAL):
            raise Refuse(
                f"{key}: `{datum}` is an ellipsoidal height, and the archive is orthometric "
                "metres; convert the product before it reaches the tail"
            )
        if not any(name in datum for name in ORTHOMETRIC):
            raise Refuse(
                f"{key}: `{datum}` names no orthometric height system this tool recognises; "
                "add it to ORTHOMETRIC in sources/base.py if the agency documents one"
            )

    def covers(self, bbox) -> bool:
        west, south, east, north = bbox
        a, b, c, d = self.extent
        return not (east < a or west > c or north < b or south > d)

    def fetch(self, bbox, workdir) -> list[Path]:
        raise Refuse(f"{self.key} has no adapter; fetch the rasters by hand and pass --input")


class ManualSource(Source):
    """A registered product with no unattended fetch, and `why` in the refusal.

    A source is here when a download needs a credential, when it is a whole-country
    archive, or when its index is a shape the tool does not read yet. The row stays in the
    registry either way, so the licence, the attribution and the datum are recorded and
    `--input` treats the hand-fetched rasters exactly like fetched ones.
    """

    def __init__(self, *args, why, **kw):
        super().__init__(*args, **kw)
        self.why = why

    def fetch(self, bbox, workdir) -> list[Path]:
        raise Refuse(f"{self.key}: {self.why}. Fetch the rasters by hand and pass --input")
