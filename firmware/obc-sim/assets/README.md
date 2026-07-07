# Simulator fixture assets

The committed `.obcm` maps the simulator (and several workspace tests) load.
They are **built artifacts** — their sources are not in the repo — so their
provenance is pinned here and in [`repack.sh`](repack.sh), the only supported
way to regenerate them.

| file | what / why | source | extract bbox (lon,lat,lon,lat) |
| :-- | :-- | :-- | :-- |
| `grimsel.obcm` | Grimsel Pass region — alpine showcase: contours, switchbacks, sparse POIs, 63-component nav graph | Geofabrik `europe/switzerland-latest.osm.pbf` | `8.15034,46.48261,8.46007,46.72070` |
| `monaco.obcm` | Central Monaco — dense urban: POI-rich (opening hours), tight street grid | Geofabrik `europe/monaco-latest.osm.pbf` | `7.39,43.71,7.47,43.77` |
| `grimsel-climb.gpx` / `.obcr` | Hand-made climb route inside the grimsel bbox (GPX + packed OBCR) | authored, not extracted | — |

Both maps are packed with `packer/presets/default.json` via
`cargo run --release --bin obc-pack -- <extract> <preset> <out>` — `repack.sh`
runs the whole chain (download → `osmium extract` → pack).

## The bbox-ratchet trap (read before re-packing)

**Never derive an extract bbox from an existing fixture's header.** The header
bbox is computed from the *packed content* and is always somewhat wider than
the extract bbox (osmium keeps complete ways crossing the boundary, and stray
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
