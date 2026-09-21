"""Austria: the BEV's national 1 m ALS terrain model.

BEV publishes the whole country as 50 km squares of the ETRS89-LAEA grid, one 6.5 GB COG
each, so the box's window is read over HTTP and nothing else is transferred. The date in
the path is the delivery the registry points at; a later delivery is a new date.

The per-state models are the same survey flights gathered per state, and they come in
eight horizontal CRSs, three height systems and one city datum with a 156.68 m offset.
One national grid in one CRS is the better row, so the registry carries that; `README.md`
lists the state products as documented alternatives for `--input`.
"""

from .cog import CogGrid

# BEV states it for the whole raster: "Grundsätzlich: EVRF2000 Austria, orthometrische
# Höhen (EPSG:9274)". The squares dated 2021-09-15 are the one exception and are on the
# older Adria-Triest practical heights, about half a metre away — far below a lift.
AT = CogGrid(
    "at", "Austria", "ALS DTM 1 m (BEV)", 1.0,
    "CC BY 4.0", "Bundesamt für Eich- und Vermessungswesen (BEV)",
    "EVRF2000 Austria, orthometric (EPSG:9274)", (9.5, 46.3, 17.2, 49.1),
    base="https://data.bev.gv.at/download/ALS/DTM/20250915/",
    name="ALS_DTM_CRS3035RES50000mN{north}E{east}.tif",
    epsg=3035,
    tile_m=50000,
)
