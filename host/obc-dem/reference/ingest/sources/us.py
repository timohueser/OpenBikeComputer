"""United States: 3DEP, served by USGS as an ArcGIS ImageServer."""

from .protocols import ArcGisSource

# 3DEP publishes orthometric heights on NAVD88, through the GEOID12B/GEOID18 model.
US = ArcGisSource(
    "us", "United States", "3DEP 1 m", 1.0,
    "Public domain", "USGS 3DEP", "NAVD88", (-179.0, 17.0, -65.0, 72.0),
    url="https://elevation.nationalmap.gov/arcgis/rest/services/3DEPElevation/ImageServer/exportImage",
)
