import importlib.util
import sys
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).parents[1] / "loc_report.py"
SPEC = importlib.util.spec_from_file_location("loc_report", MODULE_PATH)
loc = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = loc
SPEC.loader.exec_module(loc)


class LocReportTests(unittest.TestCase):
    def test_firmware_crate_source_is_implementation(self):
        self.assertEqual(
            loc.classify("firmware/obc-render/src/lib.rs"),
            ("firmware", "obc-render", "implementation"),
        )

    def test_device_bench_and_vendor_are_not_implementation(self):
        self.assertEqual(
            loc.classify("firmware/obc-fw-nrf54l/src/bin/flat_store_bench.rs"),
            ("firmware", "obc-fw-nrf54l", "support"),
        )
        self.assertIsNone(
            loc.classify("firmware/obc-fw-nrf54l/vendor/embassy-usb-synopsys-otg/src/lib.rs")
        )

    def test_ios_source_and_tests_share_a_component(self):
        self.assertEqual(
            loc.classify("companion-ios/Packages/OBCKit/Sources/OBCTransport/Link.swift"),
            ("ios", "OBCTransport", "implementation"),
        )
        self.assertEqual(
            loc.classify("companion-ios/Packages/OBCKit/Tests/OBCTransportTests/LinkTests.swift"),
            ("ios", "OBCTransport", "support"),
        )

    def test_colocated_frontend_test_is_support(self):
        self.assertEqual(
            loc.classify("builder/app/src/lib/catalog/client.test.ts"),
            ("web", "frontend/catalog", "support"),
        )
        self.assertEqual(
            loc.classify("builder/app/src/lib/zip.test.ts"),
            ("web", "frontend/core", "support"),
        )

    def test_build_scripts_are_support(self):
        self.assertEqual(
            loc.classify("firmware/obc-app/build.rs"),
            ("firmware", "obc-app", "support"),
        )
        self.assertEqual(
            loc.classify("builder/build-wasm-bridges.sh"),
            ("web", "frontend/build", "support"),
        )

    def test_pipeline_and_host_tools_are_distinct(self):
        self.assertEqual(
            loc.classify("host/obc-pack/src/main.rs"),
            ("pipeline", "obc-pack", "implementation"),
        )
        self.assertEqual(
            loc.classify("host/obc-bench/src/main.rs"),
            ("tools", "obc-bench", "support"),
        )

    def test_embedded_language_lines_are_included(self):
        stats = {
            "code": 10,
            "blobs": {"CSS": {"code": 4, "blobs": {"JavaScript": {"code": 3}}}},
        }
        self.assertEqual(loc.code_lines(stats), 17)


if __name__ == "__main__":
    unittest.main()
