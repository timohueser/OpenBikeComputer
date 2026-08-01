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
4. **One schema, many skins.** Geometry, routing, LODs, and style-id assignment
   are baked once. A skin contains presentation only and is stamped during
   assembly.
5. **Deterministic and loud.** Stable inputs produce stable documents apart from
   the explicit generation timestamp. Missing cells, mixed revisions, malformed
   bands, and digest disagreements are errors.

## 2. Published objects

```text
catalog.json
schema.json
skins/<id>.json
cells/<band>/index.json
cells/<band>/<i>/<j>.obcm
cells/<band>/<i>/<j>.obcm.json
regions/<region_id>/region.json
regions/<region_id>/boundary.poly
regions/<region_id>/cells.json
```

`catalog.json` is the root. A named region is a selection preset, not an OBCM
artifact: its satellite lists cell ids for each band. Cell indexes are keyed by
band rather than only by cell size because two semantic bands MAY use the same
`cell_log2`.

The root and satellites are complete JSON documents. Unknown optional fields MAY
be ignored. Missing required fields, unknown enum values, duplicate ids, unknown
references, unsafe URLs, or invalid ordering MUST reject the containing document.

## 3. Root document

```jsonc
{
  "schema_version": 2,
  "generated_at": "2026-07-30T09:00:00Z",
  "schema": { /* SchemaEntry */ },
  "skins": [ /* SkinEntry, sorted by id */ ],
  "regions": [ /* RegionEntry, sorted by id */ ],
  "cell_index": [ /* CellIndexRef */ ]
}
```

| Field | Type | Meaning |
| :-- | :-- | :-- |
| `schema_version` | integer | MUST equal `2`. |
| `generated_at` | string | RFC 3339 UTC, exactly `YYYY-MM-DDTHH:MM:SSZ`. |
| `schema` | object | The catalog's single `SchemaEntry`. |
| `skins` | array | Non-empty presentation choices, sorted by `id`. |
| `regions` | array | Named selections, sorted by `id`. |
| `cell_index` | array | Exactly one pinned index per schema band. |

All fields are required. `generated_at` is the only wall clock introduced while
generating the catalog and MAY be supplied explicitly for reproducible output.

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

These roles determine volume-set placement as specified by OBCA §5. A consumer
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
| `partial_cell_count` | integer | Selected cells whose square is not fully covered. |
| `partial_cell_count_by_band` | object | Optional additive-v2 per-band split of `partial_cell_count`. |
| `cells_url` | string | Region satellite URL. |
| `cells_bytes` | integer | Exact satellite byte length. |
| `cells_sha256` | string | Lowercase SHA-256 of the exact satellite bytes. |

New v2 producers MUST publish `partial_cell_count_by_band`, including zeroes for
bands with no partial cells. Its keys MUST be band ids, each value MUST be no
greater than that band's `cell_count`, and its values MUST sum to
`partial_cell_count`. Consumers MUST accept an older v2 root that omits the
additive field; without it they cannot distinguish normal coarse-context
partials from detail-band partials until the region satellite and cell indexes
have resolved.

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
  }
}
```

Cell ids are sorted in each band. The satellite MUST agree with the root's schema
revision and region id. Every named band MUST exist in the schema, and every cell
id MUST exist in that band's pinned cell index.

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
| `known_empty_count` | integer | Cells covered by `known_empty` ranges. Optional additive-v2; absent means zero. |
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
      "url": "https://maps.example.org/cells/fine/1204/1052.obcm",
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

`known_empty` is an optional additive-v2 array; its absence means `[]`.
Each entry is an inclusive run from `start` through `end`. Both ids MUST be
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
Consumers of an older v2 root or satellite MUST default the absent additive
fields to zero and an empty array respectively.

## 9. URLs and integrity

Object URLs are either absolute HTTP(S) URLs or root-relative paths. A desktop
consumer MUST restrict all satellite and cell requests to the configured catalog
origin; it MUST NOT follow the catalog to an unrelated origin. Plain HTTP is
permitted only for loopback development.

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

## 11. Publication

A publish MUST make all referenced content available before replacing the root:

1. generate the satellites and root from the verified tree;
2. upload cells, sidecars, schema, skins, previews, regions, and satellites;
3. verify that every uploaded object is fetchable at the expected size;
4. replace `catalog.json` last.

A failure before step 4 leaves the previously published root authoritative.
`catalog.json` SHOULD use a short cache lifetime (at most 60 seconds or
revalidation). Because current cell paths are stable and may be replaced on a
re-bake, cell and satellite responses MUST revalidate rather than be treated as
immutable content-addressed keys.

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
exact serialized coordinates, and the resulting mounted volume set follows
[OBCA_Spec.md](OBCA_Spec.md).

The website and desktop app MUST use the same selection arithmetic, verified
cell bytes, skin, and assembler. Host-specific file saving MUST NOT alter the
assembled bytes.

A consumer MAY stream assembler output directly to a connected device instead
of first saving it. It MUST accept at most one emitted file at a time, verify
that file's announced length and SHA-256 before transfer, preserve assembler
order, and commit the volume-set manifest last. Cancellation or failure after
one or more shards have been staged MUST abandon the incomplete set; it MUST NOT
leave those shards selectable as a map.
