# OBCC Catalog Manifest Specification

OBCC (OpenBikeComputer Catalog) is the JSON contract between the central cell
bakery and map-building consumers. The website and desktop app read the same
catalog, select the same [OBCA](OBCA_Spec.md) cells, and feed them to the same
assembler. The device does not read OBCC.

The current envelope has `"schema_version": 2`. Consumers MUST reject any other
value before interpreting the rest of the document. Version 1 never shipped and
is not supported; this document describes only the current cell-catalog envelope.

The code authority is
[`host/obc-pack/src/catalog.rs`](../host/obc-pack/src/catalog.rs). Its checked-in
JSON Schema and worked examples are:

- [`catalog.schema.json`](../host/obc-pack/schema/catalog.schema.json)
- [`catalog.example.json`](../host/obc-pack/schema/catalog.example.json)
- [`cell-index.example.json`](../host/obc-pack/schema/cell-index.example.json)
- [`region-cells.example.json`](../host/obc-pack/schema/region-cells.example.json)
- [`terrain-index.example.json`](../host/obc-pack/schema/terrain-index.example.json)

The key words MUST, MUST NOT, SHOULD, and MAY are interpreted as in RFC 2119.

## 1. Design principles

1. **Bytes are authoritative.** Size, SHA-256, OBCM version, and cell bbox are
   read from or checked against the emitted bytes. Recipe intent is never
   published as an observed fact.
2. **Knowable before transfer.** A consumer can price a selection, detect partial
   coverage, and reject an unreadable format without downloading cells.
3. **Pinned composition.** The small root pins every satellite by byte length and
   SHA-256. Satellites pin every cell. A consumer either has one consistent
   publish or rejects it.
4. **Immutable referenced bytes.** Every pinned object's published key contains
   its SHA-256. Replacing the root therefore leaves every object referenced by an
   older, cached root available under its old key.
5. **One schema, many skins.** Geometry, routing, LODs, and style-id assignment
   are baked once. A skin contains presentation only and is stamped during
   assembly.
6. **Deterministic and loud.** Stable inputs produce stable documents apart from
   the explicit generation timestamp. Missing cells, mixed revisions, malformed
   bands, and digest disagreements are errors.
7. **One artifact class, one revision track.** The OBCM cell store is lockstep on
   its OBCM version and schema revision (§10). Terrain (§13) is a *second* artifact
   class with a *separate* lockstep, because it derives from a DEM that changes on a
   years cadence rather than from OSM. A bump on either track MUST NOT invalidate an
   object on the other.

## 2. Published objects

```text
catalog.json
LICENSE.txt
schema.json
terrain.json
skins/<id>.json
cells/<band>/<i>/<j>.obcm.json
cells/terrain/<i>/<j>.obcd.json
regions/<region_id>/region.json
regions/<region_id>/boundary.poly
cells/<band>/index.<sha256>.json
cells/<band>/<i>/<j>.<sha256>.obcm
cells/terrain/index.<sha256>.json
cells/terrain/<i>/<j>.<sha256>.obcd
regions/<region_id>/cells.<sha256>.json
previews/<id>.<sha256>.png
```

`catalog.json` is the root. A named region is a selection preset, not an OBCM
artifact: its satellite lists cell ids for each band. Cell indexes are keyed by
band rather than only by cell size because two semantic bands MAY use the same
`cell_log2`.

`terrain` is a **reserved** segment under `cells/`: it holds the terrain artifact
class (§13), which is not a band. A schema MUST NOT declare a band with that id.

`LICENSE.txt` is the store's human-readable provenance and licence statement,
generated from the root's `source` block (§3.1); it keeps a stable key because a
person, not a pin, is its consumer. `schema.json`, skin documents, cell sidecars,
region metadata, and boundaries are producer records and MAY retain stable keys
because no root points at them.
Every root-referenced cell, satellite, and preview uses the immutable form above;
the digest immediately before its final extension is the same lowercase SHA-256
carried by its pin. Local bake trees keep the unsuffixed names, so content
addressing changes publication rather than the resumable bake layout.

The root and satellites are complete JSON documents. Unknown optional fields MAY
be ignored. Missing required fields, unknown enum values, duplicate ids, unknown
references, unsafe URLs, or invalid ordering MUST reject the containing document.

## 3. Root document

```jsonc
{
  "schema_version": 2,
  "generated_at": "2026-07-30T09:00:00Z",
  "source": { /* SourceEntry, §3.1 */ },
  "schema": { /* SchemaEntry */ },
  "skins": [ /* SkinEntry, sorted by id */ ],
  "regions": [ /* RegionEntry, sorted by id */ ],
  "cell_index": [ /* CellIndexRef */ ],
  "terrain": { /* TerrainEntry, §13 — optional */ },
  "network_terrain_revision": 4
}
```

| Field | Type | Meaning |
| :-- | :-- | :-- |
| `schema_version` | integer | MUST equal `2`. |
| `generated_at` | string | RFC 3339 UTC, exactly `YYYY-MM-DDTHH:MM:SSZ`. |
| `source` | object | The cell store's data provenance and licence (§3.1). |
| `schema` | object | The catalog's single `SchemaEntry`. |
| `skins` | array | Non-empty presentation choices, sorted by `id`. |
| `regions` | array | Named selections, sorted by `id`. |
| `cell_index` | array | Exactly one pinned index per schema band. |
| `terrain` | object | Optional. The terrain artifact class (§13). |
| `network_terrain_revision` | integer | Optional. The terrain revision the `core` band's nav ascents were integrated from (§13.4). |

Every field but `source`, `terrain` and `network_terrain_revision` is required.
`terrain` and `network_terrain_revision` are absent for a terrain-less catalog,
which is complete and valid; `source` is required of every producer (§3.1) and
absent only from catalogs published before it existed, which consumers MUST
tolerate. `generated_at` is the only wall clock introduced while generating the
catalog and MAY be supplied explicitly for reproducible output.

### 3.1 Source declaration

```jsonc
"source": {
  "dataset_id": "openstreetmap",
  "attribution": "© OpenStreetMap contributors",
  "license": "ODbL-1.0",
  "license_url": "https://opendatacommons.org/licenses/odbl/1-0/"
}
```

| Field | Type | Meaning |
| :-- | :-- | :-- |
| `dataset_id` | string | Kebab-case id of the source dataset the cells derive from. |
| `attribution` | string | The dataset's required credit, verbatim. |
| `license` | string | SPDX-style identifier of the licence the store is offered under. |
| `license_url` | string | Where that licence's text lives. |

The cell store is a derivative database of its source dataset, and for
OpenStreetMap-derived cells the ODbL's share-alike terms require the published
store to say so: that it derives from OSM, and that it is itself available under
the ODbL. This block is that statement, machine-readable and in the one document
every consumer reads first.

All four fields are required and MUST be non-empty when the block is present. A
**producer MUST publish it** — the compat carve-out in §3 exists for documents
that predate the field, not as a licence to omit it. §13.5's display rule applies
here the same way it applies to terrain: a consumer that describes the map data —
a builder's summary card, a docs page, a device credits screen — SHOULD take the
strings from the catalog rather than hard-coding them, so a source change carries
its own notice with it. (A device with no live catalog in reach hard-codes
necessarily; the bound is that anything *reading this document* has no excuse.)

The publish also carries the same statement for a human at a stable key:
`LICENSE.txt` at the store root, beside `catalog.json` (§11). The generator
derives it from this block — and from §13.1's `attribution` when terrain is
published — so the two can never disagree.

## 4. SchemaEntry

| Field | Type | Meaning |
| :-- | :-- | :-- |
| `id` | string | Stable kebab-case schema id. |
| `revision` | integer | Monotone content revision shared by every cell. |
| `name`, `description` | string | Non-empty display text. |
| `obcm_version` | integer | Version read from the cells' OBCM headers. |
| `grid` | object | `origin_udeg` and `world_side_udeg` from OBCA §1.1. |
| `lods` | array | Coarsest first: `index`, `max_mpp`, and owning `band`. |
| `bands` | array | Band id, `cell_log2`, LODs, sections, and assembly role. |
| `styles` | array | Canonical `{ id, feature_type }` assignment. |
| `routing` | object | `min_component_edges` and supported profile names. |
| `chunk_size` | integer | Per-LOD OBCM chunk capacity used by the bake. |

Every LOD belongs to exactly one band. The `nav` and `poi` sections each belong
to exactly one band. Roles obey these rules:

- exactly one `core` band carries `nav` and `poi`, and no LODs;
- at most one `coarse` band carries LODs and no sections;
- every other band is `geometry`, carries LODs, and has no sections.

These roles named volume-set placement, which OBCA §5 specified and OBCM v14 (#1420) superseded —
one map is one file, so a band's role no longer decides which file its content lands in. The roles
survive as what they always also were: the **partition** of the schema's content, which is why the
rule below is the one that matters and is unchanged. A consumer
MUST reject a band partition that loses or duplicates content.

Style ids are schema data because cell feature headers contain them. A schema
revision change invalidates the entire cell store, even when the OBCM format
number does not change.

## 5. SkinEntry

| Field | Type | Meaning |
| :-- | :-- | :-- |
| `id` | string | Stable kebab-case id. |
| `name`, `description` | string | Non-empty display text. |
| `version` | integer | Skin content version. |
| `marker_color` | integer | RGB565 position-marker color. |
| `styles` | array | One presentation record per schema feature type. |
| `preview` | object | Optional digest-pinned canonical rendering (§5.1). |

A style record contains `feature_type`, `color`, `weight`, `z_index`,
`priority`, `dashed`, and nullable `color2`. A skin MUST cover every
`schema.styles[].feature_type` exactly once and MUST name no other feature type.

A skin MUST NOT carry feature selection, LOD thresholds, simplification,
routing, merge passes, chunk size, or any other geometry-producing setting.
Changing a skin never invalidates a cell because the selected skin is stamped
into the assembled output.

### 5.1 Skin preview

`preview`, when present, contains `url`, exact `bytes`, and lowercase `sha256`
for a PNG image. The image is presentation-only: a consumer MUST NOT use it to
select cells, price a map, or assemble output. Producers SHOULD render every
skin over the same geometry, camera, dimensions, and renderer so the images are
an honest visual comparison. `obc-bake` uses a fixed 240×240 Teningen scene and
the production map renderer.

The object is optional so a conforming generic catalog producer need not carry
a rendering fixture. A consumer that displays it MUST apply the same origin and
digest restrictions as every other pinned artifact (§9).

## 6. RegionEntry and region satellite

A region id is a slash-separated Geofabrik-style id such as
`europe/germany/baden-wuerttemberg`.

| Field | Type | Meaning |
| :-- | :-- | :-- |
| `id`, `name` | string | Stable id and display name. |
| `parent` | string | Optional enclosing region id. |
| `boundary` | object | Simplified display outline (§7). |
| `bytes` | integer | Sum of real cell bytes across all bands. |
| `bytes_by_band` | object | Per-band sums; MUST sum to `bytes`. |
| `cell_count` | object | Per-band cell counts. |
| `partial_cell_count_by_band` | object | Per-band counts of selected cells whose square is not fully covered. |
| `terrain` | object | Optional `{ cell_count, known_empty_count, bytes }` for this region's terrain selection (§13.3). |
| `cells_url` | string | Region satellite URL. |
| `cells_bytes` | integer | Exact satellite byte length. |
| `cells_sha256` | string | Lowercase SHA-256 of the exact satellite bytes. |

`partial_cell_count_by_band` is required and includes zeroes for bands with no
partial cells. Its keys MUST be band ids and each value MUST be no greater than
that band's `cell_count`. There is no redundant aggregate partial count.

The pinned satellite has this shape:

```jsonc
{
  "schema_version": 2,
  "schema_revision": 7,
  "region_id": "europe/switzerland",
  "cells": {
    "coarse": ["20/0301/0263"],
    "fine": ["18/1204/1052"],
    "network": ["18/1204/1052"]
  },
  "terrain": ["19/0600/0526"]
}
```

Cell ids are sorted in each band. The satellite MUST agree with the root's schema
revision and region id. Every named band MUST exist in the schema, and every cell
id MUST exist in that band's pinned cell index.

`terrain` is a separate field rather than a key of `cells`, because `cells` is keyed
by *schema band* and terrain is not one: it has no LOD, no section, no assembly role
and no schema revision. It is absent or empty for a terrain-less catalog. §13.3
specifies it.

The list is stored, not derived from the simplified boundary. That prevents
outline simplification or point-in-polygon differences from silently dropping an
edge cell.

## 7. Boundary

```jsonc
{
  "tolerance_udeg": 2000,
  "rings": [
    [[47500000, 7500000], [47600000, 7500000], [47500000, 7500000]]
  ]
}
```

Coordinates are integer microdegrees in `[latitude, longitude]` order. Every ring
is closed. All rings are interpreted together with the even-odd fill rule, so
multiple exteriors, islands, exclaves, and holes need no separate role marker.

A boundary is presentation-only. It MUST NOT be used to derive the region's cell
set, price, packer bbox, or live coverage. A live selection's coverage outline is
the union of its selected grid squares.

## 8. CellIndexRef and cell index

Each root reference contains:

| Field | Type | Meaning |
| :-- | :-- | :-- |
| `band` | string | Schema band id. |
| `cell_log2` | integer | That band's grid size. |
| `cell_count` | integer | Number of downloadable `cells` entries in the satellite. |
| `known_empty_count` | integer | Cells covered by `known_empty` ranges. |
| `bytes` | integer | Exact satellite byte length. |
| `sha256` | string | Lowercase SHA-256 of its exact bytes. |
| `url` | string | Satellite URL. |

The pinned cell index has this shape:

```jsonc
{
  "schema_version": 2,
  "schema_revision": 7,
  "band": "fine",
  "cells": [
    {
      "id": "18/1204/1052",
      "bytes": 552,
      "sha256": "8e2803c1749d16d151ec22abb5f541b06cfdc7c102b46ce080807f5cf0504f83",
      "url": "https://maps.example.org/cells/fine/1204/1052.8e2803c1749d16d151ec22abb5f541b06cfdc7c102b46ce080807f5cf0504f83.obcm",
      "built_at": "2026-07-30T02:12:55Z",
      "sources": [
        { "extract_id": "europe/switzerland", "snapshot": "2026-07-19" }
      ],
      "partial": false
    }
  ],
  "known_empty": [
    {
      "start": "18/1204/1055",
      "end": "18/1204/1058",
      "built_at": "2026-07-30T02:13:11Z",
      "sources": [
        { "extract_id": "planet", "snapshot": "2026-07-19" }
      ]
    }
  ]
}
```

Artifact entries are sorted by cell id. `id` is `<cell_log2>/<i>/<j>`. `built_at` is
RFC 3339 UTC. Every source records its extract id and `YYYY-MM-DD` snapshot.

There is no stored cell bbox. The id defines the exact grid square, and the
producer MUST verify that the cell's OBCM header bbox equals that square. It also
MUST read `bytes`, `sha256`, and OBCM version from the artifact rather than a
recipe or sidecar.

`partial` is true exactly when the baked sources do not fully cover the cell
square. Consumers MUST expose partial coverage rather than presenting it as
canonical. A canonical cell MUST NOT be replaced by a partial bake of the same
schema revision.

`known_empty` is required; an empty list means the band has no verified-empty
coverage. Each entry is an inclusive run from `start` through `end`. Both ids MUST be
canonical cells of this band, MUST have the same latitude index `i`, and the
runs MUST be sorted by `(i, j)`, non-overlapping, and non-empty. Adjacent runs
with identical `built_at` and `sources` MUST be merged. The inclusive cell total
MUST equal the root reference's `known_empty_count`. An artifact entry and a
known-empty run MUST NOT cover the same `(band, cell)`.

A known-empty cell is canonical zero-byte coverage established against its
recorded source set. It is not a missing cell, a partial cell, or an empty OBCM
artifact. Consumers MUST include its id in selection coverage, region
cross-checks, assembly-bbox calculation, and hole detection, but MUST NOT fetch
or graft an object for it. `cell_count` continues to count downloadable
artifacts only; `known_empty_count` counts the expanded ranges separately.

## 9. URLs and integrity

Object URLs are either absolute HTTP(S) URLs or root-relative paths. A desktop
consumer MUST restrict all satellite and cell requests to the configured catalog
origin; it MUST NOT follow the catalog to an unrelated origin. Plain HTTP is
permitted only for loopback development.

Producers MUST place the pinned object's lowercase SHA-256 immediately before
the URL's final extension. The path is immutable: producers MUST NOT later serve
different bytes at that key and MUST retain an object while a published root may
still reference it. Consumers MUST verify that the URL contains the exact stated
digest in that position and MUST NOT derive a URL from an id or digest themselves.

For every pinned object, consumers MUST:

1. download the complete body;
2. compare its exact byte length;
3. compare lowercase SHA-256;
4. parse or assemble it only after both checks pass.

A mismatch rejects that object and the operation that needed it. A consumer MUST
NOT patch a new satellite into an older root, ignore a failed pin, or provide a
verification bypass.

## 10. Version and lockstep law

Every cell MUST have the OBCM version named by `schema.obcm_version` and the
revision named by `schema.revision`. Producers MUST reject a mixed tree and
assemblers MUST reject mixed inputs.

An OBCM version change or schema revision change requires a complete store
cutover. A skin version change does not. Consumers MUST NOT offer an assembly to
a device whose reader does not accept `schema.obcm_version`.

The schema revision, grid constants, band table, style-id assignment, routing
settings, and chunk size are assembly invariants. Equality is exact; there is no
best-effort compatibility between revisions.

This law is about the **OBCM cell store only**. Terrain objects are not cells of a
band and are not covered by it; §13.2 states their lockstep, and the two are
independent in both directions.

## 11. Publication

A publish MUST make all referenced content available before replacing the root:

1. generate the satellites, `LICENSE.txt`, and root from the verified tree;
2. upload cells, sidecars, schema, skins, previews, regions, `LICENSE.txt`, and satellites;
3. verify that every uploaded object is fetchable at the expected size;
4. replace `catalog.json` last.

A failure before step 4 leaves the previously published root authoritative.
`catalog.json` SHOULD use a short cache lifetime (at most 60 seconds or
revalidation). Digest-addressed cells, satellites, and previews SHOULD use a long
immutable cache lifetime.

Generation is deterministic for a fixed tree and `generated_at`: objects and
entries are sorted by their specified ids, JSON spelling is stable, and the root
pins the exact serialized satellite bytes.

## 12. Consumption and assembly

A consumer first validates the root, then fetches only the region satellites and
band indexes needed by the user's coverage. Regions, drawn boxes, and buffered
route corridors all reduce to cell-id sets. Set union deduplicates overlapping
parts before pricing and downloading.

The displayed byte estimate MUST sum the real `bytes` values of the deduplicated
artifact entries; known-empty cells contribute zero. After digest verification,
artifact bytes, selected known-empty identities, and the selected skin are
passed to the OBCA assembler. Island pruning happens at assembly, seams unify only
exact serialized coordinates, and the resulting mounted map follows
[OBCA_Spec.md](OBCA_Spec.md) §4. (It read "volume set" before OBCM v14 / #1420 made a map one
file; OBCA §5 is superseded.)

The website and desktop app MUST use the same selection arithmetic, verified
cell bytes, skin, and assembler. Host-specific file saving MUST NOT alter the
assembled bytes.

A consumer MAY stream assembler output directly to a connected device instead
of first saving it, and MUST verify the emitted file's announced length before transfer. The web
and desktop builder do so from the assembler's OPFS-backed `Blob`: the map remains disk-backed and
the flat-store v4 client reads it in bounded slices, rather than materialising a country-sized file
in the webview heap. A streaming consumer has no independent digest to compare the bytes against;
delivery is guaranteed by the transport's whole-object CRC-32, which the device verifies before it
commits.
Cancellation or failure MUST abandon the incomplete transfer and MUST NOT leave anything selectable
as a map. (Before OBCM v14 / #1420 this rule sequenced several files and committed a volume-set
manifest last; with one file the store's own commit is the sequencing. The old multi-file producing
path was deleted in FS7.5b2; the current direct path is its one-object flat-store-v4 replacement.)

## 13. Terrain artifacts

Terrain is [OBCT](OBCT_Spec.md) raster on the same OBCA grid, published as a
**second artifact class with its own revision track**. It is not a band, not a
section, and not covered by §10.

The reason is a property of the data rather than a preference. A cell store re-bakes
on every OBCM or schema bump (OBCA §6.3); terrain derives from a DEM that is
re-released on a years cadence. Inside the OBCM lockstep, a routine schema bump would
re-publish hundreds of MiB of byte-identical raster and a rider would re-download it.
Outside it, both stores move when their own inputs move and never otherwise.

A catalog with no `terrain` block is complete and valid. Everything degrades to "no
elevation is known here", which is exactly the behavior of every map before terrain
existed: profiles are flat, the router's ascents are zero, the altimeter has no
reference. Consumers MUST treat an absent block that way and MUST NOT synthesize
elevation from any other source.

### 13.1 The terrain block and its pinned index

```jsonc
"terrain": {
  "dataset_id": "copernicus-glo-30",
  "dataset_version": "2021-1",
  "posting_log2": 9,
  "cell_log2": 19,
  "terrain_revision": 4,
  "attribution": "produced using Copernicus WorldDEM-30 © DLR e.V. …",
  "cell_index": {
    "cell_count": 812,
    "known_empty_count": 37,
    "bytes": 96432,
    "sha256": "…",
    "url": "https://maps.example.org/cells/terrain/index.<sha256>.json"
  }
}
```

| Field | Type | Meaning |
| :-- | :-- | :-- |
| `dataset_id` | string | Kebab-case id of the source dataset. |
| `dataset_version` | string | Its release identity. Opaque: compared for equality, never parsed. |
| `posting_log2` | integer | `log2(P)` of the sample lattice, µdeg (OBCT §1.1), `4 … 16`. |
| `cell_log2` | integer | `log2(S)` of the terrain cell, µdeg, `10 … 28`. Independent of any band's. |
| `terrain_revision` | integer | Monotone content revision of the terrain store, ≥ 1. |
| `attribution` | string | Non-empty source credit (§13.5). |
| `cell_index` | object | The single pinned index: `cell_count`, `known_empty_count`, `bytes`, `sha256`, `url`. |

All are required when the block is present. The pin is the §8 machinery reused whole
— exact byte length, lowercase SHA-256, and a URL carrying that digest immediately
before the final extension — and §9's integrity rules apply to it and to every object
it names, unchanged.

The pinned index:

```jsonc
{
  "schema_version": 2,
  "terrain_revision": 4,
  "dataset_id": "copernicus-glo-30",
  "dataset_version": "2021-1",
  "posting_log2": 9,
  "cell_log2": 19,
  "cells": [
    {
      "id": "19/0600/0527",
      "bytes": 2097188,
      "sha256": "…",
      "url": "https://maps.example.org/cells/terrain/0600/0527.<sha256>.obcd",
      "built_at": "2026-08-01T04:12:55Z"
    }
  ],
  "known_empty": [
    { "start": "19/0600/0530", "end": "19/0600/0534", "built_at": "2026-08-01T04:13:02Z" }
  ]
}
```

Entries are sorted by cell id, which is `<cell_log2>/<i>/<j>` on the terrain grid with
OBCA §1.3's zero padding. There is no bbox and no per-cell source list: the id is the
square, and the provenance is one dataset stated once in the root block rather than
repeated on thousands of entries.

The index carries **no `schema_revision`**. That absence is normative: a terrain cell
does not know which OBCM schema it will be used beside, and a field naming one would
make an OBCM bump rewrite this document.

A producer MUST verify each artifact's own OBCT header against its id before
publishing it (OBCT §4.2): `Posting Log2` and `Cell Log2` MUST equal the block's,
`Cell Rows` and `Cell Cols` MUST both be `1`, and `Cell Min I`/`Cell Min J` MUST equal
the id's `i`/`j`. A published terrain cell is a container whose rectangle is `1 × 1`
(OBCT §4.1); a wider rectangle is an assembly raster and MUST NOT be published as a cell.

### 13.2 The terrain lockstep — the whole rule

> Every terrain cell in one assembly MUST share `(dataset_version, posting_log2,
> cell_log2, terrain_revision)`.

That is the entire rule. In particular:

- An **OBCM version bump MUST NOT invalidate a terrain object.**
- A **schema-revision bump MUST NOT invalidate a terrain object.**
- A **terrain re-bake MUST NOT invalidate an OBCM object.**

A producer MUST reject a terrain store that mixes any of the four keys, and MUST NOT
re-publish objects on one track because the other moved. A skin version, a band table
change, a chunk-size change and a style-id renumbering are all likewise invisible to
terrain.

A terrain re-bake is a complete terrain cutover for the same reason a schema bump is a
complete OBCM cutover: a raster resampled at a new posting or from a new dataset
release does not join at a seam with one that was not.

### 13.3 A region's terrain selection

A region's satellite lists its terrain cell ids in `terrain` (§6), by the same
intersect rule its band lists use, applied to the terrain grid: every terrain cell
whose square the region's coverage polygon touches. Ids are sorted, canonical, and
each MUST be either an artifact entry or inside a known-empty run of the terrain
index.

The root's `RegionEntry.terrain` prices that selection:

| Field | Type | Meaning |
| :-- | :-- | :-- |
| `cell_count` | integer | Downloadable terrain cells selected. |
| `known_empty_count` | integer | Selected squares that are canonically void. |
| `bytes` | integer | Sum of the real `bytes` of the downloadable ones. |

These bytes are **not** part of `bytes` or `bytes_by_band`, which are the OBCM volume
set's per-file projection (OBCA §5.7). A rider may take the map without the raster or
the raster without the map, so the two prices are separate numbers and a consumer MUST
present them separately.

### 13.4 The one coupling, stated and guarded

Network-band cells are baked **sampling OBCT**: the OBCM §8.3 per-edge `Ascent M` is
integrated from the raster at bake time. Those bytes are therefore a function of a
particular terrain revision, and the root records which one:

```jsonc
"network_terrain_revision": 4
```

It is `null`/absent when the cell store was baked with no terrain, whose ascents are
all zero and depend on nothing. Every cell in the store MUST have been baked against
the same value; a store where some cells sampled terrain and others did not is
refused, because half a nav graph integrated from a different surface is a router that
is right nowhere.

The field is at the root and **not** in `SchemaEntry` deliberately: the schema is the
identity of the OBCM store, and a terrain field in it would make a terrain re-bake look
like a schema change to every consumer that compares schemas.

The bake guard MUST check it. When a catalog publishes `terrain` and
`network_terrain_revision` is not that block's `terrain_revision`, the guard MUST fail,
naming **both** revisions and the remedy (re-bake the cells). The failure is real: the
router's baked ascents and the raster the device draws its profile from would be two
different surfaces, and every file would still parse. A generator MAY still produce the
document — an operator has to be able to inspect a drifted store — but it MUST report
the drift.

Note the asymmetry, which is the design: the coupling runs *from* terrain *into* the
cell bake and never back. Nothing about the OBCM store is an input to a terrain bake.

### 13.5 Attribution

`attribution` carries the source dataset's required credit verbatim. Consumers that
display terrain — the map builder, the docs, anything that ships derived rasters —
MUST take the string from the catalog rather than hard-coding it, so a dataset change
carries its own notice with it. A producer MUST NOT publish a terrain block with an
empty `attribution`.

### 13.6 Known-empty terrain

An all-`NODATA` terrain cell — open ocean, or outside the source's coverage — has no
object at all: OBCT §4.3 makes an absent cell and an all-void one answer identically,
so writing 2 MiB of sentinel would buy nothing. The catalog says so instead, with the
same inclusive row runs §8 uses: `start` and `end` MUST be canonical ids of the terrain
grid with the same `i`, runs MUST be sorted by `(i, j)`, non-overlapping and non-empty,
and adjacent runs with identical `built_at` MUST be merged. Their inclusive total MUST
equal `cell_index.known_empty_count`. A square MUST NOT be both an artifact entry and
inside a known-empty run.

A known-empty terrain square is canonical coverage that happens to be void. A consumer
MUST include it in selection coverage and hole detection and MUST NOT fetch an object
for it — the same treatment §8 gives an empty band cell.
