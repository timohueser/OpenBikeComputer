# Simulator fixture assets

The committed `.obcm` maps the simulator (and several workspace tests) load.
They are **built artifacts** — their sources are not in the repo — so their
provenance is pinned here and in [`repack.sh`](repack.sh), the only supported
way to regenerate them.

| file | what / why | source | extract bbox (lon,lat,lon,lat) |
| :-- | :-- | :-- | :-- |
| `grimsel.obcm` | Grimsel Pass region — alpine showcase: contours, switchbacks, sparse POIs, 63-component nav graph | Geofabrik `europe/switzerland-latest.osm.pbf` | `8.15034,46.48261,8.46007,46.72070` |
| `grimsel-demo.obcm` | Landing-page live-demo map (epic #624 S4, #629) — a **tight corridor** around the climb, **shipped in the wasm only** (`include_bytes!` in `obc-web-demo`'s `demo.rs`). NOT a shared test fixture. Padded ~2 km around the `grimsel-climb.gpx` track so the demo tours have Lodging POIs + a routable nav graph. ~5× smaller than `grimsel.obcm`. | Geofabrik `europe/switzerland-latest.osm.pbf` | `8.26,46.54,8.37,46.67` |
| `monaco.obcm` | Central Monaco — dense urban: POI-rich (opening hours), tight street grid | Geofabrik `europe/monaco-latest.osm.pbf` | `7.39,43.71,7.47,43.77` |
| `grimsel-climb.gpx` / `.obcr` | Hand-made climb route inside the grimsel bbox (GPX + converted OBCR). The GPX is also the `ui-snapshots.sh` default replay track. The `.obcr` is pinned byte-for-byte against `gpx_to_obcr` on the `.gpx` beside it by `routes.rs`'s `committed_route_asset_matches_the_gpx_conversion`, so an OBCR format bump re-cuts it: `cargo test -p obc-sim regenerate_committed_route_asset -- --ignored` (not an `.obcm`, so the repack-provenance rules don't apply) | authored, not extracted | — |
| `grimsel-climb-demo.gpx` | Demo-only trimmed replay for the wasm demo (epic #624 S4, #629): the upper ~62 % of `grimsel-climb.gpx` (ele 1530→2151 m, past Grimsel Hospiz to the pass), every 5th point, **original timestamps kept** (rebased to a 08:00:00 start; Δt≤0 decimation artifacts dropped) so the on-screen speed/stats stay honest — a ~52 min ride segment, ≈17 min wall-clock per loop at the demo's `set_speed(3.0)` playback. `include_str!`'d in `obc-web-demo`'s `demo.rs`. Regenerate by re-decimating `grimsel-climb.gpx` — a hand-made GPX, not an `.obcm`, so the repack-provenance rules don't apply. | authored, decimated from `grimsel-climb.gpx` | — |
| `monaco-upahead.gpx` | The **"Up ahead" fixture route** (epic #946, U3): a hand-made ~2.7 km line across central Monaco, authored to sit inside `monaco.obcm` so the 300 m route corridor catches real Resupply / Pharmacy / Lodging POIs. Its waypoints cover five of the six categories via `<sym>`, two Generic ones, offsets on both sides of the line (two inside the 50 m side-hint threshold, three past it), and a name long enough to ellipsize — i.e. every case the merged timeline draws. Doubles as its own replay track (`<time>` one point per minute), so `--gpx ... --at SEC` rides along it. `ui-snapshots.sh` **imports it at run time** (`obc-sim --import`) rather than committing a second `.obcr`, so an OBCR format bump needs no re-cut here | authored, not extracted | — |
| `vector-loop-replay.gpx` | Synthetic replay tracing `specs/vectors/route-waypoints.obcr` ("Vector Loop") — 6 trackpoints at that route's own vertices (from `obc-vectors/src/route-source.gpx`), timestamped at a constant ~6 m/s, ending at vertex 6 (~1.40 km, ~300 m short of the "Pass Summit" waypoint). Lies *on* the route so the matcher locks and progress drives the waypoint chip/ticks; the `ui-snapshots.sh` waypoint frames use it (the Grimsel climb GPX is far off that 48°N route). Regenerate: re-derive from the route vertices — no packer step (a hand-made GPX, not an `.obcm`, so the repack-provenance rules don't apply) | authored, not extracted | — |
| `TP1.OBT` | Fixture **trip** object (epic #526, TR2): "Alpen Traverse", stage ids `[0, 1, 99]` — the first two are the sorted-scan session ids of any two routes in the same folder, 99 a deliberate dangling ref (read-tolerance, spec §7.7). **Copy it into your `--routes-dir` beside two or more routes and rescan** to get a groupable menu: one folder (the first two routes filed) + the rest loose. TR3's snapshot harness stages exactly this file. Pinned byte-for-byte against `obc_route::write_trip` by `trips.rs`'s `committed_trip_asset_matches_the_production_writer`; regenerate with `write_trip("Alpen Traverse", &[0, 1, 99], …)` (not an `.obcm`, so the repack-provenance rules don't apply) | authored via `obc_route::write_trip` | — |

All three maps are packed with `builder/presets/default.json` via
`cargo run --release --bin obc-pack -- <source> <preset> <out> --bbox <bbox>` —
`repack.sh` runs the whole chain (download → pack). `grimsel-demo.obcm` shares
the Switzerland snapshot with `grimsel.obcm` (`./repack.sh grimsel-demo`).

The crop used to be a separate `osmium extract` step; since #910 the packer does
it during ingest, reproducing osmium's default *complete_ways* strategy, so
regenerating a fixture needs no `osmium-tool`. All three pinned bboxes were
checked byte-for-byte across the two paths before the switch — **the fixtures
were not re-packed for it**, and none of the pinned bboxes changed.

## The bbox-ratchet trap (read before re-packing)

**Never derive an extract bbox from an existing fixture's header.** The header
bbox is computed from the *packed content* and is always somewhat wider than
the extract bbox (a way crossing the boundary is kept whole, and stray
coastline/boundary features can stretch it far offshore). Extracting from a
header bbox therefore widens the fixture on every re-pack — that ratchet is
how monaco once ballooned to 14.5 MB and grimsel silently grew ~77 % in area
during the v9 bump. The bboxes in `repack.sh` are canonical; change them only
deliberately, in a reviewed commit.

Geofabrik `-latest` files roll daily, so re-packs still pick up OSM edits —
expect small content diffs between snapshots. When you commit a re-packed
fixture, update the log below.

## Pack log

| date (OSM snapshot) | file | note |
| :-- | :-- | :-- |
| 2026-07-07 | `grimsel.obcm` | v9; re-packed to the canonical bbox (the first v9 pack had self-sourced a wider bbox from the v8 header) |
| 2026-07-07 | `monaco.obcm` | v9; tight Monaco bbox established (PR #548 follow-up commit) |
| 2026-07-07 | `grimsel.obcm` | v10; line-style record (epic #556 #557). 3.80 MB → 3.80 MB (+84 B, the style table's 2-byte-per-record growth) |
| 2026-07-07 | `monaco.obcm` | v10; line-style record (epic #556 #557). Re-packed via `repack.sh` at the canonical bbox: 2.56 MB → 1.10 MB (the committed v9 file predated the repack.sh provenance pass and used a wider bbox; this is the tighter canonical size) |
| 2026-07-07 | `grimsel.obcm` | v10; rail/admin line styles (#558). `default.json` `railway.rail`/`light_rail` → dashed + white `color2` base (railway stripe), `admin_level.2` → dashed. Same snapshot, so size is unchanged (3.80 MB, 0 net bytes — records are already 8 bytes); only 7 style-table bytes differ (the two flag bits + `color2` per record). |
| 2026-07-07 | `monaco.obcm` | v10; rail/admin line styles (#558). Same preset change; unchanged size (1.10 MB, 0 net bytes), 7 style-table bytes differ. |
| 2026-07-08 | `grimsel.obcm` | v10; road casing (#559). `default.json` paved `highway` classes gained a darker `color2` (orange→`0xAAA0`, yellow→`0xAD40`, grey→`0x0000`) for the finest-LOD casing pass. Same snapshot, so size is unchanged (3.80 MB, 3797843 B → 3797843 B, 0 net bytes — records are already 8 bytes); only the paved-highway style-table records' `color2` bytes + flag bit differ. |
| 2026-07-08 | `monaco.obcm` | v10; road casing (#559). Same preset change; unchanged size (1.10 MB, 1098449 B → 1098449 B, 0 net bytes), paved-highway `color2` bytes + flag bit differ. |
| 2026-07-08 | `grimsel.obcm` | v10; polygon outlines (#560). `default.json` `building.yes` gained a darker-grey `color2` (`0x52AA`) for the finest-LOD building-ring outline. Same snapshot, so size is unchanged (3.80 MB, 3797843 B → 3797843 B, 0 net bytes — records are already 8 bytes); only the `building.yes` style-table record's `color2` bytes + flag bit differ. |
| 2026-07-08 | `monaco.obcm` | v10; polygon outlines (#560). Same preset change; unchanged size (1.10 MB, 1098449 B → 1098449 B, 0 net bytes), `building.yes` `color2` bytes + flag bit differ. |
| 2026-07-09 | `grimsel-demo.obcm` | v10; **new** demo-only map (epic #624 S4, #629). Canonical corridor bbox `8.26,46.54,8.37,46.67` from the `europe/switzerland-latest.osm.pbf` 2026-06-20 snapshot. 751518 B (734 KB) raw / 241 KB gzip vs `grimsel.obcm` 3797843 B (3.62 MB) / 1.20 MB gzip — ~5× smaller. 10 Lodging POIs, single 236 km / 1007-node routable nav component. |
| 2026-07-30 | all three | v11; tight chunks + compact feature header (#1009). `grimsel.obcm` 3797843 B → 2614924 B, `monaco.obcm` 1098449 B → 683532 B, `grimsel-demo.obcm` 751518 B → 635216 B. **These deltas are not the format's**: the committed files predate `default.json`'s preset v4 (#984, 2026-07-29), so this re-pack carries that restyle too. The format alone, measured by packing one extract with both packers: monaco 1597945 B → 683532 B (2.34×), grimsel 6189979 B → 2614924 B (2.37×), `grimsel-demo` 1567286 B → 635216 B (2.47×) — `obcm_diff --dump` identical across the bump in all three. Switzerland snapshot 2026-06-21, Monaco 2026-06-16. |
