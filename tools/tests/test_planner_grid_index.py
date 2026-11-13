"""The planner release document names the step indexes instead of listing their files."""

import hashlib
import json
from pathlib import Path
import tempfile
import unittest

from tools import planner_grid as grid, planner_grid_index as grid_index, planner_grid_offline as grid_offline

OPTIONS = {"region": "test", "name": "Test", "bounds": [7.8, 48.0, 9.0, 48.4], "attribution": "OSM",
           "landcover_attribution": "Daylight"}
ARCHIVES = {"9-267-177": "a" * 64, "9-268-177": "b" * 64}


def entry(name):
    sha256 = hashlib.sha256(name.encode()).hexdigest()
    return {"bytes": 1, "sha256": sha256, "transport": {"bytes": 1, "sha256": sha256, "encoding": "identity"}}


def write_indexes(root):
    """The index of each grid step of a release of two cells, at `root`, as the layers of a step request."""
    bounds, cells = OPTIONS["bounds"], dict(grid.cells(OPTIONS["bounds"]))
    files = lambda *names: {name: entry(name) for name in names}
    tiles = lambda kind, *cells: [f"maps/tiles/{kind}/{cell}.pmtiles" for cell in cells]
    routing = files("routing/blocks.json", *(f"routing/packs/{key}/{name}" for key in ARCHIVES.values()
                                             for name in ("pages.bin", "pages.idx")),
                    *(f"{folder}/{cell}.json" for cell in cells for folder in ("offline/routing-cells", "routes/tiles")))
    package = files("routing/package.json", "routing/directory.bin", "routing/objects/" + "d" * 64)
    indexes = {
        "routing": {"files": routing, "graph": {"source": "s" * 64, "data": {"bounds": bounds, "region": "test",
            "source_sha256": ["o" * 64, "t" * 64], "metrics": {"road": {}}}}},
        "basemap": {"files": files("maps/basemap.json", *tiles("basemap", "9-266-177", "9-267-177", "9-268-177")),
                    "metadata": {"bounds": bounds}},
        "places": {"files": files(*tiles("places", "0-0-0")), "metadata": {"bounds": bounds, "osm_sha256": "o" * 64}},
        "overlays": {"files": files(*tiles("overlays", "6-33-22")),
                     "metadata": {"bounds": bounds, "routing_package": package["routing/directory.bin"]["sha256"]}},
        "routing-cells": {"files": package},
        "terrain": {"files": files(*tiles("terrain", "9-267-177", "9-269-177", "13/9-267-177")),
                    "metadata": {"bounds": [7, 47, 10, 49], "source_sha256": json.dumps(["t" * 64]), "attribution": "GLO-30"}},
        "assets": {"files": files("maps/assets/sprites/v4.json")},
        "model": {"files": files("search/model/model.bin")},
        "fonts": {"files": files("offline/fonts/Sans.pbf"), "aliases": {"maps/assets/fonts/Sans/0-255.pbf": "offline/fonts/Sans.pbf"}},
    }
    for component in ("pois", "addresses"):
        names = {cell: f"search/tiles/{component}/{cell}.sqlite" for cell in cells}
        indexes[component] = {"files": files(*names.values()), "cells": [
            {"id": cell, "bounds": box, "files": [names[cell]]} for cell, box in cells.items()],
            "metadata": {"schema": 6, "time_zone": "Europe/Berlin", "bounds": bounds, "osm_sha256": "o" * 64,
                         "component": component, "counts": {"place": 1}}}
    layers = {}
    for kind, index in indexes.items():
        step = {"pois": "planner/search/pois/grid", "addresses": "planner/search/addresses/grid",
                "routing-cells": "planner/routing/cells/grid"}.get(kind, f"planner/{kind}/grid")
        path = root / "indexes" / step / "index.json"
        path.parent.mkdir(parents=True)
        path.write_text(json.dumps({"format": 1, "kind": kind, **index}))
        layers[step] = {"index.json": str(path)}
    return layers


def run(step, layers, output):
    output.mkdir()
    step({"layers": layers, "options": OPTIONS, "output": str(output), "metrics": str(output.parent / f"{output.name}.json")})


class PlannerGridIndex(unittest.TestCase):
    def test_the_release_names_each_step_index_and_lists_no_file_of_one(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            layers = write_indexes(root)
            run(grid_offline.step, layers, root / "offline")
            layers["planner/offline/grid"] = {"index.json": str(root / "offline/index.json")}
            run(grid_index.step, layers, root / "index")
            document = json.loads((root / "index/release.json").read_bytes())
            self.assertEqual((document["format"], document["grid"]), (2, {"format": 3, "zoom": 9}))
            self.assertEqual(list(document["indexes"]), sorted(layers))
            for step, item in document["indexes"].items():
                data = Path(layers[step]["index.json"]).read_bytes()
                self.assertEqual((item["bytes"], item["sha256"]), (len(data), hashlib.sha256(data).hexdigest()))
            self.assertEqual(document["indexes"]["planner/routing/grid"]["folders"], ["offline", "routes", "routing"])
            self.assertEqual(document["indexes"]["planner/offline/grid"]["folders"], ["offline"])
            self.assertEqual(list(document["files"]), ["search/test.grid.json"])
            directory = json.loads(Path(layers["planner/routing/cells/grid"]["index.json"]).read_bytes())
            self.assertEqual(document["routing_package"], directory["files"]["routing/directory.bin"]["sha256"])
            self.assertTrue((root / "index/public/maps/tiles/basemap/7-66-44.json").is_file(),
                            "the pointers name the files of the step indexes")


if __name__ == "__main__":
    unittest.main()
