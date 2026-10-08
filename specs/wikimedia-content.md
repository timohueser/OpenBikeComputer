# Shared Wikimedia content

This contract defines acquired landmark and peak content. The regional capture and
compiled device formats in [obc-data](obc-data.md) and [OBCM](OBCM_Spec.md) remain
separate outputs. Shared acquisition uses the existing Store and source release pins. The regional
capture integration does not yet implement this shared boundary.

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
failure, incomplete pagination and server pressure are unresolved work, never
confirmed missing content. An interrupted or failed operation cannot publish a
complete capture. The next run reuses its successful responses.

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

A proposed fallback uses one directly claimed P373 category, one nonrecursive
category response and at most twenty files. The files still need subject evidence
and valid credits. This bound is a proposal, not an enabled selection rule.
The image-selection implementation selects the final bound from measured coverage;
it must not retain the current unbounded category crawl as its replacement.

The Store keeps inputs reachable from pinned releases, active plans and resumable
operations under its existing retention rules. Compact shared records and final
device assets use bounded publication bundles. Bundles retain their source pins
and licence identities. Temporary thumbnails, full originals, rejected candidates
and per-request logs are not R2 publication outputs. Cleanup follows Store
reachability; content does not create an independent permanent history cache.
