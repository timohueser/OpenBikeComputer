"""Portugal: the LiDAR terrain models of the DGT data centre, mainland 2 m and the Azores 1 m.

The data centre (CDD) indexes both with one STAC API. Its search answers anybody, but only as a
POST: a GET is sent to the login. An item's `bbox` is on the national grid and not in degrees,
so nothing here reads it; the search filters on the item's `geometry`. An asset's `href` names a
digest and no file, so a download is named after its item.

A download needs a free account. The CDD has no key for programs: it signs a user in with a
Keycloak login form and then reads a session cookie. So `fetch` signs in once, the way a browser
does, and every download carries the cookie. The password goes only to the DGT login form, and a
download that answers the login page instead of a GeoTIFF is refused and not kept.

The mainland heights are on Cascais. The Azores model is the regional government's; its metadata
names no height system, and each island has its own tide gauge (EPSG:6180–6187).

Madeira is DROTe's model of 2018/19, matched from aerial images, keyless: one zip that holds a
GeoTIFF per island group. The row takes the 5 m zip (114 MB) and not the 2 m one (460 MB): the 2 m GeoTIFF is
64 959 × 171 894 float32, about 45 GB unpacked, which the tail cannot hold whole, and every tile
of a prepare downloads the zip again. The 5 m step is shorter than the east side of a lattice
pixel there (about 6 m), so each lattice pixel still gets a source centre.
"""

import html
import http.cookiejar
import re
import urllib.parse
import urllib.request
from pathlib import Path

from ..lattice import Refuse
from .base import HTTP_TIMEOUT, Credential, host_of, inside, with_retry
from .bulk import BulkSource
from .stac import StacSearchSource, stac_items

LOGIN = "https://cdd.dgterritorio.gov.pt/auth/login"
DOMAIN = "dgterritorio.gov.pt"
SESSION = "connect.sid"
# Every Keycloak login form posts to its `login-actions/authenticate` endpoint.
FORM = re.compile(r'action="([^"]*/login-actions/authenticate[^"]*)"')
TIFF = (b"II*\x00", b"MM\x00*", b"II+\x00", b"MM\x00+")


def login_action(page: str, what: str) -> str:
    """Where the login page posts the password, which must be the DGT's own https host."""

    match = FORM.search(page)
    if match is None:
        raise Refuse(f"{what}: the login page holds no Keycloak login form")
    action = html.unescape(match.group(1))
    if urllib.parse.urlsplit(action).scheme != "https" or not inside(action, DOMAIN):
        raise Refuse(f"{what}: the login form posts to `{host_of(action) or action}`, which is "
                     f"not https on {DOMAIN}, so the password is not sent there")
    return action


class CddSearch(StacSearchSource):
    """The CDD's STAC search, and downloads signed in with its session cookie."""

    def __init__(self, *args, **kw):
        super().__init__(*args, **kw)
        self.session = None

    def files(self, bbox):
        body = {"bbox": list(bbox), "collections": [self.collections], "limit": self.page}
        wanted = []
        for feature in stac_items(self.search, body):
            asset = feature.get("assets", {}).get(self.asset)
            if asset is None:
                raise Refuse(f"{self.key}: item {feature.get('id')} has no `{self.asset}` asset")
            wanted.append((f"{feature['id']}.tif", asset["href"]))
        return sorted(set(wanted))

    def sign_in(self) -> str:
        """The session cookie of a CDD login with the row's user and password."""

        what = f"{self.key} login"
        jar = http.cookiejar.CookieJar()
        opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))

        def page():
            with opener.open(LOGIN, timeout=HTTP_TIMEOUT) as response:
                return response.read().decode("utf-8", "replace")

        action = login_action(with_retry(page, what), what)
        user, password = self.credential.values()
        form = urllib.parse.urlencode({"username": user, "password": password,
                                       "credentialId": ""}).encode()

        def post():
            with opener.open(urllib.request.Request(action, data=form), timeout=HTTP_TIMEOUT) as response:
                return response.geturl()

        landed = with_retry(post, what)
        session = next((cookie.value for cookie in jar if cookie.name == SESSION), None)
        # A refused login answers its own form again, on the login host.
        if host_of(landed) != host_of(LOGIN) or session is None:
            raise Refuse(f"{what}: the CDD did not sign the user in; check "
                         f"{self.credential.names}")
        return f"{SESSION}={session}"

    def headers_for(self, url: str) -> dict[str, str]:
        super().headers_for(url)  # the https and host checks; the Basic header is not sent
        return {"Cookie": self.session}

    def fetch(self, bbox, workdir) -> list[Path]:
        self.session = self.sign_in()
        rasters = super().fetch(bbox, workdir)
        for path in rasters:
            with path.open("rb") as handle:
                magic = handle.read(4)
            if magic not in TIFF:
                path.unlink()
                raise Refuse(f"{self.key} {path.name}: the download is not a GeoTIFF; the CDD "
                             "answers its login page when the session has ended")
        return rasters


SEARCH = "https://cdd.dgterritorio.gov.pt/dgt-be/v1/search"

PT = CddSearch(
    "pt", "Portugal (mainland)", "LiDAR MDT 2 m (DGT)", 2.0,
    "CC BY 4.0",
    "Cascais (Cascais height, EPSG:5780)",
    credential=Credential("pt"),
    credential_hosts=(DOMAIN,),
    search=SEARCH,
    asset="data",
    collections="MDT-2m",
)

AZ = CddSearch(
    "pt-20", "Portugal (Azores)", "LiDAR MDE 1 m (Governo dos Açores)", 1.0,
    "CC BY 4.0",
    "Azores island heights, one tide gauge per island (EPSG:6180–6187)",
    credential=Credential("pt"),
    credential_hosts=(DOMAIN,),
    search=SEARCH,
    asset="data",
    collections="ACORES-MDE-1",
)


class OneZip(BulkSource):
    """A region's rasters in one published zip."""

    def __init__(self, *args, url, **kw):
        super().__init__(*args, **kw)
        self.url = url

    def files(self, bbox):
        return [(Path(self.url).name, self.url)]


# The metadata states no height system; the heights match the summits on the tide gauges of
# Funchal (Cais da Pontinha) and Porto Santo (Cais da Vila).
MA = OneZip(
    "pt-30", "Portugal (Madeira)", "MDT 5 m 2018/19 (DROTe, Região Autónoma da Madeira)", 5.0,
    "no conditions apply",
    "Madeira island heights, Cais da Pontinha and Cais da Vila (EPSG:6178, EPSG:6179)",
    url="https://irig-storage.madeira.gov.pt/irig-catalogo/mdt_5m.zip",
)
