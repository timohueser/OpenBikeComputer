"""New Zealand: the LINZ 1 m DEM, open on S3 with a static STAC catalogue.

It answers without a key, and the heights are normal-orthometric on NZVD2016, so it can be
ingested. There is no adapter yet for one reason: the tiles are named after the NZTopo50
map sheets, and the catalogue is a static collection of 424 links with no bbox in it and no
`/search` endpoint, so finding the sheet a box needs is a fourth kind of index. The row
records the facts; `--input` takes the sheets once they are on disk.

    https://nz-elevation.s3.ap-southeast-2.amazonaws.com/new-zealand/new-zealand/dem_1m/2193/

The tiles are LERC-compressed COGs. GDAL reads them; a minimal TIFF reader does not.
"""

from .base import ManualSource

NZ = ManualSource(
    "nz", "New Zealand", "LiDAR DEM 1 m (LINZ)", 1.0,
    "CC BY 4.0", "Sourced from LINZ, CC BY 4.0", "NZVD2016 (EPSG:7839), normal-orthometric",
    (166.3, -47.4, 178.9, -34.0),
    why="the open S3 tiles are named after NZTopo50 sheets and the static STAC carries no "
        "box, so the sheet a bbox needs cannot be worked out yet",
)
