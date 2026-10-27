"""Australia: ELVIS, the national elevation and depth portal.

ELVIS has no interface a program can ask a box of. Every path under
`elevation.fsdf.org.au` answers the same single-page application, the order is a form, and
the delivery is a link sent by e-mail with a 15 GB cap per request. So this row has no
adapter at all and never will have one: the owner orders the finest DEM over the area as
GeoTIFF, and `OBC_REFERENCE_AU_INPUT` names the directory of the zip.

The zip holds one `*_DEM.tif` per dataset the order covered, and an order taken as ESRI
ASCII holds a `.prj` beside each grid. Both are read as delivered — ELVIS publishes each
state's survey in that state's MGA zone, so no single grid can be stated for the row and
the file's own CRS is what places it.

Some ELVIS datasets are ellipsoidal, which stands tens of metres from an orthometric
height and is the size of a lift. The row cannot tell which an order held, so it does not
guess: `confirm_datum` makes the fetch refuse until the owner has read the order's
metadata and set `OBC_REFERENCE_AU_DATUM=AHD`.

The row states no `resolution_m`. The step of an order is the step of whichever survey it
covered, 1 m to 5 m, so claiming one number would be a claim about data nobody has seen.
"""

from .base import ManualSource

AU = ManualSource(
    "au", "Australia", "ELVIS DEM 1–5 m, per order", None,
    "CC BY 4.0 (the licensor is the contributing agency named in the order)",
    "AHD (Australian Height Datum)",
    confirm_datum="AHD",
    why="ELVIS answers no box: the order is a web form and the delivery is a link sent "
        "by e-mail",
)
