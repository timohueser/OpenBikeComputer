"""Denmark: DHM/Terræn, the 0.4 m national terrain model, over a WCS behind a token.

The capabilities answer anybody and list both coverages; `DescribeCoverage` and
`GetCoverage` answer `HTTP 403 — User not authorized` without a key. So the adapter is an
ordinary WCS 1.0.0 row plus one query parameter, and the box comes from the capabilities
document's own `lonLatEnvelope`.

The agency's own example URL names the format `GTiff`, which is this MapServer's word for
a GeoTIFF, and states the grid in EPSG:25832.

The credit the agency asks for names the month of the delivery, so the row's attribution
holds `{month}` and `{year}` and `credit` fills them from the day the ingest fetched.
"""

from .base import Credential
from .protocols import Wcs10Source

# The months as the Danish credit spells them, because the credit sentence is Danish.
MONTHS = ("januar", "februar", "marts", "april", "maj", "juni",
          "juli", "august", "september", "oktober", "november", "december")


class DhmWcs(Wcs10Source):
    """DHM/Terræn, whose credit names the month the data came from."""

    def credit(self, fetched: str) -> str:
        year, month, _ = fetched.split("-")
        return self.attribution.format(month=MONTHS[int(month) - 1], year=year)


DK = DhmWcs(
    "dk", "Denmark", "DHM/Terræn 0.4 m", 0.4,
    "CC BY 4.0", "Indeholder data fra Klimadatastyrelsen, Danmarks Højdemodel, {month} {year}",
    "DVR90", (8.008, 54.435, 15.598, 57.769),
    credential=Credential("dk", "token"),
    url="https://api.dataforsyningen.dk/dhm_wcs_DAF",
    coverage="dhm_terraen",
    epsg=25832,
    image_format="GTiff",
    steps=(
        "Open https://dataforsyningen.dk and select `Opret bruger` to make a free account.\n"
        "The account is the same one the whole Dataforsyningen catalogue uses.",

        "Sign in, select the user icon, and open\n"
        "`Administrer token til webservices og API'er`.",

        "Select `Opret ny token`. The token is account-wide: it is not per service, so one\n"
        "token answers for DHM and for everything else in the catalogue. Copy it.",

        "The wizard asks for that token in a moment and keeps it in this process only.\n"
        "Go on to the next step instead if you would rather download the tiles.",

        "To download by hand, open https://dataforsyningen.dk/data/930, select `Download`,\n"
        "draw the area, and take `DHM/Terræn (0,4 m grid)` as GeoTIFF.",

        "Unpack the delivery into one directory. The tiles are GeoTIFF in EPSG:25832 with\n"
        "−9999 for no data; leave the file names as they are.",
    ),
)
