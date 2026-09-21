"""Finland: Korkeusmalli 2 m, the NLS elevation model, over a WCS behind an API key.

The service answers `HTTP 401` to everything without `api-key`, including the
capabilities, so the coverage id, the axes and the grid come from the agency's technical
description rather than from a probe. It states WCS 2.0.1, the coverage `korkeusmalli_2m`,
the subsets named `E` and `N`, and ETRS-TM35FIN (EPSG:3067).

The row takes the native 2 m step (`scale=False`): `scalesize` is not in what the agency
documents, and asking a server for fewer pixels than the box holds is how a reference
comes back quietly coarser. `request_boxes` already keeps a box inside the pixel cap at
the native step.
"""

from .base import Credential
from .protocols import Wcs20Source

FI = Wcs20Source(
    "fi", "Finland", "Korkeusmalli 2 m (NLS)", 2.0,
    "CC BY 4.0", "© Maanmittauslaitos", "N2000", (19.0, 59.7, 31.6, 70.1),
    credential=Credential("fi", "api-key"),
    url="https://avoin-karttakuva.maanmittauslaitos.fi/ortokuvat-ja-korkeusmallit/wcs/v2",
    coverage="korkeusmalli_2m",
    epsg=3067,
    axes=("E", "N"),
    scale=False,
    steps=(
        "Open https://omatili.maanmittauslaitos.fi and select `Rekisteröidy` to make a\n"
        "free NLS account (`Register` in the English version).",

        "Sign in, open `API-avaimet` / `API keys`, and create a key for the open data\n"
        "interfaces. Copy the key, which looks like a UUID.",

        "Set the key in this shell so the adapter can fetch:\n"
        "    export OBC_REFERENCE_FI_TOKEN=<the API key>\n"
        "Then stop here: `ingest fi --bbox ... --archive ...` fetches by itself. Go on\n"
        "only if you would rather download the tiles.",

        "To download by hand instead, open\n"
        "https://asiointi.maanmittauslaitos.fi/karttapaikka/tiedostopalvelu/korkeusmalli,\n"
        "select `Korkeusmalli 2 m`, pick the map sheets over the area, and order them as\n"
        "GeoTIFF. The service sends a download link by e-mail.",

        "Unpack the delivery into one directory. The tiles are GeoTIFF in EPSG:3067,\n"
        "float32 metres with −9999 for no data, one 3 km square per file.",
    ),
)
