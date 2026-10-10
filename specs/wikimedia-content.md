# Shared Wikimedia content

This contract defines acquired landmark and peak content. The regional capture and
compiled device formats in [obc-data](obc-data.md) and [OBCM](OBCM_Spec.md) remain
separate outputs. Shared acquisition uses the existing Store and source release pins. Normal preparation uses this shared boundary. Retained regional captures are read-only adoption inputs.

## Policy owners

`host/obc-pack/src/landmarks/policy.json` owns the category roots and exclusions.
`policy.rs` owns category precedence and its tests. The sixteen category ids and
the excluded glacier group remain unchanged. Ordinary OSM service-place filters
remain separate. Peak content does not need a landmark category.

`specs/content-languages.json` owns supported languages and order: English,
German, French, Spanish. `locale.rs` selects a usable local language through
P131/P37, then P17/P37, then that order. Administrative traversal has depth eight
and at most 64 entities. These dependencies are pinned source inputs.

The existing landmark module owns article, name, photo and credit eligibility.
Both collection compilers use it. Acquisition records facts and API outcomes;
compilation applies these rules. A policy edit recompiles retained facts. Only a
missing fact requires acquisition. There is no separate acquisition rule language.

## Manual snapshot preparation

`obc data content` prepares the reusable source on the operator's machine. The Content
screen in the terminal interface uses the same commands. Preparation is a retained operation.
Closing the terminal leaves it running. Stop preserves successful imports and images.

`content configure FILE` saves a JSON configuration. Relative paths refer to the directory
of that file. Each archive has `date` and either `path` or an HTTPS `url`. Optional `sha256`
and `bytes` require that exact compressed input. A remote Wikidata JSON archive also needs
`suffix: ".json.bz2"`; remote SQL archives need `suffix: ".sql.gz"`.

```json
{
  "wikidata": {"path": "wikidata.json.bz2", "date": "2026-01-01"},
  "wikipedia": {
    "en": {"path": "enwiki-NS0.json.tar.gz", "date": "2026-01-01"},
    "de": {"path": "dewiki-NS0.json.tar.gz", "date": "2026-01-01"},
    "fr": {"path": "frwiki-NS0.json.tar.gz", "date": "2026-01-01"},
    "es": {"path": "eswiki-NS0.json.tar.gz", "date": "2026-01-01"}
  },
  "wikidata_pages": {"path": "wikidatawiki-page.sql.gz", "date": "2026-01-01"},
  "wikidata_redirects": {"path": "wikidatawiki-redirect.sql.gz", "date": "2026-01-01"},
  "langlinks": {
    "en": {"path": "enwiki-langlinks.sql.gz", "date": "2026-01-01"},
    "de": {"path": "dewiki-langlinks.sql.gz", "date": "2026-01-01"},
    "fr": {"path": "frwiki-langlinks.sql.gz", "date": "2026-01-01"},
    "es": {"path": "eswiki-langlinks.sql.gz", "date": "2026-01-01"}
  },
  "osm": ["planet.osm.pbf"]
}
```

Wikipedia inputs are Wikimedia Enterprise namespace-zero HTML snapshots. Wikidata inputs
are the full JSON dump. SQL inputs supply item redirects and language links without page API
calls. The page table must match the redirect table. Small scopes can use `entities` and
`links` arrays instead of `osm`. OSM discovery requires `osmium`.

`content prepare` streams compressed records into a disk-backed temporary index. It never
extracts full archives. The compressed inputs and this index need local disk space together.
Downloads resume only with the same upstream length and ETag. Processing retains a 4 GiB
disk reserve. A changed local file or producer starts a separate preparation. An updated
remote snapshot needs an updated configuration date or URL.

Native selection rules choose the articles and images. Only manual preparation can call
Commons for metadata, bounded categories and selected 500 px thumbnails. The native photo
converter produces the standard 216 × 240 RGB222 bytes. A preparation retains their source
identity, revision witnesses, credits and transform-code digest. It publishes no thumbnail,
full archive, request log or temporary projection.

The complete manifest has `schema: 1`, `complete: true`, source `origins`, root `coverage`,
`coverage_sha256`, `records`, producer identity, acquisition-code identity,
`photo_transform_sha256` and `files`.
An origin retains the compressed input SHA-256, size and source date where available.
Each file has `name`, `sha256`, `bytes` and `kind`: `index`, `bundle` or `image`.
There is one SQLite lookup index. JSON fact bundles are at most 16 MiB. Each image is
51,840 bytes. The manifest SHA-256 is the content version.

`content plan SHA` reviews publication. `content publish SHA` uploads immutable objects to
`content/objects/SHA` and verifies them. It publishes `content/manifests/SHA.json` last.
No pointer changes and no objects are removed. Interrupted publication can be repeated.

Preparation selects its completed version locally. `content use SHA --from-r2` restores
the manifest and index on a bake machine. It downloads only required bundles and images
as requests need them. Source dates remain explicit; a scheduled bake never refreshes these
inputs through Wikimedia. New OSM identities outside prepared coverage block preparation.
Invalid page proof and failed imports remain unresolved. Confirmed absence needs a complete
source scan; item absence also needs the complete page and redirect inputs.
No-item identities require the source language's complete langlinks table. Their canonical
page follows supported-language order. A changed image transform requires a new preparation.

## Identity and source pins

A subject id is the resolved Wikidata QID. An exact Wikipedia link without a
Wikidata item uses `wiki-<language>-<pageid>`, as the peak compiler does. The
canonical language follows supported-language order over confirmed language links.
Titles and coordinates are not stable ids. Acquisition retains the exact API
normalization, redirect and language-link proof used to resolve an input link.
It does not infer identity from a name, proximity, filename or camera position.

A pin contains source identity, source revision and SHA-256 of retained bytes:

| Source | Identity | Revision |
| --- | --- | --- |
| Entity, class or locale | canonical QID | Wikidata `lastrevid` |
| Article | wiki and page id | Wikipedia revision id |
| Commons metadata | Commons page id | description-page revision id |
| Image bytes | Commons file identity | file timestamp and upstream SHA-1 |

An image retains its canonical filename and source URL as well as its identity.
Metadata and file revisions are separate: a credit edit must not require another
image download. A successful check time controls freshness, not content identity.
A confirmed missing response retains the exact requested identity, response digest
and check time. It has no invented revision. A refresh may replace that outcome.

A region, OSM extract, transport implementation or image transform is not part of
a source pin. Release manifests pin the complete source dependency set used for a
compile. The same pin can serve more than one region and collection.

## Retained inputs and Store boundary

The acquired record keeps only fields needed for compilation and provenance:
subject id; supported labels; coordinates; relevant claims and sitelinks; class
and locale dependency pins; exact link proofs; and per-asset outcomes.
Each article retains language, canonical title, page id, revision id, revision
source URL, compact lead HTML, licence and required original notices.
The article representation must bind that input to its stated revision.
The REST page `with_html` response supplies HTML, page id, current revision and
licence together. Retain its lead and required notices, not its full envelope.
A rendered lead or a later API response must not inherit an earlier revision id.
Snapshot HTML supplies its own page id and revision in the Parsoid header. That rendered
revision owns the article pin. A different envelope revision is retained as a separate
declared revision; its timestamp is not attributed to the rendered text.

Each image candidate retains its source claim or eligible article identity,
Commons identity, metadata/file pins, thumbnail URL, dimensions, relevant subject
metadata and required licence/credit fields. Device conversion uses the standard
500 px thumbnail. Temporary thumbnails are not published content.
Full API envelopes, rejected image payloads and request logs are not permanent
shared content. Required proofs and notices must survive compaction.

The API/Store seam has three operations within the existing fetch machinery:

1. Resolve explicit links to canonical subjects and retain the identity proofs.
2. Acquire absent or stale entity, dependency, article and image inputs. Reuse
   pins across regions. Retain each successful response before the next request.
3. Compile a pinned subject set with regional OSM associations and compiler policy.
   This operation makes no Wikimedia request. Missing pins fail preparation.

Acquisition returns confirmed present or confirmed missing facts. Transport
failure, incomplete required identity, language-link or dependency pagination,
and server pressure are unresolved work, never confirmed missing content. An
interrupted or failed operation cannot publish a complete capture. The next run reuses its successful responses.

### Acquisition adapter bytes

`tools/wikimedia_acquire.py` is an adapter for a retained Store operation. Its
caller supplies `--work`, `--out`, `--requests` and `--inputs` paths. It does not
write Store objects or maintain a separate permanent index.

The request object has `check_id`, a stable operation token, and `refresh`, a
boolean. Optional arrays name `entities` by QID, `links` by
`wikidata:<QID>` or `wikipedia:<language>:<title>`, `articles` by objects with
`language` and `title`, and `commons`, `categories`
and retained `files` by exact filename or category title. Entity acquisition
includes the shared class closure and bounded locale dependencies.

The input array contains admitted pin records. Each record has `kind`, `key`,
`status`, `checked_at`, `path` and `sha256`. Present versioned facts also have
`identity` and `revision`. `path` is the resolved Store object path. File inputs
also have `asset_path`, the resolved image object path. The adapter verifies
input digests before acquisition.

The output `manifest.json` has `schema: 1`, `complete`, `records`, `assets` and
`failures`. Output pin paths are relative to `--out`. Each compact JSON object
repeats its kind, key, status, identity and revision where present. Check times
stay in the manifest. A check without changed content keeps the content digest.

| Kind | Compact fact |
| --- | --- |
| `entity` | Supported labels, relevant ranked claims and supported sitelinks |
| `link` | Canonical subject and exact normalization, redirect and language-link proof |
| `article` | Wiki, page id, title, revision, lead HTML, licence and original notices |
| `commons` | File identity, description revision, file revision, categories and image metadata |
| `mediainfo` | MediaInfo identity, revision and ranked P180 statements |
| `category` | One bounded nonrecursive member response, limit and continuation proof |
| `file` | File identity and revision, plus a separately retained image asset pin |

A category response has no invented page revision. Its digest pins the bounded
membership observation. `bounded`, `limit`, `limit_reached` and `truncated`
describe its scope. Required metadata pagination must finish; a bounded category
response may retain continuation without following it.

An image asset has relative `path`, `sha256`, `bytes`, `url` and `input` fields.
`input` is `thumbnail500` or a validated retained `original`. Source snapshots
own the JSON facts and every asset they reference. Temporary request envelopes
stay under `--work`. They are not source snapshot or publication outputs.

An article pin records the resolved page's actual Wikidata item and alias proof.
A redirect to another subject is a present source fact. Compilation rejects that
article for the requested subject and records an identity omission. Other eligible
languages remain usable. An unresolved API response still blocks acquisition.
A QID subject requires the article's matching Wikidata item. A no-item article
remains eligible for a confirmed synthetic Wikipedia subject.

A thumbnail file fact retains `revision_before` and `revision_after` witnesses.
Both match the canonical filename, file timestamp and SHA-1, required credit
fields, and description revision when present. A retained thumbnail without both
coherent witnesses does not become a shared file pin.

The adapter returns a nonzero exit status for unresolved work. It retains each
successful compact fact before another request. A retry with the same `check_id`
reuses those successes. A new freshness check uses a new token. Compilation
uses admitted pins and makes no Wikimedia requests.

`--adopt` accepts a retained capture root. It validates source digests and article
and image revision evidence without changing the capture. A response without a
source revision remains capture evidence. It does not become a versioned shared
pin. Missing revision information requires a bounded acquisition check.

## Selection and projections

Selection retains a reason for each selected subject and omitted asset. The
selected reason names its landmark category or explicit peak link. Omission
reasons distinguish policy exclusion, no matching category, outside region,
identity failure, absent source content, unsupported text, credit rejection and
image rejection. An API outcome is distinct from a compiler rejection.
A selected article states the chosen language and locale dependency evidence.
A selected photo states its candidate source and identity evidence.

Landmarks require usable supported article text. Peaks may have article text,
a photo, or both. A peak without content remains an OSM summit. A missing optional
article or photo does not invalidate a map. An unresolved source failure remains
visible in preparation and does not masquerade as optional absence.

One acquired subject can produce both collections. Landmark OSM links, hours and
routable approaches remain separate from peak node ids and summit coordinates.
Peak records never enter the landmark or service-place query collection.
Regional clipping selects associations without changing the subject identity.

The device projection contains bounded short text, required attribution and
216 × 240 RGB222 images under the existing OBCM contracts. It works offline.
The planner projection contains selected subject identity, location, category,
article text and provenance, online image references and required credits.
Routing data and downloaded iOS routing maps contain no image payload.
Offline planner detail retains useful text and explains an unavailable photo.
Both projections derive from the same selected inputs and policy.

## Photo fallback and reachability

Photo order starts with explicit P18, then identity-checked Wikipedia lead images.
A nearby camera position or matching filename is not subject evidence. Coverage
and relevance are measured separately before the old category path is replaced.

Fallback uses one directly claimed P373 category, one nonrecursive category response
and at most the file bound in `photo_policy.fallback` of `policy.json`.
Continuation beyond that deliberate candidate bound is a complete bounded outcome.
The record states the limit reached. Files require subject evidence and valid credits.

The Store keeps inputs reachable from pinned releases, active plans and resumable
operations under its existing retention rules. Compact shared records and final
device assets use bounded publication bundles. Bundles retain their source pins
and licence identities. Temporary thumbnails, full originals, rejected candidates
and per-request logs are not R2 publication outputs. Cleanup follows Store
reachability; content does not create an independent permanent history cache.

Normal intermediate content layers publish `shared-content/` bundles of at most
16 MiB, the existing compiler JSON input bound. Each bundle keeps exact compact
fact bytes, their SHA-256 and source pins. Entity dependency facts, article aliases,
Commons description metadata and MediaInfo statements remain reusable.
Local image inputs are excluded. The Wikimedia source declarations set
`r2_copy=false`; ownership for Store reachability does not select a source mirror.
Final device images remain publication outputs of the device cell layers.

A warm pinned bake needs no network. A new machine restores the prepared manifest, index,
required facts and RGB222 images from R2. It makes no Wikimedia request. A changed photo
transform refuses the old prepared image until the operator prepares a new content version.

Expired identity inputs remain available for revision checks. A separate refresh
stage checks those roots and their bounded dependencies. Unchanged revisions reuse
content bytes. Fresh and missing identities use the ordinary acquisition stage.
An explicit move carries refresh intent outside the identity key and uses a new
operation check id. Failed source checks block preparation; they are not missing facts.

`Content.aliases` maps exact Wikidata aliases to canonical subject ids.
`Content.wikipedia_aliases` maps proven `language:title` aliases of accepted article
variants to those ids. Planner search uses these exact identities for OSM association.
Peak node ids and OSM coordinates remain the regional association.
Online photo credits omit device transform notices. A photo reference includes
`file_identity`, `page_revision` and `file_revision`; incomplete provenance gives a
source-page link. Consumers recheck that identity before displaying online photos.
A mutable CDN URL is not a permanent certificate for future bytes.

Operation fetch results report newly issued HTTP attempts and newly read response
body bytes. API body bytes are counted before gzip decoding. Media body bytes
are counted as the HTTP client reads them. These are payload bytes, not total
network traffic. Cached and adopted historical totals remain unknown. The existing
fetch `bytes` field reports retained output size and does not measure transfer.
