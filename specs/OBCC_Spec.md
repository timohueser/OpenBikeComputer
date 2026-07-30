# OBCC Catalog Manifest Specification (schema_version 1, with the schema_version 2 delta in §11)

OBCC (OpenBikeComputer Catalog) is the **map catalog manifest**: a single JSON
document describing every pre-baked [`OBCM`](OBCM_Spec.md) map a distribution
publishes — what regions exist, in which styles, how big they are, what they cover,
what they were built from, and **which OBCM format version they are**.

> **Two envelope versions live in this document.** §1–§10 define `schema_version 1`, the
> region × preset artifact catalog. **§11 defines `schema_version 2`**, in which the unit of
> baking becomes a grid **cell** ([`OBCA_Spec.md`](OBCA_Spec.md)), named regions become cell-set
> selections, and presets split into a **schema** and **skins**. A consumer implements one or
> both and rejects a `schema_version` it does not implement (§1). Everything §1–§10 says about
> atomicity, timestamps, determinism, and the version law carries into v2 unchanged unless §11
> says otherwise.

It is the contract between the *bakery* (which builds artifacts once, centrally, and
uploads them to object storage) and every *consumer* that hands one to a device (the
hosted static site, the desktop app). It is the only file a consumer reads before it
knows what exists.

This document is normative. Its code authority is
[`host/obc-pack/src/catalog.rs`](../host/obc-pack/src/catalog.rs), which is also
the only sanctioned producer; the generated JSON Schema is checked in at
[`host/obc-pack/schema/catalog.schema.json`](../host/obc-pack/schema/catalog.schema.json)
and a worked example at
[`host/obc-pack/schema/catalog.example.json`](../host/obc-pack/schema/catalog.example.json).

Unlike [`OBCM`](OBCM_Spec.md) / [`OBCR`](OBCR_Spec.md) / [`OBCU`](OBCU_Spec.md), this
format is JSON rather than bytes: nothing on the device ever reads it, and every
consumer is a browser or a desktop host. It sits beside them because it is the same
*kind* of thing — a normative, versioned contract between independently released
components, where a producer and a consumer that disagree produce a broken device.

The key words MUST, MUST NOT, SHOULD and MAY are to be interpreted as in RFC 2119.

## Design principles

1. **The artifact describes itself.** Every fact that can be read out of the `.obcm`
   bytes — its OBCM version, its coverage box, its size, its digest — is read out of
   the bytes, never taken from the build recipe. A manifest that reports what the
   builder *meant* to produce is worse than no manifest, because it is trusted.
   Facts the bytes cannot state (the region's name, when it was built, which extract
   and which preset revision it came from) are **recorded at bake time** and read back
   verbatim, never re-derived from whatever the tree looks like when the manifest is
   generated. Re-derivation is the same failure wearing a different coat: it makes a
   stale artifact describe something it is not.
2. **Knowable before the download.** A country-scale artifact is hundreds of
   megabytes. Everything a consumer needs to decide *whether to fetch it at all* —
   format version, size, coverage, freshness — is in the manifest.
3. **One document, all or nothing.** The manifest is a single self-delimiting JSON
   document written atomically. There is no state in which half of it is usable.
4. **Deterministic.** The same tree produces byte-identical output. Ordering is
   content-derived, not filesystem-derived, and the wall clock enters exactly one
   field.
5. **Loud, not lenient.** A malformed tree fails the bake. A region that quietly did
   not build is indistinguishable, to a user, from a region deliberately not offered
   — so the generator refuses to guess.

---

## 1. The document

```jsonc
{
  "schema_version": 1,
  "generated_at": "2026-07-26T09:00:00Z",
  "presets": [ /* PresetEntry, sorted by id */ ],
  "artifacts": [ /* ArtifactEntry, sorted by (region_id, preset_id) */ ]
}
```

| Field | Type | Description |
| :-- | :-- | :-- |
| `schema_version` | integer | This document's envelope version. `1`. |
| `generated_at` | string | When the manifest was generated (§5). |
| `presets` | array | Every style preset the catalog offers (§2). Sorted by `id`. |
| `artifacts` | array | Every published artifact (§3). Sorted by `(region_id, preset_id)`. |

All four fields are REQUIRED. Consumers MUST reject a document whose
`schema_version` they do not implement (§7); producers MUST NOT reuse
`schema_version` `1` for an incompatible shape.

Adding a new OPTIONAL field is **not** a breaking change and MUST NOT bump
`schema_version`; consumers MUST ignore fields they do not recognise.

### Why one flat `artifacts` array

`region_name` repeats across the artifacts of one region rather than being hoisted
into a `regions[]` level. That redundancy is deliberate: a consumer's primary
operation is "filter the artifact list", and every entry is then self-contained. The
producer guarantees consistency — two artifacts in the same region directory that
disagree about `region_name` fail the bake (§4).

### What is deliberately absent

- **No top-level `obcm_version`.** §6 requires every artifact in a catalog to carry
  the same version, so a top-level copy could only ever be redundant — and a
  consumer must check per-artifact regardless, because that is the granularity at
  which it refuses a download.
- **No in-band digest of the manifest itself.** A second file (`catalog.json.sha256`)
  would double the failure modes of a publish swap: a consumer would have to decide
  which of two independently-cached objects is authoritative. One document, one
  atomic swap, and TLS + a full-body parse instead (§7).

## 2. `PresetEntry`

A style preset: one of the small number of looks the catalog is baked in.

| Field | Type | Required | Description |
| :-- | :-- | :-- | :-- |
| `id` | string | yes | Stable id, `^[a-z0-9]+(-[a-z0-9]+)*$`, e.g. `default`. |
| `name` | string | yes | Display name, e.g. `Bikepacking`. Non-empty. |
| `description` | string | yes | One line describing what the preset draws. Non-empty. |
| `version` | integer | yes | The preset's **current** content version; bumped when the styling changes. |
| `preview` | string | no | Reference to a rendered preview asset, resolved like `url` (§3). Absent until a preview exists. |

`version` describes the preset **as it is now** — what a fresh bake would produce. It
is *not* a claim about the artifacts: an artifact states what it was built with in its
own `preset_version` (§3), and the two are allowed to differ.

`presets` is sorted by `id`. Display order is the consumer's decision, not the
manifest's.

Every preset listed MUST be used by at least one artifact — a catalog MUST NOT
advertise a preset it cannot serve.

## 3. `ArtifactEntry`

One published `.obcm` file: a (region, preset) pair.

| Field | Type | Description |
| :-- | :-- | :-- |
| `region_id` | string | Slash-separated region id mirroring the Geofabrik hierarchy, `^[a-z0-9]+(-[a-z0-9]+)*(/[a-z0-9]+(-[a-z0-9]+)*)*$`, e.g. `europe/switzerland`. |
| `region_name` | string | Human-readable region name, e.g. `Switzerland`. |
| `preset_id` | string | The preset it was built with; matches a `presets[].id`. |
| `preset_version` | integer | The preset's `version` **recorded by the bake job that produced this artifact** — never re-derived from the tree's current preset config. |
| `obcm_version` | integer | OBCM format version, **read from the artifact's header** (§6). |
| `bytes` | integer | Size of the artifact in bytes. |
| `sha256` | string | Lowercase hex SHA-256 of the artifact bytes, `^[0-9a-f]{64}$`. |
| `bbox` | object | Coverage box (§4). |
| `built_at` | string | When the artifact was packed (§5). |
| `source_snapshot` | string | Date of the OSM extract it was packed from, `YYYY-MM-DD`. |
| `url` | string | Where the artifact can be fetched: absolute `https://…`/`http://…`, or root-relative `/…`. |

All fields are REQUIRED.

A **region** is a unit of coverage, and regions nest: `europe/switzerland` and
`europe/switzerland/ticino` may both be baked, independently. A consumer MUST treat
each `region_id` as its own selectable entry and MUST NOT assume a parent's artifact
subsumes a child's, or vice versa.

### `preset_version` — a record of the bake, not a copy of the config

`preset_version` MUST be the version the producing bake job recorded (§8), and MUST NOT
be re-derived from the preset config in the tree when the manifest is generated.

The distinction is load-bearing, not pedantry. **A preset restyle invalidates only that
preset's artifacts**, so unlike an OBCM bump (§6) it does not force a full re-bake — and
with a full bake costing tens of CPU-hours, a partial re-bake is the normal operation.
So the interesting state is routine: a preset moves from version 2 to 3, some regions
are re-baked and some are not. If `preset_version` were copied from the current config
it would equal `presets[].version` by construction, could never signal anything, and
every not-yet-re-baked artifact would silently claim styling it does not have — with no
other field a consumer could use to notice.

Recorded at bake time, the two numbers carry meaning:

| Relation | Means | Consumer |
| :-- | :-- | :-- |
| `preset_version == presets[].version` | current styling | nothing to say |
| `preset_version < presets[].version` | built with an older revision of this preset | MAY surface it as "older styling"; MUST NOT refuse the artifact — it is valid, readable, and complete, just styled the way the preset looked earlier |
| `preset_version > presets[].version` | MUST NOT occur; the catalog is malformed (§7) | reject the document |

A consumer MUST NOT infer anything else from the value — in particular, `preset_version`
says nothing about map data freshness (that is `source_snapshot`) or about whether the
device can read the file (that is `obcm_version`).

Producers: see §8 for what a generator does with each relation.

## 4. `bbox` — coverage, not extract

```jsonc
"bbox": { "min_lat": 43724355, "min_lon": 7409055, "max_lat": 43751930, "max_lon": 7439812 }
```

Four integers in **microdegrees** (1e-6 degrees), copied verbatim from the OBCM
header's global bounding box ([`OBCM_Spec.md` §1](OBCM_Spec.md), bytes 5–20). Ranges:
latitude ±90 000 000, longitude ±180 000 000, with `min_* <= max_*`.

Microdegrees, not degrees, because that is the format's own unit and integers
serialize deterministically; a consumer that wants degrees divides by 1e6.

**This box is what the packed file covers, not the box it was cut from.** The packer
computes it from the packed content, so it is always a little wider than the extract
bbox: completing partially-in-box ways and pulling in coastline and boundary features
pushes it out.

That distinction carries a rule.
[`apps/obc-sim/assets/repack.sh`](../apps/obc-sim/assets/repack.sh) forbids
deriving an *extract* bbox from an existing artifact's header, because self-sourcing
ratchets the box wider on every re-pack — a fixture once grew to 14.5 MB that way.
The rule is about **inputs**; this field is an **output**, and the honest answer to
the only question a catalog consumer asks: *what does this download cover?*

Accordingly: this `bbox` MUST NOT be used as a packer input bbox, and a bakery MUST
keep its extract boxes in its own curated region list rather than reading them back
out of a manifest.

## 5. Timestamps

`generated_at` and `built_at` are RFC 3339 UTC instants in exactly one spelling:

```
YYYY-MM-DDTHH:MM:SSZ
```

Twenty characters. No offsets other than `Z`, no lowercase `z`, no fractional
seconds, no leap seconds. `source_snapshot` is a bare `YYYY-MM-DD` calendar date.

Producers MUST emit this spelling and MUST reject any other; consumers MAY therefore
compare and sort these fields as plain strings. Dates that do not exist
(`2026-02-30`, `2023-02-29`) are rejected.

`generated_at` is the **only** wall-clock read on the generation path. `built_at`
comes from the bake, not from the generator's clock or the artifact's mtime, so
re-running the generator over an unchanged tree changes nothing but that one field
— and passing `--generated-at` explicitly makes the run fully reproducible.

## 6. The version law

> **An OBCM format bump invalidates every baked artifact.**

An OBCM version bump is a hard cut: the reader supports exactly one version
([`OBCM_Spec.md`](OBCM_Spec.md) — *"v10 is the only supported version; earlier maps
get repacked"*). Every `.obcm` in the catalog is therefore unreadable the moment the
format moves, and a catalog is a large, cached, globally-distributed pile of files
that nobody re-checks by hand.

Three mechanisms enforce this, at three different moments:

**(a) At bake time — the generator refuses a mixed or stale catalog.** Every
artifact's `obcm_version` is read from its own 40-byte header, and MUST equal the
version the producing `obc-pack` writes. A tree containing an artifact from any other
version fails generation with the path and both versions. There is no override flag:
a catalog that mixes versions, or is entirely one version behind, is precisely the
failure this rule exists to prevent, and an escape hatch would be used exactly when
it must not be.

**(b) In the repository — the format constant cannot move quietly.**
`catalog.rs` pins the OBCM version it was written against as a literal
(`PINNED_OBCM_VERSION`) and asserts it against `obc_formats::obcm::VERSION`. Moving
OBCM's version therefore breaks the packer's own test suite, and the failure names
the four steps required: re-bake the catalog, republish the manifest, regenerate the
checked-in example, and only then move the pin. A second test regenerates the
checked-in example manifest through the real generator, so the pin cannot be bumped
without the manifest path itself having been re-run.

**(c) At consumption time — the site refuses what the device cannot read.** The
version is in the manifest, so a consumer knows it *before* fetching hundreds of
megabytes. A consumer with a known target firmware MUST NOT offer an artifact whose
`obcm_version` that firmware does not support; it SHOULD show the region as
unsupported with the reason, rather than hiding it or letting the download proceed.
With no device attached, a consumer MAY offer the download and state the version.

(a) and (b) are the producer's problem; (c) is the consumer's. They are independent:
(a) fires when the *bakery* is behind, (c) fires when the *rider's firmware* is
behind. Neither substitutes for the other.

A bakery MUST re-bake on an OBCM version bump. Making that mandatory rather than
remembered is the bakery's own CI guard; this specification's contribution is that
the version is in the manifest, so the guard has something to check.

## 7. Never partially consumed

A manifest is either wholly valid or wholly rejected. This is a property of the
writer *and* the reader, and both halves are required.

**Producers MUST:**

- write the manifest to a temporary file in the **same directory** as the target,
  `fsync` it, and `rename` it into place — atomic on every filesystem this runs on,
  so a concurrent reader sees the complete previous manifest or the complete new one;
- publish to object storage as a single object replacement, never an append or a
  multi-part update a consumer could observe mid-flight, and swap it in only after
  every artifact it references is fetchable;
- emit exactly one JSON document, UTF-8, no BOM, trailing newline.

**Producers SHOULD** serve the manifest with a **short** cache lifetime — a `max-age`
of at most 60 seconds, or a revalidation policy — while the artifacts it references,
which are content-addressed by `sha256` and never rewritten in place, MAY be cached
indefinitely. A consumer cannot compensate for an over-cached manifest: it has no way
to know a newer one exists, so a fresh bake stays invisible for as long as the cache
says. This makes the manifest's *home* a real decision rather than a convenience — a
host that cannot set response headers (GitHub Pages, notably) is not a place to
publish one, and publishing it beside a consumer rather than beside the artifacts
would mean every bake needs that consumer redeployed.

**Consumers MUST:**

- read the **entire** response body before parsing, and parse it as one JSON
  document. A truncated manifest cannot survive that: JSON is self-delimiting, so no
  proper prefix of a valid document parses. Streaming or incremental consumption
  forfeits this guarantee and MUST NOT be used;
- check `schema_version` before reading any other field, and reject the whole
  document on an unrecognised value;
- reject the whole document, retaining any previously cached manifest, if any
  REQUIRED field is missing or malformed — never fall back to a partially-populated
  catalog;
- verify a downloaded artifact against the manifest's `bytes` and `sha256` before
  writing it to a device, and surface a mismatch as an error rather than a corrupt
  file on the rider's card.

## 8. The bake tree

The generator walks a **self-describing** output tree:

```
<tree>/
  presets/
    <preset_id>.json                 the preset's current definition; `_meta` describes it
  regions/
    <segment>/…/<segment>/
      <preset_id>.obcm               the artifact; its directory path is the region id
      <preset_id>.obcm.json          its sidecar
  catalog.json                       the generated manifest (default output path)
```

Per-artifact **sidecars** rather than one central index: a matrix bake runs one job
per (region, preset), and each job writes only its own two files. Nothing has to be
merged, so nothing can race.

A sidecar carries exactly the facts the artifact's bytes cannot state:

```json
{
  "region_name": "Switzerland",
  "preset_version": 3,
  "built_at": "2026-07-20T02:14:07Z",
  "source_snapshot": "2026-07-19"
}
```

All four are REQUIRED, and **unknown keys are rejected** — a sidecar is machine-
written, so a misspelled key is a bug, not metadata riding along. (This differs
deliberately from the packer's *config* parser, which ignores unknown keys so
user-authored tooling metadata can ride along.)

Every field in a sidecar is a **record of the bake**, fixed at the moment the artifact
was written. A generator MUST NOT re-derive any of them from the tree's current state.
`preset_version` is the one where that rule has teeth: it MUST be the `_meta.version`
of the config the bake job actually packed with, which is why a bake job writes it
rather than a generator reading it back off `presets/`.

`presets/<preset_id>.json` is the **current** definition of the preset. The catalog
reads only its `_meta` block (`id`, `name`, `description`, `version`, and the optional
`preview`), and `_meta.id` MUST match the filename. It is a description, not a record:
the restyle that bumps it does not touch artifacts already baked, which is precisely
the state `preset_version` exists to make visible.

**A generator MUST compare each artifact's recorded `preset_version` against that
config and act on the three cases in §3:**

- equal — nothing to report;
- artifact **lower** — a lagging partial re-bake. Report it as a **warning** and publish
  it. It MUST NOT be fatal: unlike an OBCM bump, the artifact is fully readable, and
  refusing the catalog would convert a cosmetic lag into total unavailability for every
  region not yet re-baked — making a full re-bake a precondition for any publish at all.
  Whether a given publish tolerates a lag is the bakery's policy, taken from that
  warning list; it is not the format's decision;
- artifact **higher** — the tree's preset config is older than an artifact built from
  it (a reverted or wrongly-copied config). The catalog cannot describe that artifact's
  styling at all, so generation MUST fail.

Walk rules:

- a directory under `regions/` that contains `*.obcm` files **is** a region; its path
  below `regions/` joined with `/` is the `region_id`. A region directory MAY also
  contain sub-region directories;
- each path segment and each preset id MUST be lowercase kebab-case;
- every `*.obcm` MUST have a matching `*.obcm.json`, and vice versa;
- entries whose name begins with `.` are ignored (`.DS_Store`, `.gitkeep`);
- **any other entry fails generation.** Silence here would let a mis-named artifact
  read to a user as "region not covered", which is indistinguishable from a
  deliberate curation choice;
- `url` is `<base-url>/<path of the artifact relative to the tree root>`, so the
  published layout is the tree layout;
- a region that is missing some presets is reported as a **warning**, not an error: a
  partial bake is legitimate, but it must never be silent.

## 9. Generating

```
obc-pack catalog <bake-tree> --base-url <url> [--out <path>|-] [--generated-at <ts>]
obc-pack schema --catalog        # print this format's JSON Schema
```

- `--base-url` is REQUIRED: it is where the tree gets published, and there is no
  sensible default to guess.
- `--out` defaults to `<tree>/catalog.json` and is written atomically (§7). `--out -`
  writes to stdout for inspection and is **not** a publish path — stdout cannot be
  swapped in atomically.
- `--generated-at` sets `generated_at`; absent, the system clock is read. CI SHOULD
  pass it so a re-run of the same bake is byte-reproducible.

The generator is a pure function of (tree, options): directory entries are sorted
before use, so output never depends on filesystem enumeration order, and running it
twice over one tree is byte-identical.

## 10. Where this lives

- Generator, types, schema, and the version-law tests:
  [`host/obc-pack/src/catalog.rs`](../host/obc-pack/src/catalog.rs)
- Generated JSON Schema (checked in for consumers):
  [`host/obc-pack/schema/catalog.schema.json`](../host/obc-pack/schema/catalog.schema.json)
- Worked example manifest:
  [`host/obc-pack/schema/catalog.example.json`](../host/obc-pack/schema/catalog.example.json)
- The `schema_version 2` producer of §11, and the `.poly` → outline reduction of §11.8:
  [`host/obc-pack/src/catalog/v2.rs`](../host/obc-pack/src/catalog/v2.rs) and
  [`host/obc-pack/src/catalog/boundary.rs`](../host/obc-pack/src/catalog/boundary.rs), generated
  by `obc-pack catalog <cell-tree> --base-url <url> --v2`
- Generated v2 JSON Schema — root plus both satellite documents, under `$defs`:
  [`host/obc-pack/schema/catalog.v2.schema.json`](../host/obc-pack/schema/catalog.v2.schema.json)
- Worked v2 examples, one per document:
  [`catalog.v2.example.json`](../host/obc-pack/schema/catalog.v2.example.json),
  [`cell-index.v2.example.json`](../host/obc-pack/schema/cell-index.v2.example.json),
  [`region-cells.v2.example.json`](../host/obc-pack/schema/region-cells.v2.example.json)
- The OBCM header the version and bbox are read from:
  [`OBCM_Spec.md` §1](OBCM_Spec.md); its code authority
  [`firmware/obc-formats/src/obcm.rs`](../firmware/obc-formats/src/obcm.rs)
- The shipped style presets: [`builder/presets/`](../builder/presets)
- The cell grid, assembly contract, and volume sets the `schema_version 2` catalog of §11
  publishes: [`OBCA_Spec.md`](OBCA_Spec.md)

---

## 11. `schema_version 2` — cells, skins, and cell-set regions

Epic #1016 changes what a catalog *contains*. In v1 the bakery pre-bakes a matrix of
(region × preset) whole-map artifacts, so every combination a rider might want has to be on the
shelf and every new preset multiplies the store. In v2 the bakery bakes **grid cells** once, and
the map a rider downloads is an **assembly** of cells built client-side. Combinations stop being
a storage problem; styling stops being a bake at all.

This section is the delta. It reuses v1's principles verbatim — the artifact describes itself,
knowable before the download, all-or-nothing, deterministic, loud not lenient — and its
timestamp (§5), atomicity (§7), and generation (§9) rules. The byte-level meaning of a cell, a
band, an assembly, and a volume set is [`OBCA_Spec.md`](OBCA_Spec.md); this section only
publishes them.

### 11.1 The documents

A v2 catalog is a **small root document plus digest-pinned satellites**:

```
catalog.json                        the root (§11.2) — schema, skins, regions, cell-index refs
cells/<band>/index.json             one per band: every published cell of that band (§11.6)
regions/<region_id>/cells.json      one per named region: its cell ids per band (§11.7)
cells/<band>/<i>/<j>.obcm           the cell artifacts themselves
```

Object paths are keyed by **band**, not by `log2(S)`: two bands may share a cell size — `fine`
and `network` are both `2^18` at the v1 band table
([`OBCA_Spec.md` §1.5](OBCA_Spec.md)) — so a `<log2>`-keyed path is not a function of
(band, cell) and the two bands' indices and artifacts would collide. Every `url` is published
explicitly anyway (§11.6), with the band's `cell_log2` beside it, so nothing a consumer does
depends on the spelling of a path.

v1 kept everything in one document because everything fit. A cell catalog does not: DACH is
~2 000–4 000 cells across four bands, and a planet-scale store is two orders of magnitude more.
Rather than weaken v1's *all-or-nothing* principle, v2 preserves it per document and **pins each
satellite by `bytes` + `sha256` from the root**, so a consumer that has read a valid root and a
matching satellite has exactly the same guarantee it had with one file: it either has the whole,
consistent thing or it has nothing. A satellite whose digest does not match the root MUST be
rejected, and the root retained rather than patched.

Cache policy follows §7: the **root** is short-cached (≤ 60 s `max-age` or revalidation), while
satellites and cell artifacts are content-addressed by `sha256`, never rewritten in place, and MAY
be cached indefinitely.

### 11.2 The root document

```jsonc
{
  "schema_version": 2,
  "generated_at": "2026-07-30T09:00:00Z",
  "schema": { /* SchemaEntry (§11.3) */ },
  "skins": [ /* SkinEntry, sorted by id (§11.4) */ ],
  "regions": [ /* RegionEntry, sorted by id (§11.5) */ ],
  "cell_index": [ /* CellIndexRef, sorted by cell_log2 descending (§11.6) */ ],
  "artifacts": [ /* OPTIONAL — v1 ArtifactEntry, §11.9 */ ]
}
```

| Field | Type | Required | Description |
| :-- | :-- | :-- | :-- |
| `schema_version` | integer | yes | `2`. |
| `generated_at` | string | yes | §5 spelling; the only wall clock on the generation path. |
| `schema` | object | yes | The catalog's **single** schema (§11.3). |
| `skins` | array | yes | Every skin offered (§11.4). Non-empty, sorted by `id`. |
| `regions` | array | yes | Named selections (§11.5). Sorted by `id`. |
| `cell_index` | array | yes | One entry per band (§11.6). |
| `artifacts` | array | no | Legacy whole-region artifacts during migration (§11.9). |

There is **exactly one** `schema`, not an array. That is the D2 decision made concrete: the
hosted catalog carries the 7-LOD bikepacking ladder and nothing else, because it is the ladder
tested to render inside the device's RAM and map-complexity budget. A second schema would make
the whole cell store exist twice, and a superset schema would make every map carry complexity the
device cannot honour. Custom schemas remain a desktop local-bake affair and never appear in a
hosted catalog.

### 11.3 `SchemaEntry`

Everything a consumer needs to price a selection and an assembler needs to stamp a skin, without
any out-of-band constant.

| Field | Type | Description |
| :-- | :-- | :-- |
| `id` | string | Stable id, `^[a-z0-9]+(-[a-z0-9]+)*$`, e.g. `bikepacking`. |
| `revision` | integer | Monotone content revision. Every cell states the revision it was baked at; a bump invalidates the whole store ([`OBCA_Spec.md` §6.3](OBCA_Spec.md)). |
| `name`, `description` | string | Display strings, non-empty. |
| `obcm_version` | integer | OBCM format version, **read from the cells' own headers** (§6). Every cell MUST agree. |
| `grid` | object | `{ "origin_udeg": -268435456, "world_side_udeg": 536870912 }` — [`OBCA_Spec.md` §1.1](OBCA_Spec.md)'s constants, restated so no consumer has to hard-code them. |
| `lods` | array | One per ladder level, coarsest first: `{ "index", "max_mpp", "band" }`. `max_mpp` is `null` for the `+inf` coarsest level. |
| `bands` | array | `{ "id", "cell_log2", "lods": [int], "sections": [string], "role": "core" \| "coarse" \| "geometry" }`. `sections` may contain `"nav"` and `"poi"`. The `role` names which file of a volume set the band's content is assembled into ([`OBCA_Spec.md` §5.1](OBCA_Spec.md)). |
| `styles` | array | The **canonical style-id assignment**: `{ "id", "feature_type" }` in schema order. |
| `routing` | object | `{ "min_component_edges": int, "profiles": [string] }` — the island-prune threshold is schema data, never skin data. |
| `chunk_size` | integer | The per-LOD chunk capacity bound the cells were written with (`OBCM_Spec.md` §3). |

Producers MUST satisfy [`OBCA_Spec.md` §1.2](OBCA_Spec.md)'s partition rule — every ladder LOD in
exactly one band, and the nav and POI sections in exactly one band. A consumer MUST reject a schema
that violates it: a LOD in no band is a map that is blank at that zoom, and a LOD in two bands is a
map that carries it twice.

The `role` values are also constrained, because they decide which physical file each band's bytes end
up in ([`OBCA_Spec.md` §5.1](OBCA_Spec.md)):

- exactly **one** band has `"role": "core"`, it carries the nav and POI `sections`, and it carries
  **no** `lods` — the core file is the one file of a set that cannot be split by bbox, so no geometry
  may be assembled into it;
- **at most one** band has `"role": "coarse"`, and it carries `lods` and no `sections` — its content
  becomes the single whole-assembly coarse shard that keeps a zoomed-out viewport a one-file read;
- every remaining band has `"role": "geometry"`, with `lods` and no `sections`.

A consumer MUST reject a schema that breaks any of these — a `core` band carrying a LOD would put
unsplittable bytes in the file whose headroom is the design's hard limit
([`OBCA_Spec.md` §5.7](OBCA_Spec.md)).

`styles` is the load-bearing field for the skin split. `obc-pack` numbers feature types `1`-based
in config document order and those ids are referenced by **every feature header in every chunk**
(`OBCM_Spec.md` §2, §5.2), so the assignment is part of the cells' bytes and therefore part of the
schema, not of any skin.

### 11.4 `SkinEntry`

| Field | Type | Required | Description |
| :-- | :-- | :-- | :-- |
| `id` | string | yes | `^[a-z0-9]+(-[a-z0-9]+)*$`, e.g. `default`. |
| `name`, `description` | string | yes | Display strings, non-empty. |
| `version` | integer | yes | The skin's content version. |
| `marker_color` | integer | yes | RGB565 user-position marker color (`OBCM_Spec.md` §1). |
| `styles` | array | yes | One entry per `schema.styles` feature type: `{ "feature_type", "color", "weight", "z_index", "priority", "dashed", "color2" }`. `color2` is `null` when absent. |
| `preview` | string | no | Rendered preview asset, resolved like a `url`. |

Skins are **inlined in the root**, not referenced: one is ≈ 2 KB, the builder needs all of them at
once to draw a picker, and an assembler needs the chosen one at assembly time. A skin MUST cover
every `feature_type` in `schema.styles` and MUST NOT name one the schema lacks — a missing style
would ship a map with an invisible layer, and an unknown one is a stale skin claiming a feature
that no longer exists.

> **What v2 deletes.** v1's `preset_version` (§3) existed because a restyle invalidated baked
> artifacts and a partial re-bake left some of them a revision behind. In v2 a skin is stamped at
> **assembly** time onto ~2 KB of the output ([`OBCA_Spec.md` §4.7](OBCA_Spec.md)), so no artifact
> can be a revision behind and there is nothing for the field to say. The whole
> lagging-artifact apparatus of §3 and §8 disappears, and with it the class of bug where a map
> silently claims styling it does not have.

### 11.5 `RegionEntry`

A named region is no longer an artifact — it is a **selection preset**: a boundary to draw and a
cell set to fetch.

| Field | Type | Required | Description |
| :-- | :-- | :-- | :-- |
| `id` | string | yes | Slash-separated, same grammar as v1's `region_id`, e.g. `europe/switzerland`. |
| `name` | string | yes | Display name. |
| `parent` | string | no | The enclosing region's `id`, when the curation nests. |
| `boundary` | object | yes | Simplified outline (§11.8). |
| `bytes` | integer | yes | Total bytes of every cell in this region's cell set, across all bands. |
| `bytes_by_band` | object | yes | Those bytes split per band: `{ "<band_id>": integer }`, summing to `bytes`. |
| `cell_count` | object | yes | Cells per band: `{ "<band_id>": integer }`. |
| `partial_cell_count` | integer | yes | How many of those cells are `partial` (§11.6). `0` for fully covered curation. |
| `cells_url` | string | yes | Where the region's cell-id list lives (§11.7). |
| `cells_bytes` | integer | yes | Size of that document. |
| `cells_sha256` | string | yes | Its digest, `^[0-9a-f]{64}$`. |

`bytes`, `bytes_by_band`, `cell_count`, and `partial_cell_count` sit in the **root** on purpose: they
are what a builder needs to price a region and to warn about coverage gaps, and pricing must not cost
a second round trip. That is v1's *knowable before the download* principle applied to a selection
rather than to a file.

`bytes_by_band` is what makes that pricing **per file** rather than merely per set. A volume set's
roles partition by band ([`OBCA_Spec.md` §5.1](OBCA_Spec.md)), so the `core` band's bytes are the core
file's bytes, the `coarse` band's are the coarse shard's, and the rest are the geometry shards'. The
core is the one file with a hard ceiling it can realistically approach, and
[`OBCA_Spec.md` §5.7](OBCA_Spec.md) requires a consumer to refuse an over-ceiling selection *before*
downloading anything — which is only arithmetic if the split is published here.

Regions still nest and a consumer MUST NOT assume a parent's cell set is the union of its
children's, or vice versa — curation decides both independently. Unlike v1, though, overlap is now
free: two regions that share ground share **the same cells**, and the store pays for them once.
That is the epic's headline saving and the reason `europe/germany` alongside its sixteen
Bundesländer stops costing double.

### 11.6 `CellIndexRef` and `CellEntry`

One `cell_index` entry per band in the schema:

| Field | Type | Description |
| :-- | :-- | :-- |
| `band` | string | The band's `id` from `schema.bands`. |
| `cell_log2` | integer | Its cell size, `log2(µdeg)`; matches the band. |
| `cell_count` | integer | Cells in the referenced document. |
| `bytes` | integer | Size of the referenced document. |
| `sha256` | string | Its digest. |
| `url` | string | Where it lives, resolved like v1's `url` (§3). |

Each referenced document is:

```jsonc
{
  "schema_version": 2,
  "schema_revision": 7,
  "band": "fine",
  "cells": [ /* CellEntry, sorted by (i, j) */ ]
}
```

A `CellEntry`:

| Field | Type | Description |
| :-- | :-- | :-- |
| `id` | string | Canonical cell id, `^\d{1,2}/\d{4,}/\d{4,}$` ([`OBCA_Spec.md` §1.3](OBCA_Spec.md)). |
| `bytes` | integer | Size of the cell artifact. |
| `sha256` | string | Its digest. |
| `url` | string | Where to fetch it. |
| `built_at` | string | §5 timestamp, recorded by the bake job. |
| `sources` | array | `[{ "extract_id": "europe/switzerland", "snapshot": "2026-07-19" }]`, sorted by `extract_id`. |
| `partial` | boolean | `true` iff the sources do not fully cover the cell's square ([`OBCA_Spec.md` §3.7](OBCA_Spec.md)). |

**There is no `bbox` on a cell entry, and that is deliberate.** A cell's coverage box is *exactly*
its grid square, which the `id` determines to the microdegree, and [`OBCA_Spec.md` §3.1](OBCA_Spec.md)
requires the artifact's own OBCM header bbox to equal it. A stored copy could only ever agree
(redundant) or disagree (a lie), so the generator instead **verifies** header bbox == cell square at
bake time and fails the bake on a mismatch. This is v1 §4's "the artifact describes itself" with the
same intent and a stronger instrument: the *identifier* describes it, and the bytes are checked
against the identifier.

`partial` is the D3 guard made publishable. A consumer MUST NOT present a partial cell as canonical
coverage; the builder shows the affected ground as a warning **inside** the selection rather than as
covered. A generator MUST NOT publish a canonical cell and a partial cell for the same `id` at the
same `schema_revision`, and MUST replace a partial cell when a covering source appears.

### 11.7 A region's cell list

```jsonc
{
  "schema_version": 2,
  "schema_revision": 7,
  "region_id": "europe/switzerland",
  "cells": { "coarse": ["20/0301/0263", …], "mid": […], "fine": […], "network": […] }
}
```

Cell ids only — every other fact is in the band's cell index (§11.6), keyed by the same id. Ids are
sorted within each band. A consumer MUST reject a region cell list naming a cell absent from the
band's index, or naming a band absent from the schema.

The list is **stored, not derived from the boundary.** `boundary` is a *simplified* polygon for
drawing; deriving a cell set from it would let a simplification error drop an edge cell, and a
dropped fine cell is a silent hole in street detail. Deriving would also make two consumers with
different point-in-polygon edge handling disagree about what a region *is*. The bakery knows the
answer exactly, so it publishes it.

### 11.8 `boundary` — an outline to draw, not a set to compute

```jsonc
"boundary": {
  "tolerance_udeg": 2000,
  "rings": [ [ [45720000, 5810000], [45730000, 5830000], … ] ]
}
```

| Field | Type | Description |
| :-- | :-- | :-- |
| `tolerance_udeg` | integer | The simplification tolerance the outline was reduced at. |
| `rings` | array | One or more closed rings of `[lat, lon]` integer **microdegree** pairs, first ring exterior, the rest holes. First and last point of each ring are equal. |

Microdegrees and `[lat, lon]` order, matching v1 §4 and the OBCM header, so nothing in the family
mixes conventions. A few KB per region: baked into the catalog because a region has to render as
the outline a user expects, and deriving one from a cell set would draw a staircase instead of a
border.

The outline is **presentation only**. It MUST NOT be used to compute a cell set (§11.7), to price a
selection (§11.5), or as a packer input bbox — the last for exactly v1 §4's reason.

The **coverage outline** the builder draws for a live selection is a different object and is not in
the catalog: it is the union of the selected cells' squares, computed client-side, and it is drawn
honestly as its true stair-edged shape. Regions render as borders; coverage renders as coverage.

### 11.9 Migration, and the version law

`artifacts` MAY appear in a v2 root with exactly v1 §3 semantics, so a bakery can keep serving
whole-region artifacts while a cell store is filled. When present it MUST satisfy v1's rules
including §6's single-`obcm_version` requirement, and a v2 consumer MAY ignore it entirely. A
catalog that has finished migrating omits it. `presets` MUST NOT appear in a v2 root — a preset is
no longer a thing that exists; it is a schema and a skin.

The version law (§6) extends rather than changes:

- every cell's `obcm_version` is read from its own 40-byte header and every cell MUST agree with
  `schema.obcm_version`; a mixed tree fails generation, with no override flag;
- a **schema-revision** bump is the same kind of hard cut as an OBCM bump, because assembly copies
  chunk bytes between files and that is only meaningful within one revision
  ([`OBCA_Spec.md` §6.3](OBCA_Spec.md)). A generator MUST refuse a tree mixing revisions, and an
  assembler MUST refuse a mixed input set;
- at consumption time a consumer MUST NOT offer an assembly to a device whose reader does not
  accept `schema.obcm_version`, and SHOULD say so with the reason rather than hide the coverage.

One thing the law no longer has to cover: a **skin** change invalidates nothing. It is not baked.
