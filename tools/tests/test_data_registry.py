import re
import subprocess
import unittest
from unittest.mock import patch

from tools import data_registry


class DataRegistryTests(unittest.TestCase):
    def test_a_python_step_reads_box_regions_only(self):
        self.assertEqual(data_registry.region_box("monaco"), [7.39, 43.71, 7.47, 43.77])
        for region in ["dach", "europe/germany"]:
            with self.assertRaisesRegex(ValueError, "needs a box region"):
                data_registry.region_box(region)

    def test_a_credit_with_a_date_needs_the_date(self):
        self.assertNotIn("{year}", data_registry.attribution("era5-land", year=2026))
        with self.assertRaises(KeyError):
            data_registry.attribution("era5-land")

    def test_a_failed_fetch_names_the_error_and_its_fix(self):
        error = '{"error": {"code": "fetch_failed", "message": "HTTP 404", "fix": "Check the pin."}}\n'
        failed = subprocess.CompletedProcess([], 4, stdout=error)
        with patch.object(data_registry.subprocess, "run", return_value=failed):
            with self.assertRaisesRegex(RuntimeError, "obc data fetch tangrams-icons: HTTP 404\nCheck the pin."):
                data_registry.fetch("tangrams-icons")

    def test_the_static_page_footers_show_the_registry_credits(self):
        # These pages are HTML that no step generates, so they keep the text and this compares it.
        footers = {"docs/index.html": ["osm-planet", "copernicus-glo-30"], "docs/templates/page.html": ["osm-planet"],
                   "docs/templates/blog_post.html": ["osm-planet"], "builder/app/src/App.svelte": ["osm-planet"]}
        for path, sources in footers.items():
            text = re.sub(r"\s+", " ", re.sub(r"<[^>]+>", "", (data_registry.ROOT / path).read_text()))
            for source in sources:
                self.assertIn(data_registry.attribution(source), text, f"{path} must show the {source} credit")


if __name__ == "__main__":
    unittest.main()
