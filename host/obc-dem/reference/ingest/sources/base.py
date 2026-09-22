"""What every adapter is: the manifest facts, and a way to obtain rasters for a box.

Every request in the registry goes through `with_retry`, so one rule covers all of them: a
dropped connection, a 429 and a 5xx are retried; every other 4xx is the server's final
answer and is refused at once, with its body, because that body is the only thing that
says what happened.

Two rules hold a credential. A refusal quotes what came off the wire, and a URL with a
token in it is a URL with a secret in it, so every message goes through `redact` first and
a keyed request is labelled by its source and box rather than by its URL. And a credential
is sent to the row's own hosts only: an index that names a download elsewhere is refused,
and a redirect that leaves the host loses the `Authorization` header.
"""

import base64
import hashlib
import os
import shutil
import time
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from pathlib import Path

from rasterio.crs import CRS

from ..lattice import Refuse

HTTP_TIMEOUT = 300

# What one member of a delivered archive may weigh unpacked. A national DEM tile is
# megabytes and an ELVIS order's largest file is gigabytes; nothing legitimate reaches
# this, and a zip that claims to is not a delivery.
MAX_MEMBER_BYTES = 16 << 30

# What the tail can open. A GeoTIFF carries its own CRS; an ESRI ASCII grid states its
# origin and its step and never its CRS, so a row published or delivered as one names the
# grid in `grid_epsg` and `placed` writes the `.prj` that format keeps a CRS in.
RASTER_SUFFIXES = {".tif", ".tiff"}
GRID_SUFFIXES = {".asc"}
READABLE = RASTER_SUFFIXES | GRID_SUFFIXES

# How long to wait before each retry. A service drops the occasional request and one box
# pulls hundreds, so the first failure is never the answer.
# A country run is days of requests, and a public service that is restarted or briefly
# overloaded answers 5xx for minutes, not seconds: the delays add up to twelve and a half.
RETRY_DELAYS = (5, 30, 120, 600)

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


def secrets() -> tuple[str, ...]:
    """Every credential the environment holds, and every form it travels in, longest first.

    The whole `OBC_REFERENCE_*` namespace is read, not one row's variables, because a
    message is redacted wherever it comes from and a token is a token, however short.

    A token rides in a URL, so a server that echoes the request echoes it **encoded**: a
    credential with a `/` or a `=` in it comes back percent-escaped and would not match
    its own plain form. Both escapings `urlencode` can produce are therefore redacted as
    well, and the longest form is replaced first so no partial match is left behind.
    """

    held = set()
    for name, value in os.environ.items():
        if not name.startswith("OBC_REFERENCE_"):
            continue
        value = value.strip()
        if value:
            held.update({value, urllib.parse.quote(value, safe=""),
                         urllib.parse.quote_plus(value)})
    return tuple(sorted(held, key=len, reverse=True))


def redact(text: str) -> str:
    """One message with every credential taken out of it.

    A service answers a 403 by quoting the request it refused, so nothing that came off
    the wire reaches a refusal until it has been through here.
    """

    for secret in secrets():
        text = text.replace(secret, "<redacted>")
    return text


def host_of(url: str) -> str:
    return urllib.parse.urlsplit(url).hostname or ""


def inside(url: str, suffix: str) -> bool:
    """Whether a URL's host is that host or one under it."""

    host = host_of(url)
    return host == suffix or host.endswith("." + suffix)


class DropAuthOnRedirect(urllib.request.HTTPRedirectHandler):
    """A redirect to another host does not carry the credential with it.

    urllib copies a request's headers onto the redirect it follows, so an `Authorization`
    header follows a `Location` anywhere. A portal that redirects a download to a content
    network would hand that network the password.
    """

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        onward = super().redirect_request(req, fp, code, msg, headers, newurl)
        if onward is not None and host_of(newurl) != host_of(req.full_url):
            onward.headers = {name: value for name, value in onward.headers.items()
                              if name.lower() != "authorization"}
        return onward


OPENER = urllib.request.build_opener(DropAuthOnRedirect)


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
                raise Refuse(redact(f"{what}: HTTP {error.code} — {body}")) from error
            if delay is None:
                raise Refuse(redact(f"{what}: HTTP {error.code} after {len(RETRY_DELAYS)} "
                                    f"retries — {body}")) from error
        except (urllib.error.URLError, TimeoutError, OSError) as error:
            if delay is None:
                raise Refuse(redact(f"{what}: {type(error).__name__}: {error}")) from error
        time.sleep(delay)
    raise AssertionError("unreachable")


def http_get(url: str, headers=None, what: str | None = None) -> bytes:
    """One response body, whole.

    `what` is what a refusal calls this request. A keyed service reads its token out of
    the URL, so the URL is not it.
    """

    def once():
        request = urllib.request.Request(url, headers=headers or {})
        with OPENER.open(request, timeout=HTTP_TIMEOUT) as response:
            return response.read()

    return with_retry(once, what or url)


def http_download(url: str, path: Path, optional: bool = False, headers=None,
                  what: str | None = None) -> Path | None:
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
        with OPENER.open(request, timeout=HTTP_TIMEOUT) as response:
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
        return with_retry(once, what or url, absent=(404,) if optional else ())
    finally:
        part.unlink(missing_ok=True)


def digest_of(path: Path) -> str:
    """The sha256 of one file's bytes, read in blocks because a delivery is large."""

    reader = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1 << 20):
            reader.update(chunk)
    return reader.hexdigest()


def extract(bundle, member: str, target: Path, archive: Path) -> None:
    """One member of an archive, streamed to disk and never silently a different file.

    The bytes are streamed rather than read whole, because a member can be gigabytes, and
    they land beside the name so a cut-short extraction is not mistaken for a complete
    one. A target that is already there has to be the same file: a delivery re-issued
    under an old name is the one case that must not pass unnoticed.
    """

    target.parent.mkdir(parents=True, exist_ok=True)
    part = target.with_name(target.name + ".part")
    try:
        written = 0
        with bundle.open(member) as source, part.open("wb") as handle:
            while chunk := source.read(1 << 20):
                written += len(chunk)
                if written > MAX_MEMBER_BYTES:
                    raise Refuse(f"{archive}: the member `{member}` is over "
                                 f"{MAX_MEMBER_BYTES >> 30} GiB unpacked, which is not a "
                                 "tile; this is not the delivery the registry expected")
                handle.write(chunk)
        if target.exists():
            if digest_of(target) != digest_of(part):
                raise Refuse(f"{target} is already there and is not the `{member}` inside "
                             f"{archive}; delete it and run again")
            return
        os.replace(part, target)
    finally:
        part.unlink(missing_ok=True)


def unpack(archive: Path, into: Path, suffixes=RASTER_SUFFIXES, sidecars=(".prj",)) -> list[Path]:
    """The rasters inside a downloaded or delivered archive, extracted once.

    A bulk product and a portal order both arrive as a zip of tiles. Only the rasters and
    the files one needs beside it are taken out, each at the path it has inside the archive
    so two tiles of the same name cannot collide, and a member that names a path outside
    the directory is refused rather than written. `sidecars` are extracted and not
    returned: a `.prj` is where an ESRI ASCII grid keeps the CRS its own format cannot.

    A zip inside the zip is only a problem when this level holds no raster of its own: an
    order that ships its documents as `metadata/docs.zip` beside the DEM is an ordinary
    delivery, and refusing it would be refusing the data over the paperwork.
    """

    rasters, nested = [], []
    with zipfile.ZipFile(archive) as bundle:
        for member in bundle.namelist():
            suffix = Path(member).suffix.lower()
            if suffix == ".zip":
                nested.append(member)
                continue
            if suffix not in set(suffixes) | set(sidecars):
                continue
            if Path(member).is_absolute() or ".." in Path(member).parts:
                raise Refuse(f"{archive}: the member `{member}` reaches outside {into}")
            target = into / member
            extract(bundle, member, target, archive)
            if suffix in suffixes:
                rasters.append(target)
    if not rasters and nested:
        raise Refuse(f"{archive}: it holds no raster of its own, only another archive, "
                     f"`{nested[0]}`. Unpack that one yourself and pass the directory it is "
                     "in: a zip inside a zip is not a delivery shape the registry reads")
    if not rasters:
        raise Refuse(f"{archive}: holds no raster; it is not what the registry expected")
    return sorted(rasters)


def placed(path: Path, source, into: Path) -> Path:
    """One raster the tail can place, and the `.prj` an ESRI ASCII grid arrives without.

    ESRI ASCII states a grid's origin and its step and never its CRS, so a row published
    or delivered as one names the grid and the CRS goes beside the file, which is where
    that format keeps it. A `.prj` the portal shipped is used as it is.

    The CRS is never written into a delivery directory: what the owner downloaded stays
    as the portal left it. So a grid that is not already in the work directory is copied
    there under its own digest, which is also what lets a re-issued grid of the same name
    land. A grid the adapter fetched is already in the work directory and is not copied.
    """

    if path.suffix.lower() not in GRID_SUFFIXES:
        return path
    if path.with_suffix(".prj").exists():
        return path
    if source.grid_epsg is None:
        raise Refuse(f"{path}: an ESRI ASCII grid names no CRS and `{source.key}` states no "
                     "grid for one; put the portal's .prj beside it")
    if into.resolve() not in path.resolve().parents:
        into.mkdir(parents=True, exist_ok=True)
        copy = into / f"{path.stem}-{digest_of(path)[:12]}{path.suffix}"
        if not copy.exists():
            shutil.copy2(path, copy)
        path = copy
    path.with_suffix(".prj").write_text(CRS.from_epsg(source.grid_epsg).to_wkt(),
                                       encoding="utf-8")
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

    `resolution_m` is the product's step, and it is `None` for a product that has no one
    step: the step of an ELVIS order is whatever survey the order covered.

    Five keywords are for a source behind an account. `credential` is what the portal
    wants before it answers and `credential_hosts` are the hosts it may be sent to;
    `grid_epsg` is the grid its ESRI ASCII files are on, because that format carries no
    CRS; `confirm_datum` is the datum an `--input` delivery has to be confirmed as, for a
    portal that also publishes an ellipsoidal one; and `steps` are the clicks only a
    person can do, which `wizard` walks and nothing else reads.
    """

    #: How this kind of adapter carries a credential: `"query"` when its requests are
    #: URLs it builds itself, `"headers"` when it downloads published files, and `None`
    #: when it carries none at all. A row whose credential does not match its adapter is
    #: refused where it is written, because a credential the fetch path drops on the floor
    #: is a request that goes out unsigned and comes back as an error page.
    credential_style = None

    def __init__(self, key, country, product, resolution_m, licence, attribution,
                 vertical_datum, extent, credential=None, credential_hosts=(),
                 grid_epsg=None, confirm_datum=None, steps=()):
        self.check_datum(key, vertical_datum)
        self.check_credential(key, credential, credential_hosts)
        self.key = key
        self.country = country
        self.product = product
        self.resolution_m = resolution_m
        self.licence = licence
        self.attribution = attribution
        self.vertical_datum = vertical_datum
        self.extent = extent
        self.credential = credential
        self.credential_hosts = credential_hosts
        self.grid_epsg = grid_epsg
        self.confirm_datum = confirm_datum
        self.steps = steps

    def check_credential(self, key: str, credential, hosts) -> None:
        """Whether this adapter can honour the credential the row states.

        The check is at the row, not at the request, because a credential the fetch path
        never reads is not a smaller problem than a wrong one: the request goes out
        unsigned and the portal answers with an error page.
        """

        if credential is None:
            if hosts:
                raise Refuse(f"{key}: credential_hosts without a credential says nothing")
            return
        wanted = "query" if credential.param else "headers"
        if wanted != self.credential_style:
            raise Refuse(f"{key}: a credential in the {wanted} cannot be carried by "
                         f"{type(self).__name__}, whose requests carry "
                         f"{self.credential_style or 'no credential'}")
        if wanted == "headers" and not hosts:
            raise Refuse(f"{key}: a credential sent as a header needs credential_hosts, the "
                         "hosts it may be sent to, because an index names where a download is")

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

    def headers_for(self, url: str) -> dict[str, str]:
        """What a download of that URL carries, which is the credential or nothing.

        A STAC index names the host a download comes from, and an index is data, not
        code: a `href` that points somewhere else must not be handed the password. So the
        row states the hosts its credential belongs to and a URL outside them is refused
        by name — not fetched unsigned, because a download that quietly drops its
        credential comes back as an error page and not as a raster.
        """

        if self.credential is None or not self.credential.headers():
            return {}
        if urllib.parse.urlsplit(url).scheme != "https":
            raise Refuse(f"{self.key}: the index named `{url}`, which is not https, and HTTP "
                         "Basic is the password in clear text; it is not sent there")
        if not any(inside(url, suffix) for suffix in self.credential_hosts):
            raise Refuse(f"{self.key}: the index named `{host_of(url) or url}`, which is not "
                         f"one of this source's hosts ({', '.join(self.credential_hosts)}), "
                         "so the credential is not sent there")
        return self.credential.headers()

    def credit(self, fetched: str) -> str:
        """The attribution a published map must carry, for a source fetched on that day.

        Most agencies ask for a fixed sentence. One asks for the month of the delivery in
        it, which is what `fetched` is for and what a row overrides this to fill.
        """

        return self.attribution

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
