#!/usr/bin/env python3
"""Summarize the fixed default-cache campaign; excluded attempts are never selected."""
import json
from pathlib import Path
from statistics import median

ROOT = Path(__file__).resolve().parent
ORDER = ['baseline', 'candidate', 'candidate', 'baseline', 'baseline', 'candidate']
PINNED_SHA = 'feb9775a380b5aea0736b01c14a7af7ea0d3cd025a9e8b74c0d629a164bf5e72'
provenance = json.loads((ROOT / 'provenance.json').read_text())
summary = {}
for workload in ['pinned', 'sequential']:
    groups = {'baseline': [], 'candidate': []}
    for i, variant in enumerate(ORDER, 1):
        raw = json.loads((ROOT / f'{workload}-{i}-{variant}.json').read_text())
        assert raw['wasm_sha256'] == provenance[f'{variant}_wasm_sha256']
        assert raw['manifest_sha256'] == provenance['input_manifest_sha256']
        assert raw['read_block_bytes'] == 'default'
        assert raw['reading']['mode'] == 'streamed' and raw['writing']['mode'] == 'disk'
        assert raw['done']['summary']['verified']
        assert raw['readback_sha256'] == raw['stored']['sha256'] == raw['done']['summary']['sha256']
        groups[variant].append(raw)
    hashes = {r['readback_sha256'] for group in groups.values() for r in group}
    assert len(hashes) == 1
    if workload == 'pinned':
        assert hashes == {PINNED_SHA}
    metrics = {}
    for variant, runs in groups.items():
        metrics[variant] = {
            'phases_ms': {phase: median(r['done']['summary']['phases_us'][phase] / 1000 for r in runs)
                          for phase in runs[0]['done']['summary']['phases_us']},
            'total_range_ms': [min(r['done']['summary']['phases_us']['total'] / 1000 for r in runs),
                               max(r['done']['summary']['phases_us']['total'] / 1000 for r in runs)],
            'io': {kind: {field: median(r['done']['io'][kind][field] for r in runs)
                          for field in ['calls', 'bytes', 'ms']} for kind in runs[0]['done']['io']},
            'wasm_capacity_bytes': max(r['done']['measurement']['wasm_capacity_bytes'] for r in runs),
        }
    baseline, candidate = (metrics[v] for v in ['baseline', 'candidate'])
    old, new = (m['phases_ms'] for m in [baseline, candidate])
    guards = {
        'verification': new['verify'] - old['verify'] <= max(0.05 * old['verify'], 50),
        'wasm_capacity': candidate['wasm_capacity_bytes'] <= baseline['wasm_capacity_bytes'],
    }
    if workload == 'pinned':
        guards['total_improvement'] = new['total'] <= 0.9 * old['total']
        guards['disjoint_ranges'] = candidate['total_range_ms'][1] < baseline['total_range_ms'][0]
    else:
        guards['total_regression'] = new['total'] - old['total'] <= max(0.05 * old['total'], 50)
    summary[workload] = {'sha256': hashes.pop(), 'metrics': metrics, 'guards': guards}
print(json.dumps(summary, indent=2))
if not all(ok for case in summary.values() for ok in case['guards'].values()):
    raise SystemExit('A predeclared adoption guard failed.')
