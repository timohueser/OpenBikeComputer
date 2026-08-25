"""Board-composition contracts for the FS7 flat route/trip catalog cutover.

The nRF crate is Thumb-only, so these checks pin the glue that its target build type-checks but a
host unit test cannot execute: a successful engine commit must cross the catalog rescan before the
upload fact it produces, while transient reads must keep the prior snapshot and re-arm that rescan.

The board drains no `HostCommand`s: the rescan is `CatalogEffect::ReadCatalog`'s body
(`read_catalogs`), the delivery is `note_catalog_uploads` writing `ExternalFacts` for the *next*
pass, and the re-arm a partial read owes is the executor's own `rescan_owed`.
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

        delivery = body(RIDE, "fn note_catalog_uploads", "/// **`CatalogEffect::ReadCatalog`")
        self.assertIn("note_route_upload", delivery)
        self.assertIn("note_trip_upload", delivery)
        self.assertIn("replaced: upload.replaced()", delivery)

        rescan = body(RIDE, "fn read_catalogs", "/// A `no_std`")
        routes = rescan.index("load_routes(flat, app)")
        trips = rescan.index("load_trips(flat, app)")
        events = rescan.index("note_catalog_uploads(app, facts)")
        self.assertLess(routes, trips)
        self.assertLess(trips, events, "typed upload ids must resolve against the newly-fed snapshots")

    def test_upload_queue_coalesces_replacements_and_has_a_loss_fallback(self) -> None:
        queue = body(FLAT_STORE, "fn queue_catalog_upload", "fn note_catalog_upload")
        self.assertIn("for _ in 0..queued", queue)
        self.assertIn("prior.same_object(upload)", queue)
        self.assertIn("events.push_back(upload)", queue)
        self.assertLess(
            queue.index("prior.same_object(upload)"),
            queue.index("if let Err(upload) = events.push_back(upload)"),
            "same-object replacements must coalesce before the capacity fallback",
        )
        self.assertIn("events.pop_front()", queue, "saturation keeps the newest fact, not a stale oldest one")

        note = body(FLAT_STORE, "fn note_catalog_upload", "pub(crate) fn take_catalog_upload")
        self.assertIn("UPLOAD_EVENTS_LOSS.store(true", note)
        self.assertNotIn("advisory deferred", note, "a dropped fact is loss, not deferred work")

        delivery = body(RIDE, "fn note_catalog_uploads", "/// **`CatalogEffect::ReadCatalog`")
        loss = delivery.index("take_catalog_upload_loss()")
        drain = delivery.index("while let Some(upload)")
        self.assertLess(loss, drain, "the conservative refresh precedes retained facts in commit order")
        fallback = delivery[loss:drain]
        self.assertIn("active_route_index()", fallback)
        self.assertIn("note_route_upload", fallback)
        self.assertIn("replaced: true", fallback)

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

        rescan = body(RIDE, "fn read_catalogs", "/// A `no_std`")
        self.assertIn("if routes_loaded && trips_loaded", rescan)
        self.assertIn("routes_loaded && trips_loaded && rides_loaded", rescan)

        # The re-arm a partial read owes. It is no longer a re-injected `HostEvent::StoreChanged`:
        # the domain is answered `Failed { Unreadable }` (so it stays free to serve the next delete)
        # and the *re-read* is the executor's own, because a store that did not move raises no new
        # revision and would therefore order no second refresh. Both halves are pinned, because
        # either one alone is the bug: the answer without the retry is a menu stuck until the next
        # commit, and the retry without the answer is a catalog domain wedged forever.
        served = body(RIDE, "if let Some(effect) = exec.effects.catalog.take() {", "// The in-flight removal")
        self.assertIn("exec.rescan_owed = !read", served)
        self.assertIn("CatalogOutcome::Failed { token, error: CatalogError::Unreadable }", served)
        self.assertIn("} else if exec.rescan_owed {", served)

    def test_menu_loader_retains_only_bounded_object_open_keys(self) -> None:
        head = body(FLAT_STORE, "struct CatalogHead", "fn retain_newest")
        self.assertIn("id: ObjectId", head)
        self.assertIn("revision: Revision", head)
        self.assertIn("size_of::<CatalogHead>() <= 16", head)
        self.assertNotIn("DisplayName", head)
        self.assertNotIn("EntryMeta", head)

        for loader in ("pub(crate) fn load_routes", "pub(crate) fn load_trips"):
            section = body(FLAT_STORE, loader, "///" if loader.endswith("load_routes") else None)
            self.assertIn("heapless::Vec<CatalogHead", section)
            self.assertNotIn(
                "heapless::Vec<EntryMeta",
                section,
                "menu loaders must not put full catalog metadata for every slot on one frame",
            )


if __name__ == "__main__":
    unittest.main()
