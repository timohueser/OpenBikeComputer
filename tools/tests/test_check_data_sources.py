import importlib.util
import sys
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).parents[1] / "check_data_sources.py"
SPEC = importlib.util.spec_from_file_location("check_data_sources", MODULE_PATH)
guard = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = guard
SPEC.loader.exec_module(guard)

REGISTRY = {"source": [
    {"id": "osm", "fetch": {"kind": "osm", "url": "https://planet.openstreetmap.org/pbf/planet-latest.osm.pbf"}},
    {"id": "wikipedia", "fetch": {"kind": "capture", "url": "https://{language}.wikipedia.org/w/api.php"},
     "hosts": ["upload.wikimedia.org"]},
    {"id": "nominatim", "fetch": {"kind": "installed"}},
]}


class HostGuardTests(unittest.TestCase):
    def setUp(self):
        self.allowed = guard.declared_hosts(REGISTRY)

    def test_a_fetch_from_an_undeclared_host_fails(self):
        files = {"tools/new_layer.py": 'URL = "https://data.undeclared.org/v1/tiles/{z}.png"\n'}
        self.assertEqual(guard.undeclared(files, self.allowed), {"data.undeclared.org": ["tools/new_layer.py"]})

    def test_declared_hosts_extra_hosts_and_placeholder_subdomains_pass(self):
        files = {"a.rs": 'const P: &str = "https://planet.openstreetmap.org/replication/hour/";',
                 "b.py": 'f"https://{lang}.wikipedia.org/w/index.php" + "https://upload.wikimedia.org/x.jpg"'}
        self.assertEqual(guard.undeclared(files, self.allowed), {})

    def test_reserved_dynamic_and_dotless_hosts_are_not_checked(self):
        text = ('"https://maps.example/x" "http://127.0.0.1:8080" "https://{site}/w/api.php" '
                r're.compile(r"https://github\.com/x")')
        self.assertEqual(guard.hosts_in(text), set())


if __name__ == "__main__":
    unittest.main()
