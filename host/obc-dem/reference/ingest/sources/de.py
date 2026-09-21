"""Germany: one row per federal state, because elevation is a state matter.

There is no national DGM service. Every state publishes its own 1 m model on its own
service, so `de-*` is a family of registry rows and not one adapter. Where the protocol is
the same the code is the same: most states answer WCS 2.0.1 on their own UTM zone.
"""

from .protocols import Wcs20Source

# DHHN2016 is the German height reference. A state that still publishes on DHHN92 is
# within a centimetre of it for this purpose, and the row says which one the state names.
NW = Wcs20Source(
    "de-nw", "Germany, North Rhine-Westphalia", "DGM1 1 m", 1.0,
    "dl-de/zero-2-0", "© Geobasis NRW", "DHHN2016", (5.8, 50.3, 9.5, 52.6),
    url="https://www.wcs.nrw.de/geobasis/wcs_nw_dgm",
    coverage="nw_dgm",
    epsg=25832,
    axes=("x", "y"),
)

DE = (NW,)
