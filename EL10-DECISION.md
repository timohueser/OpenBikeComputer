# EL10 — contour lines from OBCT: decision package

Design deliverable for [#1078](https://github.com/timohueser/OpenBikeComputer/issues/1078), part of
the elevation epic [#1068](https://github.com/timohueser/OpenBikeComputer/issues/1068).
**This branch is a review vehicle, not an implementation.** It lives only here: the memo, the
mockup PNGs, and a throwaway tracer that the implementation issue will delete and rewrite.

Everything below is measured, not estimated, unless it says "estimate". Every mockup is a real
`obc-sim` frame over a real Grimsel bake with real Copernicus terrain — the shipped packer and the
shipped renderer, both unmodified.

> Contours in this document are derived from Copernicus DEM GLO-30
> (`obc_dem::COPERNICUS_ATTRIBUTION`) — the attribution obligation travels with them.

---

## 0. TL;DR

| | |
| :-- | :-- |
| **Placement** | **(b) — bake contours into the existing `mid`/`fine` geometry cells.** Not a `relief` band. |
| **Why** | A separate band cannot buy revision independence, because contours are *styled vector features* and the style table is shared and revision-locked (`MountedSet::mount` rejects a shard stamped from a different skin). And EL5 already coupled OBCM cells to the terrain revision by baking OBCT-derived ascent into §8.3 — (b) adds no coupling that is not already there. |
| **Price** | A terrain-dataset bump (GLO-30 revision, or a posting retune) now stales `mid`/`fine` too, not just `network`. At DACH scale that is a geometry-corpus re-publish on the terrain cadence (years), on top of the one EL5's ascent field already forces. |
| **Ladder** | 100 m interval, index every 500 m, `min_lod 4` (mpp ≤ 5). No 50 m. Source geometry clamped to 15 m before packing. |
| **Style** | One grey (`0xAD55`), emphasis by **weight only** (2 vs 1). Not sepia. |
| **Labels** | **No** in v1. Argued in §6, not assumed. |
| **Cost** | **+17.7 % of an alpine map file** (grimsel, measured), **+1.4 % on flat ground** (Rhine plain, measured) = **0.77 MiB / 1000 km² alpine**, below the epic's reserved 1–2.5 MiB band. |

**Recommended mockup: `design/el10-mockups/option-d-*.png`.** Options A/B/C are the three the issue
asked for; D is the synthesis the measurements pushed me to, and it is the one I would build.

---

## 1. What was actually built and measured

### 1.1 The tracer

`host/obc-dem/src/bin/contour_probe.rs` — **throwaway, marked as such in its own module doc.**
Marching squares over an OBCT shard's sample lattice (read back through the shared
`obc_elevation::TerrainReader`, so it is not a second decoder of the container), segments chained by
lattice-edge key, Douglas–Peucker at each LOD's tolerance the way `obc-pack`'s pipeline simplifies
lines (`simplify_m / M_PER_DEG`), saddles disambiguated by the cell mean, and any cell touching
`NODATA` skipped whole — OBCT principle 6, *a hole is silence*.

```sh
cargo build --release --bin contour_probe
./target/release/contour_probe --terrain apps/obc-sim/assets/grimsel.obcd --interval 50 --stats
./target/release/contour_probe --terrain apps/obc-sim/assets/grimsel.obcd \
    --interval 50 --index-every 10 --lod 3 --osm contours.osm
```

### 1.2 The mockup seam

`--osm` writes the contours as OSM XML ways tagged `obc_contour=minor|major|index` + `ele=<m>`,
`osmium cat` turns that into a `.osm.pbf`, and `obc-pack` merges it **beside the real Switzerland
extract** — because `obc-pack` already takes `<pbf...>` and `get_style` already matches any tag key.
So a config gains one `features.obc_contour` block and nothing else in the tree is touched:

```sh
osmium cat contours.osm -o contours.osm.pbf
obc-pack switzerland.osm.pbf contours.osm.pbf cfg-d.json opt-d.obcm \
    --bbox 8.15034,46.48261,8.46007,46.72070 --terrain apps/obc-sim/assets/grimsel.obcd
obc-sim opt-d.obcm --center 8320000,46590000 --zoom 44.567 --scale 3 --png ride.png
```

This is mockup machinery and is **deliberately not committed as pipeline code** — the real
implementation traces inside the packer (or the bakery), never through OSM XML. But note what it
proves: **option (b) rendered end-to-end on the shipped packer and the shipped renderer with zero
code changes to either.** That is the strongest evidence in this document.

### 1.3 Provenance of the numbers

Both baselines reproduce their shipped fixtures **byte-for-byte in size**, so every delta below is
attributable to contours alone:

| | packed here | shipped fixture |
| :-- | --: | --: |
| grimsel (alpine, 628 km²) | 2,856,199 B | `apps/obc-sim/assets/grimsel.obcm` 2,856,199 B |
| teningen (Rhine edge, 5.8 km²) | 481,517 B | `host/obc-bake/assets/teningen-preview.obcm` 481,517 B |

Terrain: `grimsel.obcd` (512 × 768 samples, 100 % coverage, 588 … 4060 m) and
`teningen-preview.obcd` (128 × 256, 174 … 459 m).

### 1.4 Wall time

| stage | grimsel, 878 km² terrain, 50 m interval (71 levels) |
| :-- | --: |
| grid read (393 216 samples through the shared reader) | 18–40 ms |
| marching squares + chaining, all 71 levels | **~140 ms** |
| Douglas–Peucker, per LOD | 2–12 ms |
| **total tracing pass** | **~190 ms** |

Extrapolated to DACH (482 000 km²) single-threaded: **≈ 105 s**. Against a bake that already spends
~30 s on a *single* 628 km² alpine crop, the tracing pass is not a cost worth designing around.
100 m interval halves it (~75 ms for grimsel).

---

## 2. The size table (ground truth, not estimate)

Per-LOD bytes = quadtree index + offset table + chunk data, read straight off each file's OBCM §3
LOD table. `base` is the contour-free bake.

| LOD | ≤ m/px | base | A (50 m) | A clamped | C (100 m) | C clamped | **D (rec.)** |
| --: | --: | --: | --: | --: | --: | --: | --: |
| 0 | ∞ | 16K | — | — | — | — | — |
| 1 | 30 | 31K | — | — | — | — | — |
| 2 | 16 | 65K | — | — | — | — | — |
| 3 | 10 | 164K | +31K | +31K | +31K | +31K | — |
| 4 | 5 | 315K | +216K | +159K | +216K | +159K | **+159K** |
| 5 | 3 | 490K | +331K | +165K | +331K | +165K | **+165K** |
| 6 | 1.2 | 906K | +1009K | +349K | +489K | +170K | **+170K** |
| **file** | | **2.72 MiB** | **+56.9 %** | **+25.2 %** | **+38.3 %** | **+18.8 %** | **+17.7 %** |
| **MiB/1000 km²** | | — | 2.47 | 1.09 | 1.66 | 0.815 | **0.768** |

Flat ground, same option-D ladder, two independent Rhine-side boxes:

| box | area | base | with contours | Δ |
| :-- | --: | --: | --: | --: |
| teningen preview (a hillside, 174–459 m) | 5.8 km² | 481,517 B | 488,189 B | **+1.4 %** |
| Rhine plain 7.75–7.80 / 48.11–48.15 | 16.5 km² | 414,722 B | 420,529 B | **+1.4 %** |

The flat box is genuinely flat: exactly **one** contour level (200 m) crosses it, in 11 fragments.
That is the whole story of the flat case — contours on gentle ground are one line, and cost one
line's worth of bytes.

### 2.1 Against the epic's ceiling — read this before quoting a percentage

**The percentages above are alpine worst-case ratios and are NOT comparable to the epic's
DACH-wide "+5–7 % of whole map".** Grimsel is the densest terrain and the *sparsest* OSM in the
corpus, so every terrain-derived byte is divided by an unusually small map. On grimsel the already-
merged OBCT sidecar is itself ~+20 % of the map file, against the epic's DACH-wide +4.4–6.7 %.

The comparable metric is **MiB / 1000 km²**, and the epic reserved **1–2.5 MiB/1000 km²** for
contours at 50 m alpine. Measured:

- **Option D: 0.768 MiB/1000 km²** — *below* the reserve's floor.
- Option A clamped (50 m): 1.09 — at the floor.
- Option A unclamped (50 m at native LOD-6 detail): 2.47 — at the ceiling.

So the epic's reserve holds under every option tested. **The clamp (§4.3) decides where in the band
you land, and it is free.**

### 2.2 Render-time cost

`obc-sim` frame stats, same viewport, `0 dropped` in every case — the frame budget is nowhere near
saturation:

| | features | spans | SD bytes/frame |
| :-- | --: | --: | --: |
| base, riding zoom (LOD 4) | 46/108 | 4 % | 5,943 B |
| option D, riding zoom | 72/170 | 6 % | 10,913 B |
| base, street zoom (LOD 6) | 25/50 | 2 % | 6,003 B |
| option D, street zoom | 38/72 | 3 % | 6,488 B |

The real device cost is **I/O, not spans**: riding-zoom chunk reads roughly double (5.9 → 10.9 kB
per frame, 2 → 4 chunks). At the ~460 kB/s measured SD path that is **≈ +11 ms per uncached frame**.
Worth an on-glass check; not worth a redesign. Contours carry `priority 4` so they are the first
thing the eviction heap drops if a denser region ever does saturate.

---

## 3. The placement decision: (a) `relief` band vs (b) geometry cells

### 3.1 What I confirmed about the two-band render path

The issue asks whether the assembled-set render path can compose two bands' features at one mpp
today. The answer is **mechanically yes, contractually no**:

- **The device already does it.** `MountedSet::visit_candidates`
  (`firmware/obc-reader/src/volume.rs:543-582`) is a fan-out loop over *every* shard whose bbox
  intersects the view and whose LOD is non-empty, accumulating candidates; `decode_selected`
  (`:586-644`) does the same for pass B, disambiguated by a 5-bit shard tag stolen from the feature
  token. `firmware/obc-render/tests/volume_set_diff.rs:139` already merges four shards at one LOD in
  one frame and asserts pixel-identity with the monolith. **`obc-render` and `obc-map-scene` would
  need no change at all.**
- **Seven layers above it say no.** The OBCA §1.5 partition rule ("every ladder LOD MUST belong to
  exactly one band"), enforced in `host/obc-pack/src/grid.rs:418-427` and
  `host/obcm-assemble/src/schema.rs:208-215`; `Schema::band_of_lod` (`schema.rs:149-152`) returns
  *one* band, so grafting would silently drop relief cells; `plan_set`
  (`host/obcm-assemble/src/lib.rs:675-689`) plans per *role* and only knows Coarse + Geometry;
  `check_set_invariants` (`lib.rs:785-807`) and `obcs::validate`
  (`firmware/obc-formats/src/obcs.rs:410-428`) require each role's shards to partition the assembly
  without overlap; and `Role::from_id` (`obcs.rs:55-79`) has no id `3`.
- **But the shape of the fix is clean.** The existing validation *already* tolerates two **roles**
  overlapping in space — the core shard and the coarse shard both span the whole assembly (OBCA
  §5.6). So `relief` wants to be a **fourth role**, not a fourth band, and then the partition rule
  becomes per-role and everything else falls out. That is ~7 sites plus a spec amendment, one extra
  FAT handle out of `SD_SET_MAX_SHARDS = 11`, and zero renderer work.

So (a) is buildable and not even especially hard. It is still the wrong call, for a reason that has
nothing to do with difficulty.

### 3.2 Why (a) cannot buy what it is for

(a)'s entire justification is the EL3 revisioning philosophy: keep OSM re-bakes and terrain
re-bakes independent. **For contours that independence is unobtainable, and two separate facts each
kill it on their own.**

**1. Contours are styled vector features, so they are revision-locked to the style table.**
OBCT gets genuine independence because it is a *raster* — it carries no styles, and no consumer
parses it as OBCM. Contours are not a raster. They are OBCM line features referencing style ids in
the map's **global, shared** style table (OBCM §2: "Global — style IDs are shared across every
LOD"), and `MountedSet::mount` (`firmware/obc-reader/src/volume.rs:404-406`) *rejects a shard
stamped from a different skin*. So a relief shard's contour styles must be ids in the same canonical
table as the geometry bands', and any schema or skin revision — exactly the bump OBCA §6.3's
lockstep rule already re-bakes the whole cell store on — stales the relief band with everything
else. A `relief` band would be independent of OSM churn and *not* independent of the thing that
actually forces the expensive re-bakes.

**2. EL5 already coupled OBCM cells to the terrain revision, and it merged.**
OBCM v12 §8.3 stores per-direction integrated ascent, sampled from baked OBCT at pack time (epic
principle C, *one sampling truth*). The nav graph lives in the `network` band. So **a terrain
revision already stales OBCM cells today.** (b) does not create the coupling the issue's "con"
warns about; it widens an existing one from the `network` band to `mid`/`fine`.

Against that, (b)'s "pro" is not merely simplicity — it is that **the mockups in this PR are option
(b), produced with an unmodified packer and an unmodified renderer.** There is no risk left to
retire.

### 3.3 The recommendation, with its price named

> **Recommendation: (b) — trace at pack time and emit contours as ordinary OBCM line features in
> the `mid`/`fine` geometry cells.**

**The price, stated plainly:** a terrain-dataset bump — a new GLO-30 release, or retuning the
posting/cell pairing — now invalidates the `mid` and `fine` geometry cells for every affected cell,
not just the `network` band. At DACH scale that is a geometry-corpus re-bake and re-publish. The
mitigations are that terrain moves on a **years** cadence while OSM moves constantly (so the
geometry cells are re-baked for other reasons far more often anyway), that EL5 already forced the
`network` share of this cost, and that the bakery's cell-level digests mean only cells whose
contours actually changed get re-published.

**(c) device-side tracing stays rejected**, as the issue proposed: per-viewport marching squares on
the MCU is the per-frame terrain work the epic parked with hillshading, and §2.2 shows the baked
path costs ~11 ms of I/O per frame — a traced path would cost that plus the march.

---

## 4. Interval and LOD ladder

### 4.1 Recommendation

| class | interval | `min_lod` | style |
| :-- | --: | --: | :-- |
| `major` | 100 m | 4 (mpp ≤ 5) | `color 0xAD55`, `weight 1`, `z_index 8`, `priority 4` |
| `index` | 500 m | 4 (mpp ≤ 5) | `color 0xAD55`, `weight 2`, `z_index 9`, `priority 4` |

`z_index 8/9` puts contours above the landcover fills (z 2–6) and below buildings (10), water
(14/16) and every road (24+) — terrain under everything a rider follows.

### 4.2 No 50 m interval

Two measurements, one conclusion:

- **It shows nothing.** At LOD 6 (mpp ≤ 1.2) the screen is ~290 m wide and the OBCT posting is
  39 × 57 m — about **six samples across the display**. `option-a-street.png` is the evidence: the
  50 m lines there are straight bilinear-interpolation segments, not terrain.
- **It costs 183 KB** on grimsel alone (option A clamped vs option C clamped), +6.5 points of map
  size, for that.

### 4.3 Clamp the traced geometry to what the DEM knows — the single biggest lever

The packer simplifies at 3 m (LOD 5) and 0.5 m (LOD 6). Those tolerances are one to two orders of
magnitude finer than a ~40 m-posting DEM can support, so the fine LODs faithfully store
interpolation noise. Pre-simplifying the traced polylines at **15 m** before they reach the packer:

| | 50 m interval | 100 m interval |
| :-- | --: | --: |
| unclamped | +56.9 % | +38.3 % |
| clamped to 15 m | **+25.2 %** | **+18.8 %** |
| saved | 905 KB | 556 KB |

**Nothing visible changes** — 15 m is still well inside the DEM's own resolution. This is free
money and the implementation issue should treat it as a requirement, not a tuning knob.

### 4.4 `min_lod 4`, not 3

The config ladder has **no `max_lod`** — a style's `min_lod` puts it in that tier *and every finer
one*. So the only ladder lever is where contours *start*. Starting at LOD 3 (mpp ≤ 10) costs
+31 KB and is worse than free: at that zoom a hairline index contour is invisible
(`option-c-wide.png`) and a weight-2 one **reads as a road** (`option-a-wide.png` — those dark grey
lines are 500 m contours, and they look exactly like the trunk roads two zoom steps out).
Contours should appear when the rider zooms in to read terrain, not on the planning view.

---

## 5. Style: grey, and emphasis by weight

### 5.1 Not sepia — the warm band is the trail palette

Option B is the classic-topo answer and it fails on this device, in **both** skins, for the same
reason: brown and tan are already the trail colours, and trails are the feature class a bikepacker
most needs to pick out.

- `option-b-ride.png` (default skin): index contours are `0xAAA0` — the **exact** colour of
  `highway=track|path|footway`. A 500 m contour and a farm track are indistinguishable.
- `option-b-dusk-ride.png` (dusk skin): dusk lifts trails to tan `0xFD4A`, so contours in any warm
  value compete again. The mockup uses brown + khaki `0xAD4A` to stay off the taken values, and it
  is still hard to tell contour from trail at a glance.

### 5.2 Grey is the free band

`0xAD55` (170,170,170) is on the RGB222 grid, unused by any *line* style in the default skin, and
reads as "not a thing you can ride" — which is exactly what a contour is.
`option-c-ride.png` / `option-d-ride.png` show roads, trails and water all staying legible on top.

### 5.3 Emphasis by weight, never by a second colour

Option A emphasises the index contour with a darker grey (`0x52AA`). Two problems, both visible:
`0x52AA` is the local-street casing colour in the default skin, and at the planning zoom the darker
heavier line reads as a road (`option-a-wide.png`). **Option D keeps one grey and varies only
weight (2 vs 1)**, which gives the index rhythm without inventing a second mark.

### 5.4 Open item for the implementation issue

The dusk skin's grey `0xAD55` is its *street* colour, so dusk needs its own contour value. `0x52AA`
(buildings — which essentially never co-occur with alpine contours) and `0xAD4A` (khaki, unused) are
both free and both on-grid. **This was not mocked in grey for dusk** — only option B's warm pair was
— and it should be, before the skin is written.

---

## 6. Labels: no in v1 — the argument, not the assumption

**Against, and these are the reasons:**

1. **There is no map-label subsystem to reuse.** `obc-render` draws text for *chrome* only; there is
   no placement, collision, along-path baseline or heading-rotation machinery for map features.
   Labels are not a style flag, they are a new subsystem — and a large one relative to EL10's whole
   scope.
2. **The screen cannot spare the contrast.** 240 × 320, 64 colours, reflective. A legible `2500`
   needs ~24 × 8 px and must interrupt the contour it labels; at riding zoom the mockups show 8–12
   contours across the frame, so a useful label density would put text over a meaningful fraction of
   the map.
3. **The device answers the underlying question better elsewhere.** "How high am I / how much
   climbing is left" is what a rider actually wants, and EL7's real route profile, EL8's
   map-referenced Current Elevation tile and the shipped Climb screen answer it numerically. A
   numeric readout beats hunting for a contour label on a 2.7" panel.
4. **It compounds the I/O cost** (§2.2) on the pass that is already the measurable one.

**For, stated fairly:** without labels, contours give *shape* but not *absolute height*, and shape
alone does not distinguish a basin from a summit. That is a real loss.

**Why it does not carry:** the 500 m index rhythm plus the Current Elevation tile give the absolute
anchor, and slope direction is readable from the drawn route profile. Ship without labels; revisit
only if the on-glass review says shape-without-height is genuinely confusing in the field.

---

## 7. The mockups

All at Handegg / Räterichsbodensee on the Grimsel road (`--center 8320000,46590000`), chosen so
contours are judged *against* the features they must not fight: steep ground west, the pass road and
a reservoir east, tracks and a service road through the middle. Three zooms land on three rungs:

| suffix | mpp | LOD | what is present |
| :-- | --: | --: | :-- |
| `-wide` | 9.0 | 3 | planning zoom |
| `-ride` | 4.0 | 4 | the default riding zoom |
| `-street` | 1.0 | 6 | finest |

| option | interval | ladder | file Δ | verdict |
| :-- | :-- | :-- | --: | :-- |
| `baseline` | — | — | — | reference: no terrain sense at all |
| `option-a` | 50 m minor / 100 m major / 500 m index | index @3, major @4, minor @6 | +25.2 % | dark index reads as a road at `-wide`; 50 m lines are DEM noise at `-street` |
| `option-b` | same as A | same as A | +25.2 % | sepia collides with the trail palette in both skins |
| `option-b-dusk` | same as A | same as A | +25.2 % | dusk check for B — same collision, plus grey is taken by streets |
| `option-c` | 100 m + 500 m index | index @3, major @4 | +18.8 % | clean; the LOD-3 index still buys nothing |
| **`option-d`** | **100 m + 500 m index** | **both @4** | **+17.7 %** | **recommended** |

All options are the DEM-clamped geometry (§4.3) except where the table above says otherwise; A and
B are byte-identical bakes differing only in the style table (both 3,576,558 B), which is itself a
useful confirmation that colour is free.

---

## 8. Drafted follow-up sub-issues

**Not filed.** Ready to file once a pick is made; the text assumes option D and placement (b).

---

### Draft 1 — EL10a: `obc-pack` traces contours from OBCT at bake time

> Part of #1068, implements the picked design from #1078.
>
> **Scope.** Teach `obc-pack` to trace contour polylines from the OBCT tiles it is already given via
> `--terrain` (EL5 samples them for §8.3 ascent; this reads the same bytes for the same reason —
> epic principle C, one sampling truth) and inject them as ordinary OBCM line features into the
> normal per-LOD pipeline. Placement is **(b)**: they land in the existing `mid`/`fine` geometry
> cells. No new band, no new OBCS role, no OBCM version bump, no renderer change.
>
> **The pass.** Marching squares over the lattice, saddles resolved by the cell mean, segments
> chained by lattice-edge key into maximal polylines, any cell touching `NODATA` skipped whole
> (OBCT principle 6). Determinism is a contract, as in `obc-dem`: fixed iteration order, no
> `HashMap` iteration reaching the output, and a digest pin in the tests.
> `host/obc-dem/src/bin/contour_probe.rs` on the `design/elevation-el10-contours-1078` branch is a
> working sketch — **read it and delete it**, do not port it.
>
> **Required, not optional: clamp the traced geometry to 15 m before it enters the pipeline.** The
> packer's LOD 5/6 tolerances (3 m / 0.5 m) are far finer than a ~40 m-posting DEM supports, so
> without the clamp the fine LODs store interpolation noise: measured **+905 KB on grimsel**, for
> nothing visible. See #1078's memo §4.3.
>
> **Config surface** (`obc-pack schema` + `schema/config.schema.json` + the `schema_*` pinning tests
> must move with it, or the builder's editor lies):
> ```json
> "contours": { "interval_m": 100, "index_every": 5, "min_lod": 4 }
> ```
> plus the two `features.contour.{major,index}` style entries, so a skin can restyle them like any
> other feature.
>
> **Acceptance.** Grimsel bake grows **+17.7 % ± 1 %** and teningen **+1.4 % ± 0.5 %** (the memo's
> measured numbers); tracing adds < 0.5 s to the grimsel bake; output is byte-identical across two
> runs; `merge_lines` does not join contours at different elevations; contours never enter the nav
> graph or the POI table. Fixtures regenerate only via `assets/repack.sh` (fixture-provenance rule).
>
> **Docs staleness:** the packer-stages page gains a tracing stage, and the data-formats page's
> "what is in an OBCM" needs contour features mentioned. EL11 (#1079) batches the rewrite; this PR
> states the staleness.

---

### Draft 2 — EL10b: contour styles in the schema ladder and both skins

> Part of #1068, follows EL10a.
>
> **Scope.** Add the two contour style entries to `builder/presets/schema.json` and give both
> shipped skins a value for them.
>
> - Schema: `contour.major` — `0xAD55`, `weight 1`, `z_index 8`, `min_lod 4`, `priority 4`;
>   `contour.index` — `0xAD55`, `weight 2`, `z_index 9`, `min_lod 4`, `priority 4`.
>   `z_index 8/9` is above the landcover fills and below buildings, water and every road.
> - Emphasis is **weight only**. A second colour was mocked and rejected: `0x52AA` is the local-street
>   casing, and a darker heavier contour reads as a road at the planning zoom
>   (`option-a-wide.png` in #1078).
> - **Dusk needs its own pair**: dusk's `0xAD55` is its street colour. `0x52AA` and `0xAD4A` are both
>   free and both on the RGB222 grid. **Mock the grey dusk pair before writing it** — #1078 only
>   mocked the warm one, which failed.
> - Sepia is rejected for the day skin: `0xAAA0` is exactly `highway=track|path|footway`
>   (`option-b-ride.png`).
>
> **Watch out.** Adding features to `schema.json` shifts style ids, which re-pins `geos_smoke` and
> every fixture digest — the trap from the default-preset-v4 round. Both skins must restate the
> contour values identically or only the canonical merged id gets styled (the dusk `skin_note` rule).
>
> **Acceptance.** Catalog skin tests pass; both skins land on the RGB222 grid; a grimsel bake under
> each skin renders contours distinguishable from trails at the riding zoom.

---

### Draft 3 — EL10c: on-glass review and the I/O check

> Part of #1068, closes out EL10. **Hardware, not code.**
>
> **Scope.** The mockups are simulator frames; the panel is reflective, and grey-on-cream is exactly
> the pairing a simulator flatters. Verify on the DK:
>
> - [ ] Contours at the riding zoom are visible in daylight and do not compete with trails.
> - [ ] The 500 m index rhythm is readable from weight alone (weight 2 vs 1) at 1 px differences.
> - [ ] The planning zoom (LOD 3) reads correctly **without** contours — confirm the `min_lod 4`
>       cut is right and not merely cheap.
> - [ ] **Frame I/O:** #1078 measured riding-zoom chunk reads roughly doubling (5.9 → 10.9 kB/frame,
>       2 → 4 chunks) ≈ **+11 ms per uncached frame** at the ~460 kB/s SD path. Measure real pan
>       latency on glass; if it bites, `min_lod 5` is the lever (costs the riding zoom, saves LOD 4).
> - [ ] Dusk skin at night, same checks.
> - [ ] `nav_step` stack high-water unchanged (contours are render-path only, but confirm).
>
> **Open question for the field:** shape without absolute height. #1078 argues labels are not worth
> a whole map-label subsystem on this panel (§6) and that EL7's profile plus EL8's elevation tile
> answer the rider's real question. This checklist is where that argument gets tested.
