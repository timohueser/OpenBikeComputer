"""Board-composition contracts for the FS7 flat route/trip catalog cutover.

The nRF crate is Thumb-only, so these checks pin the glue that its target build type-checks but a
host unit test cannot execute: a successful engine commit must cross the catalog rescan before the
upload fact it produces, while transient reads must keep the prior snapshot and re-arm that rescan.

The board drains no `HostCommand`s: the rescan is `CatalogEffect::ReadCatalog`'s body
(`read_catalogs`), the delivery is `note_catalog_uploads` writing `ExternalFacts` for the *next*
pass, and a partial read is answered `Failed { Unreadable }` so that `CatalogMachine` re-offers the
read (#1541) — the executor keeps no retry of its own.
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
        publish = body(FLAT_STORE, "fn publish_upload", "const ENGINE_STAGE: usize = 512;")
        self.assertIn("UploadEnd::Committed { id, replaced }", publish)
        # One expression per kind, so the kind, the committed id and the replaced flag are pinned
        # together rather than through a separate `kind` binding.
        self.assertIn("note_catalog_upload(CatalogUpload::new(CatalogUploadKind::Route, id.0, replaced))", publish)
        self.assertIn("note_catalog_upload(CatalogUpload::new(CatalogUploadKind::Trip, id.0, replaced))", publish)
        # A committed map is checked before its card, and never produces a catalog upload fact.
        self.assertIn("ObjectKind::MapShard => fault = check_committed_map(store, ObjectId(id.0))", publish)

        delivery = body(RIDE, "fn note_catalog_uploads", "async fn read_catalogs")
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

        delivery = body(RIDE, "fn note_catalog_uploads", "async fn read_catalogs")
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
        stages = [
            "app.begin_catalog_refresh()",
            "Request::ReconcileMetadata",
            "let start = crate::flat_store::catalog_scope(flat)",
            "load_routes(flat, app)",
            "load_trips(flat, app)",
            "load_rides(flat, app)",
            "if !routes_loaded || !trips_loaded || !rides_loaded",
            "return Err(CatalogError::Unreadable)",
            "load_metadata(flat, app).map_err(catalog_metadata_error)?",
            "if start != crate::flat_store::catalog_scope(flat)",
            "return Err(CatalogError::Stale)",
            "Ok(start)",
        ]
        for previous, following in zip(stages, stages[1:]):
            self.assertLess(
                rescan.index(previous),
                rescan.index(following),
                "catalog scope requires a complete catalog and metadata load at one stable identity",
            )

        # The executor returns the captured scope or the actual failure. Retry remains owned by
        # CatalogState; a removal does not compose a second rescan beside its outcome.
        served = body(RIDE, "if let Some(effect) = exec.effects.catalog.take() {", "// The in-flight removal")
        self.assertIn("Ok(scope) => CatalogOutcome::CatalogRead { token, scope: Some(scope) }", served)
        self.assertIn("Err(error) => CatalogOutcome::Failed { token, error }", served)
        self.assertNotIn("read_catalogs", served.split("CatalogEffect::RemoveObject", 1)[1])

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
