"""Denmark: DHM/Terræn, the 0.4 m national terrain model, over a WCS behind a token.

The capabilities answer anybody and list both coverages; `DescribeCoverage` and
`GetCoverage` answer `HTTP 403 — User not authorized` without a key. So the adapter is an
ordinary WCS 1.0.0 row plus one query parameter, and the box comes from the capabilities
document's own `lonLatEnvelope`.

The agency's own example URL names the format `GTiff`, which is this MapServer's word for
a GeoTIFF, and states the grid in EPSG:25832.
"""

from .base import Credential
from .protocols import Wcs10Source

DK = Wcs10Source(
    "dk", "Denmark", "DHM/Terræn 0.4 m", 0.4,
    "Danish free geographic data, attribution required", "© Klimadatastyrelsen",
    "DVR90", (8.008, 54.435, 15.598, 57.769),
    credential=Credential("dk", "token"),
    url="https://api.dataforsyningen.dk/dhm_wcs_DAF",
    coverage="dhm_terraen",
    epsg=25832,
    image_format="GTiff",
    steps=(
        "Open https://dataforsyningen.dk and select `Opret bruger` to make a free account.\n"
        "The account is the same one the whole Dataforsyningen catalogue uses.",

        "Sign in, open `Min side`, and select `Token`.\n"
        "Create a token for the DHM services and copy it.",

        "Set the token in this shell so the adapter can fetch:\n"
        "    export OBC_REFERENCE_DK_TOKEN=<the token>\n"
        "Then stop here: `ingest dk --bbox ... --archive ...` fetches by itself and needs\n"
        "no download by hand. Go on only if you would rather download the tiles.",

        "To download by hand instead, open https://dataforsyningen.dk/data/930, select\n"
        "`Download`, draw the area, and take `DHM/Terræn (0,4 m grid)` as GeoTIFF.",

        "Unpack the delivery into one directory. The tiles are GeoTIFF in EPSG:25832 with\n"
        "−9999 for no data; leave the file names as they are.",
    ),
)
