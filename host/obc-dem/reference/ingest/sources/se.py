"""Sweden: Markhöjdmodell, the 1 m national terrain model, indexed openly and served
behind a Geotorget account.

This is the one source here whose index and whose data sit on different sides of the
login. The STAC search at `api.lantmateriet.se/stac-hojd` answers anybody and names the
COG of every tile; the COG itself answers `HTTP 401` with
`WWW-Authenticate: Basic realm="Authorization Server"`. So the adapter is the STAC search
the registry already speaks, and the credential is a user and a password on the download.

A tile is 10 km square, 10 000 x 10 000 float32 at 1 m, which is a few hundred megabytes.
The COGs are downloaded whole, the way Lower Saxony's are, so keep the work directory on a
disk with room for the box.
"""

from .base import Credential
from .stac import StacSearchSource

SE = StacSearchSource(
    "se", "Sweden", "Markhöjdmodell 1 m (Lantmäteriet)", 1.0,
    "CC BY 4.0", "© Lantmäteriet", "RH2000 (the tiles are EPSG:5845, SWEREF99 TM + RH2000)",
    (9.08, 55.16, 25.54, 69.07),
    credential=Credential("se"),
    search="https://api.lantmateriet.se/stac-hojd/v1/search",
    asset="data",
    collections="dtm-cog",
    steps=(
        "Open https://geotorget.lantmateriet.se and select `Skapa konto` to make a free\n"
        "Geotorget account. A Swedish e-mail address is not needed.",

        "Sign in, open `Geodataprodukter`, find `Markhöjdmodell Nedladdning, grid 1+`,\n"
        "and order access to it. Access to the open products is granted at once.",

        "Open `Mina beställningar` / `Mina produkter` and create the consumer user for\n"
        "the download API. Copy the user name and the password it shows you once.",

        "Set both in this shell so the adapter can fetch:\n"
        "    export OBC_REFERENCE_SE_USER=<the consumer user>\n"
        "    export OBC_REFERENCE_SE_PASSWORD=<the password>\n"
        "Then stop here: `ingest se --bbox ... --archive ...` reads the open STAC index\n"
        "and downloads the tiles with that user. Go on only if you would rather download\n"
        "the tiles in the browser.",

        "To download by hand instead, open the same product in Geotorget and select the\n"
        "10 km tiles over the area. Each one is a GeoTIFF (COG) named `m<sheet>.tif`.",

        "Put the tiles in one directory. They are float32 metres in EPSG:5845\n"
        "(SWEREF 99 TM with RH 2000 heights) and −9999 for no data.",
    ),
)
