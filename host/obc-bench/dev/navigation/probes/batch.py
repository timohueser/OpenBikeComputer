#!/usr/bin/env python3
"""Fixed cold-plan batches; table/reader counters must be disabled in both binaries."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import statistics
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('baseline', type=Path)
parser.add_argument('candidate', type=Path)
parser.add_argument('output', type=Path)
parser.add_argument('--maps', type=Path, default=Path.home() / '.cache/openbikecomputer/fixtures/by-id')
args = parser.parse_args()
root = Path(__file__).resolve().parents[5]
manifest = json.loads((root / 'host/obc-bench/dev/navigation/inputs.json').read_text())
expected = {c['case']: c['runs'][0]['output_sha256'] for c in json.loads((root / 'host/obc-bench/dev/navigation/baseline.json').read_text())['results']}
args.output.mkdir(parents=True, exist_ok=False)
digest = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
results = []
for case in manifest['cases']:
    map_path = args.maps / case['map']
    assert digest(map_path) == manifest['maps'][case['map']]['sha256']
    common = None
    groups = {'baseline': [], 'candidate': []}
    for pair in range(3):
        labels = ['baseline', 'candidate'] if pair % 2 == 0 else ['candidate', 'baseline']
        for label in labels:
            output = args.output / f"{case['name']}-{label}-{pair}.obcr"
            cmd = [str(getattr(args, label).resolve()), str(map_path), *map(str, case['from'] + case['to']), str(case['profile']), str(output)]
            if 'original' in case:
                cmd.append(str(args.output / f"{case['original']}-{label}-0.obcr"))
            stdout = subprocess.check_output(cmd, text=True, env={**os.environ, 'OBC_NAV_BATCH': '250'})
            runs = []
            for line in stdout.splitlines():
                row = json.loads(line)
                if 'iteration' in row:
                    assert row['iteration'] == len(runs)
                    runs.append({})
                else:
                    runs[-1].update(row)
            assert len(runs) == 250
            assert digest(output) == expected[case['name']]
            for run in runs:
                assert run['outcome'] == case['outcome']
                assert not any(k in run for k in ['table_slot_probes', 'decoded_junctions']), 'instrumented binary'
                fixed = {k: v for k, v in run.items() if not k.endswith('_ns')}
                if common is None:
                    common = fixed
                assert fixed == common, f"behavior changed: {case['name']} {label}"
                if case.get('interior'):
                    assert run['from_interior']
            totals = [r['total_ns'] for r in runs]
            groups[label].append({
                'pair': pair,
                'order': labels.index(label),
                'mean_total_ns': statistics.mean(totals),
                'median_total_ns': statistics.median(totals),
                'p95_total_ns': sorted(totals)[237],
                'total_ns': totals,
                'snap_ns': [r['phase_ns'][0] for r in runs],
                'search_ns': [r['phase_ns'][1] for r in runs],
                'emit_ns': [r['phase_ns'][2] for r in runs],
                'max_step_ns': [r['max_step_ns'] for r in runs],
            })
    results.append({'case': case['name'], 'input_sha256': digest(map_path), 'output_sha256': expected[case['name']], 'invariant_per_plan': common, **groups})
report = {'platform': platform.platform(), 'warmups': 10, 'measured_plans_per_process': 250, 'pairs': 3, 'instrumentation': 'All nav-metrics disabled. Every iteration verifies bytes/outcome/workspace against first run; driver verifies the common OBCR SHA against pinned NG1 output and checks all other invariant fields per iteration.', 'binaries': {label: digest(getattr(args, label)) for label in ['baseline', 'candidate']}, 'results': results}
text = json.dumps(report, indent=2)
text = re.sub(r'\[\s*(\d+(?:,\s*\d+)*)\s*\]', lambda m: '[' + re.sub(r'\s+', ' ', m[1]) + ']', text)
(args.output / 'batch.json').write_text(text + '\n')
print(args.output / 'batch.json')
