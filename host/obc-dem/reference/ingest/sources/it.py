"""Italy: the alpine provinces, which publish region by region.

Alpine Italy is the border-ridge gap. A ridge along the Swiss, Austrian or French border
is lifted on one side only until Italy is covered, and that step shows in the contours and
in a profile. South Tyrol is the province that closes most of it, and it publishes a plain
WCS with no key and no attribution obligation.

Trentino, Piedmont, Lombardy and Aosta Valley are not here, and `README.md` records what
each one answered: Trentino's WCS lists no coverage and its data comes out of an
asynchronous merge service, Piedmont publishes 5 m with no documented vertical datum,
Lombardy asks for an email request, and Aosta Valley needs an Italian identity login.
"""

from .base import ManualSource
from .protocols import Wcs20Source

# The province publishes heights as "m s.l.m." — metres above sea level, orthometric — but
# names no geoid model. That is enough to register: the archive wants orthometric metres,
# and the decimetres a geoid model would move them cannot make or unmake a 10 m lift.
BZ = Wcs20Source(
    "it-bz", "Italy, South Tyrol", "DTM 2.5 m (Provincia autonoma di Bolzano)", 2.5,
    "CC0 1.0", "Autonome Provinz Bozen – Provincia autonoma di Bolzano",
    "Italian levelling network (m s.l.m.), geoid model not named by the province",
    (10.3, 46.2, 12.5, 47.1),
    url="https://geoservices9.civis.bz.it/geoserver/ows",
    coverage="p_bz-Elevation__DigitalTerrainModel-2.5m",
    epsg=25832,
    axes=("E", "N"),
    scale=False,
)

# Trentino publishes a 0.5 m DTM, and its WCS lists no coverage at all. The data comes out
# of an asynchronous merge service: a WFS index of 25 201 half-kilometre tiles, a POST that
# starts a job, a poll, and a zip of nested zips of ESRI ASCII grids. Three protocols for
# one province is not worth a code path yet; `README.md` has the steps for `--input`.
TN = ManualSource(
    "it-tn", "Italy, Trentino", "DTM 0.5 m (Provincia autonoma di Trento)", 0.5,
    "CC BY 4.0", "Provincia autonoma di Trento",
    "Italian levelling network (m s.l.m.), orthometric", (10.4, 45.6, 12.0, 46.6),
    why="the province has no working WCS and its tiles come from an asynchronous merge "
        "service (see README.md)",
)

IT = (BZ, TN)
