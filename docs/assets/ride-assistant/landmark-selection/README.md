# Landmark category proposal

The question is: “What is this place, and why is it interesting?” Keep places that a rider or
hiker can identify in the landscape. A short article and an optional photo should explain the
place without internet access. This is a proposal for review, not a production extraction rule.

## Proposed selection

| Group | Include | Boundary |
| --- | --- | --- |
| Natural curiosities | Waterfalls, gorges, caves, natural arches, distinctive rocks, glacial erratics, remarkable trees, natural springs | Require an article about the feature. Ordinary trees, rocks, and water taps do not qualify. Commercial spas do not qualify. |
| Lakes and glaciers | Named natural lakes and glaciers with an article | Do not route to a lake centre or onto a glacier. Find a legal access point separately. Reservoirs are outside this first proposal. |
| Castles and fortifications | Castles, castle ruins, historic forts, towers and surviving walls | Keep small local ruins. Do not require tourist popularity or a minimum number of language editions. Verify that something remains to see. |
| Archaeology and megaliths | Standing stones, stone circles, dolmens, burial mounds, Roman remains and other visible archaeological sites | Exclude movable museum objects, buried sites with no visible feature, and abstract historical events. |
| Monasteries and abbeys | Historic complexes and their remains | Keep the physical site, not an organization or collection. Ordinary parish churches are not automatic inclusions. |
| Significant architecture | Cathedrals initially; individual notable bridges or other buildings after review | A Wikipedia article or heritage designation alone is too broad. The first automatic count includes cathedrals only. |
| Mountains and passes — decision pending | Named summits and passes with articles | Share identity and content with Peak Viewer. Do not duplicate pins or pack the same photo twice. Count this group separately before deciding where to show it. |

Exclude industry, mines, factories, power plants, dams, railways, stations, ordinary roads,
commercial attractions, shops, restaurants, hotels, sports venues, ordinary buildings,
sculptures, and generic memorials. Find a place already handles practical stops. Exclude town
and administrative-area entries. Parks, valleys, rivers, and large protected areas need an
area-oriented presentation; their centre coordinates are poor nearby landmark pins.

The Gelmerbahn stays in the existing mock UI as a text fixture. It is outside the proposed
production selection because transport and industrial landmarks are excluded.

Do not make a photo mandatory. Do not remove a small ruin because only a local-language
Wikipedia article exists. Known-closed places must be filtered at runtime; unknown hours stay
visible. This count does not evaluate opening hours.

## Source model

Wikipedia does not provide a single clean outdoor-landmark category menu. Use Wikidata's
[instance of](https://www.wikidata.org/wiki/Property:P31) and
[subclass of](https://www.wikidata.org/wiki/Property:P279) relationships for a first candidate
filter, and Wikipedia articles for readable content. Article categories are not our device UI.
One Wikidata identifier is one candidate even when it has several types or language editions.

Use the [image property](https://www.wikidata.org/wiki/Property:P18) as an image candidate,
not proof of a suitable photograph. It can point to a diagram, map, old image, or poor view.
Each accepted image still needs source, author, licence, and adaptation records from Commons.
Wikipedia lead images can also exist without P18, so this is not complete photo coverage.

The two prototype descriptions were edited to fit. Their quality does not prove that an
automatic first-paragraph excerpt will fit or explain every place well. Text extraction and
language handling need a separate content-quality check.

## Measured Swiss candidates

Captured from the Wikidata Query Service on **14 September 2026**. Scope: country P17 is
Switzerland, a coordinate P625 and a type P31 exist, and at least one English, German, French,
or Italian Wikipedia article exists. Match the selected type roots through zero or more P279
steps. Explicit excluded roots take priority. The root identifiers are in
[selection.json](selection.json); captured matching types and English labels are in
[types.json](types.json).

| Group | Candidates | With a P18 image |
| --- | ---: | ---: |
| Natural curiosities | 143 | 112 |
| Lakes and glaciers | 447 | 377 |
| Castles and fortifications | 955 | 896 |
| Archaeology and megaliths | 433 | 357 |
| Monasteries and abbeys | 171 | 138 |
| Cathedrals | 14 | 14 |
| **Distinct core total** | **1,911** | **1,662** |
| Mountains and passes, separate group | 2,353 | 1,747 |
| **Distinct core plus mountains and passes** | **4,255** | **3,400** |

Rows overlap. For example, a castle ruin can also be an archaeological site. The totals use
unique Wikidata IDs, not the sum of the rows. The broad church-and-bridge review group contains
2,074 candidates and is **not** included in either total. It illustrates why architecture needs
a stricter selection than “has a Wikipedia article.”

These are measured candidate counts, not a finished catalogue. Missing country statements,
coordinates, types, or one of the four article languages cause omissions. The country statement
is not a Swiss boundary-polygon test. Bad or broad classification causes false positives. A
sample review found a commercial thermal bath among springs, a reservoir typed only as a lake,
and former religious sites that need a check for surviving remains. The candidate filter does
not yet implement the physical-visibility and significance rules above. The snapshots are for
planning and category review, not for routing to these coordinates.

Only **857 of the 1,911 core candidates have an English article**. A production English pack
needs an explicit language policy. Requiring English now would remove over half of the local
candidates. The other-language entries are included in the storage estimate, not claimed to
have ready English summaries.

## Storage consequence

At one 160 × 120 RGB222 image per candidate with P18:

| Scope | Raw image bytes | Six-bit packed pixels |
| --- | ---: | ---: |
| Core, 1,662 images | 31.91 MB | 23.93 MB |
| Core plus mountains/passes, 3,400 images | 65.28 MB | 48.96 MB |

MB is decimal. These amounts cover pixels only. Text, credits, source URLs, indexes, and file
headers are additional. Even if every core candidate received a photo, pixels would total
36.69 MB. Final image selection can reduce these figures; additional images outside P18 can
increase them. Do not project compression ratios from the two demonstration pictures.
No decoder or map-storage format decision is needed to establish this scale.

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
