"""Sweden: Markhöjdmodell, the 1 m national terrain model, indexed openly and served
behind a Geotorget account.

This is the one source here whose index and whose data sit on different sides of the
login. The STAC search at `api.lantmateriet.se/stac-hojd` answers anybody and names the
COG of every tile; the COG itself, on `dl1.lantmateriet.se`, answers `HTTP 401` with
`WWW-Authenticate: Basic realm="Authorization Server"`. So the adapter is the STAC search
the registry already speaks, and the credential is a user and a password on the download:
the consumer of `Markhöjdmodell Nedladdning, grid 1+` at https://geotorget.lantmateriet.se,
with Basic authentication.

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
    "CC BY 4.0", "RH2000 (the tiles are EPSG:5845, SWEREF99 TM + RH2000)",
    credential=Credential("se"),
    credential_hosts=("lantmateriet.se",),
    search="https://api.lantmateriet.se/stac-hojd/v1/search",
    asset="data",
    collections="dtm-cog",
)
