"""England: the Environment Agency's LIDAR Composite DTM, over Defra's WCS 2.0.1.

The ArcGIS ImageServer this product used to answer on was withdrawn at the end of 2024,
and the WCS replaced it with an endpoint that no longer carries the survey year, so the
coverage id is stable. The id is a UUID and a name, exactly as the capabilities spell it.

England only: Scotland and Wales publish their LiDAR separately.

The year in the attribution is the composite release the endpoint serves, and the agency
asks for it verbatim. The endpoint itself no longer carries a year, so the year here has
to be checked against the service's metadata when the composite is re-released.
"""

from .protocols import Wcs20Source

UK = Wcs20Source(
    "uk", "United Kingdom (England)", "EA LIDAR Composite DTM 1 m", 1.0,
    "Open Government Licence v3",
    "© Environment Agency copyright and/or database right 2022. All rights reserved.",
    "Ordnance Datum Newlyn", (-6.5, 49.8, 2.0, 55.9),
    url="https://environment.data.gov.uk/spatialdata/lidar-composite-digital-terrain-model-dtm-1m/wcs",
    coverage="13787b9a-26a4-4775-8523-806d13af58fc__Lidar_Composite_Elevation_DTM_1m",
    epsg=27700,
    axes=("E", "N"),
    scale=False,
)
