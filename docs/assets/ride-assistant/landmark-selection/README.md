# Deterministic landmark selection

Selection uses fixed rules over a versioned source snapshot. There is no AI classification,
manual per-place decision, popularity score, or review queue. The rules below are the current
proposal for map creation. The stored count script implements the type gate; it is not a
complete production extractor.

## Category rules

An item needs a coordinate, a Wikipedia article, and a permitted Wikidata type. Match P31
(instance of) through zero or more P279 (subclass of) steps. Exact root IDs are in
[selection.json](selection.json). Deduplicate by Wikidata ID. Explicit excluded roots win
when an item has multiple types. This deliberately gives false-negative exclusions priority
over a conflicting category assignment.

| Group | Fixed rule |
| --- | --- |
| Natural curiosities | Permit waterfalls, canyons/gorges, caves, natural arches, rocks, glacial erratics, boulders, remarkable trees, and springs through the listed roots. Article and coordinate are mandatory. |
| Remarkable trees | Require type [Q811534](https://www.wikidata.org/wiki/Q811534) or a subclass. This is a source classification, not a judgment computed from age, size, photographs, or prose. Ordinary tree records and species articles do not match this rule. |
| Castles and fortifications | Permit the castle, castle ruin, château, fortress, and fortification roots. Exclude the deserted-castle-site root Q1015644. No minimum article popularity or number of languages. |
| Archaeology | Permit the archaeological-site, dolmen, tumulus, menhir, megalithic-tomb, and megalithic-site roots. This captures stone circles through the megalithic hierarchy. Events and movable finds do not qualify merely through their own types. |
| Monasteries and abbeys | Permit the monastery and abbey roots; exclude destroyed-monastery records. A religious organization or collection alone does not qualify. |
| Architecture | Cathedrals and subclasses qualify. Ordinary churches and bridges do not. Additional architecture needs another explicit rule before inclusion; there are no individually selected exceptions. |
| Glaciers | Exclude the glacier root Q35666 and subclasses. They are landscape features to look at, rather than visit destinations. |
| Passes | Include the mountain-pass root Q133056 and subclasses. The [sample review](../glacier-pass-study/README.md) supports keeping their descriptions, including entries without photos. |

**Lakes, mountains, and glaciers are excluded**, even if another permitted type is also present. Lake
names belong on the map; mountains belong in Peak Viewer. Settlements, administrative areas,
industry, transport facilities, ordinary buildings, and commercial venues remain outside the
intended scope. Existing industrial-building and industrial-archaeology exclusions take
priority. A misclassified commercial spring can still pass the current type gate: this is a
source error, not a reason to add a hand-maintained exception for that individual place.

## What the rules cannot prove

“Verify that something remains to see” was not an implementable rule. It is removed.
A castle-ruin type is positive structured evidence of ruins. A deserted-site or destroyed-site
type is a reason to omit the item. If the source just says castle or archaeological site, the
current rule keeps it. It does not prove that visible remains exist. This admits occasional
empty or buried sites; a strict visibility guarantee is not possible from these fields alone.

Do not use [P576](https://www.wikidata.org/wiki/Property:P576) as a blanket deletion test.
It covers dissolution and abolition as well as demolition. A dissolved abbey can have intact
buildings, and a destroyed castle can have ruins. The captured data does not contain P576;
the current count makes no claim to apply such a filter. If stronger exclusion is needed,
add a fixed rule using explicit physical-state data rather than interpreting article prose.

Likewise, “significant architecture” must become a whitelist of types or formal designations,
not an editorial judgment. Cathedrals are the initial type rule. A future extension could
require a bridge/building type **and** a listed national-level heritage designation (P1435),
with exact accepted designation IDs configured per country. No generic “has heritage status”
rule and no per-building human or AI ranking are proposed.

A coordinate is an identity/location point, not a route destination. Map creation must find
an accessible mapped entrance or approach point before offering a visit. A pass coordinate does not prove that the pass is accessible by bicycle. If no approach is known, information can
still be shown, but route creation must not silently snap onto the feature itself.

Known-closed places are filtered at runtime; unknown hours remain visible. The country count
does not evaluate opening hours. The Gelmerbahn stays as an old UI fixture, outside the
production selection.

## Text and pictures

Use Wikipedia articles for text and Wikidata for identity and types. The sample prose is
edited for the prototype; Gutzgletscher is translated from German. This is separate
from the random draw and the deterministic selection policy. It does not demonstrate an
automatic summarizer or commit us to AI-generated map content.

A reproducible image rule can use P18, then the chosen article's lead image, and reject files
without supported formats or complete reusable-licence metadata. Stable source order resolves
multiple candidates. It cannot guarantee a picturesque or explanatory view. Our lead-image
samples include a ski vehicle and a hotel. Keep absent images absent. Do not add visual AI
ranking or hand-pick substitutes to hide these limitations.

Large 216 × 240 ordered-dither images are the accepted direction. The current demo embeds
pixels in firmware, not on SD. Use basic lossless compression in production; the exact codec
and device decode cost still need measurement. The observed 5–10 kB range is a useful target,
not a guaranteed per-image limit. See the [lossless storage experiment](../glacier-pass-study/README.md#image-storage).

## Swiss count after the category changes

The snapshot was captured on 14 September 2026. Scope: P17 Switzerland, P625 coordinates,
P31 type, and an English, German, French, or Italian Wikipedia article. This is not a boundary
polygon scan. Source omissions and type errors remain. The counts are candidates, not a
finished map pack.

| Group | Candidates | With P18 image |
| --- | ---: | ---: |
| Natural curiosities | 141 | 110 |
| Castles and fortifications | 914 | 863 |
| Archaeology and megaliths | 385 | 317 |
| Monasteries and abbeys | 171 | 138 |
| Cathedrals | 14 | 14 |
| Passes | 286 | 236 |
| **Distinct accepted-category candidates** | **1,703** | **1,482** |

Group rows overlap. Totals deduplicate IDs. Glaciers are explicitly excluded. Only 729 accepted
candidates have an English article. Non-English articles remain candidates, not ready English
summaries. P18 presence does not guarantee a suitable photo, and lead images can exist without
P18. The random sample used the full glacier/pass type pools before these exclusions, with no
English-language or image requirement: 149 glaciers and 293 passes. It was not redrawn.

At one large image for each P18-bearing candidate, pixel storage is **76.83 MB raw / 57.62 MB
six-bit packed**. At an illustrative 5–10 kB per photo, images would occupy **7.41–14.82 MB**.
These decimal figures exclude text, credits, indexes, and headers. Compression is measured on
seven photos only; no country-wide compression ratio is claimed.

## Reproduce and refine

Run from the repository root:

```sh
python3 docs/assets/ride-assistant/landmark-selection/count.py
```

This uses only the captured data and prints [counts.json](counts.json). Change each group's
`include` flag in selection.json to compare choices. Root changes must be covered by the saved
type closure; query and regenerate types.json when adding new roots. The count script does not
fetch data or add a map-building backend.

[switzerland.json.gz](switzerland.json.gz) contains 20,506 unique IDs, their direct types,
available article languages, and a P18-presence flag. It was reduced from 63,481 rows returned
by [switzerland.sparql](switzerland.sparql). Multiple articles, images, and types were collapsed
per ID; image presence is true if any returned row has P18. The compressed JSON preserves no
article prose or image bytes. [types.sparql](types.sparql) returns the type closure. types.json
retains only types observed in the country result, plus their English Wikidata labels.
These Wikidata structured-data extracts are CC0.

To refresh the raw source results, from this directory:

```sh
curl -f -G --max-time 60 -A 'OpenBikeComputer landmark scope study' \
  --data-urlencode query@switzerland.sparql --data format=json \
  https://query.wikidata.org/sparql -o /tmp/landmark-switzerland-raw.json
curl -f -G --max-time 60 -A 'OpenBikeComputer landmark scope study' \
  --data-urlencode query@types.sparql --data format=json \
  https://query.wikidata.org/sparql -o /tmp/landmark-types-raw.json
```

The query service can time out. Check HTTP success and complete JSON before replacing any
snapshot. A fresh query can differ as editors change Wikidata. The committed snapshots make
the reported count repeatable without relying on the live service.
