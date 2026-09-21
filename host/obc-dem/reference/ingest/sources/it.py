"""Italy: the alpine provinces, which publish region by region.

Alpine Italy is the border-ridge gap. A ridge along the Swiss, Austrian or French border
is lifted on one side only until Italy is covered, and that step shows in the contours and
in a profile. South Tyrol is the province that closes most of it, and it publishes a plain
WCS with no key and no attribution obligation.

Piedmont, Lombardy and Aosta Valley are not here, and `README.md` records what each one
answered: Piedmont publishes 5 m with no documented vertical datum, Lombardy asks for an
email request, and Aosta Valley needs an Italian identity login.
"""

from .bulk import BulkSource
from .grid import grid_squares
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

# Trentino's 0.5 m DTM is published as plain files, keyless, one ESRI ASCII grid per square,
# so neither its WCS, which lists no coverage, nor its geoportal job API is needed. The square's
# name is arithmetic: the tile whose corner is (643 500, 5 112 000) in EPSG:25832 is
# `5h643551120_DTM.asc`, which is the corner in hundreds of metres. A square the survey did not
# cover answers 404.
class TrentoGrids(BulkSource):
    """Half-kilometre ESRI ASCII grids, named after the corner they start at."""

    skip_missing = True

    def __init__(self, *args, base, **kw):
        super().__init__(*args, **kw)
        self.base = base

    def files(self, bbox):
        return [(name, self.base + name)
                for name in (f"5h{east // 100:04d}{north // 100:05d}_DTM.asc"
                             for east, north in grid_squares(bbox, self.grid_epsg, 500))]


TN = TrentoGrids(
    "it-tn", "Italy, Trentino", "DTM 0.5 m (Provincia autonoma di Trento)", 0.5,
    "CC BY 4.0", "Provincia autonoma di Trento",
    "Italian levelling network (m s.l.m.), orthometric", (10.4, 45.6, 12.0, 46.6),
    grid_epsg=25832,
    base="https://siatservices.provincia.tn.it/stemdata/2014_lidar_dtm_asc/",
)

IT = (BZ, TN)
