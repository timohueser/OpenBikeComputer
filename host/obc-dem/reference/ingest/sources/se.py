"""Sweden: Markhöjdmodell, the 1 m national terrain model, indexed openly and served
behind a Geotorget account.

This is the one source here whose index and whose data sit on different sides of the
login. The STAC search at `api.lantmateriet.se/stac-hojd` answers anybody and names the
COG of every tile; the COG itself, on `dl1.lantmateriet.se`, answers `HTTP 401` with
`WWW-Authenticate: Basic realm="Authorization Server"`. So the adapter is the STAC search
the registry already speaks, and the credential is a user and a password on the download.

Because the index names where the download is, `credential_hosts` names where the password
may go: an index is data, and a `href` that pointed anywhere else would otherwise be
handed it. The index and the data are on two hosts under one domain, so the suffix is that
domain.

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
    credential_hosts=("lantmateriet.se",),
    search="https://api.lantmateriet.se/stac-hojd/v1/search",
    asset="data",
    collections="dtm-cog",
    steps=(
        "Open https://geotorget.lantmateriet.se and select `Logga in`, then create a\n"
        "private account.",

        "Open `Geodataprodukter` and find `Markhöjdmodell Nedladdning, grid 1+`.",

        "Accept the terms of the national geodata platform (NGP) for the product.",

        "Under `Bli konsument`, select `Skicka in ansökan` to apply for access, and\n"
        "choose Basic authentication rather than OAuth. Lantmäteriet answers the\n"
        "application, and then the consumer user name and its password are yours to copy.",

        "The wizard asks for that user name and password in a moment and keeps them in\n"
        "this process only. Go on to the next step instead if you would rather download\n"
        "the tiles in the browser.",

        "To download by hand, open the same product in Geotorget and select the 10 km\n"
        "tiles over the area. Each one is a GeoTIFF (COG) named `m<sheet>.tif`.",

        "Put the tiles in one directory. They are float32 metres in EPSG:5845\n"
        "(SWEREF 99 TM with RH 2000 heights) and −9999 for no data.",
    ),
)
