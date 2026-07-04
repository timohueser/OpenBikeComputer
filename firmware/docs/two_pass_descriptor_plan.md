# Cross-Chunk Priority Rendering — As Built

> **Status: implemented.** This supersedes the original "two-pass descriptor"
> proposal (kept below under *Rejected alternative* for context). The shipped
> design is a **header-scan multi-pass**, which reaches the same goal — decode
> each feature's coordinates at most once per frame while honouring priority
> globally across chunks — without the descriptor buffer or its failure mode.

## Problem

The renderer's frame buffers (`spans`, `frame_points`, `frame_ring_lens`) are
fixed-size (heapless) and a dense viewport holds far more geometry than fits.
On real data this is routine, not a corner case: a whole-map view of Freiburg
at LOD 0 has **15,736 features across 250 chunks** but only `MAX_SPANS = 1536`
span slots — ~90% of features must be dropped. The dropped ones must be the
*least important* (buildings, minor paths), never land/sea/roads, regardless of
which chunk they live in.

Two distinct ways the old code lost high-priority data:

1. **In-buffer saturation, chunk-ordered.** A single collect pass walked chunks
   in quadtree order and filled the buffers first-come. Low-priority features in
   early chunks could fill the buffers before high-priority features in later
   chunks were ever reached.
2. **Chunk-list saturation (silent).** `query_into` collected leaves into a
   `Vec<_, MAX_CHUNKS=128>`. The Freiburg LOD-0 view overlaps 250 leaves, so
   ~122 chunks — and everything in them — were dropped *before* any priority
   logic ran. This was invisible (no stat, no log).

## Design (implemented)

### 1. Priority levels in the format (already in place)

Each style carries a 2-bit priority level (1 = highest/draw-first … 4 =
lowest), packed into the style record's flags byte. See `OBCM_Spec.md`.

### 2. Reader: skip-don't-decode (`for_each_feature_filtered`)

`Reader::for_each_feature_filtered(.., should_decode: impl Fn(u8) -> bool, visit)`
reads each feature's 12-byte header and consults `should_decode(style_id)`
**before** touching coordinates:

- `true`  → decode the geometry and hand a `FeatureRef` to `visit` (as before).
- `false` → advance the byte offset past the geometry with **no** coordinate
  math and **no** buffer writes, via `skip_ring` (mirrors `read_ring`'s offset
  arithmetic exactly — a test, `filtered_decode_skips_without_drifting`, pins
  them together so they can't drift).

`for_each_feature` is now just `for_each_feature_filtered(.., |_| true, ..)`.

### 3. Reader: uncapped chunk walk (`for_each_chunk`)

`Reader::for_each_chunk(lod, view, visit)` streams every overlapping leaf
through a callback — **no `MAX_CHUNKS` cap**. The walk only reads the index
(bbox tests over `u32` nodes, no decode), so re-running it per pass is cheap.
`query`/`query_into` (the capacity-bounded twins) have since been removed
(#334) — their only callers were tests, which now collect via `for_each_chunk`.
Covered by `for_each_chunk_has_no_cap`.

### 4. Renderer: one pass per priority level

```
for level in 1..=4:
    for_each_chunk(lod, view):           # all chunks, uncapped
        for_each_feature_filtered(.., should_decode = style.priority == level):
            bbox-cull, capacity-check, push into frame buffers
```

Because the passes run lowest-number-first and each fills the buffers before the
next begins, saturation always drops the lowest-priority features — across all
chunks. Each feature matches exactly one level, so its coordinates are decoded
**at most once per frame** (header-scanned and skipped in the other passes).
The final `spans` sort by `(z, seq)` (painter's order) is unchanged.

### 5. Observability

`RenderStats` gains `chunks_visited`, shown in the sim's Render Stats panel and
the headless line, so chunk-set growth is visible (e.g. `250 chunks` at LOD 0)
and the old silent cap can't return unnoticed.

## Why not the descriptor approach (rejected alternative)

The original plan scanned headers into a sortable `descs: Vec<FeatureDesc,
MAX_SPANS>`, sorted by `(priority, z, seq)`, then decoded accepted features. It
hits the same goal but has two liabilities the header-scan design avoids:

- **`descs` overflow reintroduces the bug.** With `descs` capped at 1536 and
  filled in chunk-scan order, a 250-chunk / 15,736-feature view exhausts the
  descriptor buffer on early chunks and never even *describes* the high-priority
  land polygons in later chunks. The multi-pass has no intermediate cap — it
  fills the frame buffers priority-first, so this can't happen.
- **No per-feature bbox cull before decode** either way (the header carries only
  the anchor, not an extent), so the descriptor scan's "decode only what's
  drawn" advantage shrinks to "skip decoding the saturated tail" — a small win
  over the multi-pass's at-most-once-per-feature, paid for with ~15–80 KB of
  descriptor RAM, a sort, and the overflow risk above.

## Verification

- `cargo test --workspace` — reader tests `filtered_decode_skips_without_drifting`
  and `for_each_chunk_has_no_cap` cover the two new mechanisms; the rest of the
  suite (incl. the `obc-app` marker render) confirms no regression.
- Headless render of `freiburg.obcm` (v5, 213 MB): LOD 0 reports
  `1536/15736 features (250 chunks, … 14200 dropped) | spans 100% points 53%`.
  Land + sea (priority 1) render across the whole map; roads (priority 2) fill
  the remaining span budget — exactly the intended priority outcome.

## Follow-ups

- **`spans` raised 1536 → 3072 (done).** It was the binding limit at coarse zoom
  (100% while points sat ~53%). `Span` was also shrunk to 14 bytes (`u16`
  offsets) so the bump fits the budget; a compile-time assertion in
  `obc-render` keeps the renderer's static buffers under 200 KB on the MCU.
- **Within-level drops are still quadtree-ordered** (roads cluster NW under
  saturation). Acceptable, but a spatial-stride or importance weighting could
  spread them if ever desired.
