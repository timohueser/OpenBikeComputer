---
copy: ai
---

# Small corrections for long rides

## Result

We compared three one-coefficient extensions with the frozen ride-time estimator. The replay uses the same **234 rides and 26 long rides from one athlete** as the long-ride pilot. This dataset was already inspected. These are exploratory results, not a new independent test.

**The uphill correction gives a small benefit. The sustained-climb and duration corrections change little. None resolves the large error on the nine-hour ride.** The duration learner had little prior long-ride evidence and learned no positive decay before that ride.

## Fixed hypotheses

| Variant | Additional log-pace term |
| --- | --- |
| Uphill correction | b × clip(gradient / 8%, 0, 1) |
| Sustained climb | b × uphill weight × clip(accumulated ascent / 300 m, 0, 1) |
| Duration correction | b × clip((moving minutes − 120) / 240, 0, 1) |

Each coefficient starts at zero. Gradient and sustained-climb coefficients are bounded by ±ln(1.5). The duration coefficient is bounded between zero and ln(1.5). At maximum feature exposure, this permits at most a 50% increase in pace, meaning minutes per kilometre. These are authored test settings, not physiological thresholds or values fitted on this rider. No nonzero population fatigue prior was available.

Accumulated ascent increases on blocks above 2% gradient and resets after 200 m of non-climbing distance. Short interruptions do not reset it. It is a route-geometry proxy for a sustained climb, not an estimate of exhaustion or recovery.

## Learning and causal forecasts

Each variant retains its own original scalar learner and live multiplier. Persistent parameters remain fixed through the ride. At ride end, the added coefficient is fitted from the log-pace residual relative to the pre-ride scalar baseline. The scalar learner removes the pre-ride added correction from its observations. The live adjustment does not supply training targets.

The gradient correction removes the ride mean from both feature and residual before regression. The other two corrections remove separate means within each ride and nearest gradient-anchor group. Thus a ride with flat terrain only early and climbing only late cannot by itself identify duration decay. Coarse gradient groups reduce, but do not remove, terrain and pacing confounding.

For feature x, residual y, distance weight w, and group means x̄ and ȳ, calculate X = Σw(x−x̄)² and Y = Σw(x−x̄)(y−ȳ). Residuals are clipped to ±ln(2). Normalize X and Y by eligible ride distance W. With s = min(W/10 km, 1), update A ← ρA + 0.25sX/W and h ← ρh + 0.25sY/W, where ρ = 2^(−s/20). The candidate is h/(A+0.1), subject to the coefficient bounds and a maximum change of 0.03s. No evidence or decay is applied when X/W is below 10⁻⁶.

A duration forecast starts from observed moving time so far. It walks through the remaining route, using predicted arrival time at each block to calculate future duration exposure. It never reads recorded future times for that calculation. Live observations divide by baseline time including the modeled current duration effect. The forecast therefore applies current fatigue once, then adds only the modeled future change.

All variants use the same source screen, bike default, fixed gradient curve, observation blocks, and route-profile proxy. No parameter search or combined model was run. The freeze manifest predates these predictions. The baseline reproduces all original continuous-pilot forecasts within 3.8e-15 relative difference.

## Point accuracy on all long rides

Mean absolute percentage error of full remaining moving time; each cell has 26 rides. These are the same target rides for every variant.

| Model | Departure | After 10 min | After 30 min | After 60 min |
| --- | --- | --- | --- | --- |
| Current model | 5.76% | 6.04% | 6.60% | 4.56% |
| Uphill correction | 5.40% | 5.73% | 6.30% | 4.39% |
| Sustained climb | 5.75% | 6.03% | 6.59% | 4.54% |
| Duration correction | 5.76% | 6.07% | 6.57% | 4.58% |

Mean absolute error in minutes:

| Model | Departure | After 10 min | After 30 min | After 60 min |
| --- | --- | --- | --- | --- |
| Current model | 11.87 | 13.90 | 10.35 | 4.58 |
| Uphill correction | 10.71 | 12.96 | 9.78 | 4.53 |
| Sustained climb | 11.83 | 13.87 | 10.31 | 4.51 |
| Duration correction | 11.87 | 13.94 | 10.31 | 4.60 |

![Error changes with and without the longest ride](/assets/research/ride-time/endurance-comparison.svg)

Negative changes mean lower error. The graph shows paired-cohort mean differences, not uncertainty bounds. All rides belong to one athlete and share histories. We do not bootstrap them as independent riders.

## Does the largest error dominate the result?

The following table excludes the longest ride from scoring. It stays in the chronological learning history for later rides. Each cell has 25 target rides.

| Model | Departure | After 10 min | After 30 min | After 60 min |
| --- | --- | --- | --- | --- |
| Current model | 5.13% | 4.92% | 6.18% | 4.67% |
| Uphill correction | 4.89% | 4.73% | 5.91% | 4.45% |
| Sustained climb | 5.13% | 4.92% | 6.17% | 4.67% |
| Duration correction | 5.13% | 4.95% | 6.15% | 4.70% |

The uphill correction still gives a small improvement. This rules out the longest outcome as the sole source of the average benefit. It does not establish transfer to another rider.

## What was learnable before the nine-hour ride?

| Variant | Earlier identifiable ride updates | Coefficient before ride | Maximum pace multiplier |
| --- | --- | --- | --- |
| Uphill correction | 81 | 0.1024 | 1.108× |
| Sustained climb | 78 | 0.0057 | 1.006× |
| Duration correction | 8 | 0.0000 | 1.000× |

The longest earlier retained ride lasted 3.52 moving hours. No earlier ride reached the duration feature's six-hour saturation point. An identifiable update means the feature had some within-group variation; it does not mean that the ride supplied a strong or repeatable fatigue signal.

The duration correction had 8 earlier identifiable updates, but its constrained coefficient was zero when the long ride started. It consequently issued the same forecasts as the baseline on that ride. Learning from the completed ride cannot repair forecasts already issued.

A shared nonzero duration prior could act before personal long-ride evidence exists. This experiment does not test one: its shape is shared, but its initial strength is zero. Choosing a population strength from this one failure would be tuning on the example we want to explain.

## The nine-hour ride

| Moving minutes since departure | Actual remaining | Current model | Uphill correction | Sustained climb | Duration correction |
| --- | --- | --- | --- | --- | --- |
| 0 | 588 | 461 | 481 | 461 | 461 |
| 10 | 578 | 381 | 400 | 382 | 381 |
| 30 | 558 | 461 | 468 | 462 | 461 |
| 60 | 528 | 536 | 514 | 535 | 536 |
| 480 | 108 | 58 | 63 | 58 | 58 |

All values are minutes. The uphill correction moves the early estimate in the right direction, but the miss remains large. At some later checkpoints it is worse. A static correction does not represent the rider's reduction in climbing pace during this day.

The previous broad gradient diagnostic found little median climbing-versus-flat discrepancy in 37 qualifying earlier rides. The new coefficient uses a different sample and statistic: all eligible blocks, including rides with less than five minutes of steep climbing, with distance weights, ride centering, regularization, and chronological forgetting. A positive fitted coefficient is compatible with that near-zero median. Neither statistic alone establishes a stable rider trait.

## Interpretation and device cost

Keep the single uphill coefficient as a candidate for a new independent comparison. Its small gain on both the full cohort and the other long rides is more useful evidence than fitting a special curve to the nine-hour ride. It does not yet justify restoring a complete personal gradient curve.

The near-zero duration result is inconclusive. Limited late-ride exposure, strong regularization, the chosen shared shape, and the zero prior all limit adaptation. We have not shown that fatigue is unimportant, that this shape is correct, or that a population prior would fail. The sustained-climb proxy is likewise only one simple hypothesis.

The additional persistent fit needs three numbers: coefficient, curvature, and target. Grouped ride statistics can use five sums per gradient group, or 180 bytes for nine groups of float32 values. This Python experiment uses host arrays and does not measure device memory or timing.

The duration variant has an additional cost beyond storing one coefficient: each forecast currently walks the remaining route to estimate future time. It cannot use the original constant-time remaining baseline sum. A device implementation would need a bounded route summary and a timing check. No firmware change is recommended from this pilot.

## Next evidence

Obtain multiple timestamped four-to-eight-hour rides per rider, with ordinary shorter rides between them. Include reliable bike labels and, where available, power data for diagnosis. Keep some riders or later rides untouched before fitting a shared duration prior. Compare the current model, the single uphill coefficient, and one constrained duration adjustment on that evidence.

No new prediction-range claim is made. The old FitRec reference ranges do not establish coverage for these changed predictors; range calibration and validation would be separate work.

## Reproduce the experiment

Protocol SHA-256: `0df9c3e49c9332a077a5594042ab23cf6505493494654b0a3f912c714eb79fdb`. The four-model replay took 19.3 seconds on this host. The prototype README gives the exact freeze, run, and report commands. Per-ride predictions and coefficient histories remain in private artifacts. Public output contains aggregate results. The original estimator and previous experiment outputs remain unchanged.
