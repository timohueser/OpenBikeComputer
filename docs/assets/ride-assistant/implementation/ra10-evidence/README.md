# Landmarks implementation evidence

The production screen reads installed OBCM v16 content with the existing latitude-indexed query,
four compact identity rows, and one selected page buffer. That buffer also holds source pages.
RA08 supplies the name, actual language, text and attribution. RA09 supplies photo preparation.
RA06 and RA05 supply the shared detail, complete preview and durable acceptance.

Main code commits: `7bb45cd4`, `db1ec4b9`, `47617697`. Parents: Find `f075a732`, photo `7e262186`.
Inherited RA09 data and photo changes have separate reviews. Landmarks state is 1,688 bytes on
the host ABI. This is not a board measurement. No image, route or profile buffer was added.
No resource capacity or stack limit changed.

Closed landmarks remain readable for identification. Current hours govern Visit; a known-closed
site cannot be accepted. Unknown hours do not claim Open. Missing mapped access leaves the site
information-only. Unreadable photo credits omit the image while valid article text remains usable.

## Real offline data

West Cork uses a copied persistent card from the ordinary regional baker and native assembler,
with captured RA01 OSM/Copernicus inputs and corrected RA08 content. Map SHA-256:
`a48ebe53b9a545492b94ef4d59cdd2f371e70705683f092d9370112cccc29b23`.
Input and coverage limits are recorded on [#1734](https://github.com/timohueser/OpenBikeComputer/issues/1734).

Every simulator invocation used `sandbox-exec` with `(deny network*)`. No study was enabled.
The explicit simulated fix identifies the test position; it is not a recorded GPS measurement.
Dunlough Castle Q5315471 has two text pages, an 8,096-byte compressed photo and 16 source pages.

| Frame | Evidence |
| --- | --- |
| [Nearby](cork-map.png) | Source name, kind, direct distance and stable card |
| [Text one](cork-text.png), [text two](cork-text-two.png) | Complete source line breaks |
| [Photo](cork-photo.png) | Real compressed pixels through the persistent card reader |
| [First Sources](cork-sources.png), [last Sources](cork-sources-last.png) | All 16 selected article/photo attribution pages reachable |
| [Shared detail](cork-visit-detail.png) | Source category, unknown hours and mapped approach |
| [Visit preview](cork-visit-near-approach.png) | Real 110 m arrival, 0 m ascent and candidate geometry |
| [Accepted](cork-accepted.png) | Durable acceptance returns to Map with the route active |
| [Grimsel Pass](grimsel-text.png) | Source-derived Swiss text-only site; no photo page |

Photo pixels after Sources/Back match the first complete capture byte for byte. The second text
page also restores byte for byte. The indexed-query refinement was exercised again: photo
restoration and the real Visit preview match the earlier captures exactly.

The successful Visit origin is -9.825560,51.485575. Its explicit OSM approach is node 6298144844
at -9.824560,51.485575. The normal planner returned Preview and then Accepted on a copied card.
An earlier origin on another graph component returned `Plan(NoPath)` and stayed unavailable.
No cost or access was invented. This direct-destination acceptance did not start recording.

## Reproduction and checks

Build `cargo build -p obc-sim --bin obc-sim --locked`, then use the prepared card:

```sh
sandbox-exec -p '(version 1)(allow default)(deny network*)' target/debug/obc-sim \
  --card west-cork.obc --center -9829419,51482665 --heading 0 \
  --script 'L f p f u f C p f b f' --expect-screen LandmarkPhoto --png photo.png
```

`L` is a temporary entry to `App::open_landmarks`; remove it when RA12 connects the ordinary
Assistant menu. Other tokens use normal device controls and host execution. At the Visit origin,
`L f p f p f p f f f` reaches Preview. Add `p f f` on a separate card copy to accept.

Focused checks passed:

- `./tools/obc test -p obc-app -p obc-reader`.
- `./tools/obc test -p obc-host-core`.
- `cargo clippy -p obc-app -p obc-reader -p obc-host-core --all-targets -- -D warnings`.
- `./tools/obc suites check`, workspace and standalone-root formatting, `git diff --check`.
- `python3 docs/build_docs.py --check-links`.

Tests cover colocated QIDs and complete pages, query failure/cancellation, exact reading-page
restoration, page 256 of attribution, glyph/layout bounds, invalid photo credits beside valid text,
and exact map-revision replacement even after a full photo redraw. Existing shared hours and
Visit suites cover eligibility and acceptance fences. The host photo suite covers missing images,
failed reads, partial decoding, drawers and fresh-frame reconstruction.

No UI sweep, local shipping image, resource-base rebuild, full CI mirror, wake-profile isolation,
or device test was run. CI and independent review remain merge gates.

## Adversarial review corrections

Review of `e816cb53` found three defects: a later photo-credit page failure blocked the article;
Press did not refresh a populated failed or stale card; and linked photo pages always said Visit.
Commit `bfb3142e` isolates optional photo failures, restores article sources, enables the displayed
refresh action, and shares the selected-site availability decision between text and photo pages.
The photo footer keeps Up/Down navigation. A failed photo remains omitted for that selection.

The shared hours-cache fix from Find `8547bf96` was merged in `a63ea79d`. Landmark preparation
also records the selected OSM source, or zero for an information-only record. It reloads hours if
another detail overwrites that cache. A retained detail must prepare its own source before Visit.
Availability also checks the selected bike profile's explicit approach bit.

Focused validation: whole `./tools/obc test -p obc-app`, App Clippy with all targets and warnings
as errors, `./tools/obc suites check`, workspace and standalone formatting, and documentation
link checking. Tests cover a valid photo-credit header with a later unsupported page, restored
article text and all article credits, populated map invalidation and failure followed by Press,
retained detail cache ownership, and shared text/photo access decisions. No new simulator sweep,
shipping image, resource measurement, or hardware test ran. The real-data frames above remain
pre-correction evidence; final ordinary-entry acceptance stays with RA12/RA13.

## Remaining acceptance

RA12 must connect the ordinary Assistant entry and remove `L` plus the remaining opt-in study
assets. RA13 must repeat the ordinary journey while recording, review integrated layouts, and run
the final resource gate. Hardware acceptance stays pending: load the prepared card on SD; measure
list/text/Sources/photo latency; check partial photo, drawers, redraw and card replacement; repeat
Visit while recording; measure stacks with the normal release tools. No device was connected.

## Attribution

Photo: **© Superbass / CC BY-SA 4.0 (via Wikimedia Commons)**.
[Original](https://commons.wikimedia.org/wiki/File:2019-07-30-Dunlough_Castle-0819.jpg),
[licence](https://creativecommons.org/licenses/by-sa/4.0/). Resized, white padded and ordered-dithered
to RGB222. The adapted photo remains under CC BY-SA 4.0.

Text: Wikipedia contributors, CC BY-SA 4.0; source excerpts with changed typography and pagination.
Complete captured notices and display pages: [Cork](cork-attribution.json),
[Grimsel](grimsel-attribution.json). Map data: © OpenStreetMap contributors, ODbL.
