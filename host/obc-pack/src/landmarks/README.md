# Offline landmark content

Build the host command from the repository root:

```sh
cargo build -p obc-bake --locked
tools/obc fixtures sync assistant-inputs
target/debug/obc-bake landmarks \
  --snapshot "$HOME/.cache/openbikecomputer/fixtures/by-id/assistant-wiki/manifest.json" \
  --boundary "$HOME/.cache/openbikecomputer/fixtures/by-id/assistant-wiki/regions.geojson" \
  --out .artifacts/landmarks
```

Use an empty output directory. The command uses local files only. The example package contains
four review sites; it does not establish full Switzerland coverage. The output copies the source
coverage declaration and reports omissions separately from usable content.

The input is the schema 1 source manifest used by `assistant-wiki`. Each raw response has a relative
path, URL, byte count and SHA-256 digest. Entity responses are `entities/QID.json`; raw parent-class
responses are in `classes/`. Article captures identify a language, title, revision, exact revision
URL, query JSON path and rendered HTML path. Image captures identify a P18 or Wikipedia lead source,
Commons metadata and original image path. The compiler verifies registered source bytes before it
selects sites. It derives coordinates and types from raw entities, not the manifest's display data.
Raw `wbgetentities` class responses can contain an explicit redirect. The compiler preserves
the edge to the canonical class before it follows parent classes, so redirected category or
exclusion roots retain their meaning.

The boundary is GeoJSON Polygon, MultiPolygon or a FeatureCollection of these geometries. Border
points are included. Country claims do not select sites. `policy.json` owns category roots and
exclusions. Claims use Wikidata best-rank semantics: preferred claims when present, otherwise normal
claims; deprecated claims are excluded. Every usable article in the UI languages (en, de, fr, es) is retained. The shared mapping is
`specs/content-languages.json`, checked against the device language order. Irish and Italian are
not capture languages. The compiler takes no map-wide language argument.

`content.json` is an internal schema 2 manifest for map serialization. It contains:

- Input and policy SHA-256 digests. Input identity includes the source manifest and boundary bytes.
  Policy identity includes the category file, extraction code, dependency lock, image recipe and
  supported-language mapping and locale rules. A separate category-policy digest lets acquisition check its discovery roots.
- Candidate QIDs before article selection. A source manifest with empty article/image lists can use
  the same compiler to select which assets to acquire.
- Counts for captured sites, candidates, usable text, photos and raw photo bytes. A null approach
  count means that the OSM approach join has not run.
- Records sorted by QID, with category 1–6, display coordinate, default language, fallback-source
  QIDs, and every usable language variant. Each variant has its language, at most four text pages,
  and its own article attribution. Colocated QIDs remain separate records.
- Separate article and photo source, revision, license, exact original notices and readable Sources
  pages. Each asset permits at most 8 KiB of attribution; the pair permits at most 256 pages.
- Omissions with QID, asset and reason. A rejected photo leaves usable text available.

The capture also pins raw Wikidata entities under `locales/`. For a local fallback, the compiler
follows the place's best-rank P131 administrative chain and reads P37 official-language claims at
the nearest depth with usable text. It then checks P17 country entities and their P37 claims.
Traversal is limited to eight levels and 64 administrative entities per place; country entities
are captured separately. Cycles, missing claims, and unsupported local languages cannot reject
an otherwise usable article. Ties and final fallbacks use the shared UI language order. The
record's `fallback_sources` identifies the locale QIDs used; an empty list means the final UI-order
fallback. The source manifest pins their bytes and URLs. This uses the place's captured location
claims, not the map rectangle or source merge order. It does not claim finer local knowledge than
those claims supply.

All variants share one photo. P18 images precede the union of usable article lead images, in
normalized filename order. Selection does not depend on device language. Reject a photo if its
credits cannot fit beside every retained article's credits; retain those articles. The internal
article bundle uses relative offsets, so assembly can copy all languages without changing them.

CC BY and CC BY-SA photos require a nonempty captured Artist identity. A generic source credit
such as "Own work" does not identify the creator. Missing or empty Artist metadata produces
`photo_creator_missing`; the compiler does not infer an author from a filename or linked page.
CC0 does not require this field. Supplied notices still pass the separate representation limits.

Each photo file contains exactly 51,840 row-major bytes: 216 columns by 240 rows, one RGB222 pixel
per byte (`00RRGGBB`). The host applies orientation, Lanczos3 fit, white padding and the fixed 4×4
ordered dither. The manifest records its path, size and digest. The map serializer owns compression,
source-linked approaches, shared opening-hours references and the device format.

Verification uses the complete `obc-pack` and `obc-bake` package suites and scoped Clippy. Rebuild
the captured package into two empty directories and compare every output byte. The compiler does
not need network access, a simulator or a connected device.
