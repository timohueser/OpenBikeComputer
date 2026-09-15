#!/usr/bin/env python3
"""Three fixed browser pairs on pinned and render-only inputs; no size sweep."""
import argparse
import json
import os
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
for name in ['baseline_root', 'candidate_root', 'inputs', 'output']:
    parser.add_argument(name, type=Path)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=False)
profiles = args.output / 'profiles'
profiles.mkdir()
expected = {
    ('pinned', 'baseline'): 'feb9775a380b5aea0736b01c14a7af7ea0d3cd025a9e8b74c0d629a164bf5e72',
    ('pinned', 'candidate'): '531eedbc42b97db78b69149c6046e325c869ee058ae016e4e54da976d201e739',
    ('sequential', 'baseline'): '9e8b350a342320c30adc5e8665460a3f65e64c09c5fd67519e8a9c4e0ca28469',
}
report = []
for workload in ['pinned', 'sequential']:
    for pair in range(3):
        labels = ['baseline', 'candidate'] if pair % 2 == 0 else ['candidate', 'baseline']
        for label in labels:
            processes = subprocess.check_output(['ps', '-eo', 'pid,comm'], text=True)
            active = [line.strip() for line in processes.splitlines()
                      if line.split()[-1] in ['cargo', 'rustc', 'wasm-opt']]
            if active:
                raise RuntimeError(f'Compiler active before sample: {active}')
            result = args.output / f'{workload}-{pair}-{label}.json'
            command = ['node', str(args.candidate_root / 'host/obcm-assemble/dev/navigation/browser.mjs'),
                       str(args.inputs), str(result), 'default', workload]
            with result.with_suffix('.log').open('w') as log:
                subprocess.run(command, env={**os.environ, 'OBC_BROWSER_PROFILE_ROOT': str(profiles), 'OBC_BROWSER_ROOT': str(getattr(args, label + '_root'))},
                               stdout=log, stderr=subprocess.STDOUT, check=True)
            data = json.loads(result.read_text())
            key = workload, label
            expected.setdefault(key, data['readback_sha256'])
            assert data['readback_sha256'] == expected[key]
            summary = data['done']['summary']
            if workload == 'sequential':
                neutral = expected.setdefault(('sequential', 'neutral'), data['format_neutral_sha256'])
                assert data['format_neutral_sha256'] == neutral, 'render-only bytes differ beyond the version byte'
            assert summary['nav']['dropped_nodes'] == 0
            assert summary['nav']['degree_truncated'] == 0
            assert summary['verified']['nav_nodes'] == (1010635 if workload == 'pinned' else 0)
            report.append({'pair': pair, 'variant': label, 'workload': workload,
                           'compiler_processes_before': active, **data})
            (args.output / 'browser.json').write_text(json.dumps(report, indent=2) + '\n')
            print(workload, pair, label, summary['phases_us'], flush=True)
