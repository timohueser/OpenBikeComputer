"""The tool preparation and offline source boundary of the basemap step."""

import io
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from tools import basemap_tool as tool, planner_basemap as basemap


class BasemapTest(unittest.TestCase):
    def test_preparation_packages_the_pinned_planetiler_and_keeps_only_the_jar(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive, jar = root / "source.tar.gz", root / "basemap.jar"
            with tarfile.open(archive, "w:gz") as bundle:
                pom = b"<project><properties><planetiler.version>0.10.2</planetiler.version></properties></project>"
                info = tarfile.TarInfo("basemaps-pinned/tiles/pom.xml")
                info.size = len(pom)
                bundle.addfile(info, io.BytesIO(pom))

            def package(argv, *, cwd, check):
                self.assertEqual(argv[:4], ["mvn", "-B", "-q", "package"])
                self.assertIn("-DskipTests", argv)
                self.assertTrue(check)
                (cwd / "target").mkdir()
                (cwd / "target/protomaps-basemap-HEAD-with-deps.jar").write_bytes(b"prepared jar")

            with patch.object(tool.subprocess, "run", side_effect=package) as run:
                tool.prepare(archive, jar)
            self.assertEqual(run.call_count, 1)
            self.assertEqual(jar.read_bytes(), b"prepared jar")
            self.assertEqual(set(root.iterdir()), {archive, jar})
            for version in ("0.10.2-SNAPSHOT", "LATEST", "[0.10.2,0.11.0)"):
                with self.subTest(version=version):
                    with tarfile.open(archive, "w:gz") as bundle:
                        pom = f"<project><properties><planetiler.version>{version}</planetiler.version></properties></project>".encode()
                        info = tarfile.TarInfo("basemaps-pinned/tiles/pom.xml")
                        info.size = len(pom)
                        bundle.addfile(info, io.BytesIO(pom))
                    with patch.object(tool.subprocess, "run") as run:
                        with self.assertRaisesRegex(ValueError, "pin a Planetiler release"):
                            tool.prepare(archive, jar)
                        run.assert_not_called()

    def test_the_bake_has_every_store_input_and_disables_downloads(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "output"
            output.mkdir()
            snapshots = {}
            for source, name in basemap.SOURCES.items():
                path = root / source
                path.write_bytes(b"input")
                snapshots[source] = {name: str(path)}
            jar, osm = root / "jar", root / "osm"
            jar.write_bytes(b"jar")
            osm.write_bytes(b"osm")
            snapshots["protomaps-basemaps"] = {"basemap.jar": str(jar)}
            request = {"output": str(output), "snapshots": snapshots,
                       "layers": {"planner/osm": {"osm.pbf": str(osm)}},
                       "options": {"bounds": [7, 47, 8, 48], "attribution": "credits"}}

            def bake(argv, *, cwd, check):
                self.assertIn("--download=false", argv)
                self.assertNotIn("--download", argv)
                self.assertIn(f"--osm-path={osm}", argv)
                self.assertIn("--bounds=7,47,8,48", argv)
                self.assertIn("--attribution=credits", argv)
                self.assertTrue(check)
                for source, name in basemap.SOURCES.items():
                    self.assertEqual((cwd / "data/sources" / name).resolve(), (root / source).resolve())
                (output / "basemap.pmtiles").write_bytes(b"PMTiles")

            with patch.object(basemap.subprocess, "run", side_effect=bake):
                basemap.step(request)
            self.assertEqual((output / "basemap.pmtiles").read_bytes(), b"PMTiles")
            for source in basemap.SOURCES:
                with self.subTest(source=source):
                    path = root / source
                    path.unlink()
                    with patch.object(basemap.subprocess, "run") as run:
                        with self.assertRaisesRegex(ValueError, "missing input file"):
                            basemap.step(request)
                        run.assert_not_called()
                    path.write_bytes(b"input")


if __name__ == "__main__":
    unittest.main()
