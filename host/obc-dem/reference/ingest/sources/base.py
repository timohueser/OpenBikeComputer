"""What every adapter is: the manifest facts, and a way to obtain rasters for a box.

Every request in the registry goes through `with_retry`, so one rule covers all of them: a
dropped connection, a 429 and a 5xx are retried; every other 4xx is the server's final
answer and is refused at once, with its body, because that body is the only thing that
says what happened.
"""

import base64
import os
import time
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from pathlib import Path

from rasterio.crs import CRS

from ..lattice import Refuse

HTTP_TIMEOUT = 300

# What the tail can open. A GeoTIFF carries its own CRS; an ESRI ASCII grid states its
# origin and its step and never its CRS, so a row published or delivered as one names the
# grid in `grid_epsg` and `placed` writes the `.prj` that format keeps a CRS in.
RASTER_SUFFIXES = {".tif", ".tiff"}
GRID_SUFFIXES = {".asc"}
READABLE = RASTER_SUFFIXES | GRID_SUFFIXES

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
    "DVR90", "RH2000", "N2000", "AHD", "m s.l.m.", "EGM2008",
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


def http_get(url: str, headers=None) -> bytes:
    """One response body, whole."""

    def once():
        request = urllib.request.Request(url, headers=headers or {})
        with urllib.request.urlopen(request, timeout=HTTP_TIMEOUT) as response:
            return response.read()

    return with_retry(once, url)


def http_download(url: str, path: Path, optional: bool = False, headers=None) -> Path | None:
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
        request = urllib.request.Request(url, headers=headers or {})
        with urllib.request.urlopen(request, timeout=HTTP_TIMEOUT) as response:
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


def unpack(archive: Path, into: Path, suffixes=RASTER_SUFFIXES, sidecars=(".prj",)) -> list[Path]:
    """The rasters inside a downloaded or delivered archive, extracted once.

    A bulk product and a portal order both arrive as a zip of tiles. Only the rasters and
    the files one needs beside it are taken out, each at the path it has inside the archive
    so two tiles of the same name cannot collide, and a member that names a path outside
    the directory is refused rather than written. `sidecars` are extracted and not
    returned: a `.prj` is where an ESRI ASCII grid keeps the CRS its own format cannot.
    """

    rasters = []
    with zipfile.ZipFile(archive) as bundle:
        for member in bundle.namelist():
            suffix = Path(member).suffix.lower()
            if suffix not in set(suffixes) | set(sidecars):
                continue
            if Path(member).is_absolute() or ".." in Path(member).parts:
                raise Refuse(f"{archive}: the member `{member}` reaches outside {into}")
            target = into / member
            if not target.exists():
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(bundle.read(member))
            if suffix in suffixes:
                rasters.append(target)
    if not rasters:
        raise Refuse(f"{archive}: holds no raster; it is not what the registry expected")
    return sorted(rasters)


def placed(path: Path, source) -> Path:
    """One raster, with the CRS its format cannot carry.

    ESRI ASCII states a grid's origin and its step and never its CRS. The tail cannot
    place a raster whose CRS nobody named, so a row published or delivered as one names
    the grid and the CRS goes beside the file, which is where that format keeps it. A
    `.prj` the portal shipped is left alone.
    """

    if path.suffix.lower() not in GRID_SUFFIXES:
        return path
    prj = path.with_suffix(".prj")
    if prj.exists():
        return path
    if source.grid_epsg is None:
        raise Refuse(f"{path}: an ESRI ASCII grid names no CRS and `{source.key}` states no "
                     "grid for one; put the portal's .prj beside it")
    prj.write_text(CRS.from_epsg(source.grid_epsg).to_wkt(), encoding="utf-8")
    return path


class Credential:
    """What a portal wants before it answers, read from the environment and never argv.

    These portals read a key out of the URL or out of an `Authorization` header, so a
    credential is one of two shapes: a `param` that `OBC_REFERENCE_<KEY>_TOKEN` becomes,
    or HTTP Basic from `OBC_REFERENCE_<KEY>_USER` and `_PASSWORD`. Either way the secret
    is in the environment, the rule `publish` already holds to, because argv is readable
    by every process on the box.
    """

    def __init__(self, key: str, param: str | None = None):
        prefix = f"OBC_REFERENCE_{key.upper().replace('-', '_')}"
        self.param = param
        self.variables = ((f"{prefix}_TOKEN",) if param
                          else (f"{prefix}_USER", f"{prefix}_PASSWORD"))

    @property
    def names(self) -> str:
        return " and ".join(self.variables)

    def values(self) -> tuple[str, ...]:
        return tuple(os.environ.get(name, "").strip() for name in self.variables)

    def present(self) -> bool:
        return all(self.values())

    def query(self) -> str:
        """The credential as the query parameter the portal reads it from, `&` first."""

        if not self.param:
            return ""
        return "&" + urllib.parse.urlencode({self.param: self.values()[0]})

    def headers(self) -> dict[str, str]:
        if self.param:
            return {}
        pair = base64.b64encode(":".join(self.values()).encode()).decode()
        return {"Authorization": f"Basic {pair}"}


class Source:
    """One national product: the manifest facts, and a way to obtain rasters for a box.

    `vertical_datum` is the datum the product's heights are on, and it has to name a
    height system `ORTHOMETRIC` recognises. The archive is orthometric metres, and an
    ellipsoidal height differs from one by tens of metres, which is the size of a lift.

    Three keywords are for a source behind an account. `credential` is what the portal
    wants before it answers; `grid_epsg` is the grid its ESRI ASCII files are on, because
    that format carries no CRS; and `steps` are the clicks only a person can do, which
    `wizard` walks and nothing else reads.
    """

    def __init__(self, key, country, product, resolution_m, licence, attribution,
                 vertical_datum, extent, credential=None, grid_epsg=None, steps=()):
        self.check_datum(key, vertical_datum)
        self.key = key
        self.country = country
        self.product = product
        self.resolution_m = resolution_m
        self.licence = licence
        self.attribution = attribution
        self.vertical_datum = vertical_datum
        self.extent = extent
        self.credential = credential
        self.grid_epsg = grid_epsg
        self.steps = steps

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

    def require_credential(self) -> None:
        """Refuse a fetch the portal will not answer, before any request is made.

        This is the whole difference a credential makes to the tool: with it set the
        adapter fetches live, and without it the refusal names the variable, `--input`
        and the wizard, so a source nobody can fetch is not a dead end.
        """

        if self.credential is None or self.credential.present():
            return
        raise Refuse(
            f"{self.key} needs {self.credential.names} in the environment to fetch live. "
            f"Set it, or download the files by hand and pass --input <dir>; "
            f"`python3 ingest.py wizard {self.key}` walks the portal step by step"
        )

    def headers(self) -> dict[str, str]:
        """What a download carries, which is the credential when the portal wants Basic."""

        return self.credential.headers() if self.credential else {}

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
        walk = (f"; `python3 ingest.py wizard {self.key}` walks the portal step by step"
                if self.steps else "")
        raise Refuse(f"{self.key}: {self.why}. Fetch the rasters by hand and pass --input{walk}")
