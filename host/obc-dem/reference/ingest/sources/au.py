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

Some ELVIS datasets are ellipsoidal. The ones this row names are on AHD; an order that
says otherwise must be converted before it reaches the tail, which the archive contract
states and no code here does.
"""

from .base import ManualSource

AU = ManualSource(
    "au", "Australia", "ELVIS DEM 1–5 m", 1.0,
    "CC BY 4.0 (the licensor is the contributing agency named in the order)",
    "Sourced from ELVIS – Elevation and Depth, © the contributing agency",
    "AHD (Australian Height Datum)", (112.0, -44.0, 154.0, -9.0),
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
        "`ingest au --input <dir>` opens a zip by itself and takes the `*_DEM.tif` out.",

        "Check the order's metadata for the vertical datum. This row is AHD; stop and ask\n"
        "the owner if the delivery says an ellipsoidal height, because the archive is\n"
        "orthometric metres and nothing here converts one.",
    ),
)
