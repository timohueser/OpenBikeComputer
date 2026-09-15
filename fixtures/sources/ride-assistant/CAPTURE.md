# Capture geographic landmark sources

This is a one-shot acquisition command. It uses the existing fixture package store. The
`obc-bake landmarks` compiler selects sites and builds content without network access.
Acquisition does not use P17 country claims, a curated list, or the demonstration prose.

Build the compiler and acquire the pinned country boundary first. Run from the repository root:

```sh
cargo build -p obc-bake --locked
tools/obc fixtures sync assistant-osm
python3 tools/landmark_capture.py \
  --boundary "$HOME/.cache/openbikecomputer/fixtures/by-id/assistant-osm/switzerland-boundary.geojson" \
  --policy host/obc-pack/src/landmarks/policy.json \
  --select-with target/debug/obc-bake \
  --out .artifacts/switzerland-wiki
```

Use the same arguments and output directory to resume. Completed requests are reused only
after their URL, byte count and SHA-256 digest are checked. Failed requests remain failed.
Add `--retry-failed` to make one more attempt at each failed request. Previous failure records
are kept in `attempts/`. A changed boundary or category policy requires a new output directory.
Do not run two capture processes against one output directory.

The command uses two workers and at most four requests per second in total. It queries each
included category root inside the boundary's bounding box. If a class-first query fails,
one equivalent box-first query is attempted and both outcomes are kept. Every root must have
a complete response. QIDs are deduplicated before raw entity capture. Wikidata best-rank
semantics apply: preferred statements when present, otherwise normal statements. Deprecated
statements are excluded. Coordinates, types and P279 ancestors come from captured entities.
If an EntityData class request follows a redirect, a captured `wbgetentities` response keeps
the explicit old-to-canonical ID mapping. The closure follows the canonical class itself, then
its parents. This preserves category and exclusion roots at the redirect target.

The first compiler pass applies the exact polygon, including border points, and the category
exclusions. Its candidate QIDs select which assets to acquire. Acquisition checks that its
category file digest matches the compiler's embedded policy digest. No separate Python
polygon or category selector can disagree with the production compiler.
The executable digest is checked before and after selection. Keep a separate copy of the
built executable when another build can replace the normal target binary.

For each eligible site, acquisition captures available en/de/fr/it/ga sitelinks. Each article
has a raw revision query, exact-revision rendered HTML, and the supplied notices and footer.
It captures P18 and each language's first lead image candidate in filename order. Commons
metadata is kept before JPEG/PNG original bytes are downloaded. The limit is 32 MiB per
response. Unsupported formats, absent sitelinks, absent images and request failures have
different outcomes. The offline compiler makes the final text, image and attribution decisions.

`manifest.json` uses the same schema 1 interface as `assistant-wiki`. Successful response bytes
have source URLs, retrieval timestamps, byte counts and SHA-256 digests. Raw responses are in
`queries/`, `entities/`, `classes/`, `articles/` and `images/`. The manifest also contains the
request outcomes, coverage, and per-place article/image outcomes. The boundary, policy, recipe,
candidate union and production selection result remain in the package. A source package is a
capture interval, not an assertion that all upstream pages changed at one instant.

An incomplete acquisition exits with status 2 and keeps usable captured bytes. A recovered
query failure remains in the request history; it does not make a successful equivalent query
incomplete. Unresolved root queries, source requests or class closure make country coverage
incomplete. Do not report their absence as zero landmarks or zero images. Interrupted work
does not establish a completed snapshot. Source selection and asset preparation are separate
from OSM approach joins and final compressed map counts.

Compile twice into empty directories to check offline reproducibility:

```sh
target/debug/obc-bake landmarks --snapshot .artifacts/switzerland-wiki/manifest.json \
  --boundary .artifacts/switzerland-wiki/boundary.geojson --language en --out .artifacts/content-a
target/debug/obc-bake landmarks --snapshot .artifacts/switzerland-wiki/manifest.json \
  --boundary .artifacts/switzerland-wiki/boundary.geojson --language en --out .artifacts/content-b
diff -r .artifacts/content-a .artifacts/content-b
```

Publish only through the [fixture package workflow](../../README.md#adding-or-replacing-data):
pack the capture directory, register its immutable hash and byte count, publish it, then sync
and verify from an empty cache. Do not put captures in the production maps bucket. Use an
explicit fixture profile for the full-country source package; it is too large for routine tests.

## Pinned Switzerland capture

The `assistant-country` profile contains `assistant-wiki-switzerland`. It is separate from the
four-site `assistant-wiki` review package and routine fixture tests. Its 5,348,363,006-byte archive
has SHA-256 `1907429595fa7adaaf6288ae8b84df84453bd53da6b618bfbf14dc62562ad5c8`.
The source interval is 2026-09-14 22:15 UTC through 2026-09-15 05:23 UTC. All 24 category queries
completed, and all 6,221 candidate entities and their class closure were captured. There are no
unresolved requests or missing classes. The package retains 17 earlier failed-attempt records.

Run an explicit country recount with the optimized compiler:

```sh
cargo build -p obc-bake --release --locked
tools/obc fixtures sync assistant-country
target/release/obc-bake landmarks \
  --snapshot "$HOME/.cache/openbikecomputer/fixtures/by-id/assistant-wiki-switzerland/manifest.json" \
  --boundary "$HOME/.cache/openbikecomputer/fixtures/by-id/assistant-wiki-switzerland/boundary.geojson" \
  --language en --out .artifacts/switzerland-content
```

Use a fresh output directory for each pass. The source package supplies 3,013 exact-revision
article captures and 1,481 unique original images. These acquisition counts are not usable-content
counts: the compiler applies text, license and attribution limits after capture.

The [recorded recount](switzerland-recount.json) pins compiler commit `7b5f9d50`, source and
policy hashes, output hashes, language counts and omissions. For requested English, the result is:

| Measure | Count |
| --- | ---: |
| Geographic/type candidates before article selection | 3,508 |
| Usable texts | 1,495 |
| Usable photos | 1,119 |
| Raw RGB222 photo bytes | 58,008,960 |
| English / German / French / Italian texts | 669 / 692 / 117 / 17 |

Tellenburg castle ruin (Q1247344) keeps its English text but omits its P18 photo because the
captured creator, Хрюша, uses unsupported glyphs. Grimsel Pass (Q666668) keeps its English text
but omits its CC BY-SA photo because the captured metadata does not identify its creator. Both
omissions retain the original response, exact photo identity and explicit reason. No source asset
was changed to produce these cases.

Two compiler passes ran with network access denied by macOS `sandbox-exec`. All 1,120 output files
were byte-identical. Their total size is 66,835,644 bytes, including the 8,826,684-byte internal
content manifest. The fixture publisher checked the complete public archive; an empty fixture
cache then downloaded, extracted and verified the package with the normal fixture commands.

The mapped-approach count and final compressed map, index and Sources storage are RA09/RA13
outputs. They remain unset in this source recount. Physical SD latency and stack high-water
acceptance remain pending; no device is connected.
