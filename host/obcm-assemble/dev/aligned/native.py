#!/usr/bin/env python3
"""Run three alternating complete native assemblies per variant, with full validation."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('baseline', type=Path)
parser.add_argument('candidate', type=Path)
parser.add_argument('inputs', type=Path)
parser.add_argument('output', type=Path)
args = parser.parse_args()
root = Path(__file__).resolve().parents[4]
subprocess.run(['python3', str(root / 'host/obcm-assemble/dev/navigation/native.py'), str(args.baseline), str(args.inputs), str(args.output), '--check-inputs'], check=True)
args.output.mkdir(parents=True, exist_ok=False)

def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()

expected = {'baseline': 'feb9775a380b5aea0736b01c14a7af7ea0d3cd025a9e8b74c0d629a164bf5e72', 'candidate': '531eedbc42b97db78b69149c6046e325c869ee058ae016e4e54da976d201e739'}
report = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(), 'platform': platform.platform(), 'binaries': {label: digest(getattr(args, label)) for label in expected}, 'runs': []}
for pair in range(3):
    labels = ['baseline', 'candidate'] if pair % 2 == 0 else ['candidate', 'baseline']
    for label in labels:
        output = args.output / f'{pair}-{label}.obcm'
        command = [str(getattr(args, label).resolve()), '--cells', str(args.inputs / 'cells.json'), '--skin', str(args.inputs / 'skin.json'), '--terrain', str(args.inputs / 'terrain.json'), '--out', str(output), '--merge-budget-bytes', '16777216', '--json', '--accept-partial']
        process = subprocess.run(command, text=True, capture_output=True)
        (args.output / f'{pair}-{label}.log').write_text(process.stderr)
        if process.returncode:
            report['failure'] = {'pair': pair, 'label': label, 'stderr': process.stderr, 'exit_code': process.returncode}
            (args.output / 'native.json').write_text(json.dumps(report, indent=2) + '\n')
            process.check_returncode()
        summary = json.loads(process.stdout)
        observed = digest(output)
        assert observed == summary['sha256'] == expected[label]
        assert output.stat().st_size == summary['bytes']
        assert summary['nav']['nodes'] == 1010635 and summary['nav']['edges'] == 1317371
        assert summary['nav']['dropped_nodes'] == 0
        assert summary['phases_us']['verify'] > 0
        report['runs'].append({'pair': pair, 'label': label, 'order': labels.index(label), 'command': command, 'summary': summary, 'readback_sha256': observed, 'ledger': process.stderr})
        (args.output / 'native.json').write_text(json.dumps(report, indent=2) + '\n')
        print(pair, label, summary['phases_us'], flush=True)
