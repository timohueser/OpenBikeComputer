"""Board-composition contracts for the FS7 flat route/trip catalog cutover.

The nRF crate is Thumb-only, so these checks pin the glue that its target build type-checks but a
host unit test cannot execute: a successful engine commit must cross the catalog rescan before the
typed app event, while transient reads must keep the prior snapshot and re-arm that rescan.
"""

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[3]
FLAT_STORE = (ROOT / "firmware/obc-fw-nrf54l/src/flat_store.rs").read_text()
RIDE = (ROOT / "firmware/obc-fw-nrf54l/src/ride.rs").read_text()


def body(source: str, start: str, end: str | None) -> str:
    """Return one deliberately delimited production section."""
    first = source.index(start)
    return source[first:] if end is None else source[first : source.index(end, first)]


class Fs7BoardCompositionTests(unittest.TestCase):
    def test_successful_upload_is_typed_only_after_catalog_rescan(self) -> None:
        publish = body(FLAT_STORE, "fn publish_upload", "// ══════════════════════════ the protocol-v4 engine")
        self.assertIn("UploadEnd::Committed { id, replaced }", publish)
        self.assertIn("ObjectKind::Route => Some(CatalogUploadKind::Route)", publish)
        self.assertIn("ObjectKind::Trip => Some(CatalogUploadKind::Trip)", publish)
        self.assertIn("note_catalog_upload(CatalogUpload::new(kind, id.0, replaced))", publish)

        delivery = body(RIDE, "fn apply_catalog_uploads", "/// A `no_std`")
        self.assertIn("HostEvent::RouteUploaded", delivery)
        self.assertIn("HostEvent::TripUploaded", delivery)
        self.assertIn("replaced: upload.replaced()", delivery)

        rescan = body(RIDE, "if host_pass.rescan {", "// ── On-device route delete")
        routes = rescan.index("load_routes(flat, app)")
        trips = rescan.index("load_trips(flat, app)")
        events = rescan.index("apply_catalog_uploads(app)")
        self.assertLess(routes, trips)
        self.assertLess(trips, events, "typed upload ids must resolve against the newly-fed snapshots")

    def test_transient_catalog_reads_preserve_snapshot_and_rearm_retry(self) -> None:
        for loader, setter in (
            ("pub(crate) fn load_routes", "app.set_routes_with_ids"),
            ("pub(crate) fn load_trips", "app.set_trips"),
        ):
            section = body(FLAT_STORE, loader, "///" if loader.endswith("load_routes") else None)
            transient = section.index("Ok(Err(obc_formats::io::Error::Io)) | Err(_)")
            abort = section.index("return false", transient)
            update = section.index(setter)
            self.assertLess(abort, update, "a retryable read must not publish a partial replacement snapshot")
            self.assertIn("Ok(Err(_))", section, "definitively malformed objects remain omittable")

        rescan = body(RIDE, "if host_pass.rescan {", "// ── On-device route delete")
        self.assertIn("if routes_loaded && trips_loaded", rescan)
        self.assertIn("app.apply_event(obc_app::HostEvent::StoreChanged)", rescan)


if __name__ == "__main__":
    unittest.main()
