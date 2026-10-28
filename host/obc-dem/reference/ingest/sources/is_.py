"""Iceland: ÍslandsDEM v1.0, the national 10 m model, one file for the whole country.

ÍslandsDEM is ArcticDEM's 2 m strips co-registered to Copernicus GLO-30 and mosaicked. Its 2 m
tiles come only through a map viewer; the 10 m mosaic is one tiled GeoTIFF of 2.7 GB on a keyless
server that answers byte ranges, so the box's window is read out of it, as for Austria.

The module is `is_` because `is` is a Python keyword.
"""

from .cog import OneFile

IS = OneFile(
    "is", "Iceland", "ÍslandsDEM v1.0 10 m", 10.0,
    "CC BY 4.0",
    "ISH2004 (IceGeoid)",
    base="https://ftp.natt.is/gisdata/raster/IslandsDEMv1.0_10x10m_isn2016_zmasl.tif",
    epsg=8088,
)
