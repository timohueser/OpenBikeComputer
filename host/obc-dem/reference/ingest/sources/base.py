"""What every adapter is: the manifest facts, and a way to obtain rasters for a box."""

import os
import time
import urllib.error
import urllib.request
import zipfile
from pathlib import Path

from ..lattice import Refuse

HTTP_TIMEOUT = 300

# What is worth taking out of a downloaded archive. A `.asc` grid carries no CRS of its
# own, so a source that ships one states the CRS in its registry row.
RASTER_SUFFIXES = {".tif", ".tiff", ".asc"}


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


def http_download(url: str, path: Path) -> Path:
    """Stream one file to disk, which is how a bulk product of several gigabytes arrives.

    The bytes land beside the name and are moved onto it at the end, so a download that
    was cut short is never mistaken for a complete one by the next run.
    """

    if path.exists():
        return path
    path.parent.mkdir(parents=True, exist_ok=True)
    part = path.with_name(path.name + ".part")
    try:
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
        print()
    except urllib.error.HTTPError as error:
        part.unlink(missing_ok=True)
        raise Refuse(f"{url}: HTTP {error.code}") from error
    os.replace(part, path)
    return path


def unpack(archive: Path, into: Path) -> list[Path]:
    """The rasters inside a downloaded archive, extracted once.

    A bulk product arrives as a zip of tiles. Only rasters are taken out of it, and a
    member whose name reaches outside the directory is refused rather than written.
    """

    rasters = []
    with zipfile.ZipFile(archive) as bundle:
        for member in bundle.namelist():
            suffix = Path(member).suffix.lower()
            if suffix not in RASTER_SUFFIXES:
                continue
            target = (into / Path(member).name).resolve()
            if not str(target).startswith(str(into.resolve())):
                raise Refuse(f"{archive}: the member `{member}` reaches outside {into}")
            if not target.exists():
                into.mkdir(parents=True, exist_ok=True)
                target.write_bytes(bundle.read(member))
            rasters.append(target)
    if not rasters:
        raise Refuse(f"{archive}: holds no raster; it is not what the registry expected")
    return sorted(rasters)


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
