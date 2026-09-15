---
copy: ai
---

# Ride-time accuracy with OSM path and surface data

**Status: experimental Python study.** The shared corrections are fitted on development riders. The final comparison uses the original test riders. It assumes the future route is known from the recorded track.

## Main result

**This regional test does not show a clear improvement from the combined path and surface correction.**

After 10 accepted minutes, the primary comparison changes rider-weighted mean absolute percentage error by **+0.89 percentage points** versus the bike-category model. The paired rider bootstrap interval is **-0.29 to +3.00 points** (95%). Error falls for 5 of 9 scored riders. Negative changes mean improvement.

![Test-set error at start and after ten accepted minutes](/assets/research/ride-time/surface-accuracy.svg)

The test cohort contains only three MTB riders before forecast screening, and two supply scored live forecasts. MTB results are descriptive; many forecasts from three people do not create a large independent sample.

## Data and separation

| Split | Rides | Riders | MTB rides | MTB riders |
| --- | --- | --- | --- | --- |
| development | 364 | 33 | 138 | 15 |
| calibration | 259 | 12 | 54 | 4 |
| test | 205 | 11 | 22 | 3 |

Use every eligible ride fully contained in longitude 9–10° E and latitude 56–57° N. The region was chosen from development data for the earlier matching pilot. Keep the original rider hash split, first 60 candidate records and overlap exclusions. Only regional rides contribute to personal history in this study. The shared gradient curve comes from the original development-only fit. All four models also receive a development-fitted regional offset, so this is a new regional comparison, not a rerun of the original global benchmark.

The previous global test results were already known. No regional calibration or test forecast errors were used to choose these coefficients or settings. Source code, match caches, cohort and fit were frozen before either cohort was replayed. Calibration results are a diagnostic comparison; they did not trigger tuning.

## Test accuracy

MAPE is the mean absolute percentage error. Average errors within each rider, then give riders equal weight. Signed bias uses the same weighting: positive means the estimate is too long. The 90th-percentile error is pooled over forecasts and gives frequent riders more weight. Each model receives exactly the same offered and scored targets within a phase. Start and live phases have different scored targets and riders, so their difference is not a paired estimate of the benefit from waiting ten minutes.

### At the original ride start

| Activity | Model | Riders | Targets | MAPE | Bias | 90th-percentile error |
| --- | --- | --- | --- | --- | --- | --- |
| all | Gradient | 10 | 324 | 15.99% | -2.46% | 19.38% |
| all | + bike category | 10 | 324 | 13.68% | -2.87% | 19.01% |
| all | + path type | 10 | 324 | 12.70% | -1.51% | 18.51% |
| all | + surface | 10 | 324 | 12.77% | -1.41% | 18.52% |
| MTB | Gradient | 3 | 14 | 23.33% | -21.94% | 28.37% |
| MTB | + bike category | 3 | 14 | 14.59% | -10.22% | 19.28% |
| MTB | + path type | 3 | 14 | 14.29% | -9.07% | 23.20% |
| MTB | + surface | 3 | 14 | 14.62% | -8.85% | 22.86% |
| other | Gradient | 8 | 310 | 12.52% | +4.81% | 18.49% |
| other | + bike category | 8 | 310 | 12.32% | -0.08% | 18.60% |
| other | + path type | 8 | 310 | 11.21% | +1.51% | 18.45% |
| other | + surface | 8 | 310 | 11.22% | +1.59% | 18.45% |

### After ten accepted minutes

| Activity | Model | Riders | Targets | MAPE | Bias | 90th-percentile error |
| --- | --- | --- | --- | --- | --- | --- |
| all | Gradient | 9 | 472 | 7.68% | +1.13% | 15.28% |
| all | + bike category | 9 | 472 | 7.76% | +0.88% | 15.54% |
| all | + path type | 9 | 472 | 8.70% | -0.59% | 15.45% |
| all | + surface | 9 | 472 | 8.66% | -0.53% | 15.48% |
| MTB | Gradient | 2 | 37 | 13.34% | +0.31% | 32.20% |
| MTB | + bike category | 2 | 37 | 14.30% | +4.01% | 33.14% |
| MTB | + path type | 2 | 37 | 14.04% | +3.05% | 30.33% |
| MTB | + surface | 2 | 37 | 14.25% | +2.84% | 29.64% |
| other | Gradient | 8 | 435 | 6.56% | +1.95% | 14.00% |
| other | + bike category | 8 | 435 | 6.27% | +0.76% | 13.97% |
| other | + path type | 8 | 435 | 7.21% | -1.01% | 13.97% |
| other | + surface | 8 | 435 | 7.10% | -0.94% | 14.07% |

### Separate the surface contribution

| Activity | Comparison after 10 min | MAPE change | 95% paired rider bootstrap |
| --- | --- | --- | --- |
| all | Path + surface versus bike | +0.89 pp | -0.29 to +3.00 pp |
| all | Surface versus path | -0.05 pp | -0.32 to +0.14 pp |
| MTB | Path + surface versus bike | -0.04 pp | -0.85 to +0.76 pp |
| MTB | Surface versus path | +0.21 pp | +0.08 to +0.35 pp |
| other | Path + surface versus bike | +0.84 pp | -0.92 to +3.41 pp |
| other | Surface versus path | -0.11 pp | -0.41 to +0.11 pp |

These intervals resample riders with replacement 20,000 times, retaining each rider's paired mean errors. They describe uncertainty in the average comparison conditional on the fitted models and this region. They do not include training uncertainty or map errors. With only two MTB riders in the live comparison, bootstrap tails cannot describe the diversity of MTB users. Secondary and subgroup comparisons are exploratory, without correction for multiple comparisons. These are not ETA prediction intervals for an individual ride.

### Post-hoc failure inspection

Most of the mean worsening comes from one rider with 3 scored live targets: error rises from 3.54% to 12.59%. That rider remains in the primary result. This is a failure inspection selected after scoring, not a separate validation sample.

In the observed prefix, 85% of accepted distance carries the path-group feature and 91% has agreed asphalt. In the next 10 km, those figures are 2% and 95%. The shared model assigns a roughly 19% pace penalty to the path group, including paved paths. The live adjustment can absorb that excessive baseline penalty as unusually good rider form, then carry the speedup onto the following road. This is a plausible explanation for the failure, not proof of the historical road or surface. It identifies a risk of applying one path penalty across materials. No model was retuned and no rider was removed in response.

### Calibration diagnostic

| Activity after 10 min | Gradient | + bike category | + path type | + surface |
| --- | --- | --- | --- | --- |
| all | 10.05% | 9.95% | 9.93% | 10.05% |
| MTB | 15.36% | 15.88% | 15.81% | 16.15% |
| other | 9.56% | 9.41% | 9.39% | 9.52% |

## How much route information is available?

| Split | Activity | Recorded km | Route-proxy surface | Past-only surface | Route-proxy highway | Past-only highway |
| --- | --- | --- | --- | --- | --- | --- |
| development | MTB | 2618 | 48.3% | 46.1% | 62.9% | 59.7% |
| development | other | 7805 | 78.8% | 77.1% | 76.6% | 74.0% |
| calibration | MTB | 1575 | 61.8% | 60.0% | 75.7% | 73.6% |
| calibration | other | 6242 | 83.7% | 81.8% | 76.7% | 73.7% |
| test | MTB | 429 | 41.2% | 39.8% | 75.3% | 73.3% |
| test | other | 5547 | 89.4% | 87.8% | 81.9% | 79.0% |

Percentages allocate agreed tag fractions to all original GPS chord distance, including intervals that fail the outcome screen. Unlike the earlier matching audit, feature extraction does not use measured speed to accept or reject a match. It retains the geometry, offset and gap checks. Therefore these figures have a different cohort and acceptance screen from the 80-ride audit. Attribute agreement remains conditional on the candidate paths considered; it does not verify physical material or historical map accuracy.

## Model and safeguards

For an interval, predicted pace is `exp(b(grade) + clip(x·beta, -ln(3), ln(3)) + personal + live)`. Here `b` is the fixed log-pace gradient curve. The shared features are a regional offset, an MTB indicator, three path fractions and four surface fractions. Forecast time is the sum of distance times predicted pace. The live factor applies to the entire remaining target. At ride start it is one.

The models add features in this order: regional offset; bike category; track/path/cycleway; firm/soft/rough/generic unpaved surfaces. Path includes path, footway, bridleway and steps. Firm groups compacted, fine gravel, gravel and pebblestone. Soft groups ground, dirt, earth, grass, sand, mud and clay. Rough groups cobblestone, sett, unhewn cobblestone, rock and stone. Unknown and other values add zero correction; they are not declared paved. The map retains raw tags before grouping.

Fractions are lower bounds common to the considered paths. They describe composition, not the position of each material. Weighting log corrections by these fractions is a simple approximation for mixed intervals; it is not an exact arithmetic sum over known surface subsegments. There are no gradient interactions, personal surface coefficients, trail-rating terms or weather inputs in this comparison.

Fit the shared coefficients once with bounded ridge least squares. Clip observed log-pace residuals to ±ln(3). Within each learnable ride, weight observations by distance and normalize their total to one. Divide that weight by the rider's number of learnable rides. Center design and target within each ride, retaining half of the mean, so between-ride residuals have one quarter of the squared-error weight. Ridge strength is 0.25 per coefficient and 0.1 for the regional offset. Bound each coefficient to ±ln(2), and the offset to ±ln(1.5). Cap the combined correction to a factor from 1/3 to 3. No parameter sweep was performed.

Each model then uses the original bounded scalar personal learner and five-minute live adjustment. Personal state stays fixed through the ride and updates at ride end. Past-only matching feeds both personal learning and live updates. Forecasts are issued before consuming the next observation.

### Shared fitted multipliers

| Feature | Gradient | + bike category | + path type | + surface |
| --- | --- | --- | --- | --- |
| regional_offset | 0.988× | 0.901× | 0.896× | 0.897× |
| mtb | — | 1.368× | 1.360× | 1.368× |
| track | — | — | 0.926× | 0.959× |
| path | — | — | 1.188× | 1.194× |
| cycleway | — | — | 0.991× | 0.989× |
| firm | — | — | — | 0.929× |
| soft | — | — | — | 0.972× |
| rough | — | — | — | 1.001× |
| unpaved | — | — | — | 0.989× |

These are fitted predictive associations, not physical speed penalties. Correlated bike, path and surface features can share the same effect. A coefficient near one can mean weak support rather than no real effect.

## Forecast selection and practical limits

| Phase | Target km | Offered | Scored | Median actual min |
| --- | --- | --- | --- | --- |
| Start | 1 | 196 | 124 | 2.4 |
| Start | 3 | 195 | 106 | 6.8 |
| Start | 10 | 187 | 78 | 22.1 |
| Start | 30 | 71 | 16 | 63.5 |
| After 10 min | 1 | 192 | 177 | 2.4 |
| After 10 min | 3 | 189 | 158 | 6.9 |
| After 10 min | 10 | 178 | 116 | 22.7 |
| After 10 min | 30 | 64 | 21 | 62.8 |

Targets are the first sample boundary at least 1, 3, 10 or 30 km ahead. Score a target only if every intervening interval passes the original screen: at most 30 seconds, at least 3 m displacement, 1–80 km/h, and slope within ±50%. Personal learning additionally requires at least 10 m and adjacent slope change at most two percentage points. Rejected windows are not joined or replaced. The outcome is movement-screened elapsed time, not verified moving time. Hidden short stops remain, while some pushing and difficult terrain are excluded.

Recorded future GPS geometry, altitude and offline tags substitute for a known planned route. No independent planned route exists here. Past-only matching prevents future coordinates from entering observation updates, but the route forecast remains retrospective. The current OSM snapshot can differ from the recorded roads and surfaces from 2009–2015. This is one lowland region, with few independent MTB test riders and limited long-horizon evidence. Recent rides with known routes and dense samples remain necessary.

No individual-ride uncertainty bands were refitted in this small study. The previous global bands cannot be assumed calibrated after changing the model. This report assesses point accuracy and paired sampling uncertainty.

## Device cost and next decision

The test replay took 6.5 seconds on the host, excluding map matching and fitting. A scalar ride-end update took a median 0.066 ms and a maximum 0.291 ms in this run. Its persistent and current-ride NumPy arrays total 32 bytes. This excludes Python objects, route storage, the live state, prediction-range buffers and scratch memory.

The largest shared model needs nine coefficients: 36 bytes as float32. The personal learner still has one parameter and fixed scalar sufficient statistics. Evaluating a segment adds at most nine products, a clamp and the existing exponential; it does not fit a shared model at ride end. The shared fit runs only on the host. The local OSM graph and Python matcher are research tools and do not meet the device memory budget. Route attribute storage and device map matching are separate unresolved integration work. Host array sizes and timings do not establish complete nRF54LM20 memory or execution time.

Keep the simpler personal scalar and live adjustment as the implementation candidate. This run does not justify a blanket path penalty or the added material coefficients. The small surface-only change also does not establish that real surface conditions are unimportant. Preserve the enrichment pipeline and test recent rides with known surfaces, especially transitions between paved paths, roads and technical trails. A small combined road/surface classification is a candidate for that next independent experiment. Keep firmware integration and personal terrain parameters pending broader evidence.

## Reproduction and sources

The [prototype README](src:host/ride-time-prototype/README.md) gives the extraction, matching, freeze, replay and report commands. The portable report embeds its chart. Public artifacts contain aggregates; raw coordinates and per-forecast records remain private. [Earlier matching study](/docs/software/ride-time-matching/).

Ride data: [FitRec](https://cseweb.ucsd.edu/~jmcauley/datasets/fitrec.html). Map data © [OpenStreetMap contributors](https://www.openstreetmap.org/copyright), ODbL 1.0; extract supplied by Geofabrik. No ride coordinates were sent to a matching service.
