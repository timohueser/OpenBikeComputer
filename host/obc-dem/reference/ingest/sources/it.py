"""Italy: INGV's national TINITALY, and the alpine provinces, which publish region by region.

TINITALY is the base: 10 m over the whole country, on INGV's keyless WCS. It is coarser than
the lattice, so its centres reach only some lattice pixels and `fill_gaps` fills the others.
Every finer model ranks above it.

Alpine Italy is the border-ridge gap. A ridge along the Swiss, Austrian or French border
is lifted on one side only until Italy is covered finely, and that step shows in the contours and
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
    "CC0 1.0",
    "Italian levelling network (m s.l.m.), geoid model not named by the province",
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
    "CC BY 4.0",
    "Italian levelling network (m s.l.m.), orthometric",
    grid_epsg=25832,
    base="https://siatservices.provincia.tn.it/stemdata/2014_lidar_dtm_asc/",
)

# TINITALY was built mostly from the contours and spot heights of the regional topographic
# maps, which are on the national levelling network. The WCS answers 10 m at its own step; the land
# abroad and the sea are its declared nodata.
TINITALY = Wcs20Source(
    "it", "Italy", "TINITALY 1.1 10 m (INGV)", 10.0,
    "CC BY 4.0",
    "Italian levelling network (m s.l.m.), from the regional topographic maps",
    url="https://tinitaly.pi.ingv.it/TINItaly_1_1/wcs",
    coverage="TINItaly_1_1__tinitaly_dem",
    epsg=32632,
    axes=("E", "N"),
    scale=False,
)

IT = (BZ, TN, TINITALY)
