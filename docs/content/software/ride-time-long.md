---
copy: ai
---

# Long rides after getting the computer

## Result

The frozen estimator was replayed on **234 rides from one athlete**, with 8,777 km of retained history. **26 rides lasted at least two moving hours.** This is an external, single-athlete pilot. It does not establish accuracy for the user population or for MTB.

On the same 23 long rides, mean departure error fell from 13.2% with no history to 5.2% with at least 100 km of preceding rides. Live adjustment reduced the difference between history amounts. More history did not produce a steadily better estimate.

**The long-day failure matters:** on the almost ten-hour ride, the estimate after ten minutes was more than three hours too optimistic, and the upper end of the range also missed the actual duration. Good average errors do not make this a reliable worst-case estimate.

## Source and selection

We downloaded the [GoldenCheetah example archive](https://raw.githubusercontent.com/GoldenCheetah/OpenData/59a0062c5e79c7e9a6751a3fff972fdb0bdb3347/examples/033874ce-e20d-44ba-9cc9-125030b6662f.zip). It has 731 activity summaries and 660 sample files. The summaries include 592 cycling activities and 105 with at least two reported riding hours. The retained recordings span 2008–2017. The main OSF file API timed out during this study; only the public GitHub example was used.

We require an unambiguous metadata match, Bike sport, original GPS and altitude flags, finite samples, and at least five moving minutes and one kilometre. A whole ride is excluded for a recording gap over 30 seconds, nonmonotonic time or distance, interval speed over 80 km/h, or a moving-interval gradient over 50%. We do not join usable sections across an unknown gap.

| First exclusion reason | Sample files |
| --- | --- |
| ambiguous metadata | 12 |
| grade above 50 percent | 66 |
| no original gps or altitude | 183 |
| nonmonotonic | 4 |
| not bike | 72 |
| recording gap | 28 |
| speed above 80 | 61 |

Potential duplicate recordings on the same date, within 5% of both distance and elapsed time, are reduced to the finer recording. Overlapping retained activities are excluded. These checks are conservative heuristics; they cannot identify every duplicate or indoor/virtual ride. No surviving candidate was removed by the duplicate or overlap checks in this archive.

The final subset contains 26 rides of at least two hours, 1 of at least four hours, and 1 of at least six hours. Thus the evidence mostly concerns two-to-four-hour rides. The strict screen can exclude real fast descents and steep pushing, as well as sensor faults. Longer recordings have more chances to fail a screen. These results apply to the retained subset, not all source rides.

GPS coordinates are absent from the export. The original GPS flag indicates source-channel availability, not verified outdoor travel. The [GoldenCheetah source](https://github.com/GoldenCheetah/GoldenCheetah/blob/master/src/FileIO/RideFile.cpp) defines the flag. Bike categories are unavailable; every ride uses the frozen non-MTB default.

## What the replay measures

The core estimator, gradient curve, bike default, and reference error distributions are unchanged from the FitRec study. SHA-256 hashes freeze those inputs and this pilot's adapter before predictions are run. Shared parameters and reference ranges are not refitted on this athlete. Personal state still learns normally from earlier rides. The sample was inspected for suitability before the protocol was frozen.

Distance increments and altitude provide an approximately 200 m trailing gradient. Positive-motion samples form roughly 20 m observation blocks, flushed at stops. Baseline block time is the sum of the sample distances times their default paces. Personal learning also requires at least 10 m and no more than two percentage points of gradient variation in a block.

The outcome excludes zero-distance intervals. Slow positive movement, including pushing, has no minimum speed cutoff. There are no explicit pause events: distance rounding can remove slow movement, and GPS drift can turn a stop into apparent motion. This is a moving-time proxy, not exact ground truth.

Forecasts use the full remaining recorded distance/elevation profile as a substitute for a planned route. Future riding times do not enter a forecast. This does not test map elevation errors or route deviations. A forecast is issued before the next observation block, approximately every ten moving minutes. The live multiplier applies to the entire remaining route.

All eligible preceding rides can update personal pace. The first forecast in each phase/duration group can update the personal error buffer, only after its ride ends. This pilot queries full remaining routes; the earlier FitRec study queried fixed-distance targets. The reference error tables are not refitted.

## Continuous new-device replay

The simulated computer starts empty at the first retained ride and keeps its state through the archive. All rows below score the same long rides; the target is the time remaining at the stated checkpoint. Mean error is mean absolute percentage error. Negative bias means an estimate is too optimistic.

| Forecast | Rides | Mean error | Mean absolute minutes | Bias (min) | Range coverage |
| --- | --- | --- | --- | --- | --- |
| Departure | 26 | 5.8% | 11.9 | -9.3 | 92.3% |
| After 10 min | 26 | 6.0% | 13.9 | -9.4 | 84.6% |
| After 30 min | 26 | 6.6% | 10.4 | -7.3 | 92.3% |
| After 60 min | 26 | 4.6% | 4.6 | -0.3 | 100.0% |

| Forecast | 90th percentile absolute error | Median range width (min) |
| --- | --- | --- |
| Departure | 13.8% | 55.6 |
| After 10 min | 13.9% | 38.7 |
| After 30 min | 12.0% | 32.4 |
| After 60 min | 8.4% | 24.3 |

The range width is the upper estimate minus the lower estimate. Coverage must be read together with width: a broad range can cover more outcomes without giving a useful precise estimate.

A missing or rejected source ride contributes no history. Recorded kilometres therefore mean retained device-observed distance, not this athlete's total riding. Fitness changes during unrecorded rides remain unobserved. The archive is not a complete account of the athlete's life.

## Same long rides, different amounts of history

For each target ride, start a fresh device before the latest whole preceding rides that reach the specified distance budget. Replay those rides in order, then score the target. The target itself never enters its own prior history. Only targets with at least 1,000 km of preceding retained history enter any paired row.

| Minimum history | Actual km: median [min–max] | Departure error | After 10 min | After 30 min | After 60 min |
| --- | --- | --- | --- | --- | --- |
| 0 km | 0 [0–0] | 13.2% | 6.8% | 6.4% | 4.5% |
| 50 km | 75 [51–96] | 5.8% | 6.2% | 6.5% | 4.5% |
| 100 km | 124 [101–157] | 5.2% | 6.1% | 6.5% | 4.5% |
| 200 km | 223 [200–261] | 5.0% | 6.0% | 6.5% | 4.5% |
| 500 km | 520 [501–664] | 5.0% | 6.0% | 6.6% | 4.5% |
| 1000 km | 1023 [1003–1063] | 5.3% | 6.0% | 6.6% | 4.5% |

Whole rides can overshoot a small distance budget substantially. A 50 km row must not be read as exactly 50 km of learning. Different budgets also include different recent dates and numbers of ride-end updates.

![History and range coverage on matched long rides](/assets/research/ride-time/long-history.svg)

The same targets make this a within-athlete comparison of history amounts. It is still descriptive: rides share training history, terrain, and one rider. We do not present a population confidence interval. Coverage is the fraction of outcomes inside the nominal 90% range, not a mathematical coverage guarantee.

## Annual purchase dates

A second replay resets the computer at the first retained ride of every calendar year. Each ride occurs once in this view. These are alternative purchase scenarios for the same athlete, not additional users.

| Forecast | Rides | Mean error | Mean absolute minutes | Bias (min) | Range coverage |
| --- | --- | --- | --- | --- | --- |
| Departure | 26 | 5.7% | 11.7 | -7.5 | 92.3% |
| After 10 min | 26 | 5.8% | 13.7 | -9.0 | 84.6% |
| After 30 min | 26 | 6.6% | 10.3 | -7.3 | 92.3% |
| After 60 min | 26 | 4.6% | 4.6 | -0.3 | 100.0% |

| Prior retained distance | Long rides | Departure error |
| --- | --- | --- |
| 0–50 km | 1 | 5.7% |
| 50–100 km | 0 | No observations |
| 100–200 km | 1 | 5.3% |
| 200–500 km | 3 | 2.5% |
| 500–1000 km | 6 | 7.0% |
| 1000+ km | 15 | 5.8% |

These distance groups contain different target rides and can be small. They must not be interpreted as a controlled learning curve; the paired comparison above addresses that question.

## The longest ride

The longest retained ride covers 175.7 km in 9.79 moving hours. It was selected for this diagnostic by duration, not by error. Each range below concerns remaining time.

| Moving minutes since start | Actual remaining min | Predicted remaining min | Nominal 90% range (min) |
| --- | --- | --- | --- |
| 0 | 588 | 461 | 370–552 |
| 10 | 578 | 381 | 307–426 |
| 30 | 558 | 461 | 371–516 |
| 60 | 528 | 536 | 432–600 |
| 120 | 468 | 445 | 358–497 |
| 240 | 348 | 357 | 287–400 |
| 480 | 108 | 58 | 47–65 |

![Predicted finish throughout the longest retained ride](/assets/research/ride-time/long-trajectory.svg)

The finish forecast changes substantially. The early live pace does not represent every later section. The single live multiplier cannot resolve all changes in terrain, effort, or conditions, and this archive does not identify their causes. Applying the multiplier to the full route is not a worst-case bound.

## Stop-definition sensitivity

The fixed sensitivity counts internal zero-distance runs of at most ten seconds as riding. Longer stops remain excluded. It uses the same source rides, model, and defaults, and replays their histories under the alternate definition. The target cohort remains the primary set of long rides.

| Forecast | Rides | Mean error | Mean absolute minutes | Bias (min) | Range coverage |
| --- | --- | --- | --- | --- | --- |
| Departure | 26 | 6.0% | 12.3 | -9.9 | 92.3% |
| After 10 min | 26 | 6.4% | 14.6 | -9.4 | 84.6% |
| After 30 min | 26 | 6.7% | 10.6 | -7.6 | 88.5% |
| After 60 min | 26 | 4.5% | 4.5 | -0.7 | 100.0% |

Across all retained rides, this adds 71.1 minutes (0.35% of primary moving time). This checks one ambiguity. It does not validate stop detection.

## Decision and next evidence

The pilot supports retaining the simple learner for further testing. A small amount of preceding history helps departure estimates for this athlete, while live adjustment supplies most of the in-ride adaptation. There is no basis here for more personal gradient or surface parameters.

The uncertainty system remains unfinished. Its coverage varies with checkpoint and history, and it misses a large long-day error. Do not use these ranges as guaranteed upper bounds for water warnings.

Next, obtain several riders with contiguous recorded histories, reliable bike labels, and multiple four-to-eight-hour rides. Original timestamped recordings and pause events are preferable. Include ordinary short rides in the histories. Reserve riders or later chronological periods before any new tuning; keep this pilot as inspected development evidence.

## Reproducibility

Archive SHA-256: `9f3b5a8667148a05203d9f3139422a5e1d9e40d4291124313b124289e10e409f`. Pilot protocol SHA-256: `6ec2f482ffbe69a2d8d38e6f5d533d7871d02bfe29f8ef3c87dd20fca8f2ad25`. The replay took 33.6 seconds on this host; that is not an nRF54LM20 timing result.

The host prototype README contains the exact preparation, freeze, replay, and report commands. Private artifacts retain per-ride predictions and prepared recordings. Public artifacts contain aggregate metrics and the selected diagnostic chart. No estimator parameters changed in this experiment.
