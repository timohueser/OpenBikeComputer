"""Freeze the private Komoot cohort and evaluation plan before ETA comparisons."""

import argparse
from datetime import datetime, timezone
import json
from pathlib import Path

from komoot_data import OUTPUT, SOURCE
from long_data import digest
from long_replay import parameters

ROOT = Path(__file__).resolve().parent
CUTOFF = "2024-01-01T00:00:00Z"


def inputs(source, output):
    names = ("komoot_data.py", "komoot_protocol.py", "model.py", "final_model.py",
             "endurance.py", "long_replay.py", "results/final-v1.json")
    paths = [ROOT/name for name in names]
    paths += [source/"manifest.json", output/"audit.json", output/"rides.json"]
    paths += sorted(output.glob("*.npz"))
    return {str(path.resolve()): digest(path) for path in paths}


def split(rides):
    cutoff = datetime.fromisoformat(CUTOFF.replace("Z", "+00:00")).timestamp()
    # No warm-up ride may end in the evaluation period.
    return dict(warmup=[r["id"] for r in rides if r["end"] < cutoff],
                evaluation=[r["id"] for r in rides if r["start"] >= cutoff],
                crosses_cutoff=[r["id"] for r in rides if r["start"] < cutoff <= r["end"]])


def freeze(source, output):
    path = output/"protocol.json"
    if path.exists():
        raise ValueError("Refusing to replace frozen protocol")
    parameters()  # Enforce the original estimator hashes.
    audit = json.loads((output/"audit.json").read_text())
    if audit["source_manifest_sha256"] != digest(source/"manifest.json"):
        raise ValueError("Source manifest changed since preparation")
    rides = json.loads((output/"rides.json").read_text())
    cohort = split(rides)
    evaluation_ids = set(cohort["evaluation"])
    eligible = [r for r in rides if r["id"] in evaluation_ids and r["proxy_eligible"]]
    protocol = dict(version=1, created_at_utc=datetime.now(timezone.utc).isoformat(),
        status="Plan and prepared data frozen; no model predictions produced. Lock the new runner before its first evaluation.",
        scope="One private rider. Split chosen after source-quality inspection, before ETA errors. Not a population test.",
        cutoff_utc=CUTOFF, cohort=cohort,
        proxy_targets=[r["id"] for r in eligible],
        strict_point_targets=[r["id"] for r in rides if r["id"] in evaluation_ids and r["point_eligible"]],
        long_proxy_targets=[r["id"] for r in eligible if r["moving_minutes"] >= 120],
        primary="Conditional recorded-motion replay of the fixed scalar baseline with the live multiplier applied to all remaining accepted route sections. Held-out-period proxy-eligible rides >=120 observed moving minutes; ride-weighted absolute percentage error at 10 observed moving minutes. This is not exact full-ride ETA ground truth.",
        secondary="Signed and absolute error in minutes, median and 90th-percentile APE, checkpoints 0/30/60 minutes, all durations and MTB/other separately. Always report sample counts.",
        history="Start empty at the first warm-up ride. Commit personal updates only after each original ride ends. Evaluation-period completed rides may train later rides; no tuning of shared parameters.",
        observations="Use only usable intervals. Aggregate approximately 20 m blocks within uninterrupted spans; flush at resets and never bridge missing distance. A block learns only if every interval permits learning, distance >=10 m, and gradient spread <=2 percentage points. Use frozen live update and ride-end evidence cap. Reset live state at unknown intervals and track breaks; retain live state across identified pauses. Do not reset persistent personal state at gaps.",
        history_eligibility="Non-overlapping retained rides with >=5 observed moving minutes and >=1 observed km. Incomplete rides may provide valid observed spans for personal pace. Only proxy-eligible targets supply conditional recorded-motion range errors; strict full-ride outcomes remain separate.",
        unknown_gaps="Unknown intervals contribute neither observed route distance nor observed outcome time. Preserve and report their full duration and chord distance separately; do not label them as breaks. Proxy targets require >=99% recorded chord distance retained and observed duration agreement with source moving summary within max(120 s,5%). Strict point targets additionally require no unknown/profile flags. No replacing rejected long targets with shorter targets.",
        motion="Positive GPS chord displacement after subtracting unioned explicit pauses. No minimum speed. Zero-displacement intervals are a stop proxy. GPS jitter and short hidden stops remain possible; source-summary agreement is a check, not ground truth.",
        route="The offline quality mask defines the conditional observed route for both models; it is not a device prediction of future GPS outages. Recorded geometry/elevation substitute for the known planned route. After the declared offline cohort/mask preparation, no future interval durations enter the forecast calculation. Checkpoints count observed motion only; never call them exact minutes since real-world departure. Reset trailing 200 m observed grade at discontinuities. Do not use the old replay's block builder without the new masks.",
        bike="Source mtb -> MTB prior; racebike/touringbicycle -> other prior. Shared personal scalar. Labels are not verified physical setups.",
        comparisons="Frozen baseline versus the existing single uphill coefficient from endurance.py, initially zero. No surface, duration, sustained-climb, weather, or new hyperparameter trials.",
        candidate="Use the existing bounded within-ride-centred uphill update unchanged, with the same valid blocks and completed-ride learning. Compare paired rides/checkpoints; no new prediction-range claims for the candidate.",
        sparse_history="Same eligible >=120-minute evaluation targets with >=1000 observed prior km; reset before latest complete training rides reaching 0/50/100/200/500/1000 km. Report actual km and overshoot; histories use only earlier rides. If the cohort is small, report it as inconclusive.",
        history_groups_km=[0, 50, 200, 500, 1000],
        ranges="Baseline only, original FitRec reference knots. Earliest prospective forecast per phase/duration group per eligible ride updates after completion. Report nominal-90% coverage and width by duration/bike; no guarantee and no retuning on this rider.",
        sensitivities="Report source Komoot moving totals versus observed proxy and missing-time/distance coverage for every duration group. Do not claim point accuracy for the excluded gaps. No redistribution of unknown time to force summary agreement.",
        uncertainty="One rider: descriptive paired differences and per-ride distributions. Overlapping forecasts and repeated history resets are not independent samples. No population confidence interval or claim of universal calibration.",
        decision="Keep the selected baseline unless the candidate shows consistent paired benefit across rides and duration/bike groups. A sparse or heavily censored long-ride cohort cannot settle the gradient question.",
        cohort_design="A strict data-quality survey retained no >=2 h fully accounted rides. The recorded-motion proxy and its 99% distance / 5% duration safeguards were defined before inspecting any ETA predictions. Preserve the strict-survey audit. Do not adjust these thresholds after model evaluation.",
        execution="Before first prediction, record runner and test hashes plus software versions in a separate write-once execution manifest referencing this protocol hash. Verify all frozen inputs before every replay. Preserve failed runs; no overwrite or quiet protocol edits.",
        hashes=inputs(source, output))
    with path.open("x") as f:
        f.write(json.dumps(protocol, indent=2)+"\n")
    return {"protocol": str(path), "sha256": digest(path), "warmup": len(cohort["warmup"]),
            "evaluation": len(cohort["evaluation"]), "proxy_targets": len(eligible),
            "long_proxy_targets": len(protocol["long_proxy_targets"])}


def verify(source, output):
    protocol = json.loads((output/"protocol.json").read_text())
    if protocol["hashes"] != inputs(source, output):
        raise ValueError("Frozen Komoot inputs changed")
    return {"verified": True, "protocol_sha256": digest(output/"protocol.json")}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", choices=("freeze", "verify"))
    parser.add_argument("--source", type=Path, default=SOURCE)
    parser.add_argument("--output", type=Path, default=OUTPUT)
    args = parser.parse_args()
    print(json.dumps((freeze if args.stage == "freeze" else verify)(args.source, args.output), indent=2))
