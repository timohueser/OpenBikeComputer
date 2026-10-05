import unittest

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


if __name__ == "__main__":
    unittest.main()
