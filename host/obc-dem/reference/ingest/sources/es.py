"""Spain: MDT05, IGN's 5 m model from the PNOA LiDAR flights, over an INSPIRE WCS 2.0.1.

The coverage is geographic (ETRS89), so its subsets carry the `long` and `lat` axis labels
the capabilities document names, and not `x` and `y`.
"""

from .protocols import Wcs20Source

# IGN publishes MDT05 as orthometric heights on REDNAP, the Spanish levelling network,
# whose origin is the mean sea level at Alicante. IGN documents REDNAP as connected to the
# European levelling network, so the datum is EVRS-aligned without being EVRF2000 itself.
ES = Wcs20Source(
    "es", "Spain", "MDT05 / PNOA LiDAR 5 m", 5.0,
    "CC BY 4.0", "© Instituto Geográfico Nacional",
    "REDNAP (Alicante mean sea level), EVRS-aligned", (-18.2, 27.6, 4.4, 43.9),
    url="https://servicios.idee.es/wcs-inspire/mdt",
    coverage="Elevacion4258_5",
    epsg=4326,
    axes=("long", "lat"),
)
