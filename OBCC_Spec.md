# OBCC Catalog Manifest Specification (schema_version 1)

OBCC (OpenBikeComputer Catalog) is the **map catalog manifest**: a single JSON
document describing every pre-baked [`OBCM`](OBCM_Spec.md) map a distribution
publishes — what regions exist, in which styles, how big they are, what they cover,
what they were built from, and **which OBCM format version they are**.

It is the contract between the *bakery* (which builds artifacts once, centrally, and
uploads them to object storage) and every *consumer* that hands one to a device (the
hosted static site, the desktop app). It is the only file a consumer reads before it
knows what exists.

This document is normative. Its code authority is
[`firmware/obc-pack/src/catalog.rs`](firmware/obc-pack/src/catalog.rs), which is also
the only sanctioned producer; the generated JSON Schema is checked in at
[`firmware/obc-pack/schema/catalog.schema.json`](firmware/obc-pack/schema/catalog.schema.json)
and a worked example at
[`firmware/obc-pack/schema/catalog.example.json`](firmware/obc-pack/schema/catalog.example.json).

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
[`firmware/obc-sim/assets/repack.sh`](firmware/obc-sim/assets/repack.sh) forbids
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
  [`firmware/obc-pack/src/catalog.rs`](firmware/obc-pack/src/catalog.rs)
- Generated JSON Schema (checked in for consumers):
  [`firmware/obc-pack/schema/catalog.schema.json`](firmware/obc-pack/schema/catalog.schema.json)
- Worked example manifest:
  [`firmware/obc-pack/schema/catalog.example.json`](firmware/obc-pack/schema/catalog.example.json)
- The OBCM header the version and bbox are read from:
  [`OBCM_Spec.md` §1](OBCM_Spec.md); its code authority
  [`firmware/obc-formats/src/obcm.rs`](firmware/obc-formats/src/obcm.rs)
- The shipped style presets: [`packer/presets/`](packer/presets)
