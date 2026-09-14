# Landmark lane adversarial review — round 1

Reviewed the complete epic, all 13 child headings/dependency declarations, and full RA01, RA08, RA09, RA10, RA13 bodies. Also read RA02 and RA05 for query/hours/access and visit interfaces. Cross-checked map render/preparation/frame ownership, source extraction, hours, font coverage and existing fixture paths. No edits to repository files, builds, tests or publication.

**Ready status: not ready until L1 and L2 are resolved.** The architecture and issue split otherwise fit the repo. No new coordinator, per-country runtime database or speculative backend service is needed. Three modest spec corrections below should avoid larger implementation detours.

## Blocking findings

### L1 — RA09 Photo format and bounded execution; RA10 Runtime and failures: no complete display lifetime for the required streamed photo

The spec simultaneously requires (a) no I/O in draw, (b) only row/tile-ready output, (c) responsive incremental decode, and (d) no full photo buffer, but does not name where completed image rows live or how a full redraw obtains them. This is a concrete gap in the current seams, not merely an implementation detail. `App::render_scene_map_timed` in `firmware/obc-app/src/app.rs` calls `UiRuntime::prepare_base` once and then gives screen draw read-only state. Preparation receives readers but no pixel sink. `firmware/obc-fw-nrf54l/src/map_plane.rs::MapDisplay` owns the resident Frame64 separately. Scripted simulator frames in `apps/obc-sim/src/main.rs` allocate a fresh framebuffer for each render. Sources drawers can incur a base redraw or dim the base, and dismissal incurs a base redraw. A single row held in App cannot reproduce the accepted full photo in these cases.

**Required fix:** name one supported photo-presentation contract in RA09 and add RA10 acceptance for it. A minimal route is a shared bounded photo preparation/render coordinator that writes decoded rows to the host's existing resident frame through an explicit mutable-target phase outside screen draw, keeps only identity/rectangle/completion descriptors, and replays decode when a fresh target/base redraw needs pixels. The host must not clear already completed rows between steps. Opening/closing Sources, switching pages, fresh headless capture and partial/failing decode must use this same contract on simulator and board. Specify when presentation may expose partial rows, how cancellation clears/restarts the rectangle, and how source/scratch lifetimes end before async present. Alternatively choose another small existing-seam-compatible design, but do not leave an impossible row-only read-only full redraw as the default. No new full framebuffer or permanent photo allocation is necessary.

### L2 — RA08 Images and attribution/Text and language; RA10 Sources: required attribution cannot currently be represented for all accepted assets

RA08 has a bounded prose policy and says names/credits are preserved independently, but its unsupported-letter rule is not explicitly applied to required credits. An otherwise valid Commons photo may require a creator/notice outside ASCII + Latin-1 + Latin Extended-A; a source attribution URL can also exceed small strings. The firmware font map in `firmware/obc-render/src/text.rs` cannot display arbitrary creator scripts. Keeping raw UTF-8 in the map/manifest alone does not meet the stated offline Sources requirement if on-device text is `?`, truncated or silently transliterated. Article imports can carry extra notices; the text subsection currently captures license notices but does not explicitly preserve extra imported attribution in the distributable record.

**Required fix:** add a deterministic attribution representation gate before accepting text/photos. Keep complete supplied article import/credit/copyright notices, exact original creator/title, license and URLs in the offline content record and distributable manifest. Explicitly define permitted display normalization (safe typography/percent-encoded URL display), supported glyphs and page/byte caps for required metadata separately from prose. Reject an asset with an omission reason when required credit cannot be faithfully represented or falls outside a supported bound; never silently shorten or replace it with uploader/article-license defaults. Do not invent a generic transliteration or font project just to increase coverage. Pin a real unsupported-creator/long-notice test and prove full Sources can be read after photo decode failure.

This is a data/acceptance closure, not a demand for another legal approval step. The linked Wikimedia Terms and Commons guidance support retaining imported notices and original creator attribution. This review makes no broader compliance guarantee.

## Non-blocking precision correction

### L3 — RA08 Source and category policy; RA09 Format and pipeline: hours/link merge ownership should be explicit

The plan says capture hours, store an optional hours link and remap pooled offsets, but the existing service hours references are file-local indexes into `PoiDirectory`'s pool. Landmark-linked OSM entities may never classify as one of the ordinary service POIs. A producer could otherwise retain an input index or accidentally omit its schedule because that OSM entity has no service record.

**Fix:** state that RA08 hands a normalized schedule/source identity to RA09; RA09 adds it to the same shared schedule representation, interns/remaps it across whole-map and cell assembly even for non-service landmarks, and RA10 uses RA02's tri-state policy. Test a non-service castle with hours and two cells whose input hours_ref values collide. This can fit inside the current issue boundaries.

## Coverage and other conclusions

- **Epic integration:** accepted categories and four features preserved; unknown versus closed explicit; ordinary map/card/runtime paths and mock deletion are required. RA01 early capture and RA13 cross-feature handoff avoid a demo-only implementation. Dependency graph is coherent for this lane. RA10 dependency on RA06 reasonably ensures one place/visit-details path.
- **RA01:** ready. Real regional inputs, registry ownership, no author-checkout dependency and separate authored motion are clear. Full Switzerland content measurement remains intentionally RA08/RA13 work, not a claim that small regions demonstrate country coverage.
- **RA08:** resolve L2; otherwise ready. No hand-authored prose, AI or image ranking; P17 versus coverage, QID identity, deterministic language and image fallback, independent acquisition and replay are addressed. Four pages and a fixed language list are reasonable bounded first policies, not a new translator.
- **RA09:** resolve L1 and clarify L3; otherwise ready. One OBCM section, all producers/assembler/WASM, independent compressed streams and constrained decoder are appropriate. No need to add a codec registry or sidecar storage protocol. Choose actual history-window and working memory before format freeze as written.
- **RA10:** resolve L1/L2/L3 interface effects; otherwise ready. The 10 km scope and paging are reasonable concrete defaults. Stable QIDs, info-only access, closed-now filtering, photo failure isolation and visit sharing are covered.
- **RA13:** ready subject to upstream blockers. Real-data simulation, exact committed route bytes, no mock-only evidence, resource baseline and deferred physical SD/stack checks are explicit. Do not falsely block useful simulator delivery on unavailable hardware measurements; its own text correctly keeps those pending for the device session.
- **RA02/RA05 interfaces:** shared conservative hours policy repairs the observed overnight issue; information-only approach failure avoids false routing guarantees. No need to add automatic timezone inference, global access heuristics or a second landmark planner.

No additional full review is needed. Delta check should examine the photo presentation/lifetime contract, attribution gate and hours linkage only.
