from pathlib import Path
import tarfile
import tempfile
import unittest

from tools.fixtures import Catalog, FixtureError, Store, build_package, resolve_path, sha256_file


def catalog_text(base_url: str, digest: str, size: int) -> str:
    return f'''schema = 1
base_url = "{base_url}"

[packages.sample]
summary = "Sample package"
archive = "packages/{digest}.tar.gz"
sha256 = "{digest}"
bytes = {size}
provenance = "Authored for the fixture-tool test"
license = "GPL-3.0-only"

[scenarios.sample]
summary = "Sample scenario"
packages = ["sample"]
map = "sample:map.obcm"
gpx = "sample:ride.gpx"
args = ["--clock", "2026-08-09T17:00"]

[profiles.test]
summary = "Everything needed by the test"
scenarios = ["sample"]
'''


class FixtureRegistryTests(unittest.TestCase):
    def test_package_build_is_deterministic_and_tree_verifies(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            source = root / "source"
            source.mkdir()
            (source / "map.obcm").write_bytes(b"map bytes")
            (source / "ride.gpx").write_text("<gpx/>\n", encoding="utf-8")
            first = root / "first.tar.gz"
            second = root / "second.tar.gz"

            one = build_package("sample", source, first)
            two = build_package("sample", source, second)

            self.assertEqual(one, two)
            self.assertEqual(first.read_bytes(), second.read_bytes())

    def test_catalog_rejects_scenario_path_escape(self):
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch) / "catalog.toml"
            path.write_text(
                catalog_text("https://fixtures.example/", "a" * 64, 10).replace(
                    'map = "sample:map.obcm"', 'map = "sample:../map.obcm"'
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(FixtureError, "unsafe path"):
                Catalog(path)

    def test_catalog_rejects_non_ascii_identifiers(self):
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch) / "catalog.toml"
            path.write_text(
                catalog_text("https://fixtures.example/", "a" * 64, 10).replace(
                    "[packages.sample]", '[packages."ß"]'
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(FixtureError, "lowercase letters"):
                Catalog(path)

    def test_resolve_requires_files_and_directories_to_match_their_fields(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            catalog_path = root / "catalog.toml"
            catalog_path.write_text(catalog_text("https://fixtures.example/", "a" * 64, 10), encoding="utf-8")
            catalog = Catalog(catalog_path)
            store = Store(catalog, root / "cache")
            package = store.package_root("sample")
            (package / "map.obcm").mkdir(parents=True)
            (package / "routes").write_text("not a directory", encoding="utf-8")

            with self.assertRaisesRegex(FixtureError, "map is not a file"):
                resolve_path(catalog, store, "sample:map.obcm", "map")
            with self.assertRaisesRegex(FixtureError, "routes_dir is not a directory"):
                resolve_path(catalog, store, "sample:routes", "routes_dir")

    def test_extract_rejects_symlink_even_with_a_valid_archive_digest(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            archive = root / "bad.tar.gz"
            with tarfile.open(archive, "w:gz") as output:
                info = tarfile.TarInfo("escape")
                info.type = tarfile.SYMTYPE
                info.linkname = "/tmp"
                output.addfile(info)
            digest = sha256_file(archive)
            catalog_path = root / "catalog.toml"
            catalog_path.write_text(catalog_text("https://fixtures.example/", digest, archive.stat().st_size), encoding="utf-8")
            catalog = Catalog(catalog_path)
            store = Store(catalog, root / "cache")
            cached = store.archive_path("sample")
            cached.parent.mkdir(parents=True)
            cached.write_bytes(archive.read_bytes())

            with self.assertRaisesRegex(FixtureError, "unsafe member"):
                store._extract("sample", cached)

    def test_profile_expands_scenario_packages_once(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            path = root / "catalog.toml"
            path.write_text(catalog_text("https://fixtures.example/", "b" * 64, 42), encoding="utf-8")
            catalog = Catalog(path)
            self.assertEqual(catalog.package_ids_for(["test", "sample"]), ["sample"])

    def test_archive_cannot_escape_base_url(self):
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch) / "catalog.toml"
            path.write_text(
                catalog_text("https://fixtures.example/", "c" * 64, 42).replace(
                    'archive = "packages/', 'archive = "https://evil.example/'
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(FixtureError, "safe relative path"):
                Catalog(path)

    def test_archive_key_must_be_derived_from_the_package_digest(self):
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch) / "catalog.toml"
            path.write_text(
                catalog_text("https://fixtures.example/", "c" * 64, 42).replace(
                    f'archive = "packages/{"c" * 64}.tar.gz"', 'archive = "packages/latest.tar.gz"'
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(FixtureError, "content-addressed key"):
                Catalog(path)

    def test_sync_repairs_a_corrupt_extracted_tree_from_the_verified_archive(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            source = root / "source"
            source.mkdir()
            (source / "map.obcm").write_bytes(b"correct map")
            (source / "ride.gpx").write_text("<gpx/>\n", encoding="utf-8")
            archive = root / "sample.tar.gz"
            size, digest = build_package("sample", source, archive)
            catalog_path = root / "catalog.toml"
            catalog_path.write_text(catalog_text("https://fixtures.example/", digest, size), encoding="utf-8")
            store = Store(Catalog(catalog_path), root / "cache")
            cached_archive = store.archive_path("sample")
            cached_archive.parent.mkdir(parents=True)
            cached_archive.write_bytes(archive.read_bytes())

            self.assertTrue(store.sync("sample"))
            (store.package_root("sample") / "map.obcm").write_bytes(b"corrupt")
            self.assertTrue(store.sync("sample"))
            self.assertEqual((store.package_root("sample") / "map.obcm").read_bytes(), b"correct map")

    def test_prune_removes_a_stale_by_id_link_after_a_catalog_digest_change(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            catalog_path = root / "catalog.toml"
            catalog_path.write_text(catalog_text("https://fixtures.example/", "d" * 64, 42), encoding="utf-8")
            store = Store(Catalog(catalog_path), root / "cache")
            stale_package = store.packages / ("e" * 64)
            stale_package.mkdir(parents=True)
            store.by_id.mkdir(parents=True)
            (store.by_id / "sample").symlink_to(stale_package, target_is_directory=True)

            stale, _ = store.prune(apply=True)

            self.assertEqual(stale, [stale_package])
            self.assertFalse((store.by_id / "sample").exists())
            self.assertFalse((store.by_id / "sample").is_symlink())


if __name__ == "__main__":
    unittest.main()
