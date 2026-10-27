"""Poland: GUGiK's NMT, the national 1 m ALS terrain model, on a keyless WCS 2.0.1.

GUGiK states that the NMT is free of charge and free for any use. The service declares no
nodata and answers 0 for the land abroad, so 0 is the row's void. A pixel at sea level is lost
with it, which costs little: a lattice pixel pools many source pixels.

EPSG:2180 calls the northing `x`, but this service's `x` is the easting, and the subsets
follow the service.
"""

from .protocols import Wcs20Source

PL = Wcs20Source(
    "pl", "Poland", "NMT 1 m (GUGiK)", 1.0,
    "free for any use (GUGiK)",
    "PL-KRON86-NH (Kronstadt 86 normal heights)",
    url="https://mapy.geoportal.gov.pl/wss/service/PZGIK/NMT/GRID1/WCS/DigitalTerrainModelFormatTIFF",
    coverage="DTM_PL-KRON86-NH_TIFF",
    epsg=2180,
    axes=("x", "y"),
    scale=False,
    void=0.0,
)
