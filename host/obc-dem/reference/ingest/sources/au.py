"""Australia: ELVIS, the national elevation and depth portal.

ELVIS has no interface a program can ask a box of. Every path under
`elevation.fsdf.org.au` answers the same single-page application, the order is a form, and
the delivery is a link sent by e-mail with a 15 GB cap per request. So this row has no
adapter at all and never will have one: `wizard au` walks the order and `--input` takes
the zip.

The zip holds one `*_DEM.tif` per dataset the order covered, and an order taken as ESRI
ASCII holds a `.prj` beside each grid. Both are read as delivered — ELVIS publishes each
state's survey in that state's MGA zone, so no single grid can be stated for the row and
the file's own CRS is what places it.

Some ELVIS datasets are ellipsoidal, which stands tens of metres from an orthometric
height and is the size of a lift. The row cannot tell which an order held, so it does not
guess: `confirm_datum` makes the ingest refuse until the owner has read the order's
metadata and said `--datum AHD`, and the wizard asks the question outright.

The row states no `resolution_m`. The step of an order is the step of whichever survey it
covered, 1 m to 5 m, so claiming one number would be a claim about data nobody has seen;
the run prints the step each delivered raster actually has instead.
"""

from .base import ManualSource

AU = ManualSource(
    "au", "Australia", "ELVIS DEM 1–5 m, per order", None,
    "CC BY 4.0 (the licensor is the contributing agency named in the order)",
    "Sourced from ELVIS – Elevation and Depth, © the contributing agency",
    "AHD (Australian Height Datum)", (112.0, -44.0, 154.0, -9.0),
    confirm_datum="AHD",
    why="ELVIS answers no box: the order is a web form and the delivery is a link sent "
        "by e-mail",
    steps=(
        "Open https://elevation.fsdf.org.au and find the area on the map.",

        "Select `Menu` → `Order Data`, then draw or upload the area. Keep it small: one\n"
        "request is capped at 15 GB and a 1 m DEM is about 4 MB per square kilometre.",

        "In the product list, select the finest `Digital Elevation Model` over the area —\n"
        "1 m where a LiDAR survey covers it, else 2 m or 5 m — and take GeoTIFF.",

        "Give an e-mail address and send the order. ELVIS answers with a link, usually\n"
        "in minutes and sometimes in hours.",

        "Download the zip from the link and put it in one directory, unpacked or not:\n"
        "`ingest au --input <dir>` opens a zip by itself and takes the `*_DEM.tif` out.\n"
        "A zip inside the zip has to be unpacked by hand; the tool refuses one and says so.",

        "Open the order's metadata and read the vertical datum. The next question is\n"
        "about that, and answering it wrongly puts heights tens of metres out.",
    ),
)
