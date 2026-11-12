"""Norway: the national elevation model (Nasjonal høydemodell), over Kartverket's WCS 1.0.0."""

from .protocols import Wcs10Source

NO = Wcs10Source(
    "no", "Norway", "NHM DTM 1 m", 1.0,
    "CC BY 4.0", "NN2000",
    url="https://wcs.geonorge.no/skwms1/wcs.hoyde-dtm-nhm-25833",
    coverage="nhm_dtm_topo_25833",
    epsg=25833,
    # 20 a minute, about twice the average of a Europe prepare: it smooths the bursts of several
    # blocks that Kartverket's gateway refused, and it states no limit of its own.
    gap=3.0,
)
