"""Denmark: DHM/Terræn, the 0.4 m national terrain model, over a WCS behind a token.

The capabilities answer anybody and list both coverages; `DescribeCoverage` and
`GetCoverage` answer `HTTP 403 — User not authorized` without a key. So the adapter is an
ordinary WCS 1.0.0 row plus one query parameter, and the box comes from the capabilities
document's own `lonLatEnvelope`.

The agency's own example URL names the format `GTiff`, which is this MapServer's word for
a GeoTIFF, and states the grid in EPSG:25832. The token is account-wide: a free account at
https://dataforsyningen.dk makes one under `Administrer token til webservices og API'er`.

The credit the agency asks for names the month of the delivery, so the row's attribution
holds `{month}` and `{year}` and `fill` fills them from the day of the fetch.
"""

from .base import Credential
from .protocols import Wcs10Source

# The months as the Danish credit spells them, because the credit sentence is Danish.
MONTHS = ("januar", "februar", "marts", "april", "maj", "juni",
          "juli", "august", "september", "oktober", "november", "december")


class DhmWcs(Wcs10Source):
    """DHM/Terræn, whose credit names the month the data came from."""

    def fill(self, attribution: str, fetched: str) -> str:
        year, month, _ = fetched.split("-")
        return attribution.format(month=MONTHS[int(month) - 1], year=year)


DK = DhmWcs(
    "dk", "Denmark", "DHM/Terræn 0.4 m", 0.4,
    "CC BY 4.0",
    "DVR90",
    credential=Credential("dk", "token"),
    url="https://api.dataforsyningen.dk/dhm_wcs_DAF",
    coverage="dhm_terraen",
    epsg=25832,
    image_format="GTiff",
)
