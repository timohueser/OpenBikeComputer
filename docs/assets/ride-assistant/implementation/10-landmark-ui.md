# RA10 — Load Landmarks from the map and share real visit previews

Parent: #1734. Depends on RA05, RA06 and RA09. Preserve the reviewed nearby map/card, readable text
pages, large ordered-dither photo and Down + Back Sources drawer.

Implementation dependencies: [RA05 — #1740](https://github.com/timohueser/OpenBikeComputer/issues/1740), [RA06 — #1741](https://github.com/timohueser/OpenBikeComputer/issues/1741), [RA09 — #1744](https://github.com/timohueser/OpenBikeComputer/issues/1744).

## Production behavior

Replace static `Fixture`/`Stop`/`Landmark` and `include_bytes` inputs in
`screen/assistant/landmarks.rs` and `assistant_demo/photos.rs` with bounded selected-item data from
RA09. Use the ordinary installed map, real position and stable QID identity. Keep nearest
straight-line ordering for identification, not routed cost ranking. Initial radius is 10 km; paged
More/refresh navigation must retain access to every available result within the stated scope.
Deduplicate by QID, not screen coordinate. Distinct colocated sites remain selectable.

Keep map bounds/order stable while browsing. Show source-derived name, kind and straight-line
distance. Select opens the bounded complete text pages and optional photo; a missing image simply
removes that page. Up from the first text page can wrap to the photo as reviewed. Use actual
supported language and glyph-safe pagination from RA08; do not reuse byte-slicing mock paragraph
code. Four text pages is the source budget, not permission to truncate attribution.

Sources is for the selected item, not all landmarks in the map. Page the full article and photo
attribution/license/modification details supplied by RA08. Keep it reachable without putting a
license footer beneath every text page. Back restores the exact selected item and reading page. No
permanent photo/image cache keyed only by a list index; identity includes map revision and QID.

Apply shared current-hours eligibility to linked visiting sites. Unknown remains visible without an
Open claim. Keep information-only landmarks when no mapped approach can be established; show why
Visit is unavailable. A glacier/mountain coordinate must not re-enter through loose category
matching. No-image passes and historic ruins should remain useful without a visual-quality score.

Visit goes through RA06 shared details and RA05's complete preview/acceptance. Straight-line
identification distance never becomes a rideable access promise. Recheck source/origin/hours before
acceptance. An already accepted visit cannot be silently replaced by browsing a landmark. Keep other
Assistant questions accessible and recording independent.

## Runtime and failures

Read/query/decode in existing prepare/work steps, not in draw. Freeze active reading pages while new
work is pending and reject stale results after selection/card/map changes. Distinguish empty scope,
missing content section, unsupported data, partial query and failed read. Text survives photo
failure, with no stale photo/credit mismatch. Enforce bounded resident page/row/credit state; no
map-sized vector and no host-only file access in the screen. Use RA09's mutable pixel-target phase
for photos, including reconstruction after Sources, full redraw and a fresh capture target. Required
credits pass RA08's separate representation gate and remain readable if photo decode fails;
unsupported attribution is an omitted asset, not a page filled with replacement glyphs. Use RA02's
hours policy on RA09's remapped schedules, including landmark-only sources.

## Acceptance and verification

Use source-extracted Swiss sites and Dunlough/West Cork from RA01/RA08, not copied mock prose. Test
text-only pass, no requested-language article, maximum-length credits, unsupported glyph treatment,
colocated QIDs, closed/unknown hours, info-only access, corrupt photo, map switch while decoding,
Back restoration, partial photo/redraw/Sources reconstruction, and shared visit recording
continuity. Run focused App/reader/host suites plus captured fixture suites. Inspect named
map/text/photo/Sources/visit frames at 240 × 320. Device loading measurements belong to RA13's
hardware handoff. Remove compiled landmark photo arrays and static source lists from the production
binary; isolated renderer fixtures may remain tests.
