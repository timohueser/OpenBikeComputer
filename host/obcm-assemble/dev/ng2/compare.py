#!/usr/bin/env python3
"""Run the frozen NG1 cases in three baseline/candidate pairs, without retries."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("baseline", type=Path)
parser.add_argument("candidate", type=Path)
parser.add_argument("candidate_maps", type=Path)
parser.add_argument("output", type=Path)
parser.add_argument("--baseline-maps", type=Path, default=Path(os.environ.get("OBC_FIXTURE_CACHE", Path.home() / ".cache/openbikecomputer/fixtures")) / "by-id")
args = parser.parse_args()
root = Path(__file__).resolve().parents[4]
manifest = json.loads((root / "host/obc-bench/dev/navigation/inputs.json").read_text())
args.output.mkdir(parents=True, exist_ok=False)

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

results = []
failures = []
for case in manifest["cases"]:
    baseline_map = args.baseline_maps / case["map"]
    candidate_map = args.candidate_maps / case["map"]
    assert digest(baseline_map) == manifest["maps"][case["map"]]["sha256"], "baseline digest mismatch"
    runs = {"baseline": [], "candidate": []}
    for repeat in range(manifest["repeats"]):
        for label, binary, map_path in [("baseline", args.baseline, baseline_map), ("candidate", args.candidate, candidate_map)]:
            output = args.output / f"{case['name']}-{label}-{repeat}.obcr"
            command = [str(binary.resolve()), str(map_path), *map(str, case["from"] + case["to"]), str(case["profile"]), str(output)]
            if "original" in case:
                command.append(str(args.output / f"{case['original']}-{label}-0.obcr"))
            process = subprocess.run(command, check=True, text=True, capture_output=True)
            run = {}
            for line in process.stdout.splitlines():
                run.update(json.loads(line))
            if run["outcome"] != case["outcome"]:
                failures.append(f"{case['name']} {label} {repeat}: outcome {run['outcome']}")
            if case.get("interior") and not run["from_interior"]:
                failures.append(f"{case['name']} {label} {repeat}: expected interior projection")
            run["output_sha256"] = digest(output) if output.exists() else None
            runs[label].append(run)
    if len({run["output_sha256"] for group in runs.values() for run in group}) != 1:
        failures.append(f"route bytes differ: {case['name']}")
    results.append({"case": case["name"], "baseline_input_sha256": digest(baseline_map), "candidate_input_sha256": digest(candidate_map), **runs})

report = {"platform": platform.platform(), "source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(), "baseline_binary_sha256": digest(args.baseline), "candidate_binary_sha256": digest(args.candidate), "results": results, "failures": failures}
(args.output / "paired.json").write_text(json.dumps(report, indent=2) + "\n")
print(args.output / "paired.json")
if failures:
    raise SystemExit("\n".join(failures))
