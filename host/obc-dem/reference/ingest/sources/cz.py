"""Czechia: ČÚZK's DMR 5G, the national LiDAR terrain model, on a keyless ArcGIS ImageServer.

The service resamples DMR 5G's TIN to 2 m in the CRS it is asked for, as 3DEP does.
"""

from .protocols import ArcGisSource

CZ = ArcGisSource(
    "cz", "Czechia", "DMR 5G 2 m (ČÚZK)", 2.0,
    "CC BY 4.0",
    "Bpv (Baltic Vertical Datum – After Adjustment), normal heights",
    url="https://ags.cuzk.gov.cz/arcgis2/rest/services/dmr5g/ImageServer/exportImage",
    compress=True,
    nodata=-32768,
)
