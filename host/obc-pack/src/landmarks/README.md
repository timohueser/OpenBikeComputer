# Offline landmark content

Build the host command from the repository root:

```sh
cargo build -p obc-bake --locked
tools/obc fixtures sync assistant-inputs
target/debug/obc-bake landmarks \
  --snapshot "$HOME/.cache/openbikecomputer/fixtures/by-id/assistant-wiki/manifest.json" \
  --boundary "$HOME/.cache/openbikecomputer/fixtures/by-id/assistant-wiki/regions.geojson" \
  --language en --out .artifacts/landmarks
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
claims; deprecated claims are excluded. Requested language precedes en, de, fr, it and ga; the record
gives the actual language.

`content.json` is an internal schema 1 manifest for map serialization. It contains:

- Input and policy SHA-256 digests. Input identity includes the source manifest and boundary bytes.
  Policy identity includes the category file, extraction code, dependency lock, image recipe and
  requested language. A separate category-policy digest lets acquisition check its discovery roots.
- Candidate QIDs before article selection. A source manifest with empty article/image lists can use
  the same compiler to select which assets to acquire.
- Counts for captured sites, candidates, usable text, photos and raw photo bytes. A null approach
  count means that the OSM approach join has not run.
- Records sorted by QID, with category 1–6, display coordinate, actual language and at most four
  text pages. Colocated QIDs remain separate records.
- Separate article and photo source, revision, license, exact original notices and readable Sources
  pages. Each asset permits at most 8 KiB of attribution; the pair permits at most 256 pages.
- Omissions with QID, asset and reason. A rejected photo leaves usable text available.

Each photo file contains exactly 51,840 row-major bytes: 216 columns by 240 rows, one RGB222 pixel
per byte (`00RRGGBB`). The host applies orientation, Lanczos3 fit, white padding and the fixed 4×4
ordered dither. The manifest records its path, size and digest. The map serializer owns compression,
source-linked approaches, shared opening-hours references and the device format.

Verification uses the complete `obc-pack` and `obc-bake` package suites and scoped Clippy. Rebuild
the captured package into two empty directories and compare every output byte. The compiler does
not need network access, a simulator or a connected device.
