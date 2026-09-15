---
copy: ai
---

# OSM surface enrichment: feasibility pilot

**Status: development-only feasibility audit.** This experiment measures map association and attribute availability. It does not measure an improvement in ETA accuracy. A match that passes the checks is not a verified trail label.

## Findings

- **Surface enrichment is possible:** 63.9% of accepted MTB distance has an explicit surface tag.
- **Matching is the main current bottleneck:** 33.1% of recorded MTB distance passes the conservative checks; 54.4% is rejected for ambiguity between OSM ways. That is a limit of this matcher and screen, not a measurement of missing OSM paths.
- **Technical ratings are too sparse to require:** `mtb:scale` covers 9.3% of accepted MTB distance, or 3.1% of all recorded MTB distance.
- Combining matching and tag availability, explicit surface labels cover **21.1% of recorded MTB distance**. Coverage within accepted matches alone would overstate the amount of usable evidence.

## 1. Scope and selection

The pilot contains **80 rides from 33 riders**, including 40 MTB rides, entirely inside 9–10°E and 56–57°N in central Jutland, Denmark. A survey of the 18,929 eligible development histories ranked one-degree cells by distinct MTB riders. This cell ranked first. After requiring complete containment, 364 rides from 33 riders were eligible, including 138 MTB rides. Selection uses stable hashes and balances riders within 40 MTB and 40 other cycling slots. It does not use prediction errors, tag presence or match success.

Ride dates range from **2012-02-04 to 2015-12-29**. The OSM snapshot is **2026-09-13T20:21:20Z**. This 10–14 year mismatch can change geometry, connectivity and attributes. Current tags do not establish historical trail conditions. The geographic selection is useful for this pilot, but does not represent mountain terrain or worldwide mapping coverage.

## 2. Matching coverage

![Matching coverage and rejection reasons](/assets/research/ride-time/matching-coverage.svg)

| Activity | Rides | Riders | Recorded km | Accepted km | Accepted / recorded | Accepted / screened |
| --- | --- | --- | --- | --- | --- | --- |
| all | 80 | 33 | 2028.1 | 839.5 | 41.4% | 43.8% |
| MTB | 40 | 15 | 721.6 | 238.9 | 33.1% | 35.2% |
| other cycling | 40 | 22 | 1306.5 | 600.6 | 46.0% | 48.5% |

Distances use the original GPS chords. The screened denominator contains only intervals accepted by the original 30-second timing and geometry screen. The recorded denominator also includes excluded intervals. These are pooled distance shares in a deliberately balanced pilot, not population estimates.

## 3. Availability of explicit attributes

![Explicit tag coverage on accepted distance](/assets/research/ride-time/tag-coverage.svg)

| Tag | MTB / accepted | MTB / recorded | Other / accepted | Other / recorded |
| --- | --- | --- | --- | --- |
| `highway` | 100.0% | 33.1% | 100.0% | 46.0% |
| `surface` | 63.9% | 21.1% | 95.7% | 44.0% |
| `smoothness` | 0.8% | 0.3% | 2.1% | 1.0% |
| `tracktype` | 26.0% | 8.6% | 3.8% | 1.8% |
| `mtb:scale` | 9.3% | 3.1% | 1.6% | 0.7% |
| `mtb:scale:uphill` | 0.0% | 0.0% | 0.0% | 0.0% |

An absent tag stays **unknown**. No surface is inferred from activity label or road class. Generic `unpaved` remains distinct from gravel. A missing `mtb:scale` does not mean easy. For intervals crossing several ways, tags are weighted by reconstructed path length, then allocated to that interval's original GPS chord distance. This is a distance allocation, not a claim about time spent on each surface.

### Most common surface values on accepted distance

| Activity | Surface | Distance km | Share of accepted |
| --- | --- | --- | --- |
| MTB | unknown | 86.4 | 36.1% |
| MTB | asphalt | 66.6 | 27.9% |
| MTB | gravel | 51.7 | 21.6% |
| MTB | dirt | 17.7 | 7.4% |
| MTB | compacted | 6.0 | 2.5% |
| MTB | ground | 5.8 | 2.4% |
| MTB | grass | 1.9 | 0.8% |
| MTB | fine_gravel | 1.3 | 0.5% |
| other cycling | asphalt | 550.4 | 91.6% |
| other cycling | unknown | 25.5 | 4.3% |
| other cycling | gravel | 16.9 | 2.8% |
| other cycling | compacted | 3.9 | 0.7% |
| other cycling | dirt | 2.2 | 0.4% |
| other cycling | ground | 0.9 | 0.1% |
| other cycling | fine_gravel | 0.4 | 0.1% |
| other cycling | pebblestone | 0.4 | 0.1% |

### Most common road/path types on accepted distance

| Activity | OSM highway type | Distance km |
| --- | --- | --- |
| MTB | track | 86.9 |
| MTB | path | 67.4 |
| MTB | tertiary | 25.0 |
| MTB | cycleway | 24.7 |
| MTB | unclassified | 19.3 |
| MTB | service | 7.6 |
| MTB | residential | 3.4 |
| other cycling | tertiary | 330.6 |
| other cycling | unclassified | 107.7 |
| other cycling | secondary | 86.5 |
| other cycling | track | 24.3 |
| other cycling | cycleway | 19.8 |
| other cycling | path | 14.5 |
| other cycling | primary | 12.3 |

## 4. Matching method and its limits

The graph contains line ways with `highway` tags. Area polygons, motorway ways and proposed, construction or abandoned highways are excluded. Other access and one-way restrictions are not enforced: the task reconstructs observed travel. A buffered bounding box limits extraction. No GPS coordinates are sent to a matching service.

At each sample, the matcher considers up to eight nearby segments within 40 m, at most two per OSM way. A minimum-cost sequence combines GPS offset with the difference between connected-path length and GPS chord length. No learned speed or surface preference enters the matching cost. Gaps above 30 seconds, nonpositive time steps, large jumps and disconnected paths split the sequence.

An accepted interval must pass the existing recording screen, have endpoint offsets at most 20 m, and have an alternative-way cost margin of at least three at both endpoints. Matched length must be 0.5–1.5 times chord length and differ by at most 60 m. The margin is a heuristic, not a calibrated probability. Missing mapped paths or discarded candidates can still produce an incorrect accepted match. Segment identity within one OSM way is not treated as a different-way alternative.

Offline sequence matching uses later coordinates within each continuous block. This audit therefore does not validate a causal device matcher or a live ETA replay with enriched attributes. A later ETA experiment must use a genuinely known planned route and causal observation features.

The extracted graph contains 177,616 ways and 1,100,536 segments. Matching and audit took 118.6 seconds on the host. This is not an nRF54 runtime estimate.

## 5. Visual spot checks

Example overlays show recorded samples, candidate segments and nearby OSM geometry. They are private artifacts because they contain source ride geometry. Cases are selected to illustrate acceptance and failure modes, not to estimate a match-error rate. Geometry inspection alone cannot verify surface material.

- Example 1 rejects a junction approach as ambiguous. The GPS trace is also visibly offset from the mapped approach. This illustrates why rejection does not establish that the path is absent; the heuristic margin can reject a plausible road sequence.
- Example 2 follows a winding mapped MTB path near the inspected interval. The matched way has mtb:scale=2 but no surface tag. Technical difficulty and surface availability are separate questions.
- Example 3 shows closely spaced, turning GPS samples on that trail. The path-length screen rejects the inspected interval. Dense turns and GPS jitter can make chord distance a poor reference even when the overall path looks plausible.
- Example 4 has close geometric agreement on a way tagged gravel and mtb:scale=1. This is a useful candidate for enrichment; the overlay does not independently verify those tags.
- Example 5 has sparse straight chords through a winding trail network and a missing candidate at an interval endpoint. The geometry cannot distinguish GPS error, an unmapped historic trail, or a changed trail layout.
- Example 6 shows close geometric agreement on a residential segment tagged asphalt. Nearby parallel routes remain visible in the broader example.
- These six deliberately selected cases are visual spot checks, not ground-truth labels or an estimate of match precision.

## 6. Next decision

Proceed with surface and road/path type as candidate inputs, with an explicit unknown state. Use MTB difficulty only as optional evidence. Before enlarging the learner, improve and validate the matching acceptance rule: ambiguity between OSM way identifiers can be harmless when the plausible paths carry the same attributes, but this pilot does not yet establish that agreement across complete alternative paths. Obtain recent dense rides with known route geometry, and compare a near-contemporary OSM snapshot for historical FitRec rides. Keep this pilot as a development audit; it does not resolve the earlier mountain-bike prediction-range failure.

Before an ETA comparison, use development data to define a small surface/path model and an uncertainty policy for unknown attributes. Compare models on identical issued targets and report both tag coverage and forecast accuracy. Reusing inspected test riders for tuning would not provide a new independent evaluation. Dense original rides remain necessary for moving-time labels, pushing and long forecasts.

## 7. Reproduction and provenance

The [prototype README](src:host/ride-time-prototype/README.md) gives commands. [OSM source extract](https://download.geofabrik.de/europe/denmark-260913.osm.pbf) · [FitRec source](https://cseweb.ucsd.edu/~jmcauley/datasets/fitrec.html).

Map data © [OpenStreetMap contributors](https://www.openstreetmap.org/copyright), ODbL 1.0; extract by Geofabrik. The OSM source hash, extraction bounds, matching settings and source-code hashes are in the aggregate JSON. Raw archives, selected rider identifiers, coordinates and per-interval attributes remain outside tracked source.
