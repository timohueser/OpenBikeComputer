# Ride time prototype

This host-only Python prototype implements the
[ride-time algorithm](../../docs/content/software/ride-time-estimation.md).
The final scalar model has fixed gradient and bike defaults, personal pace learning,
a live multiplier, and empirical ranges. The earlier six-model comparison and enrichment
studies remain reproducible. This is not firmware or a validated moving-time estimator.

The first experiment uses raw FitRec cycling records. The real-data model has one pooled
cycling context and nine gradient anchors. It does not infer surface or bike categories.
The generic learner supports those effects for synthetic checks.

## Current decision

The research phase is complete. Keep `final_model.py` with the fixed gradient/bike defaults
and live multiplier as the baseline. The uphill coefficient remains a candidate; sustained
climb and duration effects need better evidence. These experiments do not change the selected
algorithm. No further tuning on the inspected GoldenCheetah athlete is planned.

Start with the algorithm page's
[decisions, evidence, and remaining validation](../../docs/content/software/ride-time-estimation.md#10-evidence-and-remaining-validation).
The sections below preserve exact reproduction commands and the detailed reports. The next
model decision requires new long-ride histories; prediction-range validation and nRF54LM20
integration remain separate work.

## Files

| File | Purpose |
| --- | --- |
| `data.py` | Safe parsing, complete source audit, private SQLite cache, chronological selection |
| `model.py` | Packed float32 learner, live multiplier, bounded personal error buffers |
| `replay.py` | Development-only defaults, separate calibration, held-out forecasts |
| `diagnostics.py` | Synthetic cases, numerical reference comparison, host resource measurements |
| `report.py` | Aggregate metrics, rider bootstrap, charts, portable HTML and local docs source |
| `tests/` | Authored mathematical, numerical, causality and data-boundary checks |
| `results/fitrec-v1.json` | Aggregate results, configuration, source hash and frozen experiment manifest |
| `enrichment_data.py` | Private development coordinates, geographic survey and fixed pilot selection |
| `enrichment_match.py` | Geometry-only sequence matching and conservative acceptance checks |
| `enrichment.py` | Local OSM extraction and per-interval attribute audit |
| `enrichment_report.py` | Aggregate coverage report and private example overlays |
| `results/enrichment-v1.json` | Aggregate OSM enrichment findings and source provenance |
| `matching_v2.py`, `matching_audit.py` | Revised path and attribute agreement on the same pilot |
| `matching_review.py`, `matching_validation.py`, `matching_freeze.py` | Fixed visual review sections and frozen acceptance policy |
| `matching_report.py` | Matching coverage and reviewed correctness report |
| `surface_data.py` | Regional coordinates and separate offline / past-only OSM features |
| `surface_model.py`, `surface_replay.py` | Bounded shared corrections and frozen regional ETA comparison |
| `surface_report.py` | Point accuracy, paired rider uncertainty and tag support |
| `final_model.py` | Five-float scalar learner with a closed-form ride-end update |
| `final_replay.py` | Full-cohort bike defaults, range recalibration, and frozen consistency check |
| `final_report.py` | Final accuracy, range coverage, history groups, and resource report |
| `long_data.py`, `long_replay.py`, `long_report.py` | GoldenCheetah long-ride input checks, chronological replay, and history comparison |
| `endurance.py`, `endurance_report.py` | Exploratory one-coefficient gradient, sustained-climb, and duration comparisons |
| `komoot_data.py`, `komoot_protocol.py` | Private GPX preparation, gap audit, and frozen chronological evaluation plan |
| `komoot_replay.py`, `komoot_report.py` | Frozen recorded-motion replay, paired history budgets, and private HTML report |
| `komoot_explore.py`, `komoot_explore_report.py` | Private speed distributions, matched-gradient changes within rides, and two controlled live-pace probes |
| `komoot_misses.py`, `komoot_misses_report.py` | Remaining-error decomposition, fixed-distance horizons, and private case figures |
| `komoot_motion.py`, `komoot_motion_sensitivity.py` | Geometry-only stationary candidates and a separate motion-proxy sensitivity |

## Private Komoot preparation

The Komoot workflow uses a local export of the rider's own completed activities. Raw GPX,
activity metadata, prepared arrays, and the frozen cohort remain private. The download
manifest must contain matching `rides` and `files` lists, one unique numeric activity ID per
file, original source sport labels, UTC activity dates, durations in seconds, and SHA-256
hashes. Optional `pause_events` contain millisecond offsets from the activity start.

```sh
python3 host/ride-time-prototype/komoot_data.py
python3 host/ride-time-prototype/komoot_protocol.py freeze
python3 host/ride-time-prototype/komoot_protocol.py verify
python3 host/ride-time-prototype/komoot_replay.py freeze
python3 host/ride-time-prototype/komoot_replay.py run
python3 host/ride-time-prototype/komoot_report.py
```

Use `--source PATH` and `--output PATH` for other directories. The default source is
`~/.cache/openbikecomputer/ride-time/komoot`; output is `.artifacts/ride-time-komoot`.
Preparation refuses a nonempty output directory. Freeze refuses to replace a protocol.
Verification checks the original estimator, source manifest, adapter, plan, and prepared data.

The adapter subtracts the union of explicit pause events. It preserves positive-distance
pushing without a minimum speed. Unexplained active gaps over 30 seconds, track breaks,
speed spikes, and invalid gradients do not become pace observations. Zero-displacement
intervals are a stop proxy. GPS drift and brief hidden stops can remain in observed motion.
The trailing 200 m grade restarts at discontinuities. Overlapping recordings are removed,
with finer sampling preferred; similar commutes on separate dates remain separate rides.

There are two eligibility flags:

- `point_eligible`: no unknown intervals or profile flags, at least 5 observed moving minutes
  and 1 km, and observed duration within the greater of 120 seconds or 5% of Komoot's
  moving total. This is a strict motion proxy, not independently verified ground truth.
- `proxy_eligible`: the same duration/minimum-length checks, with at least 99% of recorded
  GPS chord distance retained. Unknown intervals can remain. This supports a conditional
  comparison over accepted moving sections, not an exact full-ride ETA claim.

Each NPZ stores `raw` rows of accepted distance (km), observed time (minutes), and gradient.
Other arrays retain elapsed time, explicit pause time, unknown time, recorded chord distance,
and reset/learning masks. Do not pass these files directly to the old long-ride block builder:
it does not enforce the new masks. Source-summary agreement never redistributes missing time.

The frozen plan reserves rides starting on or after 2024-01-01 UTC for evaluation. Earlier
rides supply personal history; a ride crossing the boundary supplies neither group. Later
completed evaluation rides may teach subsequent rides, as they would on a device. Shared
parameters must remain fixed. The planned comparison is the current scalar baseline against
the existing single uphill coefficient, with paired history budgets from 0 to 1000 km.

Preparation and plan freeze produce no ETA predictions. The replay freeze locks its sources,
authored tests, Python/NumPy versions, and input protocol hash in a separate execution manifest.
`run` refuses to replace a run-start record, even after a failure. Preserve failed run records
before a corrected execution; never change the experiment because of observed ETA errors.

The runner forms about 20 m observation blocks and enforces every interval's learning mask.
Unknown positive active time resets the live multiplier; identified pauses preserve it. Block
uphill weight uses the distance-weighted mean grade, clipped as grade/8% to [0,1]. The existing
bounded uphill update is unchanged. Both persistent learners update only after a ride ends.

The report verifies the frozen inputs and result hashes. It writes private `summary.json`,
`report.md`, and standalone `report.html`, with embedded SVG charts. Regenerate presentation
without repeating the replay. It reports excluded distance/time, source-summary disagreement,
duration and bike support, and conditional range coverage. A single rider cannot establish
population accuracy. No personal ride results are published by these commands.

After the frozen replay, run the separate exploratory analysis:

```sh
python3 host/ride-time-prototype/komoot_explore.py
python3 host/ride-time-prototype/komoot_explore_report.py
```

The default output is `.artifacts/ride-time-komoot-exploration`. The computation records
its methods and source hashes before it writes results, and refuses to replace an existing
plan. It checks that the original baseline forecasts reproduce. The two probes retain the
live factor across unknown gaps or use the aggregate pace observed since departure. These
are diagnostics on inspected data, not a new validation set or a change to the baseline.
The plots use 200 m movement windows and compare ride phases within gradient bins. Their
10th–90th percentile bands describe observed distributions, not ETA confidence intervals.

Regenerate the report without repeating computation. Optional private `context.json`
contains a `rider_context` string. Optional `interpretation.json` contains a `findings`
list of strings and a `next_step` string. The report records hashes for these inputs and
embeds all six figures for offline use. Keep these files and personal results local.

To examine the remaining misses at the same one-hour checkpoints:

```sh
python3 host/ride-time-prototype/komoot_misses.py
python3 host/ride-time-prototype/komoot_misses_report.py audit
python3 host/ride-time-prototype/komoot_motion_sensitivity.py
python3 host/ride-time-prototype/komoot_misses_report.py report
```

These commands write to `.artifacts/ride-time-komoot-misses`. Each computation records a
separate plan and refuses to replace it. The report can be regenerated. The diagnostics
compare aggregate pace with the last 30 moving minutes and supported first-hour gradient
factors. They report full remaining time and 5/10/20 km horizons, with separate counts for
targets that contain no unknown intervals. Signed error is split into grade-factor/mix
sensitivity and changes within gradient bands, including unsupported terrain. These terms
do not identify physical causes or an irreducible error floor.

The motion audit flags approximately two-minute spans confined to a 30 m box diagonal.
It does not label them as verified stops. Positive GPS displacement can contain stationary
drift; agreement with a source moving-time summary does not prove real movement. The
separate sensitivity excludes completed candidate spans from past observations and tests
an alternate outcome with candidate time removed. It keeps the original forecast instants,
cohort, and remaining route costs. Future spans cannot change past observations. Results
under this changed motion proxy are not validated moving-time accuracy. Preserve pushing
when designing a real motion detector; do not replace it with a minimum-speed threshold.

## Small long-ride corrections

The [extension study](../../docs/content/software/ride-time-endurance.md) compares the
current estimator with three bounded one-coefficient hypotheses. It reuses the prepared
and frozen long-ride pilot below. This is exploratory work on an already inspected
athlete. Shared parameters and reference ranges from previous studies are unchanged.

```sh
python3 host/ride-time-prototype/endurance.py freeze
python3 host/ride-time-prototype/endurance.py run
python3 host/ride-time-prototype/endurance_report.py --publish-docs
```

Outputs use `.artifacts/ride-time-endurance`; use `--output PATH` for a separate run.
Freeze and replay refuse to replace existing results. Report generation verifies input
and output hashes and checks baseline predictions against the original continuous
pilot. The report can be rebuilt without repeating predictions. Aggregate results are
in `results/endurance-v1.json`; private predictions and coefficient histories stay in
the artifact directory. Each extension starts at zero and learns only after completed
rides. Duration and sustained-climb fitting remove within-ride gradient-group means.
The duration forecast uses predicted future times, not recorded future times.

This study evaluates point accuracy only. It does not validate new prediction ranges,
fit a nonzero population fatigue prior, or establish device resource use. In particular,
the duration forecast walks the remaining route and needs further work before a device
implementation. See the report for exact features, bounds, data support, and limitations.

## Long rides after getting the computer

The [long-ride pilot](../../docs/content/software/ride-time-long.md) uses one publicly
downloadable GoldenCheetah example athlete. It tests the frozen scalar estimator on
full remaining rides, with continuous history, annual device resets, and the same
long rides preceded by different amounts of recorded history. This is a descriptive
single-athlete study. It does not establish MTB or population accuracy.

The committed `results/final-v1.json` supplies the frozen configuration, bike default,
and reference ranges. This replay does not require the FitRec database or OSM files.
Download the 31 MB example archive into the private cache, then run:

```sh
mkdir -p ~/.cache/openbikecomputer/ride-time/goldencheetah
curl --fail --location \
  https://raw.githubusercontent.com/GoldenCheetah/OpenData/59a0062c5e79c7e9a6751a3fff972fdb0bdb3347/examples/033874ce-e20d-44ba-9cc9-125030b6662f.zip \
  --output ~/.cache/openbikecomputer/ride-time/goldencheetah/example.zip
python3 host/ride-time-prototype/long_replay.py prepare
python3 host/ride-time-prototype/long_replay.py freeze
python3 host/ride-time-prototype/long_replay.py run
python3 host/ride-time-prototype/long_report.py --publish-docs
```

The adapter checks the source SHA-256 and rejects complete rides with recording gaps
or invalid motion/profile samples. Original GPS/altitude flags restrict the cohort;
the exported samples themselves contain time, distance, and altitude. Positive-distance
intervals define the primary moving-time proxy. A fixed sensitivity also includes
internal zero-distance runs of at most ten seconds. Neither rule supplies exact pause
ground truth. The report lists exclusions and the resulting long-ride support.

Outputs use `.artifacts/ride-time-long`. The source, prepared data, estimator and replay
are frozen before evaluation. Stages refuse to replace existing preparation, protocol,
or predictions. Use `--output PATH` consistently for a separate run. Per-ride arrays,
predictions, and audit records remain private. `results/long-v1.json` contains aggregate
results; `report.html` embeds its plots for offline use. The report stage can be rerun
to change presentation without repeating the experiment.

## Final algorithm and consistency check

The [final report](../../docs/content/software/ride-time-final.md) uses the exact scalar
implementation specified in the algorithm page. It retains the original interval cache,
gradient defaults, rider split, and overlap policy. It does not need OSM or coordinate
enrichment. Prepare the original FitRec cache and run the original `configure` command
below first if `.artifacts/ride-time/config.json` is absent.

```sh
python3 host/ride-time-prototype/final_replay.py configure
python3 host/ride-time-prototype/final_replay.py calibrate
python3 host/ride-time-prototype/final_replay.py freeze
python3 host/ride-time-prototype/final_replay.py evaluate
python3 host/ride-time-prototype/final_report.py --publish-docs
```

Outputs use `.artifacts/ride-time-final`. `configure` fits bike defaults on development
riders and freezes estimator sources, configuration, and dataset hash. `calibrate` uses
calibration riders to build references for both comparison models. `freeze` locks those
references and their supporting outcomes before evaluation. Replay and report generation
verify the frozen inputs. Preserve the output set before a new experiment; do not retune
on these already inspected test riders. This is a consistency check, not a fresh test.

The report also reads the original `.artifacts/ride-time/test-30s.csv` to compare the
scalar specialization against the general learner. Reproduce that original evaluation
below if this file is absent. It does not enter fitting or calibration of the final model.
The new report includes zero, below-50 km, 50–200 km, and 200 km-or-more history groups.
Reference knots, parameters, metrics, and source hashes are in `results/final-v1.json`.
Only aggregate artifacts belong in Git. The uncertainty results remain empirical and
must not be presented as a universal 90% guarantee.

## Setup

Run commands from the repository root. Use a Python environment compatible with the pinned
requirements. The exact interpreter, platform and package versions of the recorded run
are in the result manifest.

```sh
python3 -m venv .venv/ride-time
source .venv/ride-time/bin/activate
python3 -m pip install -r host/ride-time-prototype/requirements.txt
```

## Download and assess FitRec

The raw archive is about 2 GB. Downloaded records and the derived cache stay outside Git.
Read the [dataset page](https://cseweb.ucsd.edu/~jmcauley/datasets/fitrec.html) for provenance
and published terms. Do not redistribute the source archive or private forecast records.

```sh
mkdir -p ~/.cache/openbikecomputer/ride-time
curl --fail --location --retry 2 --continue-at - \
  --output ~/.cache/openbikecomputer/ride-time/endomondoHR.json.gz \
  https://mcauleylab.ucsd.edu/public_datasets/gdrive/fitrec/endomondoHR.json.gz
python3 host/ride-time-prototype/data.py
```

`data.py` writes `fitrec.sqlite` and `fitrec.audit.json` in that cache. It refuses to replace
an existing database. Use `--source` and `--output` for other locations. Never use `eval`
to load these Python-literal records. The loader uses JSON parsing or `ast.literal_eval`.

The recorded archive SHA-256 is:

```text
31a49f8ec2fa5d038983e7b3bfe35f5f7eabee581180c803023f011f046681b9
```

## Reproduce the experiment

Use a fresh output directory for a new run. All stages accept `--output`; the default is
`.artifacts/ride-time`. That directory is ignored by Git. It contains private per-forecast
CSV files as well as the portable report. Replay and diagnostics accept `--database`.
The report reads the audit from the default cache directory.

```sh
python3 host/ride-time-prototype/replay.py configure
python3 host/ride-time-prototype/replay.py calibrate
python3 host/ride-time-prototype/report.py --freeze
python3 host/ride-time-prototype/replay.py evaluate
python3 host/ride-time-prototype/replay.py evaluate --cap 15
python3 host/ride-time-prototype/diagnostics.py
python3 host/ride-time-prototype/report.py --publish-docs
```

The freeze step records estimator-source, configuration and calibration hashes before
test evaluation. Report generation rejects changed frozen inputs. It refuses to replace
an existing freeze manifest; use another output directory for a new experiment.

`--publish-docs` writes aggregate Markdown and charts to local documentation source. It
does not upload or deploy anything. The portable `report.html` embeds its charts and can
be opened offline. `report.md` and separate SVG/PNG figures are also written to the output
directory. The committed aggregate JSON contains no rider identifiers or trajectories.

To change only the presentation, rerun `report.py`. Do not rerun the held-out experiment
or select new settings because of test results. A further tuned model needs a new
evaluation protocol.

## Evaluation contract

- Assign riders by the fixed `obc-fitrec-v1` hash rule: 60% development, 20% calibration,
  and 20% test in expectation. Exact cohort counts depend on the hash results.
- Use the first 60 candidate cycling records per rider. Exclude overlapping records
  before fitting, calibration and testing. Do not replace an excluded record to fill
  the quota. The retained ride's maximum timestamp defines its end for this rule.
- Build initial curves from development riders only. Each rider contributes at most one
  median per nearest-anchor bin. Require 20 observations for a rider's anchor median and
  10 rider medians before replacing its authored default.
- Keep the persistent baseline fixed through each original ride. With no accepted
  baseline evidence, do not update or decay it.
- Issue forecasts at the original start and the first boundary after 10 accepted
  minutes. Select targets from known route distance, before inspecting their outcomes.
- Record outcomes only when all intervening intervals pass the screen. Never join
  disjoint accepted windows. Unscorable calibration slots are not replaced.
- Give each calibration group its first available target in ascending distance order.
  Personal errors enter buffers only after the original ride completes.
- Use a pooled same-model calibration reference if a group is empty. The recorded run
  has observations in all six groups. Store 64 reference quantile knots per group.

The primary screen accepts positive timestamp spacing up to 30 seconds, at least 3 m
of GPS displacement, GPS interval-average speed from 1 to 80 km/h, and profile slope
within ±50%. Nonfinite or incompatible arrays are excluded during preparation.
The learner additionally requires at least 10 m and adjacent smoothed gradients within
two percentage points. All exclusions are data screens, not a firmware motion policy.

Distance is GPS chord distance. The slope estimate uses approximately 200 m of trailing
recorded geometry and altitude. Predictions clamp slope to the ±20% anchors. Forecasts
carry the fraction outside those anchors as a diagnostic. The source profile substitutes
for a known planned route; map errors are not simulated.

The outcome is **movement-screened elapsed interval time**. A 30-second sample can contain
a short stop. The screen excludes some pushing and difficult terrain. Do not call the
result exact moving-time validation. The 15-second sensitivity keeps the primary
configuration and reference distribution, and changes the selected cohort.

## Focused verification

```sh
python3 -m unittest discover -s host/ride-time-prototype/tests -v
python3 tools/suite_registry.py check
python3 docs/build_docs.py --check-links
```

The authored tests are registered as the manual `python.ride-time-prototype` component
suite. They require no network or FitRec data. Dataset download and replay are explicit
research steps, not hermetic CI tests. Host timings and NumPy payload sizes do not prove
nRF54LM20 timing or complete device working memory.

## OSM enrichment pilot

The [enrichment report](../../docs/content/software/ride-time-enrichment.md) measures
matching coverage and tag availability on 80 development rides in central Jutland.
It does not fit or evaluate an enriched ETA model. The original estimator files and
held-out evaluation stay unchanged.

Install `requirements-enrichment.txt` in the same environment. The following commands
use the default FitRec cache prepared above. They need about 0.5 GB for the OSM archive,
additional private caches, and host memory for the local road graph. The data stage uses
the first 60 candidate rides per development rider and the original overlap exclusions.

```sh
python3 -m pip install -r host/ride-time-prototype/requirements-enrichment.txt
python3 host/ride-time-prototype/enrichment_data.py extract
python3 host/ride-time-prototype/enrichment_data.py survey
python3 host/ride-time-prototype/enrichment_data.py select --bbox 9 56 10 57
curl --fail --location --retry 2 --continue-at - \
  --output ~/.cache/openbikecomputer/ride-time/denmark-260913.osm.pbf \
  https://download.geofabrik.de/europe/denmark-260913.osm.pbf
python3 host/ride-time-prototype/enrichment.py osm
python3 host/ride-time-prototype/enrichment.py audit
python3 host/ride-time-prototype/enrichment_report.py examples
python3 host/ride-time-prototype/enrichment_report.py report --publish-docs
```

The extraction refuses to replace its development-coordinate database. Later stages
write to `.artifacts/ride-time-enrichment` by default. The OSM source and extraction
hashes are recorded. All matching occurs locally; no rider coordinates are uploaded.
The graph ignores one-way and access restrictions when reconstructing recorded travel.
It must not be used as a legal route planner.

The matcher uses future coordinates within each continuous block. Its cost margin is
an uncalibrated acceptance heuristic. An accepted map match does not establish the
correct historical way or surface. A later live ETA replay needs causal observation
features and a separately known route.

The portable HTML report includes six private map overlays. Do not redistribute those
source ride geometries. Public documentation and the tracked aggregate JSON contain
only aggregate results and textual inspection findings. Map data are attributed to
OpenStreetMap contributors under ODbL 1.0, with the extract supplied by Geofabrik.

For a new run, inspect the generated overlays and write `inspection.json` in the output
directory with a `findings` list and `decision` string before building the final report.
Without that file, the report marks visual inspection as pending. To reproduce only the
recorded prose, the original `inspection` object is in `results/enrichment-v1.json`.
`--publish-docs` writes local files; it does not deploy or upload the report.

## Revised matching and visual checks

The [revised matching report](../../docs/content/software/ride-time-matching.md) separates
plausible paths, agreement about paths, and agreement about attributes. It uses the same
80-ride pilot and OSM cache. The first enrichment implementation and results remain
available for comparison.

The revised matcher merges duplicate junction-node candidates and recognizes different
positions on the same unbranched road. It compares complete candidate transitions using
the surrounding sequence. Internal route edges contribute to attribute composition.
The retained composition is the minimum fraction of each known value across considered
paths. It does not locate those fractions within a GPS interval.

The matcher retains one shortest path per endpoint pair. Internal alternatives with the
same endpoints are not exhaustively enumerated. Scores are uncalibrated heuristics;
geometry review cannot establish physical surface conditions or historical map accuracy.

These commands describe a fresh experiment. Preserve the existing
`.artifacts/ride-time-matching-v2` directory before starting a new run. The selection and
freeze stages refuse to replace their records.

```sh
python3 host/ride-time-prototype/matching_review.py select
python3 host/ride-time-prototype/matching_review.py render
python3 host/ride-time-prototype/matching_audit.py
```

Review cases 1–20 from the generated sheets without opening predictions. Write
`review-labels.json` in the output directory. Each record contains `case`, `phase`,
`judgeable`, `allowed_labels`, `allowed_way_ids`, and `note`. Resolve letters to way IDs
with `review-catalogue.json`. Unknown cases stay unjudgeable.

```sh
python3 host/ride-time-prototype/matching_validation.py development
python3 host/ride-time-prototype/matching_freeze.py
```

Then review cases 21–40, append their labels, and run:

```sh
python3 host/ride-time-prototype/matching_validation.py validation
python3 host/ride-time-prototype/matching_report.py --publish-docs
```

The recorded study selects a 20 m endpoint tolerance from five development candidates.
The validation sections use 20 distinct previously uninspected riders. Report generation
checks frozen source hashes. It writes a portable report with private review sheets and
aggregate public documentation. `results/matching-v2.json` contains the metrics and
provenance. `results/matching-review-labels.json` contains the recorded letter labels and
inspection notes without rider IDs, OSM way IDs or coordinates. Those labels can reproduce
the recorded scoring after resolving the regenerated catalogue; they must not stand in
for a new independent review after tuning the matcher.

## Regional ETA comparison with surface data

The [surface accuracy report](../../docs/content/software/ride-time-surface.md) compares
four small shared models. Each retains the scalar personal learner and live adjustment.
The additional features are bike category, path type and surface composition. There is
no personal gradient correction in this comparison.

Use the existing FitRec archive, interval database, fitted initial curve and local OSM
ways cache. The extraction scans the archive again and stores only eligible regional
tracks, including the original calibration and test riders. It refuses to replace its
coordinate database. All private outputs use `.artifacts/ride-time-surface`.

```sh
python3 host/ride-time-prototype/surface_data.py extract
python3 host/ride-time-prototype/surface_data.py match
python3 host/ride-time-prototype/surface_replay.py freeze
python3 host/ride-time-prototype/surface_replay.py calibration
python3 host/ride-time-prototype/surface_replay.py test
python3 host/ride-time-prototype/surface_report.py --publish-docs
```

`match` can resume completed per-ride files. It produces offline route-proxy tags and
separate past-only observation tags. It does not use measured speed to filter features.
Both use the existing geometry policy; neither establishes historical ground truth.
Recorded future geometry substitutes for a planned route. Past-only features feed the
live adjustment and personal learning. Only earlier regional rides enter personal state.

`freeze` fits development riders and records source, configuration, cohort, OSM and match
cache hashes. It refuses to replace the manifest. Replay and report generation verify
these hashes. Use a separate preserved output set before changing the fixed experiment;
do not tune on the reported test outcomes. The report exports aggregates without raw
ride identifiers or coordinates. The calibration cohort is diagnostic and does not
supply individual-ride prediction intervals in this study.

The primary comparison is path plus surface versus bike category after ten accepted
minutes. Error is averaged within riders, then across riders. Paired bootstrap intervals
resample riders, preserving the dependence of their overlapping forecast targets. These
intervals describe the comparison; they are not individual ETA uncertainty bands.

## Review record

The adversarial review accepted the core equations and required evidence-scaled movement
bounds. Its implementation check found overlap leakage and a nonfinite-objective guard
that could fail open. Both were corrected, defaults and calibration were regenerated,
and the reviewer confirmed the corrections before the first final test.

The first configuration is a reviewed starting point, not the result of a hyperparameter
search. The dataset, synthetic diagnostics, limitations and next decisions belong in
the report, including results that favour a simpler model.
