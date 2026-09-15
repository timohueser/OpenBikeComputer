---
copy: ai
---

# Map matching: revised acceptance and visual checks

**Status: experimental host prototype.** This is a development study of matching rules. It does not establish true surface conditions, production matching accuracy or an improvement in ETA accuracy.

## Main results

| Activity | Original acceptance | Plausible connected path | Path agreement | Known surface agreement |
| --- | --- | --- | --- | --- |
| MTB | 33.1% | 85.6% | 52.0% | 51.3% |
| other cycling | 46.0% | 93.4% | 71.3% | 83.5% |

![Coverage with distinct matching and attribute checks](/assets/research/ride-time/matching-v2-coverage.svg)

**The original figure mainly measured rejection by a way-identity heuristic.** A plausible path means the best connected sequence passes offset, distance and recording-quality checks. It does not mean competing paths have been ruled out. Path agreement adds the alternative-path check described below. Surface agreement is independent: competing paths can share asphalt even when their geometry is unresolved. These measures are not interchangeable.

On the 20 held-aside sections, the selected path-agreement rule accepts 13: 12 agree with identifiable reviewed paths, 0 disagree, and 1 cannot be judged from the overlay. This is encouraging, but the sample is too small and the labels too limited to certify production accuracy.

## What changed

1. Candidate projections at the same graph node are one state, even when several OSM ways meet there. Unconnected crossings stay separate.
2. The confidence check considers complete transitions between samples, with the cost of the surrounding sequence. It no longer tests the two endpoint way identifiers in isolation.
3. Alternative positions on one unbranched chain represent the same path. Chains stop at branches; closed rings are not collapsed. This separates along-road position uncertainty from road identity.
4. For other routes, up to 20 m at each endpoint may differ, but the central edges must agree and at least half of the longer path must overlap. Parallel roads and internal diversions do not become equivalent merely because they are close.
5. Every considered transition contributes its complete attribute composition. Missing tags stay unknown. Attributes are never inferred from the first and last samples alone.

### Attribute agreement

For each value, retain the smallest distance fraction across the considered paths. If the alternatives contain 70% asphalt / 30% gravel and 50% asphalt / 50% gravel, the retained composition is 50% asphalt / 30% gravel; 20% remains unresolved. If all alternatives are asphalt, the surface can be retained even without exact path agreement. A zero-length alternative supplies unknown composition.

The coverage table allocates these common fractions to original GPS chord distance, as in the first pilot. This is a lower bound across the considered compositions, not an identification of which exact metres carry each value. The visual surface check below assesses only a single known value supported at 100%, not partially agreed mixtures.

## Fixed visual review

Forty recording-screen-passing intervals were selected by stable hash before examining revised predictions. Twenty development sections came from previously inspected riders. Twenty validation sections each came from a different rider whose tracks had not been visually inspected in the earlier pilot or ambiguity diagnosis. The cohorts contain disjoint riders. No interval was selected for match success or an attractive error result.

The assistant labeled GPS/OSM overlays that did not show matcher selections or scores. Clearly identifiable paths received an allowed set of OSM ways. Unclear sections remained unjudgeable. These are visual judgments from the same recording and map, not independent field observations or a second human reviewer. Historical map errors remain possible.

Development checks compared endpoint tolerances of 0, 5, 10, 15 and 20 m. The recorded configuration selects 20 m: it retained the most reviewed paths without a clear error in that small development set. Matcher source and settings were frozen before validation labels were recorded or validation predictions inspected. Unsupervised coverage across the full 80-ride pilot was available during development.

### Development sections

| Endpoint tolerance | Accepted | Correct among judgeable | Incorrect | Unjudgeable |
| --- | --- | --- | --- | --- |
| 0 m | 8 | 8 | 0 | 0 |
| 5 m | 8 | 8 | 0 | 0 |
| 10 m | 10 | 10 | 0 | 0 |
| 15 m | 10 | 10 | 0 | 0 |
| 20 m | 13 | 13 | 0 | 0 |

### Validation sections: 20 different riders

| Rule | Accepted / 20 | Correct | Incorrect | Unjudgeable | Error interval, 95% Wilson |
| --- | --- | --- | --- | --- | --- |
| Plausible path | 20 | 18 | 0 | 2 | 0.0–17.6% |
| Path agreement, 20 m | 13 | 12 | 0 | 1 | 0.0–24.2% |
| Full surface agreement | 8 | 7 | 0 | 1 | 0.0–35.4% |

Correctness of a path means its selected central way IDs fall within the reviewed allowed set. Surface correctness means agreement with the OSM surface on the visually identified path; it does not verify the physical material. Unjudgeable accepted sections are shown separately and excluded from the error-rate denominator. The Wilson intervals describe sampling uncertainty conditional on these imperfect labels. The small review cannot certify a low production error rate.

## Remaining limitations

- The search retains at most eight candidate states per sample and one shortest path for each endpoint pair. It does not enumerate all internal detours between identical endpoints. Agreement is conditional on that search space.
- The cost margin of three is still an uncalibrated heuristic. Small visual checks do not turn it into a probability.
- Sparse samples, missing paths and changed geometry can make an incorrect path appear to be the best available one.
- These are 2012–2015 rides matched to a 2026 OSM snapshot in central Jutland. The pilot does not represent worldwide coverage or mountainous trail conditions.
- Matching uses future coordinates within continuous recording blocks. Device learning and ETA evaluation need causal observation matching and a separately known planned route.

## Recommendation

Use the revised report to separate road-finding failures from uncertainty in accepted attributes. Keep ambiguous and missing attributes explicit. Before device integration, obtain recent dense rides with known paths and increase the blinded review sample. Keep the ETA model unchanged until enriched observations have a credible validation protocol.

## Artifacts and reproduction

The [prototype README](src:host/ride-time-prototype/README.md) gives the commands. The portable local report includes the ten private review sheets. Public documentation contains only aggregate results. [Original enrichment audit](/docs/software/ride-time-enrichment/).

Map data © [OpenStreetMap contributors](https://www.openstreetmap.org/copyright), ODbL 1.0; extract by Geofabrik. Original ride data: [FitRec](https://cseweb.ucsd.edu/~jmcauley/datasets/fitrec.html). No ride coordinates were sent to a matching service.
