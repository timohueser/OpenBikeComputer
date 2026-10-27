"""Finland: Korkeusmalli 2 m, the NLS elevation model, over a WCS behind an API key.

The service answers `HTTP 401` to everything without `api-key`, including the
capabilities, so the coverage id, the axes and the grid come from the agency's technical
description rather than from a probe. It states WCS 2.0.1, the coverage `korkeusmalli_2m`,
the subsets named `E` and `N`, and ETRS-TM35FIN (EPSG:3067). The key comes from a free
account at https://omatili.maanmittauslaitos.fi, under `API-avaimet`.

The row takes the native 2 m step (`scale=False`): `scalesize` is not in what the agency
documents, and asking a server for fewer pixels than the box holds is how a reference
comes back quietly coarser. `request_boxes` already keeps a box inside the pixel cap at
the native step.
"""

from .base import Credential
from .protocols import Wcs20Source

FI = Wcs20Source(
    "fi", "Finland", "Korkeusmalli 2 m (NLS)", 2.0,
    "CC BY 4.0", "N2000",
    credential=Credential("fi", "api-key"),
    url="https://avoin-karttakuva.maanmittauslaitos.fi/ortokuvat-ja-korkeusmallit/wcs/v2",
    coverage="korkeusmalli_2m",
    epsg=3067,
    axes=("E", "N"),
    scale=False,
)
