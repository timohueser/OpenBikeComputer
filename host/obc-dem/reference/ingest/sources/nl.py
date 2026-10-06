"""The Netherlands: AHN, the national height model, over PDOK's WCS 2.0.1."""

from .protocols import Wcs20Source

NL = Wcs20Source(
    "nl", "Netherlands", "AHN DTM 0.5 m", 0.5,
    "CC BY 4.0", "NAP",
    url="https://service.pdok.nl/rws/ahn/wcs/v1_0",
    coverage="dtm_05m",
    epsg=28992,
    axes=("x", "y"),
)
