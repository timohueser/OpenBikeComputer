"""Portugal: the LiDAR terrain models of the DGT data centre, mainland 2 m and the Azores 1 m.

The data centre (CDD) indexes both with one STAC API. Its search answers anybody, but only as a
POST: a GET is sent to the login. An item's `bbox` is on the national grid and not in degrees,
so nothing here reads it; the search filters on the item's `geometry`. An asset's `href` names a
digest and no file, so a download is named after its item.

A download needs a free account. The CDD has no key for programs: it signs a user in with a
Keycloak login form and then reads a session cookie. So a `fetch` whose search finds items signs
in once, the way a browser does, and each of its downloads carries the cookie; a fetch over the
sea signs in nowhere. The password goes only to the DGT login form, and a download that answers
the login page instead of a GeoTIFF is refused and not kept.

Neither model's metadata names its height system, so each row carries a summit that the prepare
checks before it uses the model.
"""

import html
import http.cookiejar
import re
import urllib.parse
import urllib.request
from pathlib import Path

from ..lattice import Refuse
from .base import HTTP_TIMEOUT, TIFF_MAGIC, Credential, host_of, inside, with_retry
from .stac import StacSearchSource, stac_items

LOGIN = "https://cdd.dgterritorio.gov.pt/auth/login"
DOMAIN = "dgterritorio.gov.pt"
SESSION = "connect.sid"
# Every Keycloak login form posts to its `login-actions/authenticate` endpoint.
FORM = re.compile(r'action="([^"]*/login-actions/authenticate[^"]*)"')


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


def session_opener():
    """An opener that keeps the cookies of a login, and the jar it keeps them in."""

    jar = http.cookiejar.CookieJar()
    return urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar)), jar


class CddSearch(StacSearchSource):
    """The CDD's STAC search, and downloads signed in with its session cookie."""

    def __init__(self, *args, **kw):
        super().__init__(*args, **kw)
        self.session = None

    def items(self, bbox):
        return stac_items(self.search, {"bbox": list(bbox), "collections": [self.collections],
                                        "limit": self.page})

    def name_of(self, feature, href: str) -> str:
        return f"{feature['id']}.tif"

    def sign_in(self) -> str:
        """The session cookie of a CDD login with the row's user and password."""

        what = f"{self.key} login"
        opener, jar = session_opener()

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
        wanted = self.files(bbox)
        if not wanted:
            return []
        self.session = self.sign_in()
        rasters = self.download(wanted, workdir)
        for path in rasters:
            with path.open("rb") as handle:
                magic = handle.read(4)
            if magic not in TIFF_MAGIC:
                path.unlink()
                raise Refuse(f"{self.key} {path.name}: the download is not a GeoTIFF; the CDD "
                             "answers its login page when the session has ended")
        return rasters


SEARCH = "https://cdd.dgterritorio.gov.pt/dgt-be/v1/search"
UNVERIFIED = "orthometric, unverified until the summit check passes"

PT = CddSearch(
    "pt", "Portugal (mainland)", "LiDAR MDT 2 m (DGT)", 2.0,
    "CC BY 4.0",
    UNVERIFIED,
    credential=Credential("pt"),
    credential_hosts=(DOMAIN,),
    search=SEARCH,
    asset="data",
    collections="MDT-2m",
    summit=(-7.6114, 40.3214, 1993),  # Torre, Serra da Estrela
)

# Each island has its own tide gauge (EPSG:6180–6187).
AZ = CddSearch(
    "pt-20", "Portugal (Azores)", "LiDAR MDE 1 m (Governo dos Açores)", 1.0,
    "CC BY 4.0",
    UNVERIFIED,
    credential=Credential("pt"),
    credential_hosts=(DOMAIN,),
    search=SEARCH,
    asset="data",
    collections="ACORES-MDE-1",
    summit=(-28.3989, 38.4686, 2351),  # Pico
)
