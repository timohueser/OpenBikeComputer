"""Italy: INGV's national TINITALY, and the alpine provinces, which publish region by region.

TINITALY is the base: 10 m over the whole country, on INGV's keyless WCS. It is coarser than
the lattice, so its centres reach only some lattice pixels and `fill_gaps` fills the others.
Every finer model ranks above it.

Alpine Italy is the border-ridge gap. A ridge along the Swiss, Austrian or French border
is lifted on one side only until Italy is covered finely, and that step shows in the contours and
in a profile. South Tyrol is the province that closes most of it, and it publishes a plain
WCS with no key and no attribution obligation.

Piedmont is the 5 m LiDAR model of the ICE flights of 2009–2011, on a keyless WCS 1.0.0.
Its WCS 2.0.1 endpoint refuses every subset as outside the coverage, so the row speaks 1.0.0.
Friuli Venezia Giulia is the 1 m LiDAR model of the regional civil protection, 2006–2010, one
zip per element of the 1:5000 regional map. An element code is the IGM sheet, the section and the
element, not arithmetic, so `it_fvg_elements.txt` is the index: one line per element with its box
in degrees. Its first line says where it comes from and how to make it again.

Lombardy asks for an email request, and Aosta Valley needs an Italian identity login.
"""

from functools import cache
from pathlib import Path

from .bulk import BulkSource
from .grid import grid_squares
from .protocols import Wcs10Source, Wcs20Source

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

# A box outside the coverage's rectangle on EPSG:32632 is answered with `NoApplicableCode`, which is
# retried as a fault of the server, so the row's `extent` stays inside that rectangle. The heights
# are on the Genova 1942 levelling datum, and the coverage declares its void, -99.
PIE = Wcs10Source(
    "it-21", "Italy, Piedmont", "DTM 5 m, ICE 2009–2011 (Regione Piemonte)", 5.0,
    "CC BY 4.0",
    "Italian levelling network, Genova 1942 (m s.l.m.)",
    url="https://geomap.reteunitaria.piemonte.it/ws/taims/rp-01/taimsdtmwcs/wcs_ice_2009_2011_dtm",
    coverage="DTM",
    epsg=32632,
    image_format="GEOTIFF_16",
    summit=("Monviso", 7.0904, 44.6675, 3841),
)

ELEMENTS = Path(__file__).with_name("it_fvg_elements.txt")


@cache
def elements() -> tuple[tuple[str, float, float, float, float], ...]:
    """Every element of the 1:5000 map: its code, then west, south, east and north in degrees."""

    lines = ELEMENTS.read_text(encoding="utf-8").splitlines()
    return tuple((code, *map(float, box)) for code, *box in
                 (line.split() for line in lines if not line.startswith("#")))


class FvgElements(BulkSource):
    """One zip of four ESRI ASCII quadrants per element of the 1:5000 map."""

    # An element of the map that the survey did not fly has no zip.
    skip_missing = True

    def __init__(self, *args, base, **kw):
        super().__init__(*args, **kw)
        self.base = base

    def files(self, bbox):
        west, south, east, north = bbox
        return [(name, self.base + name)
                for code, w, s, e, n in elements() if w < east and e > west and s < north and n > south
                for name in (f"DTM01_PCR_{code}_RDN2008_TM33.zip",)]


# The quadrants carry no `.prj`; they are on RDN2008 / TM33, and -9999.9 is their declared void.
# The record names no height system, so the summit check stands for it.
FVG = FvgElements(
    "it-36", "Italy, Friuli Venezia Giulia", "DTM 1 m, LiDAR 2006–2010 (Protezione Civile FVG)", 1.0,
    "IODL 2.0",
    None,
    grid_epsg=6708,
    base="https://irdat.regione.fvg.it/cartogdownload/DTM_PCR/1M/6708/",
    summit=("Monte Canin", 13.4404, 46.3593, 2587),
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

IT = (BZ, TN, PIE, FVG, TINITALY)
