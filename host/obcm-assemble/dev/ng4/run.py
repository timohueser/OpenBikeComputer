#!/usr/bin/env python3
"""Run exactly three alternating baseline/probe pairs on NG1's pinned inputs."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('binary', type=Path)
parser.add_argument('inputs', type=Path)
parser.add_argument('output', type=Path)
args = parser.parse_args()
preflight = Path(__file__).resolve().parent.parent / 'navigation/native.py'
subprocess.run(['python3', str(preflight), str(args.binary), str(args.inputs), str(args.output), '--check-inputs'], check=True)
args.output.mkdir(parents=True, exist_ok=False)
runs = []
report = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(), 'source_nav_sha256': hashlib.sha256(Path('host/obcm-assemble/src/nav.rs').read_bytes()).hexdigest(), 'binary_sha256': hashlib.sha256(args.binary.read_bytes()).hexdigest(), 'platform': platform.platform(), 'rustc': subprocess.check_output(['rustc', '--version'], text=True).strip(), 'runs': runs}
for pair in range(3):
    for mode in (['baseline', 'probe'] if pair % 2 == 0 else ['probe', 'baseline']):
        output = args.output / f'{pair}-{mode}.obcm'
        command = [str(args.binary.resolve()), '--cells', str(args.inputs / 'cells.json'), '--skin', str(args.inputs / 'skin.json'), '--terrain', str(args.inputs / 'terrain.json'), '--out', str(output), '--merge-budget-bytes', '16777216', '--json', '--accept-partial']
        env = dict(os.environ)
        env.pop('OBC_NG4_PROBE', None)
        if mode == 'probe':
            env['OBC_NG4_PROBE'] = '1'
        result = subprocess.run(command, env=env, capture_output=True, text=True)
        (args.output / f'{pair}-{mode}.log').write_text(result.stderr)
        (args.output / f'{pair}-{mode}.stdout.json').write_text(result.stdout)
        if result.returncode:
            report['failure'] = {'pair': pair, 'mode': mode, 'command': command, 'exit_code': result.returncode, 'stderr': result.stderr}
            (args.output / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
            result.check_returncode()
        summary = json.loads(result.stdout)
        with output.open('rb') as stream:
            digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        assert digest == summary['sha256'] == 'feb9775a380b5aea0736b01c14a7af7ea0d3cd025a9e8b74c0d629a164bf5e72'
        assert output.stat().st_size == summary['bytes'] == 247837696
        assert 'nav-profile integrated_reference_probe' in result.stderr
        (args.output / f'{pair}-{mode}.log').write_text(result.stderr)
        runs.append({'pair': pair, 'mode': mode, 'command': command, 'probe_environment': mode == 'probe', 'summary': summary, 'readback_sha256': digest, 'ledger': result.stderr})
        (args.output / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
        print(pair, mode, summary, flush=True)
(args.output / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
