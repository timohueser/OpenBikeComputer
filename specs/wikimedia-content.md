# Shared Wikimedia content

This contract defines the landmark and peak content: the compact facts that the prepared source
[`wiki`](#the-wiki-source) holds, and how the compilers read them. The compiled device formats in
[obc-data](obc-data.md) and [OBCM](OBCM_Spec.md) are separate outputs.

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
The article API representation must bind that input to its stated revision.
The REST page `with_html` response supplies HTML, page id, current revision and
licence together. Retain its lead and required notices, not its full envelope.
A rendered lead or a later API response must not inherit an earlier revision id.

Each image candidate retains its source claim or eligible article identity,
Commons identity, metadata/file pins, thumbnail URL, dimensions, relevant subject
metadata and required licence/credit fields. Device conversion uses the standard
500 px thumbnail. The `wiki` source publishes the thumbnail of each photo that the compilers
choose; other thumbnails are not published content.
Full API envelopes, rejected image payloads and request logs are not permanent
shared content. Required proofs and notices must survive compaction.

The producer of `wiki` resolves explicit links to canonical subjects and retains the identity
proofs. It acquires the entity, dependency, article and image inputs of the subjects that the
compilers select, and retains each successful response before the next request. A region
compiles from the shards of a `wiki` version, with its OSM associations and the compiler policy,
and makes no Wikimedia request. A fact that the version lacks is no content; the step counts it.

Acquisition returns confirmed present or confirmed missing facts. Transport
failure, incomplete required identity, language-link or dependency pagination,
and server pressure are unresolved work, never confirmed missing content. An
interrupted or failed run of the producer publishes no version. The next run reuses its
successful responses.

### Compact facts

Each compact JSON object repeats its kind, key, status, identity and revision where present.

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
`input` is `thumbnail500` or a validated retained `original`. The shards hold the
JSON facts and every asset that they reference. Request envelopes stay in the work directory of
the producer; no version holds them.

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

## The `wiki` source

The prepared source `wiki` ([obc-data](obc-data.md#wiki)) holds the compact facts of each OSM
subject in the selected coverage: an object with a `wikidata` or `wikipedia` tag. Its version has these files:

| Path | Holds |
| --- | --- |
| `cells/<i>-<j>.tar.zst` | The facts of each subject whose box touches the `2^20` µdeg OBCA cell `(i, j)`; `<i>` and `<j>` have four digits or more |
| `common.tar.zst` | The classes, and the facts of each subject without a box or whose box touches more than 16 cells |
| `index.json` | `shards`: `path`, `sha256`, `bytes` and `facts` of each shard; `counts`: the subjects, those in `common`, those without a resolved link, the unresolved work of each kind, and the compiles of the producer that failed (`photo_compile_failures`) |

The version manifest records its coverage and article languages. Regional preparation selects
subjects from the regional OSM extract before it asks Commons. An article in an excluded
language is a `missing` fact with `reason: "language_not_prepared"`. This means that the version
excludes it, not that Wikipedia lacks it. It supplies no text or lead-image candidate. Subject
identity still uses the supported-language order; incomplete canonical identity proof remains
unresolved work.

A shard is a tar under zstd. Its members are `content/<kind>/<sha256>.json`, as the shared
content of a step names them. A fact that two shards need is in both, with the same bytes. A
subject has its entity, its links, the locales of the bounded P131 traversal and the articles that
the compilers read. A fact that the version lacks is no content. A member
`content/assets/<sha256>` is the 500 px thumbnail of a photo that the compilers choose for a
subject; the device cell steps convert it to the device format.

The producer asks Commons in rounds: the categories of the subjects that the compilers select, then the metadata of their photo candidates (P18, the lead images of their
articles and the members of their categories), then the thumbnails of the files that the
compilers choose. A thumbnail that fails the checks of a compiler makes it choose another file,
so the last round repeats for the subjects with new files until it asks no new file. Without
published text, each article of the version is a fact with its key and the status `missing`: the
compilers of the producer and of the readers see no article, and choose no lead image. Each
round asks only for what the rounds before have not answered, with these limits: one request at
a time, at most four a second, `maxlag` 5,
`Retry-After`, three backoffs on 429 and 503, and media at 3 MB/s. When Wikimedia is busy, the
producer waits ten minutes, then twice as long each time up to two hours, and asks again. A
subject whose compile fails alone has no photo in the version: the producer logs it and
`index.json` counts it. When more than half the subjects of a chunk fail, or on an I/O error, the
run stops.

## Photo fallback and reachability

Photo order starts with explicit P18, then identity-checked Wikipedia lead images.
A nearby camera position or matching filename is not subject evidence. Coverage
and relevance are measured separately before the old category path is replaced.

Fallback uses one directly claimed P373 category, one nonrecursive category response
and at most the file bound in `photo_policy.fallback` of `policy.json`.
Continuation beyond that deliberate candidate bound is a complete bounded outcome.
The record states the limit reached. Files require subject evidence and valid credits.

A target holds the versions of `wiki`. A bake reads the shards of the version that its target
reads, so a warm bake needs no network, and another machine gets the facts and the thumbnails
from the target. A new version refreshes the facts: the content layers and the layers that read
them bake again. Other thumbnails, full originals, rejected candidates and request logs are not
published. The device cell layers publish the final device images.

`Content.aliases` maps exact Wikidata aliases to canonical subject ids.
`Content.wikipedia_aliases` maps proven `language:title` aliases of accepted article
variants to those ids. Planner search uses these exact identities for OSM association.
Peak node ids and OSM coordinates remain the regional association.
Online photo credits omit device transform notices. A photo reference includes
`file_identity`, `page_revision` and `file_revision`; incomplete provenance gives a
source-page link. Consumers recheck that identity before displaying online photos.
A mutable CDN URL is not a permanent certificate for future bytes.
