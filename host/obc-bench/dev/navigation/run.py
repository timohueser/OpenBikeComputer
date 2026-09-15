#!/usr/bin/env python3
"""Run the fixed route manifest against an explicitly built planner binary."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('binary', type=Path)
parser.add_argument('output', type=Path)
parser.add_argument('--maps', type=Path, default=Path(os.environ.get('OBC_FIXTURE_CACHE', Path.home() / '.cache/openbikecomputer/fixtures')) / 'by-id')
parser.add_argument('--candidate', action='store_true', help='Accept converted map digests; retain observed digests in the report')
args = parser.parse_args()
manifest = json.loads(Path(__file__).with_name('inputs.json').read_text())
args.output.mkdir(parents=True, exist_ok=True)
results = []
for case in manifest['cases']:
    path = args.maps / case['map']
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    if not args.candidate and digest != manifest['maps'][case['map']]['sha256']:
        raise SystemExit(f'Input digest mismatch: {path}')
    runs = []
    for repeat in range(3):
        output = args.output / f'{case["name"]}-{repeat}.obcr'
        output.unlink(missing_ok=True)
        command = [str(args.binary.resolve()), str(path), *map(str, case['from'] + case['to']), str(case['profile']), str(output)]
        if 'original' in case:
            command.append(str(args.output / f'{case["original"]}-0.obcr'))
        process = subprocess.run(command, check=True, text=True, capture_output=True)
        run = {}
        for line in process.stdout.splitlines():
            run.update(json.loads(line))
        if run['outcome'] != case['outcome']:
            raise SystemExit(f'{case["name"]}: unexpected outcome {run["outcome"]}')
        if case.get('interior') and not run['from_interior']:
            raise SystemExit(f'{case["name"]}: expected interior projection')
        run['output_sha256'] = hashlib.sha256(output.read_bytes()).hexdigest()
        runs.append(run)
    assert len({r['output_sha256'] for r in runs}) == 1, 'Nondeterministic route output'
    results.append({'case': case['name'], 'input_sha256': digest, 'runs': runs})
report = {'platform': platform.platform(), 'machine': platform.machine(), 'binary_sha256': hashlib.sha256(args.binary.read_bytes()).hexdigest(), 'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(), 'results': results}
(args.output / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
print(args.output / 'results.json')
