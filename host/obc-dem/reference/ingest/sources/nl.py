"""The Netherlands: AHN, the national height model, over PDOK's WCS 2.0.1."""

from .protocols import Wcs20Source

NL = Wcs20Source(
    "nl", "Netherlands", "AHN DTM 0.5 m", 0.5,
    "CC BY 4.0", "© Rijkswaterstaat / AHN", "NAP", (3.2, 50.7, 7.3, 53.6),
    url="https://service.pdok.nl/rws/ahn/wcs/v1_0",
    coverage="dtm_05m",
    epsg=28992,
    axes=("x", "y"),
)
