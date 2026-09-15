---
title: Ride time estimation
description: The scalar pace algorithm, bounded state, and validation limits.
copy: ai
---

# Ride time estimation

**Status: the current research phase is complete. Keep the scalar estimator below as the
baseline.** The Python prototype implements these equations. The later uphill and duration
experiments remain separate candidates; they are not part of this specification.

The evidence supports personal pace learning and live adjustment. It does not yet establish
reliable prediction ranges for MTB or long days, or nRF54LM20 performance. See
[evidence and remaining validation](#10-evidence-and-remaining-validation) for the decisions
and next steps, and the [prototype README](src:host/ride-time-prototype/README.md) for replay
commands. Further model trials need new data; no further tuning on the inspected archive
is planned.

## 1. Product behaviour

Estimate travel time along a known route. Include reliable slow movement and pushing.
Exclude stationary time and do not predict future breaks. Arrival time is the current
clock time plus remaining travel time. During a stop, arrival time therefore moves later
as the clock advances, without the stop being learned as slow movement.

The model combines a fixed gradient curve, a fixed broad bike-category default, one
personal multiplier shared across categories, and one temporary multiplier for the ride.
The personal multiplier stays fixed during a ride. The temporary multiplier adapts during
movement and applies to the **entire remaining route**, with no return toward baseline
at longer horizons. Fatigue, a slower companion, and persistent wind can affect it.
The model does not identify the cause of a pace change.

This version has no personal gradient corrections, path or surface terms, partner modes,
or weather-service inputs. Bike categories describe broad classes, not specific setups.
The benchmark distinguishes MTB from other cycling. It does not establish separate road,
gravel, and touring defaults or personal bike-specific ratios.

Prediction ranges are empirical estimates. Their upper edge is not a worst-case bound.
Do not describe an unvalidated range as a guaranteed 90% interval. Water-gap messages also
need independent evidence about the location and availability of water.

## 2. Units and notation

Use kilometres and moving minutes, with pace in minutes per kilometre. Logarithms are
natural. Divide pace by `p_ref = 1 min/km` before taking its logarithm; the numerical
formulas below use those units. Gradient is signed rise divided by horizontal distance.

| Symbol | Meaning |
| --- | --- |
| `d_i`, `t_i` | Distance and moving duration of an observed block |
| `g`, `b` | Gradient and broad bike category |
| `f(g)`, `beta_b` | Fixed log-pace curve and bike log multiplier |
| `theta`, `u` | Personal and temporary log multipliers |
| `B_i` | Default time over a block, before personal or live correction |
| `A`, `h` | Persistent scalar evidence sums |
| `W`, `z_bar` | Current ride's learning distance and mean clipped target |
| `clip(v, lo, hi)` | Limit a value to the closed interval from `lo` to `hi` |

A pace multiplier of 1.3 means 30% more travel time over the same distance. A 30% loss
of speed instead produces a time multiplier of `1 / 0.7`.

## 3. Observation contract

Supply reliable travelled distance, moving duration, and default travel time `B_i` over
the same path. Use only information available by the block's end. Future GPS points must
not help classify past observations.

Classify intervals as moving, stationary, or uncertain. Reliable pushing counts as moving.
A minimum cycling speed must not define movement on the device. Longer observation blocks
can help distinguish slow progress from position noise. Wheel-speed input can help but is
not required by the estimator.

Skip updates for stationary or uncertain intervals, missing measurements, and recording
gaps. Do not divide endpoint distance by an unobserved gap and call it a slow pace.
Do not fill missing gradient with zero and learn the resulting error as rider form.
Pauses do not advance the live adaptation clock or decay personal evidence.

Integrate default time across changing gradient:

```text
B_i = sum over pieces j in block i:
          d_j * exp(f(g_j) + beta_b)
```

A single personal multiplier factors out of this sum. A block can therefore cross several
gradients without personal coefficients for each piece. Do not substitute the average
gradient: its predicted time usually differs from the sum of the individual times.

The replay uses one trailing, approximately 200 m gradient estimate per GPS interval,
so `B_i = d_i * exp(f(g_i) + beta_b)`. Its learning screen additionally requires at least
10 m and adjacent gradient change no larger than two percentage points. Those filters
are replay choices, not a completed device input policy.

### Limits of the recorded outcomes

FitRec replay accepts timestamp spacing up to 30 seconds, displacement of at least 3 m,
interval-average speed from 1 to 80 km/h, and gradient within ±50%. These screens exclude
some pushing and difficult terrain while permitting short hidden stops within samples.
The outcome is **movement-screened elapsed time**, not verified moving time.

Distance is GPS chord distance. Recorded geometry and altitude substitute for a planned
route. Device integration needs compatible route and observation gradient processing,
reliable route progress, and a tested movement classifier. Removing surface features
removes the surface-label dependency; it does not solve these input requirements.

## 4. Fixed defaults

Interpolate linearly between the **logarithms** of the anchor paces. Clamp gradient to
the outer anchors for prediction. Record unsupported steep sections during validation.

| Gradient | Default pace, min/km |
| --- | --- |
| −20% | 3.667844 |
| −10% | 2.458603 |
| −5% | 1.863103 |
| −2% | 2.062386 |
| 0% | 2.322987 |
| +2% | 2.796305 |
| +5% | 3.872366 |
| +10% | 5.719886 |
| +20% | 8.749802 |

The [frozen aggregate artifact](src:host/ride-time-prototype/results/final-v1.json) stores
the exact configuration and reference knots. The curve comes from development riders:
take each rider's median log pace in each nearest-anchor bin, then the median across
riders. Require 20 observations per rider/anchor and 10 riders per anchor; otherwise
retain the authored initial value recorded in the first study. This is an empirical
default curve, not a physical cycling model.

| Category | `beta_b` | Approximate pace multiplier |
| --- | --- | --- |
| Other cycling | 0.0268886981 | 1.027× |
| MTB | 0.2243923187 | 1.252× |

An unspecified category uses the other-cycling default as an authored fallback. Only the
two observed categories were evaluated. Road, touring, and gravel share that default
until evidence supports separate values. No fitness onboarding is needed to initialize
the model; additional questions have no calibrated effect in this version.

### Host fit for bike defaults

Keep the development-only gradient curve. For each learnable development ride, clip
interval log-pace residuals relative to that curve to ±ln(3), then take their
distance-weighted mean. Fit `offset + MTB_indicator * difference` to those ride means.
Each rider has total weight one, divided equally over their learnable rides. Multiply
that weight by 0.25. Use ridge penalties 0.1 and 0.25, and coefficient bounds ±ln(1.5)
and ±ln(2), respectively. This is the scalar summary of the earlier bounded bike fit.

The final fit uses 18,566 rides from 497 development riders. It includes 169 riders with
MTB observations and 476 with other-cycling observations; the groups overlap. Store the
resulting defaults. The device does not repeat this population fit.

## 5. Forecast and live adjustment

For the remaining route pieces:

```text
Q = sum_j d_j * exp(f(g_j))
T_base = exp(beta_b + theta) * Q
T_hat = exp(u) * T_base
```

Initialize `u = 0` at ride start. For each accepted moving observation:

```text
baseline_i = exp(theta) * B_i
alpha = 1 - 2^(-t_i / 5)
innovation = clip(log(t_i / baseline_i) - u, -ln(2), +ln(2))
u = clip(u + alpha * innovation, ln(0.5), ln(2.5))
```

The half-life is five accepted moving minutes. With a constant, unclipped residual,
half the remaining log-space difference is removed every five such minutes. Innovation
clipping slows the response to larger changes. The live multiplier ranges from 0.5 to
2.5. These are response and stability limits, not physical speed bounds.

Issue forecasts before consuming the next observation. A future slowdown must not alter
an earlier prediction. Keep `u` during a pause and reset it for a new ride. Keep `theta`
fixed until the current ride finishes.

Compute `Q` once per route and update the remaining cost as route progress changes.
With that summary, each live ETA refresh is constant work. Building a replacement summary
costs one pass over the route. Route storage and progress lookup are separate from the
estimator state. The Python replay stores full arrays as a host convenience.

## 6. Personal learning with five float32 values

Persistent state is `(theta, A, h)`. Current-ride state is `(z_bar, W)`. All start at zero.
The live multiplier is not subtracted from personal learning observations. Clipping and
limited ride influence make persistent adaptation slow.

### Accumulate a ride

For each reliable learning observation, use the `theta` frozen at ride start:

```text
z_i = theta + clip(log(t_i / B_i) - theta, -ln(2), +ln(2))
W_new = W + d_i
z_bar = z_bar + (d_i / W_new) * (z_i - z_bar)
W = W_new
```

Clip an observation once. Do not repeatedly clip historical evidence against the latest
baseline. Store no observation list or gradient-by-surface table.

### Finish a ride

If `W = 0`, leave persistent state unchanged. Otherwise:

```text
s = min(W / 10, 1)
rho = 2^(-s / 20)
A_new = rho * A + 0.25 * s
h_new = rho * h + 0.25 * s * z_bar

lo = max(-ln(3), theta - 0.12 * s)
hi = min(+ln(3), theta + 0.12 * s)
theta_new = clip(h_new / (A_new + 0.1), lo, hi)
```

This minimizes `0.5 * (A_new + 0.1) * theta_new^2 - h_new * theta_new` over the permitted
interval. It is the scalar specialization of the tested general learner; no iterative
solver is needed. The 0.25 factor is the retained whole-ride mean weight. With no personal
detail coefficients, the former within-ride covariance terms disappear.

A ride contributes at most one evidence unit; a shorter ride contributes proportionally
less. Historical evidence has a half-life of 20 evidence units, not calendar days. A
10 km learning ride can change log pace by at most 0.12; a 1 km ride by at most 0.012.
The personal multiplier remains between 1/3 and 3. These limits bound an isolated ride's
influence; they do not guarantee accuracy.

Reject nonfinite inputs or candidate state. Require positive quadratic curvature, valid
bounds, and no increase in the objective beyond float tolerance. Commit `theta`, `A`,
and `h` together only if all checks pass. Clear the current-ride summaries after a
successful commit, so a repeated finish call does not learn the same ride twice. Reject
an invalid update without changing persistent state.

Do not decay evidence during inactivity. Sparse-history riders continue to depend on the
defaults. After a long absence, the live multiplier responds to newly observed pace;
elapsed calendar time alone does not reveal a change in fitness.

## 7. Empirical prediction ranges

The range method is fixed for this check. Coverage remains an empirical property. This
is not a confidence interval for a fitted coefficient or a method with a conformal
finite-sample guarantee. The report measures subgroup failures as well as average coverage.

### Six groups

Use start/warm-up and established phases, each with predicted durations below 10 minutes,
10 to below 30 minutes, and 30 minutes or more. The established phase begins after ten
accepted moving minutes. Assign duration from the issued prediction, never the outcome.

Replay evaluates the original start and the first boundary after ten accepted minutes.
Using the start group during warm-up, or the established group at later refreshes, is
an extrapolation beyond those evaluated checkpoints.

### Reference and personal errors

Before test replay, collect `e = log(actual / issued_prediction)` from calibration riders
only. At each checkpoint, offer 1, 3, 10, and 30 km route targets in that order. Reserve
the first offered target in each group, with at most one calibration outcome per group
per original ride. If that outcome is unscorable, do not replace it with another target.

Store 64 reference quantile knots per group at probabilities `(k + 0.5) / 64`, for
`k = 0..63`. An empty group uses the pooled same-model reference and reports the fallback.
Also store the last 32 valid personal log errors per group in a ring buffer. Personal
errors enter only after their original ride finishes. The current ride therefore cannot
change its own ranges.

Give the reference total weight 32: each knot weighs 0.5. Each personal error weighs one.
Sort the combined weighted values and choose the first values where cumulative weight
reaches 5% and 95%. Cache those two bounds per group.

```text
range_low  = T_hat * exp(q_05)
range_high = T_hat * exp(q_95)
```

The reference retains at least half the mixture weight even with a full personal buffer.
More history need not narrow a range: it may reveal greater variability. Do not sum
independent per-segment variances; errors can remain correlated throughout a ride.
Do not recenter the empirical range merely to force it to contain the point prediction.

### Honest interpretation

Measure coverage by rider, activity, duration, and available history. An average near 90%
does not imply 90% for MTB, long rides, individual riders, or repeated live updates.
Finite reference knots, sparse buffers, outcome selection, and changing conditions all
limit calibration. Report upper-bound exceedance as well as central coverage.

The prototype cannot certify a two-hour water-gap warning. That use needs adequate
long-horizon moving-time outcomes and a separately validated upper planning bound. Keep
the numeric coverage target out of product claims until the relevant validation passes.

## 8. Ride lifecycle

```text
start:
    load defaults and persistent personal state; select the bike category
    set u = 0 and accepted_minutes = 0; clear ride summaries and pending slots
    compute route cost; issue start forecasts and reserve calibration targets

for each observation:
    issue due forecasts before consuming this observation
    if moving time and baseline path cost are reliable:
        update u and accepted_minutes
    if the stricter learning contract also passes:
        accumulate z_bar and W
    track outcomes of reserved calibration targets

finish:
    validate and atomically commit the scalar personal update
    add valid reserved errors to personal buffers
    refresh range quantiles in bounded post-ride workspace
    persist successful state updates
```

Route deviation invalidates a calibration target whose forecast no longer describes
the travelled path. A replacement route needs a new cost summary. Interrupted rides
can contribute completed, reliable observations under the same rules; persistence must
not replay them twice. Restart semantics and movement classification need firmware tests.

## 9. Memory and computation

| Numeric payload | Bytes with float32 |
| --- | --- |
| Personal state and current-ride mean/distance | 20 |
| Live multiplier and accepted-minute counter | 8 |
| Six personal buffers, uint16 counts/cursors, cached bounds | 840 |
| Six 64-knot reference distributions | 1,536 |
| Nine gradient anchors and nine log paces | 72 |
| Two bike defaults | 8 |
| Total listed payload | 2,484 |

Reference knots, anchors, and defaults can reside in read-only storage, leaving 868 bytes
of listed mutable payload. These totals exclude configuration constants, at most six
pending calibration targets, counters, alignment, stack, route storage, and input processing.
They are not a complete working-memory measurement. The target remains low single-digit
kilobytes during a ride; the pending-target representation belongs to firmware integration.

An observation updates a constant number of scalars. The personal ride-end update is
constant work, independent of ride length. Range refresh sorts at most 96 values per
group, for six groups, after the ride. It can use the released memory arena. Cached bounds
make range queries constant work. Python allocations are not a proposed device strategy.

The report includes host timings. Measure actual nRF54LM20 time, stack, and float-math cost
during the port. Host timing does not establish a device deadline.

## 10. Evidence and remaining validation

### Decisions

| Part | Current decision | Evidence |
| --- | --- | --- |
| Personal pace and live adjustment | Keep the bounded scalar model | Both improve the original fixed estimates |
| Personal gradient curve | Omit from this version | The original trial improved mean in-ride error by only 0.16 percentage points |
| One personal uphill coefficient | Keep as a research candidate | A later single-rider trial reduced departure error from 5.76% to 5.40%; the large long-day miss remained |
| Path and surface corrections | Omit from this version | The regional comparison did not establish a reliable in-ride gain |
| Sustained-climb and duration corrections | Defer pending better evidence | The tested corrections changed little; long-duration learning data were sparse |
| Prediction ranges | Keep the prototype, with unresolved validation | Overall coverage hides poor MTB coverage and large long-day misses |

Small average gains do not prove that gradient or surface effects never matter. A late
slowdown can be partly predictable, but the current data do not identify a dependable
personal decay curve. The tested duration coefficient started at zero. A shared nonzero
fatigue prior has not been fitted or tested. Any future duration model must avoid counting
slowing already represented by the live multiplier twice.

### Evidence and its limits

- The [original comparison](../ride-time-evaluation/) tests six models and explains the
  small incremental benefit of personal gradient terms. The [surface comparison](../ride-time-surface/)
  tests additional route attributes after map-matching work.
- The [final FitRec consistency check](../ride-time-final/) covers 7,114 rides from 185
  test riders. After ten accepted moving minutes, mean error is 11.39% with equal rider
  weight. Nominal 90% ranges cover 89.85% overall, but only 77.20% for MTB. Only one scored
  outcome lasts at least two hours. These riders were already inspected during earlier
  comparisons; this is not a new independent test.
- The [long-ride pilot](../ride-time-long/) retains 234 rides and 8,777 km from one
  GoldenCheetah athlete. There are 26 rides over two moving hours, but only one over four.
  On the same 23 long rides, departure error falls from 13.2% with no history to 5.2% with
  at least 100 km of preceding whole rides. That is evidence of learning for this athlete,
  not a population learning curve. Source timing and stop rules provide a moving-time proxy.
- The [small-extension study](../ride-time-endurance/) tests one uphill, sustained-climb,
  or duration coefficient on that same inspected athlete. The uphill benefit remains when
  excluding the longest ride from scoring. Duration results are inconclusive: only eight
  earlier rides provide identifiable duration variation before the nine-hour ride, and
  none reaches six hours. New prediction-range coverage is not tested for these variants.

The nine-hour ride remains the clearest failure case. After ten moving minutes, the
baseline predicts 6 h 21 min remaining, while 9 h 38 min remain. The upper range also misses.
Climbing pace falls during the ride, and recent descending pace does not transfer well to
the upcoming climb. Full-route live adjustment is therefore not a worst-case bound.

### Work needed before the next decision

1. **Collect better long-ride histories.** Obtain several riders with repeated four-to-eight-hour
   rides, ordinary shorter rides between them, and reliable broad bike labels. Prefer original
   timestamped recordings with elevation and pause events. Include MTB, pushing, and low-mileage
   riders. Power data can help diagnose causes but must not become a required estimator input.
2. **Reserve new validation data before fitting.** Keep riders or later chronological periods
   untouched. Replay empty-device starts and increasing recorded distance. Compare the current
   model with the one uphill coefficient and a constrained duration adjustment. The inspected
   studies above remain development evidence for future choices.
3. **Validate uncertainty separately.** Measure both range coverage and width across bike types,
   remaining durations, and history amounts. A nominal percentage is not a guarantee. Long-day
   and MTB misses must stay visible in the results.
4. **Verify the device integration.** Test movement and pushing classification, route-gradient
   inputs, interrupted rides, and persistence without duplicate learning. Measure nRF54LM20 time,
   peak memory, and stack use. The experimental duration forecast currently walks the remaining
   route and needs a bounded device approach if it is retained.

No additional model experiment is required to close this research phase. These next steps
depend on new recordings or firmware integration, not further fitting to the same outlier.

## Research basis

The sources informed the investigation; they do not validate this specific combination.

- [Krismer et al., Elevation Enabled Bicycle Router Supporting User-Profiles](https://ceur-ws.org/Vol-1594/paper14.pdf) motivated the investigation of gradient, road type, and rider profiles.
- [Aguilera Moreno, Cycling Race Time Prediction](https://arxiv.org/abs/2601.00604) motivated comparison with simple route-based regression.
- [Boyd and Vandenberghe, Convex Optimization](https://web.stanford.edu/~boyd/cvxbook/bv_cvxbook.pdf) supplies the convex quadratic foundations.
- [Angelopoulos and Bates, A Gentle Introduction to Conformal Prediction](https://arxiv.org/abs/2107.07511) explains coverage assumptions that must not be claimed for this empirical mixture.
