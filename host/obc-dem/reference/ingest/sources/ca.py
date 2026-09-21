"""Canada: HRDEM, over NRCan's key-free elevation WCS.

The same rasters are in the open `canelevation-dem` S3 bucket, but the seamless mosaic is
one 500 km square per file and the largest is 38 GB, so a box is read through the service
rather than downloaded. The bucket is still the place to look for what exists: its
`mosaic_tile_index.geojson` names every square, and `hrdem-lidar/<project>-extent.geojson`
names every survey.

The server advertises WCS 2.0.1 and answers 1.1.1, and its coverage ids are `dtm` and
`dsm`. `dtm` is the bare-earth model, which is what a reference must be.
"""

from .protocols import Wcs11Source

# The product specification states it plainly: "Elevations are orthometric and expressed
# in reference to the Canadian Geodetic Vertical Datum of 2013 (CGVD2013) (EPSG:6647)."
# The northern 2 m source is ArcticDEM, which is ellipsoidal, but NRCan converts it to
# CGVD2013 before publication, so nothing ellipsoidal reaches the archive.
CA = Wcs11Source(
    "ca", "Canada", "HRDEM DTM 1 m", 1.0,
    "Open Government Licence – Canada 2.0",
    "Contains information licensed under the Open Government Licence – Canada",
    "CGVD2013", (-141.0, 41.6, -52.6, 83.2),
    url="https://datacube.services.geo.ca/ows/elevation",
    coverage="dtm",
    epsg=3979,
)
