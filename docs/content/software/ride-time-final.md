---
copy: ai
---

# Final ride-time model: consistency check

**The first research implementation is specified and tested. Prediction-range guarantees and device integration remain unvalidated.** This check uses the final scalar model with fixed bike defaults. Parameters and calibration references were frozen before evaluation.

## Main results

| Group | Riders | Targets | Mean error | Bias | P90 error | Range coverage | Above upper bound | Median width / prediction |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Ride start | 168 | 6555 | 15.39% | -5.86% | 29.57% | 89.54% | 6.28% | 70.4% |
| After 10 accepted minutes | 172 | 9660 | 11.39% | +6.89% | 23.81% | 89.85% | 4.80% | 38.3% |

The check covers **7,114 test rides from 185 riders**. It scores 16,215 of 43,825 offered targets (37.0%). After ten accepted minutes, point error averages **11.39%**, and the nominal 90% range covers **89.85%** of outcomes with equal rider weight.

For MTB, coverage is **77.85% at start** and **77.20% in ride**. These subgroup results must not be hidden behind overall coverage. The current ranges do not establish a universal 90% product claim or a worst-case water-gap duration.

![History-dependent accuracy and empirical prediction-range coverage](/assets/research/ride-time/final-results.svg)

Chart error bars are 95% rider-bootstrap intervals for the aggregate metrics. They are not ranges for an individual ETA.

## Frozen algorithm

The [algorithm specification](/docs/software/ride-time-estimation/) defines the equations, defaults, state, ride lifecycle, and device input contract. The model has a fixed nine-anchor gradient curve, fixed MTB/other defaults, one slowly learned personal log multiplier shared across categories, and a temporary log multiplier with a five-minute moving-time half-life. The temporary factor applies to the entire remaining route.

There are no personal gradient, path, or surface corrections. The scalar ride-end update is a bounded division, with the same objective and movement bounds as the general learner. Each range uses one of six phase/duration groups, 64 fixed reference knots with total weight 32, and at most 32 personal errors. Personal pace and range buffers update only at original ride completion.

## Data separation and interpretation

Fit the bike defaults on 18,566 learnable rides from 497 development riders. Recalibrate both models on 6,491 rides from 177 different riders. The shared gradient curve comes from the original development-only fit. No OSM or phone processing is used in this final replay.

The fixed rider hash split, first-60 candidate limit, and overlap exclusions remain unchanged. A rider's state contains only earlier completed rides. Forecasts occur at the original start and the first boundary after ten accepted moving minutes, before the next observation is consumed.

Earlier aggregate results from these test riders were already inspected during model selection. This is a consistency check of the selected configuration, not a new independent test. No parameter or range changes were made in response to these results.

## Point accuracy and range behaviour

Mean error is mean absolute percentage error (MAPE), averaged within each rider and then across riders. Bias and coverage use the same weighting; positive bias means an estimate that is too long. P90 error and median range width pool forecasts and give frequent riders more weight. Width is `(upper - lower) / point estimate`. An upper exceedance means the actual duration is above the range's upper edge. The nominal central 90% target would leave about 5% above that edge under ideal calibration.

Both models receive identical targets within each phase. Start and live phases have different scored targets and riders; their difference is not a paired estimate of the benefit of waiting ten minutes.

### Ride start

| Group | Riders | Targets | Mean error | Bias | P90 error | Range coverage | Above upper bound | Median width / prediction |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Shared defaults | 168 | 6555 | 15.68% | -6.42% | 30.08% | 90.18% | 6.03% | 72.2% |
| Bike defaults (final) | 168 | 6555 | 15.39% | -5.86% | 29.57% | 89.54% | 6.28% | 70.4% |

| Group | Riders | Targets | Mean error | Bias | P90 error | Range coverage | Above upper bound | Median width / prediction |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| other | 162 | 6001 | 15.43% | -6.46% | 28.83% | 90.26% | 6.24% | 70.2% |
| MTB | 43 | 554 | 18.47% | -4.44% | 34.65% | 77.85% | 11.11% | 73.0% |

### After ten accepted minutes

| Group | Riders | Targets | Mean error | Bias | P90 error | Range coverage | Above upper bound | Median width / prediction |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Shared defaults | 172 | 9660 | 11.35% | +6.65% | 23.96% | 90.00% | 4.90% | 39.1% |
| Bike defaults (final) | 172 | 9660 | 11.39% | +6.89% | 23.81% | 89.85% | 4.80% | 38.3% |

| Group | Riders | Targets | Mean error | Bias | P90 error | Range coverage | Above upper bound | Median width / prediction |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| other | 167 | 8901 | 11.19% | +6.89% | 23.44% | 90.67% | 4.37% | 37.6% |
| MTB | 47 | 759 | 14.51% | +3.75% | 28.07% | 77.20% | 15.96% | 39.0% |

### Paired change from bike defaults

| Phase | MAPE change | 95% paired rider bootstrap | Riders |
| --- | --- | --- | --- |
| Start | -0.287 pp | -0.556 to -0.044 pp | 168 |
| In ride | +0.033 pp | -0.039 to +0.102 pp | 172 |

Negative change means lower error. The intervals resample riders 2,000 times with a fixed seed, keeping paired mean errors together. They describe the comparison conditional on this fit and dataset. They are not individual-ride prediction intervals and do not include model-selection or training uncertainty.

## Riders with limited history

History is accepted learning distance before the current ride, not total lifetime distance or annual mileage. Zero history can also occur after an earlier ride supplied no learnable observations. These are descriptive groups with changing riders and routes, not a controlled learning curve.

### Start by previous learning distance

| Group | Riders | Targets | Mean error | Bias | P90 error | Range coverage | Above upper bound | Median width / prediction |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Zero | 73 | 149 | 16.42% | -4.50% | 33.34% | 86.42% | 8.90% | 73.0% |
| 0 < km < 50 | 88 | 228 | 16.02% | -3.39% | 33.28% | 85.85% | 6.60% | 73.0% |
| 50 ≤ km < 200 | 130 | 741 | 17.22% | -3.48% | 30.50% | 88.47% | 4.88% | 73.0% |
| 200 km or more | 142 | 5437 | 14.45% | -6.60% | 29.36% | 92.40% | 4.44% | 68.0% |

### In ride by previous learning distance

| Group | Riders | Targets | Mean error | Bias | P90 error | Range coverage | Above upper bound | Median width / prediction |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Zero | 102 | 207 | 11.28% | +6.89% | 22.94% | 88.97% | 3.10% | 39.0% |
| 0 < km < 50 | 128 | 415 | 13.29% | +7.34% | 27.34% | 83.06% | 7.48% | 39.0% |
| 50 ≤ km < 200 | 149 | 1194 | 11.46% | +7.79% | 25.53% | 90.99% | 2.68% | 39.0% |
| 200 km or more | 143 | 7844 | 11.58% | +7.90% | 23.26% | 91.48% | 3.61% | 36.5% |

## Coverage by predicted duration

| Group | Riders | Targets | Mean error | Bias | P90 error | Range coverage | Above upper bound | Median width / prediction |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Start: <10 min | 168 | 4949 | 16.20% | -7.89% | 30.92% | 90.62% | 5.54% | 73.0% |
| Start: 10–<30 min | 140 | 1175 | 11.60% | +3.25% | 23.36% | 83.77% | 8.96% | 38.1% |
| Start: 30+ min | 100 | 431 | 10.60% | +6.28% | 22.09% | 92.64% | 3.14% | 37.9% |
| In ride: <10 min | 172 | 7239 | 11.34% | +6.59% | 24.13% | 91.05% | 4.26% | 39.0% |
| In ride: 10–<30 min | 157 | 1833 | 11.15% | +7.44% | 22.11% | 86.70% | 6.00% | 30.3% |
| In ride: 30+ min | 112 | 588 | 11.31% | +8.36% | 25.24% | 86.71% | 6.82% | 31.6% |

### MTB duration groups

| Group | Riders | Targets | Mean error | Bias | P90 error | Range coverage | Above upper bound | Median width / prediction |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Start: <10 min | 43 | 442 | 18.73% | -4.58% | 34.57% | 82.40% | 7.36% | 73.1% |
| Start: 10–<30 min | 27 | 79 | 16.79% | +1.86% | 35.01% | 62.79% | 18.91% | 38.5% |
| Start: 30+ min | 18 | 33 | 17.81% | +5.66% | 41.30% | 70.37% | 16.20% | 37.9% |
| In ride: <10 min | 47 | 539 | 14.84% | +3.84% | 30.15% | 80.68% | 12.81% | 41.7% |
| In ride: 10–<30 min | 36 | 161 | 11.79% | +4.22% | 24.77% | 77.07% | 16.90% | 32.6% |
| In ride: 30+ min | 20 | 59 | 10.01% | +3.25% | 17.32% | 72.22% | 24.45% | 31.6% |

The predicted duration selects the group. The 30+ minute group is not evidence of reliable two-hour ranges. Only 1 scored outcomes lasted at least two hours.

### Calibration support

| Group | Reference outcomes | Calibration riders | Pooled fallback |
| --- | --- | --- | --- |
| Start: <10 min | 2892 | 163 | False |
| Start: 10–<30 min | 1140 | 127 | False |
| Start: 30+ min | 404 | 85 | False |
| In ride: <10 min | 4126 | 168 | False |
| In ride: 10–<30 min | 1686 | 147 | False |
| In ride: 30+ min | 489 | 103 | False |

Only the first prospectively reserved target per group and ride supplies an error. Missing outcomes are not replaced with more convenient targets. The reference distributions pool bike categories; per-category coverage is an evaluation result, not an enforced property. More personal history need not narrow a range.

## Outcome availability and remaining limits

| Phase | Target km | Offered | Scored |
| --- | --- | --- | --- |
| Start | 1.0 | 6797 | 2985 |
| Start | 3.0 | 6636 | 2062 |
| Start | 10.0 | 5768 | 1146 |
| Start | 30.0 | 3566 | 362 |
| In ride | 1.0 | 6402 | 4384 |
| In ride | 3.0 | 6180 | 3145 |
| In ride | 10.0 | 5347 | 1689 |
| In ride | 30.0 | 3129 | 442 |

A target is scored only if every intervening interval passes the original recording screen. There is no joining of disjoint valid windows. The screen requires spacing up to 30 seconds, at least 3 m displacement, 1–80 km/h interval-average speed, and slope within ±50%. Personal learning additionally requires at least 10 m and adjacent slope change no greater than two percentage points.

These filters exclude some pushing and difficult terrain and permit hidden short stops. Recorded future geometry and altitude stand in for a known route. Results measure a movement-screened elapsed-time proxy. They do not validate true moving time, route deviations, intermediate live checkpoints, or arbitrary forecast horizons. Recent dense rides with reliable movement labels remain necessary.

## Numerical checks and device cost

No scalar observation or ride-end update was rejected in either replay. The scalar specialization matches the original scalar model on all 16,215 scored targets. The maximum relative point-prediction difference is 2.28e-07, consistent with small floating-point differences. Synthetic tests compare both learners over 250 rides with sparse evidence and outliers, check bounded movement, reject invalid state, and check forecast causality.

The full two-model test replay took 67.6 seconds on this host. A scalar ride-end update took a median 0.016 ms and a maximum 0.143 ms. These timings exclude source preparation and population fitting and do not establish nRF54LM20 timing.

Personal learning needs 20 bytes of float32 state. The listed estimator payload, including live state, personal range buffers, reference knots, gradient anchors, and bike defaults, is 2,484 bytes. Of this, 868 bytes must be mutable if fixed tables reside in read-only storage. This excludes pending calibration targets, configuration, stack, alignment, route storage, and input processing. Range refresh uses bounded post-ride workspace; no general solver or route-length-dependent ride-end fit remains.

## Decision

Use the scalar algorithm as the first implementation specification. Keep the current empirical range method identified as experimental wherever observed coverage falls short, especially for MTB. Do not retune it on this test set or describe its upper edge as a pessimistic maximum. Validate reliable moving inputs, recent technical MTB rides, long horizons, and the complete device resource budget before making those product claims.

## Artifacts and reproduction

The [prototype README](src:host/ride-time-prototype/README.md) lists the commands. The aggregate JSON includes configuration, bike defaults, calibration support and knots, source hashes, metrics, and host measurements. Raw ride data and per-forecast records stay private. The portable report embeds its chart.

Data source: [FitRec](https://cseweb.ucsd.edu/~jmcauley/datasets/fitrec.html), Ni, Muhlstein and McAuley (WWW 2019). The source archive and rider-split rules are unchanged from the first study.
