# Offline landmark content

The compiler that turns a raw capture into the landmark and peak content a map carries. It uses
local files only: no network, no simulator, no device.

## Run it

```sh
cargo build -p obc-bake --locked
tools/obc fixtures sync assistant-inputs
target/debug/obc-bake landmark-content \
  --snapshot "$HOME/.cache/openbikecomputer/fixtures/by-id/assistant-wiki/manifest.json" \
  --boundary "$HOME/.cache/openbikecomputer/fixtures/by-id/assistant-wiki/regions.geojson" \
  --out .artifacts/landmarks
```

**Use an empty output directory.** The `assistant-wiki` package is a four-site review sample, not
Switzerland coverage. Acquiring a country capture is [CAPTURE.md](../../../../fixtures/sources/ride-assistant/CAPTURE.md).

## Input

A schema 1 source manifest: each raw response has a relative path, URL, byte count and SHA-256.
Entity responses are `entities/QID.json`, parent classes are in `classes/`, locale entities in
`locales/`. The compiler verifies the registered bytes before it selects sites, and derives
coordinates and types from the raw entities rather than the manifest's display data. A raw
`wbgetentities` response can carry an explicit redirect; the compiler keeps the edge to the
canonical class before it follows parents, so a redirected category or exclusion root keeps its
meaning.

The boundary is a GeoJSON Polygon, MultiPolygon or FeatureCollection of those. Border points are
included. **Country claims do not select sites**; `policy.json` owns the category roots and
exclusions. Claims use Wikidata best-rank semantics: preferred when present, otherwise normal,
never deprecated.

Every usable article in the UI languages (en, de, fr, es) is retained. The shared mapping is
`specs/content-languages.json`. Irish and Italian are not capture languages. The compiler takes no
map-wide language argument.

## Output

`content.json` is an internal schema 2 manifest for map serialization:

| Part | What is in it |
| --- | --- |
| Identity | Input digests (source manifest, boundary) and policy digests (category file, extraction code, dependency lock, image recipe, language mapping, locale rules). A separate category-policy digest lets acquisition check its own discovery roots. |
| Candidates | The QIDs before article selection, so the same compiler can choose which assets to acquire from a manifest with empty article and image lists. |
| Counts | Captured sites, candidates, usable text, photos, raw photo bytes. A null approach count means the OSM approach join has not run. |
| Records | Sorted by QID: category 1–6, display coordinate, default language, fallback-source QIDs, and every usable language variant with at most four text pages and its own attribution. Colocated QIDs stay separate records. |
| Attribution | Article and photo source, revision, licence, the exact original notices and readable Sources pages. At most 8 KiB per asset, 256 pages per pair. |
| Omissions | QID, asset and reason. A rejected photo leaves usable text available. |

### Local language fallback

For a place's default language the compiler follows its best-rank P131 administrative chain and
reads P37 official-language claims at the nearest depth that has usable text, then checks P17
country entities and their P37 claims. Traversal stops at eight levels and 64 administrative
entities per place. Cycles, missing claims and unsupported local languages cannot reject an
otherwise usable article; ties and final fallbacks use the shared UI language order.
`fallback_sources` names the locale QIDs used, and an empty list means the final UI-order
fallback. This reads the place's captured location claims, not the map rectangle or the source
merge order.

### Photos

All variants of a site share one photo. P18 images come before the union of usable article lead
images, in normalized filename order; selection never depends on device language. **Reject a photo
if its credits cannot fit beside every retained article's credits**, and keep those articles.

CC BY and CC BY-SA photos need a nonempty captured Artist identity. A generic credit such as "Own
work" does not identify the creator, and missing or empty Artist metadata gives
`photo_creator_missing`: the compiler never infers an author from a filename or a linked page. CC0
does not need the field.

Each photo file is exactly 51,840 bytes, 216 columns by 240 rows, one RGB222 pixel per byte
(`00RRGGBB`). The host applies orientation, a Lanczos3 fit, white padding and a fixed 4 × 4
ordered dither. Compression, source-linked approaches, shared opening-hours references and the
device format belong to the map serializer.

## Peak article catalogue

The same capture program takes a regional OSM extract and produces a separate peak collection:

```sh
python3 tools/landmark_capture.py \
  --peaks-osm REGION.osm.pbf --boundary REGION.geojson \
  --out .artifacts/peak-source --select-with target/debug/obc-bake
target/debug/obc-bake peaks \
  --snapshot .artifacts/peak-source/manifest.json \
  --boundary REGION.geojson --out .artifacts/peak-content
```

`peak-candidates --osm FILE --boundary GEOJSON --out FILE` is the offline discovery entry point.
It uses the map's POI classifier and reads named summit nodes only; `summits.json` keeps the
original node IDs, coordinates and tags plus the source PBF digest.

Acquisition resolves explicit `wikidata` and `wikipedia=language:title` tags and nothing else.
Captured API normalization and redirect edges establish canonical identity. Without a QID, the
explicit Wikipedia language links select the first available UI edition in UI order and its page
ID is the identity. A truncated language-link response, a failed resolution or a conflicting pair
of explicit tags is an omission. **There are no inferred links and no generated translations.**

`peaks.json` is schema 1, collection `peaks`. Each record has an `id` and the shared article
fields but no landmark category; `associations` keeps every usable OSM node-to-article link with
the summit's original coordinates. Records are stored once per canonical identity, and all
associations and language variants share one photo.

**The two collections do not mix.** A peak capture cannot enter the landmark compiler, and
`peaks.json` cannot enter the landmark map serializer. The packer and cutter take
`--peaks PATH/peaks.json`, repeatable for several regional catalogues, and join only the full node
identities of emitted summit POIs. `obc-bake bake --peaks FILE` and the planet baker take one
global catalogue; their cache keys include the peak content fingerprint.

## Verify

```sh
tools/obc fixtures sync peak-articles
python3 fixtures/verify-peak-content.py
```

The verifier rebuilds discovery from its small source PBF and compares two complete offline
catalogue builds. It checks duplicate and direct-link associations, all UI languages, shared
photos, and text and photo omissions.

For the landmark compiler, rebuild the captured package into two empty directories and compare
every output byte. The `obc-pack` and `obc-bake` package suites cover redirected QIDs, Wikipedia
redirects and normalization, a direct article without Wikidata, unusable text, and independence
from article-entity coordinates.
