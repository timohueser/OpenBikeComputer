"""Norway: the national elevation model (Nasjonal høydemodell), over Kartverket's WCS 1.0.0."""

from .base import registry_credit
from .protocols import Wcs10Source

NO = Wcs10Source(
    "no", "Norway", "NHM DTM 1 m", 1.0,
    "CC BY 4.0", registry_credit("dtm-no"), "NN2000", (4.0, 57.8, 31.5, 71.5),
    url="https://wcs.geonorge.no/skwms1/wcs.hoyde-dtm-nhm-25833",
    coverage="nhm_dtm_topo_25833",
    epsg=25833,
)
