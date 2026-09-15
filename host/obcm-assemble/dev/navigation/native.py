#!/usr/bin/env python3
"""Measure three native assemblies with the pinned options and full verification."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('binary', type=Path)
parser.add_argument('inputs', type=Path)
parser.add_argument('output', type=Path)
args = parser.parse_args()
manifest = json.loads(Path(__file__).with_name('inputs.json').read_text())
args.output.mkdir(parents=True, exist_ok=True)
reports = []
for repeat in range(3):
    output = args.output / f'native-{repeat}.obcm'
    command = [str(args.binary.resolve()), '--cells', str(args.inputs / 'cells.json'), '--skin', str(args.inputs / 'skin.json'), '--terrain', str(args.inputs / 'terrain.json'), '--out', str(output), '--merge-budget-bytes', str(manifest['options']['merge_budget_bytes']), '--json']
    if manifest['options']['accept_partial']:
        command.append('--accept-partial')
    if manifest['options']['accept_holes']:
        command.append('--accept-holes')
    result = subprocess.run(command, check=True, capture_output=True, text=True)
    summary = json.loads(result.stdout)
    with output.open('rb') as stream:
        readback = hashlib.file_digest(stream, 'sha256').hexdigest()
    assert readback == summary['sha256']
    assert output.stat().st_size == summary['bytes']
    assert 'nav-profile collect' in result.stderr, 'Build with --features mem-profile'
    (args.output / f'native-{repeat}.log').write_text(result.stderr)
    reports.append({'command': command, 'summary': summary, 'readback_sha256': readback, 'ledger': result.stderr})
assert len({r['readback_sha256'] for r in reports}) == 1, 'Output changes between repeats'
report = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(), 'binary_sha256': hashlib.sha256(args.binary.read_bytes()).hexdigest(), 'environment_rustc': subprocess.check_output(['rustc', '--version'], text=True).strip(), 'platform': platform.platform(), 'manifest_sha256': hashlib.sha256(Path(__file__).with_name('inputs.json').read_bytes()).hexdigest(), 'runs': reports}
(args.output / 'native.json').write_text(json.dumps(report, indent=2) + '\n')
print(args.output / 'native.json')
